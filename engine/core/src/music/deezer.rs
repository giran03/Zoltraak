//! Deezer: tracks, albums, playlists and artists, from its public API.
//!
//! `api.deezer.com` answers catalogue reads without any credential — the
//! only one of the big three that does — and answers them well: ISRC, track
//! and disc numbers, release dates, genre, and a 1000px cover. Playlists
//! paginate with a `next` link, so a long one is walked to the end rather than
//! cut at the first page.
//!
//! Every track resolves to [`AudioSource::Match`]; Deezer's audio is DRM-only.

use reqwest::Client;
use serde_json::Value;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, get_json, json_str, resolve_redirects, year_of};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Track,
    Album,
    Playlist,
    Artist,
}

/// `deezer.com/[lang/]{track|album|playlist|artist}/{id}`.
fn parse(url: &str) -> Option<(Kind, u64)> {
    let path = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split_once('/')
        .map(|(_, p)| p)?;
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    while i + 1 < segments.len() {
        let kind = match segments[i] {
            "track" => Some(Kind::Track),
            "album" => Some(Kind::Album),
            "playlist" => Some(Kind::Playlist),
            "artist" => Some(Kind::Artist),
            _ => None,
        };
        if let Some(kind) = kind {
            if let Ok(id) = segments[i + 1].parse::<u64>() {
                return Some((kind, id));
            }
        }
        i += 1;
    }
    None
}

/// Playlists longer than this are cut, with a note. Nobody is well served by
/// a ten-thousand-track queue created by one paste.
const MAX_TRACKS: usize = 2000;

pub async fn analyze(client: &Client, url: &str) -> Result<MusicSource> {
    let mut url = url.trim().to_string();
    let host = super::host_of(&url);
    if host.ends_with("deezer.page.link") || host.ends_with("link.deezer.com") {
        url = resolve_redirects(client, &url).await?;
    }
    let (kind, id) = parse(&url)
        .ok_or_else(|| Error::InvalidInput("this Deezer link does not point at a track, album, playlist or artist".into()))?;

    match kind {
        Kind::Track => {
            let track = api(client, &format!("https://api.deezer.com/track/{id}")).await?;
            let t = track_of(&track, None, None);
            Ok(MusicSource {
                kind: SourceKind::Track,
                service: Service::Deezer,
                title: t.title.clone(),
                owner: t.primary_artist().map(str::to_string),
                cover_url: t.cover_url.clone(),
                source_url: t.source_url.clone(),
                tracks: vec![t],
                total: None,
                note: None,
            })
        }
        Kind::Album => {
            let album = api(client, &format!("https://api.deezer.com/album/{id}")).await?;
            let items = paged(client, &format!("https://api.deezer.com/album/{id}/tracks?limit=200")).await?;
            Ok(album_source(&album, &items, id))
        }
        Kind::Playlist => {
            let playlist = api(client, &format!("https://api.deezer.com/playlist/{id}")).await?;
            let items = paged(client, &format!("https://api.deezer.com/playlist/{id}/tracks?limit=200")).await?;
            Ok(playlist_source(&playlist, &items, id))
        }
        Kind::Artist => {
            let artist = api(client, &format!("https://api.deezer.com/artist/{id}")).await?;
            let items = paged(client, &format!("https://api.deezer.com/artist/{id}/top?limit=50")).await?;
            let name = json_str(&artist, "/name").unwrap_or_else(|| id.to_string());
            let tracks: Vec<MusicTrack> = items.iter().map(|t| track_of(t, None, None)).collect();
            Ok(MusicSource {
                kind: SourceKind::Artist,
                service: Service::Deezer,
                title: name.clone(),
                owner: Some(name),
                cover_url: json_str(&artist, "/picture_xl"),
                source_url: json_str(&artist, "/link").unwrap_or_else(|| format!("https://www.deezer.com/artist/{id}")),
                tracks,
                total: None,
                note: Some("Deezer lists an artist's fifty most popular tracks".into()),
            })
        }
    }
}

/// One API object, with Deezer's in-band error turned into ours.
async fn api(client: &Client, url: &str) -> Result<Value> {
    let body = get_json(client, url).await?;
    if let Some(error) = body.get("error") {
        let message = json_str(error, "/message").unwrap_or_else(|| "unknown error".into());
        return Err(Error::InvalidInput(format!("Deezer: {message}")));
    }
    Ok(body)
}

/// Every item of a paged list, following `next` until it ends.
async fn paged(client: &Client, first: &str) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut next = Some(first.to_string());
    while let Some(url) = next.take() {
        let page = api(client, &url).await?;
        if let Some(items) = page.get("data").and_then(Value::as_array) {
            out.extend(items.iter().cloned());
        }
        if out.len() >= MAX_TRACKS {
            break;
        }
        next = json_str(&page, "/next");
    }
    Ok(out)
}

fn album_source(album: &Value, items: &[Value], id: u64) -> MusicSource {
    let title = json_str(album, "/title").unwrap_or_else(|| id.to_string());
    let artist = json_str(album, "/artist/name");
    let cover = best_cover(album, "cover");
    let date = json_str(album, "/release_date");
    let genre = json_str(album, "/genres/data/0/name");
    let total = album.get("nb_tracks").and_then(Value::as_u64).map(|n| n as u32).or(Some(items.len() as u32));
    let context = AlbumContext {
        title: title.clone(),
        artist: artist.clone(),
        cover: cover.clone(),
        date,
        genre,
        total,
    };
    let tracks = items.iter().map(|t| track_of(t, Some(&context), None)).collect();
    MusicSource {
        kind: SourceKind::Album,
        service: Service::Deezer,
        title,
        owner: artist,
        cover_url: cover,
        source_url: json_str(album, "/link").unwrap_or_else(|| format!("https://www.deezer.com/album/{id}")),
        tracks,
        total,
        note: None,
    }
}

fn playlist_source(playlist: &Value, items: &[Value], id: u64) -> MusicSource {
    let listed = items.len();
    let tracks = items.iter().map(|t| track_of(t, None, None)).collect();
    let total = playlist.get("nb_tracks").and_then(Value::as_u64).map(|n| n as u32);
    MusicSource {
        kind: SourceKind::Playlist,
        service: Service::Deezer,
        title: json_str(playlist, "/title").unwrap_or_else(|| id.to_string()),
        owner: json_str(playlist, "/creator/name"),
        cover_url: best_cover(playlist, "picture"),
        source_url: json_str(playlist, "/link").unwrap_or_else(|| format!("https://www.deezer.com/playlist/{id}")),
        tracks,
        total,
        note: (listed >= MAX_TRACKS).then(|| format!("only the first {MAX_TRACKS} tracks were listed")),
    }
}

/// What an album's tracks inherit, since Deezer's album track list does not
/// repeat it per track.
struct AlbumContext {
    title: String,
    artist: Option<String>,
    cover: Option<String>,
    date: Option<String>,
    genre: Option<String>,
    total: Option<u32>,
}

fn best_cover(v: &Value, field: &str) -> Option<String> {
    json_str(v, &format!("/{field}_xl"))
        .or_else(|| json_str(v, &format!("/{field}_big")))
        .or_else(|| json_str(v, &format!("/{field}")))
}

fn track_of(t: &Value, album: Option<&AlbumContext>, _position: Option<u32>) -> MusicTrack {
    let id = t.get("id").and_then(Value::as_u64).unwrap_or(0);
    // Contributors carry every credited artist; `artist` only the main one.
    let mut artists: Vec<String> = t
        .get("contributors")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(|a| json_str(a, "/name")).collect())
        .unwrap_or_default();
    if artists.is_empty() {
        artists.extend(json_str(t, "/artist/name"));
    }
    let date = json_str(t, "/release_date").or_else(|| album.and_then(|a| a.date.clone()));
    MusicTrack {
        id: format!("deezer:{id}"),
        title: json_str(t, "/title").unwrap_or_else(|| "Untitled".into()),
        artists,
        album: json_str(t, "/album/title").or_else(|| album.map(|a| a.title.clone())),
        album_artist: album.and_then(|a| a.artist.clone()),
        track_number: t.get("track_position").and_then(Value::as_u64).map(|n| n as u32),
        track_total: album.and_then(|a| a.total),
        disc_number: t.get("disk_number").and_then(Value::as_u64).map(|n| n as u32),
        year: date.as_deref().and_then(year_of),
        date,
        genre: album.and_then(|a| a.genre.clone()),
        duration: t.get("duration").and_then(Value::as_f64).filter(|s| *s > 0.0),
        cover_url: best_cover(t.get("album").unwrap_or(&Value::Null), "cover").or_else(|| album.and_then(|a| a.cover.clone())),
        isrc: json_str(t, "/isrc"),
        source_url: json_str(t, "/link").unwrap_or_else(|| format!("https://www.deezer.com/track/{id}")),
        service: Service::Deezer,
        source: AudioSource::Match,
        explicit: t.get("explicit_lyrics").and_then(Value::as_bool).unwrap_or(false),
        unavailable: match t.get("readable").and_then(Value::as_bool) {
            Some(false) => Some("Deezer marks this track unavailable in your region".into()),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_are_parsed_with_an_optional_language_prefix() {
        assert_eq!(parse("https://www.deezer.com/track/3135556"), Some((Kind::Track, 3135556)));
        assert_eq!(parse("https://www.deezer.com/en/album/302127?x=1"), Some((Kind::Album, 302127)));
        assert_eq!(parse("https://deezer.com/fr/playlist/908622995"), Some((Kind::Playlist, 908622995)));
        assert_eq!(parse("https://www.deezer.com/us/artist/27"), Some((Kind::Artist, 27)));
        assert_eq!(parse("https://www.deezer.com/us/"), None);
    }

    #[test]
    fn a_track_carries_everything_the_api_says() {
        let t = json!({
            "id": 3135556, "title": "Harder, Better, Faster, Stronger", "isrc": "GBDUW0000059", "link": "https://www.deezer.com/track/3135556",
            "duration": 226, "track_position": 4, "disk_number": 1, "release_date": "2001-03-12", "explicit_lyrics": false, "readable": true,
            "contributors": [{"name": "Daft Punk"}], "artist": {"name": "Daft Punk"},
            "album": {"title": "Discovery", "cover_xl": "https://img/1000", "cover_big": "https://img/500"}
        });
        let track = track_of(&t, None, None);
        assert_eq!(track.artists, vec!["Daft Punk"]);
        assert_eq!(track.album.as_deref(), Some("Discovery"));
        assert_eq!(track.track_number, Some(4));
        assert_eq!(track.year, Some(2001));
        assert_eq!(track.isrc.as_deref(), Some("GBDUW0000059"));
        assert_eq!(track.cover_url.as_deref(), Some("https://img/1000"));
        assert_eq!(track.duration, Some(226.0));
        assert!(track.unavailable.is_none());
    }

    #[test]
    fn album_tracks_inherit_what_the_list_leaves_out() {
        let album = json!({"title": "Discovery", "artist": {"name": "Daft Punk"}, "cover_xl": "https://img/xl", "release_date": "2001-03-07",
                           "genres": {"data": [{"name": "Electro"}]}, "nb_tracks": 2, "link": "https://www.deezer.com/album/302127"});
        let items = vec![
            json!({"id": 1, "title": "One More Time", "duration": 320, "track_position": 1, "disk_number": 1, "artist": {"name": "Daft Punk"}}),
            json!({"id": 2, "title": "Aerodynamic", "duration": 212, "track_position": 2, "disk_number": 1, "artist": {"name": "Daft Punk"}, "readable": false}),
        ];
        let source = album_source(&album, &items, 302127);
        assert_eq!(source.kind, SourceKind::Album);
        assert_eq!(source.total, Some(2));
        let t = &source.tracks[0];
        assert_eq!(t.album.as_deref(), Some("Discovery"));
        assert_eq!(t.album_artist.as_deref(), Some("Daft Punk"));
        assert_eq!(t.genre.as_deref(), Some("Electro"));
        assert_eq!(t.year, Some(2001));
        assert_eq!(t.cover_url.as_deref(), Some("https://img/xl"));
        assert_eq!(t.track_total, Some(2));
        assert!(source.tracks[1].unavailable.is_some());
    }

    #[test]
    fn a_playlist_keeps_each_tracks_own_album() {
        let playlist = json!({"title": "Mix", "creator": {"name": "Alex"}, "picture_xl": "https://p/xl", "nb_tracks": 1});
        let items = vec![json!({"id": 9, "title": "Hey Jude", "duration": 429, "artist": {"name": "The Beatles"}, "album": {"title": "1", "cover_big": "https://c/big"}})];
        let source = playlist_source(&playlist, &items, 1);
        assert_eq!(source.owner.as_deref(), Some("Alex"));
        assert_eq!(source.tracks[0].album.as_deref(), Some("1"));
        assert_eq!(source.tracks[0].cover_url.as_deref(), Some("https://c/big"));
        assert_eq!(source.tracks[0].track_number, None);
    }
}
