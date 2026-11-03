//! Spotify: tracks, albums, playlists and artists, from the embed player.
//!
//! Spotify's Web API needs an application credential, and its public pages
//! are a JavaScript shell with nothing in them. The *embed* player
//! (`open.spotify.com/embed/…`) is different: it is what a blog's iframe
//! loads, so it has to render without a sign-in, and it ships its data in the
//! page as Next.js state — the track's name, artists, duration, release date
//! and cover; an album's or playlist's whole track list.
//!
//! What it does not have: an album name on a single track (the manager fills
//! that in from the YouTube Music match), and more than the first hundred
//! tracks of a playlist. Both limits are stated to the user rather than
//! papered over.
//!
//! Every track resolves to [`AudioSource::Match`]: Spotify serves no audio to
//! anything but its own DRM players, and nothing here tries.

use reqwest::Client;
use serde_json::Value;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, get_html, json_str, resolve_redirects, script_json, year_of};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Track,
    Album,
    Playlist,
    Artist,
}

impl Kind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "track" => Some(Self::Track),
            "album" => Some(Self::Album),
            "playlist" => Some(Self::Playlist),
            "artist" => Some(Self::Artist),
            _ => None,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Playlist => "playlist",
            Self::Artist => "artist",
        }
    }
}

/// The kind and id in any Spotify link shape a user pastes: a web URL (with
/// or without an `intl-xx` or `embed` prefix, or a legacy `/user/…/playlist/`
/// path), or a `spotify:` URI.
fn parse(url: &str) -> Option<(Kind, String)> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("spotify:") {
        let mut parts = rest.split(':');
        let kind = Kind::parse(parts.next()?)?;
        let id = parts.next()?;
        return valid_id(id).then(|| (kind, id.to_string()));
    }
    let path = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split_once('/')
        .map(|(_, p)| p)
        .unwrap_or("");
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    while i + 1 < segments.len() {
        if let Some(kind) = Kind::parse(segments[i]) {
            let id = segments[i + 1];
            if valid_id(id) {
                return Some((kind, id.to_string()));
            }
        }
        i += 1;
    }
    None
}

fn valid_id(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn page_url(kind: Kind, id: &str) -> String {
    format!("https://open.spotify.com/{}/{id}", kind.path())
}

pub async fn analyze(client: &Client, url: &str) -> Result<MusicSource> {
    let mut url = url.trim().to_string();
    if super::host_of(&url).ends_with("spotify.link") {
        url = resolve_redirects(client, &url).await?;
    }
    let (kind, id) = parse(&url).ok_or_else(|| {
        Error::InvalidInput("this Spotify link does not point at a track, album, playlist or artist".into())
    })?;

    let embed = format!("https://open.spotify.com/embed/{}/{id}", kind.path());
    let html = get_html(client, &embed).await?;
    let data = script_json(&html, "__NEXT_DATA__")
        .ok_or_else(|| Error::InvalidInput("Spotify's embed page did not carry its usual data; the link may be private or removed".into()))?;
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .cloned()
        .ok_or_else(|| Error::InvalidInput("Spotify could not find anything at this link — it may be private, removed, or unavailable in your region".into()))?;

    Ok(build(kind, &id, &entity))
}

/// The largest image the embed lists.
fn cover_of(entity: &Value) -> Option<String> {
    let visual = entity
        .pointer("/visualIdentity/image")
        .and_then(Value::as_array)
        .and_then(|images| {
            images
                .iter()
                .max_by_key(|img| img.get("maxWidth").and_then(Value::as_u64).unwrap_or(0))
                .and_then(|img| img.get("url").and_then(Value::as_str))
        })
        .map(str::to_string);
    visual.or_else(|| json_str(entity, "/coverArt/sources/0/url"))
}

fn build(kind: Kind, id: &str, entity: &Value) -> MusicSource {
    let name = json_str(entity, "/name").or_else(|| json_str(entity, "/title")).unwrap_or_else(|| id.to_string());
    let cover = cover_of(entity);
    let source_url = page_url(kind, id);

    match kind {
        Kind::Track => {
            let artists = artist_names(entity);
            let date = json_str(entity, "/releaseDate/isoString");
            let track = MusicTrack {
                id: format!("spotify:track:{id}"),
                title: name.clone(),
                artists: artists.clone(),
                album: None,
                album_artist: None,
                track_number: None,
                track_total: None,
                disc_number: None,
                year: date.as_deref().and_then(year_of),
                date: date.as_deref().map(|d| d.chars().take(10).collect()),
                genre: None,
                duration: entity.get("duration").and_then(Value::as_f64).map(|ms| ms / 1000.0).filter(|s| *s > 0.0),
                cover_url: cover.clone(),
                isrc: None,
                source_url: source_url.clone(),
                service: Service::Spotify,
                source: AudioSource::Match,
                explicit: entity.get("isExplicit").and_then(Value::as_bool).unwrap_or(false),
                unavailable: unavailable_reason(entity),
            };
            MusicSource {
                kind: SourceKind::Track,
                service: Service::Spotify,
                title: name,
                owner: artists.first().cloned(),
                cover_url: cover,
                source_url,
                tracks: vec![track],
                total: None,
                note: None,
            }
        }
        Kind::Album | Kind::Playlist | Kind::Artist => {
            let owner = json_str(entity, "/subtitle");
            let list = entity.get("trackList").and_then(Value::as_array).cloned().unwrap_or_default();
            let total = list.len() as u32;
            let album_name = (kind == Kind::Album).then(|| name.clone());
            let tracks: Vec<MusicTrack> = list
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let uri = json_str(item, "/uri")?;
                    let track_id = uri.rsplit(':').next().unwrap_or(&uri).to_string();
                    let title = json_str(item, "/title")?;
                    let artists: Vec<String> = json_str(item, "/subtitle")
                        .map(|s| s.split(", ").map(str::to_string).collect())
                        .unwrap_or_default();
                    Some(MusicTrack {
                        id: uri,
                        title,
                        artists,
                        album: album_name.clone(),
                        album_artist: if kind == Kind::Album { owner.clone() } else { None },
                        track_number: (kind == Kind::Album).then_some(index as u32 + 1),
                        track_total: (kind == Kind::Album).then_some(total),
                        disc_number: None,
                        year: None,
                        date: None,
                        genre: None,
                        duration: item.get("duration").and_then(Value::as_f64).map(|ms| ms / 1000.0).filter(|s| *s > 0.0),
                        // Album tracks share the album's cover; playlist
                        // items carry none of their own in the embed.
                        cover_url: if kind == Kind::Album { cover.clone() } else { None },
                        isrc: None,
                        source_url: page_url(Kind::Track, &track_id),
                        service: Service::Spotify,
                        source: AudioSource::Match,
                        explicit: item.get("isExplicit").and_then(Value::as_bool).unwrap_or(false),
                        unavailable: unavailable_reason(item),
                    })
                })
                .collect();

            let source_kind = match kind {
                Kind::Album => SourceKind::Album,
                Kind::Playlist => SourceKind::Playlist,
                _ => SourceKind::Artist,
            };
            let note = match kind {
                Kind::Playlist if tracks.len() >= 100 => Some(
                    "Spotify shows a reader who is not signed in only the first 100 tracks of a playlist; the rest cannot be listed here"
                        .to_string(),
                ),
                Kind::Artist => Some("Spotify lists an artist's ten most popular tracks; albums are separate links".to_string()),
                _ => None,
            };
            MusicSource {
                kind: source_kind,
                service: Service::Spotify,
                title: name,
                owner,
                cover_url: cover,
                source_url,
                tracks,
                total: None,
                note,
            }
        }
    }
}

fn artist_names(entity: &Value) -> Vec<String> {
    entity
        .get("artists")
        .and_then(Value::as_array)
        .map(|artists| artists.iter().filter_map(|a| json_str(a, "/name")).collect())
        .unwrap_or_default()
}

fn unavailable_reason(item: &Value) -> Option<String> {
    let playable = item.get("isPlayable").and_then(Value::as_bool).unwrap_or(true);
    if playable {
        return None;
    }
    let reason = json_str(item, "/playabilityReason").unwrap_or_else(|| "UNAVAILABLE".into());
    Some(match reason.as_str() {
        "NOT_AVAILABLE_IN_REGION" | "COUNTRY_RESTRICTED" => "not available in your region on Spotify".into(),
        "UNKNOWN" | "UNAVAILABLE" => "Spotify marks this track unavailable".into(),
        other => format!("Spotify marks this track unavailable ({})", other.to_ascii_lowercase().replace('_', " ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_are_parsed_in_every_shape() {
        let id = "4uLU6hMCjMI75M1A2tKUQC";
        for url in [
            format!("https://open.spotify.com/track/{id}"),
            format!("https://open.spotify.com/track/{id}?si=abc"),
            format!("https://open.spotify.com/intl-fr/track/{id}"),
            format!("https://open.spotify.com/embed/track/{id}"),
            format!("spotify:track:{id}"),
        ] {
            assert_eq!(parse(&url), Some((Kind::Track, id.to_string())), "{url}");
        }
        assert_eq!(
            parse("https://open.spotify.com/user/spotify/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some((Kind::Playlist, "37i9dQZF1DXcBWIGoYBM5M".into()))
        );
        assert_eq!(parse("https://open.spotify.com/artist/0gxyHStUsqpMadRV0Di1Qt"), Some((Kind::Artist, "0gxyHStUsqpMadRV0Di1Qt".into())));
        assert_eq!(parse("https://open.spotify.com/"), None);
        assert_eq!(parse("https://open.spotify.com/track/short"), None);
    }

    #[test]
    fn a_track_entity_becomes_one_matchable_track() {
        let entity = json!({
            "type": "track", "name": "Never Gonna Give You Up", "id": "4uLU6hMCjMI75M1A2tKUQC",
            "artists": [{"name": "Rick Astley"}], "releaseDate": {"isoString": "1987-11-12T00:00:00Z"},
            "duration": 213573, "isPlayable": true, "isExplicit": false,
            "visualIdentity": {"image": [
                {"url": "https://img/300", "maxWidth": 300}, {"url": "https://img/640", "maxWidth": 640}, {"url": "https://img/64", "maxWidth": 64}
            ]}
        });
        let source = build(Kind::Track, "4uLU6hMCjMI75M1A2tKUQC", &entity);
        assert_eq!(source.kind, SourceKind::Track);
        assert_eq!(source.tracks.len(), 1);
        let t = &source.tracks[0];
        assert_eq!(t.artists, vec!["Rick Astley"]);
        assert_eq!(t.year, Some(1987));
        assert_eq!(t.date.as_deref(), Some("1987-11-12"));
        assert_eq!(t.duration, Some(213.573));
        assert_eq!(t.cover_url.as_deref(), Some("https://img/640"), "the largest image wins");
        assert_eq!(t.source, AudioSource::Match);
        assert_eq!(t.source_url, "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC");
    }

    #[test]
    fn an_album_numbers_its_tracks_and_a_playlist_does_not() {
        let list = json!([
            {"uri": "spotify:track:6OmhkSOpvYBokMKQxpIGx2", "title": "Global Warming", "subtitle": "Pitbull, Sensato", "duration": 85400, "isExplicit": true},
            {"uri": "spotify:track:2iblMMIgSznA464mNov7A8", "title": "Don't Stop the Party", "subtitle": "Pitbull, TJR", "duration": 206120, "isPlayable": false, "playabilityReason": "NOT_AVAILABLE_IN_REGION"}
        ]);
        let album = json!({"name": "Global Warming", "subtitle": "Pitbull", "trackList": list, "coverArt": {"sources": [{"url": "https://img/c"}]}});
        let source = build(Kind::Album, "4aawyAB9vmqN3uQ7FjRGTy", &album);
        assert_eq!(source.kind, SourceKind::Album);
        assert_eq!(source.owner.as_deref(), Some("Pitbull"));
        assert_eq!(source.tracks[0].track_number, Some(1));
        assert_eq!(source.tracks[1].track_number, Some(2));
        assert_eq!(source.tracks[0].track_total, Some(2));
        assert_eq!(source.tracks[0].album.as_deref(), Some("Global Warming"));
        assert_eq!(source.tracks[0].artists, vec!["Pitbull", "Sensato"]);
        assert!(source.tracks[0].explicit);
        assert_eq!(source.tracks[0].cover_url.as_deref(), Some("https://img/c"));
        assert!(source.tracks[1].unavailable.as_deref().unwrap().contains("region"));

        let playlist = json!({"name": "Mix", "subtitle": "someone", "trackList": list});
        let source = build(Kind::Playlist, "37i9dQZF1DXcBWIGoYBM5M", &playlist);
        assert_eq!(source.kind, SourceKind::Playlist);
        assert_eq!(source.tracks[0].track_number, None);
        assert_eq!(source.tracks[0].album, None);
        assert!(source.note.is_none(), "a short playlist is complete");
    }

    #[test]
    fn a_hundred_track_playlist_is_flagged_as_possibly_truncated() {
        let list: Vec<Value> = (0..100)
            .map(|i| json!({"uri": format!("spotify:track:{i:022}"), "title": format!("T{i}"), "subtitle": "A", "duration": 1000}))
            .collect();
        let playlist = json!({"name": "Big", "trackList": list});
        let source = build(Kind::Playlist, "37i9dQZF1DXcBWIGoYBM5M", &playlist);
        assert_eq!(source.tracks.len(), 100);
        assert!(source.note.as_deref().unwrap().contains("100"));
    }
}
