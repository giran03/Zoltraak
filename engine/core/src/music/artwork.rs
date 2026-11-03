//! Cover art: finding a picture worth putting on a track.
//!
//! What a source hands over varies from real album art to nothing at all, and
//! the difference matters more than it looks. A track matched on YouTube comes
//! back with `i.ytimg.com/vi/<id>/hqdefault.jpg` — a 16:9 frame from the video,
//! letterboxed, often a lyric card or a still of the artist's face. Embedded as
//! album art it is what every player and every file listing will show for that
//! song forever. A Spotify or Apple Music link, by contrast, already carries
//! the release's own square cover, and nothing here should second-guess it.
//!
//! So this module answers one question: *is the picture we have actually the
//! release's cover, and if not, can a better one be found?* The lookup goes to
//! the same public catalogues the extractors already use — Apple's iTunes
//! Search and Deezer's search — both of which answer without a credential and
//! return the album's square artwork at a size worth embedding.
//!
//! A lookup that finds nothing, or finds something it is not confident about,
//! changes nothing: the track keeps whatever it had. Wrong art is worse than a
//! video thumbnail, and far worse than none.

use reqwest::Client;

use super::{MusicTrack, json_str};

/// Where a track's cover ended up coming from. Carried so the UI and the logs
/// can say, rather than implying every picture is the release's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkOrigin {
    /// The service that listed the track supplied it.
    Catalogue,
    /// Looked up on Apple's iTunes Search.
    ITunes,
    /// Looked up on Deezer.
    Deezer,
}

impl ArtworkOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Catalogue => "the source",
            Self::ITunes => "Apple Music",
            Self::Deezer => "Deezer",
        }
    }
}

/// Whether a URL is a still frame from a video rather than a release's cover.
///
/// YouTube's thumbnails are the case that matters: every track matched from
/// Spotify, Apple Music or Deezer arrives with one, and they are 16:9 pictures
/// of whatever the uploader chose. YouTube Music's own square art is served
/// from `googleusercontent`/`ggpht` and is real cover art, so it is left alone.
pub fn is_video_thumbnail(url: &str) -> bool {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // `i.ytimg.com/vi/<id>/…` and `img.youtube.com/vi/<id>/…` are video stills.
    (host.ends_with("ytimg.com") || host.ends_with("youtube.com")) && url.contains("/vi/")
}

/// Whether this track's picture is worth replacing: it has none, or the one it
/// has is a video still.
pub fn wants_better_artwork(track: &MusicTrack) -> bool {
    match track.cover_url.as_deref() {
        None => true,
        Some(url) => is_video_thumbnail(url),
    }
}

// ─── Matching ────────────────────────────────────────────────────────────────

/// Lowercased, accents folded, punctuation dropped.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = match c {
            'à'..='å' | 'À'..='Å' => 'a',
            'è'..='ë' | 'È'..='Ë' => 'e',
            'ì'..='ï' | 'Ì'..='Ï' => 'i',
            'ò'..='ö' | 'Ò'..='Ö' | 'ø' | 'Ø' => 'o',
            'ù'..='ü' | 'Ù'..='Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            'ß' => 's',
            _ => c,
        };
        if c.is_alphanumeric() || c.is_whitespace() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words that describe the *upload* or a re-recording rather than the song, so
/// a candidate carrying one the wanted title does not is a different thing.
const VARIANTS: &[&str] = &["live", "cover", "karaoke", "instrumental", "remix", "tribute", "workout", "made famous"];

/// How well a candidate release matches the track, in `0.0..=1.0`.
///
/// Deliberately strict on the artist: album art from the wrong artist is not a
/// near miss, it is a wrong file. The title carries the rest.
fn score(track: &MusicTrack, candidate_artist: &str, candidate_title: &str) -> f64 {
    let want_title = normalize(&track.title);
    let got_title = normalize(candidate_title);
    if want_title.is_empty() || got_title.is_empty() {
        return 0.0;
    }

    let want_words: Vec<&str> = want_title.split_whitespace().collect();
    let got_words: Vec<&str> = got_title.split_whitespace().collect();
    let shared = want_words.iter().filter(|w| got_words.contains(w)).count() as f64;
    let title = shared / want_words.len().max(1) as f64;

    let want_artist = normalize(&track.artist_line());
    let got_artist = normalize(candidate_artist);
    let artist = if want_artist.is_empty() {
        0.5
    } else if got_artist.contains(&want_artist) || want_artist.contains(&got_artist) {
        1.0
    } else {
        // A featured credit means the strings differ while the act does not.
        let want_first = want_artist.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        if !want_first.is_empty() && got_artist.contains(&want_first) { 0.8 } else { 0.0 }
    };

    let mut total = 0.5 * title + 0.5 * artist;
    for word in VARIANTS {
        if got_title.split_whitespace().any(|w| w == *word) && !want_title.split_whitespace().any(|w| w == *word) {
            total -= 0.3;
        }
    }
    total.clamp(0.0, 1.0)
}

/// Below this the candidate is not trusted to be the same release.
const ACCEPT: f64 = 0.7;

// ─── Sources ─────────────────────────────────────────────────────────────────

/// iTunes' `100x100bb.jpg` asked for at a size worth embedding.
fn upsize_itunes(url: &str) -> String {
    match url.rfind('/') {
        Some(i) if url[i..].contains('x') => format!("{}/1400x1400bb.jpg", &url[..i]),
        _ => url.to_string(),
    }
}

fn query_for(track: &MusicTrack) -> String {
    let artist = track.primary_artist().unwrap_or_default();
    format!("{artist} {}", track.title).trim().to_string()
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The best-scoring artwork among `candidates`, each `(artist, title, url)`.
fn best(track: &MusicTrack, candidates: impl Iterator<Item = (String, String, String)>) -> Option<String> {
    candidates
        .map(|(artist, title, url)| (score(track, &artist, &title), url))
        .filter(|(s, _)| *s >= ACCEPT)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, url)| url)
}

async fn from_itunes(client: &Client, track: &MusicTrack) -> Option<String> {
    let url = format!("https://itunes.apple.com/search?term={}&entity=song&limit=8", urlencode(&query_for(track)));
    let body = super::get_json(client, &url).await.ok()?;
    let results = body.get("results")?.as_array()?;
    best(
        track,
        results.iter().filter_map(|r| {
            Some((
                json_str(r, "/artistName")?,
                json_str(r, "/trackName")?,
                upsize_itunes(&json_str(r, "/artworkUrl100")?),
            ))
        }),
    )
}

async fn from_deezer(client: &Client, track: &MusicTrack) -> Option<String> {
    let url = format!("https://api.deezer.com/search?q={}&limit=8", urlencode(&query_for(track)));
    let body = super::get_json(client, &url).await.ok()?;
    let results = body.get("data")?.as_array()?;
    best(
        track,
        results.iter().filter_map(|r| {
            let art = json_str(r, "/album/cover_xl")
                .or_else(|| json_str(r, "/album/cover_big"))
                .or_else(|| json_str(r, "/album/cover"))?;
            Some((json_str(r, "/artist/name")?, json_str(r, "/title")?, art))
        }),
    )
}

/// Find cover art for a track that has none worth using.
///
/// Apple first, Deezer second: both answer without a credential, and Apple's
/// catalogue is the larger of the two. `None` means nothing confident was
/// found, and the caller should keep whatever the track already had.
pub async fn look_up(client: &Client, track: &MusicTrack) -> Option<(String, ArtworkOrigin)> {
    if track.title.trim().is_empty() {
        return None;
    }
    if let Some(url) = from_itunes(client, track).await {
        return Some((url, ArtworkOrigin::ITunes));
    }
    if let Some(url) = from_deezer(client, track).await {
        return Some((url, ArtworkOrigin::Deezer));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::{AudioSource, Service};

    fn track(title: &str, artist: &str) -> MusicTrack {
        MusicTrack {
            id: "t".into(),
            title: title.into(),
            artists: vec![artist.into()],
            album: None,
            album_artist: None,
            track_number: None,
            track_total: None,
            disc_number: None,
            year: None,
            date: None,
            genre: None,
            duration: None,
            cover_url: None,
            isrc: None,
            source_url: String::new(),
            service: Service::Spotify,
            source: AudioSource::Match,
            explicit: false,
            unavailable: None,
        }
    }

    #[test]
    fn a_youtube_still_is_recognised_and_real_cover_art_is_left_alone() {
        assert!(is_video_thumbnail("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
        assert!(is_video_thumbnail("https://img.youtube.com/vi/dQw4w9WgXcQ/maxresdefault.jpg"));
        // YouTube Music's own square art is a release cover, not a video frame.
        assert!(!is_video_thumbnail("https://lh3.googleusercontent.com/abc=w1200-h1200-l90-rj"));
        assert!(!is_video_thumbnail("https://i.scdn.co/image/ab67616d0000b273abc"));
        assert!(!is_video_thumbnail("https://is1-ssl.mzstatic.com/image/thumb/x/1400x1400bb.jpg"));
    }

    #[test]
    fn only_a_missing_or_video_cover_asks_for_a_better_one() {
        let mut t = track("One More Time", "Daft Punk");
        assert!(wants_better_artwork(&t), "no cover at all");
        t.cover_url = Some("https://i.ytimg.com/vi/x/hqdefault.jpg".into());
        assert!(wants_better_artwork(&t), "a video still is not album art");
        t.cover_url = Some("https://i.scdn.co/image/abc".into());
        assert!(!wants_better_artwork(&t), "the catalogue's own cover is left alone");
    }

    #[test]
    fn the_right_release_is_accepted_and_a_wrong_artist_is_not() {
        let t = track("One More Time", "Daft Punk");
        assert!(score(&t, "Daft Punk", "One More Time") >= ACCEPT);
        assert!(score(&t, "Daft Punk", "One More Time (Radio Edit)") >= ACCEPT, "a shorter edit is the same release art");
        // Album art from another artist is a wrong file, not a near miss.
        assert!(score(&t, "The Karaoke Channel", "One More Time") < ACCEPT);
        assert!(score(&t, "Some Cover Band", "One More Time (Live)") < ACCEPT);
        assert!(score(&t, "Daft Punk", "Aerodynamic") < ACCEPT, "a different song on the same album is not this song");
    }

    #[test]
    fn a_featured_credit_still_matches_the_act() {
        let mut t = track("Get Lucky", "Daft Punk");
        t.artists = vec!["Daft Punk".into(), "Pharrell Williams".into()];
        assert!(score(&t, "Daft Punk", "Get Lucky (feat. Pharrell Williams & Nile Rodgers)") >= ACCEPT);
    }

    #[test]
    fn accents_and_punctuation_do_not_block_a_match() {
        let t = track("Déjà Vu", "Beyoncé");
        assert!(score(&t, "Beyonce", "Deja Vu") >= ACCEPT);
    }

    #[test]
    fn itunes_artwork_is_asked_for_at_embed_size() {
        assert_eq!(
            upsize_itunes("https://is1-ssl.mzstatic.com/image/thumb/Music116/v4/a/artwork.jpg/100x100bb.jpg"),
            "https://is1-ssl.mzstatic.com/image/thumb/Music116/v4/a/artwork.jpg/1400x1400bb.jpg"
        );
    }

    #[test]
    fn the_best_candidate_wins_and_a_weak_field_yields_nothing() {
        let t = track("One More Time", "Daft Punk");
        let candidates = vec![
            ("Karaoke Band".to_string(), "One More Time".to_string(), "https://wrong".to_string()),
            ("Daft Punk".to_string(), "One More Time".to_string(), "https://right".to_string()),
        ];
        assert_eq!(best(&t, candidates.into_iter()).as_deref(), Some("https://right"));

        let weak = vec![("Nobody".to_string(), "Something Else".to_string(), "https://no".to_string())];
        assert_eq!(best(&t, weak.into_iter()), None, "nothing confident means keep what the track had");
    }

    // Hits Apple and Deezer for real; run on request with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_artwork_lookup() {
        let client = reqwest::Client::new();
        for (title, artist) in [("One More Time", "Daft Punk"), ("Never Gonna Give You Up", "Rick Astley")] {
            let t = track(title, artist);
            println!("{artist} - {title}: {:?}", look_up(&client, &t).await);
        }
    }
}
