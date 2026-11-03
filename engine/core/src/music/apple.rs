//! Apple Music: songs, albums, artists and playlists.
//!
//! Two sources, because Apple offers two:
//!
//! - The **iTunes Lookup API** (`itunes.apple.com/lookup`) is public,
//!   documented, and answers for any song, album or artist by its store id —
//!   with track numbers, disc numbers, release dates, genre and a cover whose
//!   size is a URL parameter. It is the better source wherever it applies.
//! - **Playlists** are not in that API. The playlist page, however, is
//!   server-rendered and carries its data in a `serialized-server-data`
//!   script: title, curator, cover, and every track with its artist, album and
//!   duration. Used only for playlists.
//!
//! Every track resolves to [`AudioSource::Match`]; Apple's audio is DRM-only.

use reqwest::Client;
use serde_json::Value;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, get_html, get_json, json_str, script_json, year_of};
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    Song { storefront: String, id: String },
    Album { storefront: String, id: String },
    Artist { storefront: String, id: String },
    Playlist { storefront: String, id: String, url: String },
}

/// `music.apple.com/{storefront}/{kind}/{slug}/{id}[?i={trackId}]`.
fn parse(url: &str) -> Option<Link> {
    let url = url.trim();
    let without_scheme = url.trim_start_matches("https://").trim_start_matches("http://");
    let (host, rest) = without_scheme.split_once('/')?;
    let host = host.to_ascii_lowercase();
    if !(host.ends_with("music.apple.com") || host.ends_with("itunes.apple.com")) {
        return None;
    }
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.split('#').next().unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // The storefront is the first segment when it is a two-letter code; some
    // links omit it.
    let (storefront, segments) = match segments.first() {
        Some(first) if first.len() == 2 && first.bytes().all(|b| b.is_ascii_alphabetic()) => {
            (first.to_ascii_lowercase(), &segments[1..])
        }
        _ => ("us".to_string(), &segments[..]),
    };
    let kind = *segments.first()?;
    let id = segments.last()?.to_string();

    let track_param = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("i="))
        .map(|v| v.split(['&', '#']).next().unwrap_or(v).to_string())
        .filter(|v| v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty());

    match kind {
        "album" => match track_param {
            Some(track) => Some(Link::Song { storefront, id: track }),
            None => numeric(&id).then(|| Link::Album { storefront, id }),
        },
        "song" => numeric(&id).then(|| Link::Song { storefront, id }),
        "artist" => numeric(&id).then(|| Link::Artist { storefront, id }),
        "playlist" => id.starts_with("pl.").then(|| Link::Playlist { storefront, id, url: url.to_string() }),
        _ => None,
    }
}

fn numeric(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// The lookup API's 100px cover, asked for at a size worth embedding.
fn upsize_artwork(url: &str) -> String {
    match url.rfind('/') {
        Some(i) if url[i..].contains("x") => format!("{}/1400x1400bb.jpg", &url[..i]),
        _ => url.to_string(),
    }
}

/// The page's `{w}x{h}bb.{f}` template filled in.
fn fill_artwork_template(url: &str) -> String {
    url.replace("{w}", "1400").replace("{h}", "1400").replace("{f}", "jpg")
}

pub async fn analyze(client: &Client, url: &str) -> Result<MusicSource> {
    let link = parse(url).ok_or_else(|| {
        Error::InvalidInput("this Apple Music link does not point at a song, album, artist or playlist".into())
    })?;
    match link {
        Link::Song { storefront, id } => lookup(client, &storefront, &id, None, SourceKind::Track).await,
        Link::Album { storefront, id } => lookup(client, &storefront, &id, Some("song"), SourceKind::Album).await,
        Link::Artist { storefront, id } => lookup(client, &storefront, &id, Some("song"), SourceKind::Artist).await,
        Link::Playlist { url, .. } => playlist(client, &url).await,
    }
}

async fn lookup(client: &Client, storefront: &str, id: &str, entity: Option<&str>, kind: SourceKind) -> Result<MusicSource> {
    let mut api = format!("https://itunes.apple.com/lookup?id={id}&country={storefront}&limit=200");
    if let Some(entity) = entity {
        api.push_str("&entity=");
        api.push_str(entity);
    }
    let body = get_json(client, &api).await?;
    let results = body.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
    if results.is_empty() {
        return Err(Error::InvalidInput(
            "Apple Music has nothing at this link in the store it names — it may have been removed, or be exclusive to another country's store".into(),
        ));
    }
    Ok(from_lookup(&results, kind, storefront, id))
}

fn from_lookup(results: &[Value], kind: SourceKind, storefront: &str, id: &str) -> MusicSource {
    let header = results
        .iter()
        .find(|r| matches!(json_str(r, "/wrapperType").as_deref(), Some("collection") | Some("artist")));
    let songs: Vec<&Value> = results.iter().filter(|r| json_str(r, "/kind").as_deref() == Some("song")).collect();

    let mut tracks: Vec<MusicTrack> = songs.iter().map(|song| song_from_lookup(song)).collect();
    if kind == SourceKind::Artist {
        // The lookup lists an artist's songs in no particular order; the
        // popular ones are what the user came for, and there is no popularity
        // in the answer — so keep the store's order but name the limit.
        tracks.truncate(50);
    }

    let (title, owner, cover) = match kind {
        SourceKind::Track => {
            let t = tracks.first();
            (
                t.map(|t| t.title.clone()).unwrap_or_else(|| id.to_string()),
                t.and_then(|t| t.primary_artist().map(str::to_string)),
                t.and_then(|t| t.cover_url.clone()),
            )
        }
        _ => (
            header
                .and_then(|h| json_str(h, "/collectionName").or_else(|| json_str(h, "/artistName")))
                .or_else(|| tracks.first().and_then(|t| t.album.clone()))
                .unwrap_or_else(|| id.to_string()),
            header.and_then(|h| json_str(h, "/artistName")),
            header
                .and_then(|h| json_str(h, "/artworkUrl100"))
                .map(|u| upsize_artwork(&u))
                .or_else(|| tracks.first().and_then(|t| t.cover_url.clone())),
        ),
    };

    let source_url = match kind {
        SourceKind::Track => tracks
            .first()
            .map(|t| t.source_url.clone())
            .unwrap_or_else(|| format!("https://music.apple.com/{storefront}/song/{id}")),
        SourceKind::Artist => header
            .and_then(|h| json_str(h, "/artistLinkUrl"))
            .unwrap_or_else(|| format!("https://music.apple.com/{storefront}/artist/{id}")),
        _ => header
            .and_then(|h| json_str(h, "/collectionViewUrl"))
            .unwrap_or_else(|| format!("https://music.apple.com/{storefront}/album/{id}")),
    };

    MusicSource {
        kind,
        service: Service::AppleMusic,
        title,
        owner,
        cover_url: cover,
        source_url: strip_tracking(&source_url),
        total: header.and_then(|h| h.get("trackCount").and_then(Value::as_u64)).map(|n| n as u32),
        note: (kind == SourceKind::Artist).then(|| "Apple Music lists an artist's catalogue in store order, up to fifty songs".to_string()),
        tracks,
    }
}

fn song_from_lookup(song: &Value) -> MusicTrack {
    let release = json_str(song, "/releaseDate");
    let artists: Vec<String> = json_str(song, "/artistName")
        .map(|a| split_artists(&a))
        .unwrap_or_default();
    let id = song.get("trackId").and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default();
    MusicTrack {
        id: format!("apple:{id}"),
        title: json_str(song, "/trackName").unwrap_or_else(|| "Untitled".into()),
        artists,
        album: json_str(song, "/collectionName"),
        album_artist: None,
        track_number: song.get("trackNumber").and_then(Value::as_u64).map(|n| n as u32),
        track_total: song.get("trackCount").and_then(Value::as_u64).map(|n| n as u32),
        disc_number: song.get("discNumber").and_then(Value::as_u64).map(|n| n as u32),
        year: release.as_deref().and_then(year_of),
        date: release.as_deref().map(|d| d.chars().take(10).collect()),
        genre: json_str(song, "/primaryGenreName"),
        duration: song.get("trackTimeMillis").and_then(Value::as_f64).map(|ms| ms / 1000.0).filter(|s| *s > 0.0),
        cover_url: json_str(song, "/artworkUrl100").map(|u| upsize_artwork(&u)),
        isrc: None,
        source_url: json_str(song, "/trackViewUrl").map(|u| strip_tracking(&u)).unwrap_or_else(|| format!("https://music.apple.com/song/{id}")),
        service: Service::AppleMusic,
        source: AudioSource::Match,
        explicit: json_str(song, "/trackExplicitness").as_deref() == Some("explicit"),
        unavailable: None,
    }
}

/// `Daft Punk & Pharrell Williams` → two artists. The store joins credited
/// artists with ` & ` and `, `; a band whose *name* contains an ampersand
/// (`Simon & Garfunkel`) is split too, which is the lesser evil — tags carry
/// the joined line either way, and only matching uses the parts.
fn split_artists(line: &str) -> Vec<String> {
    line.split(&[',', '&'][..])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn strip_tracking(url: &str) -> String {
    url.split("&uo=").next().unwrap_or(url).replace("?uo=4", "")
}

async fn playlist(client: &Client, url: &str) -> Result<MusicSource> {
    let html = get_html(client, url).await?;
    let data = script_json(&html, "serialized-server-data")
        .ok_or_else(|| Error::InvalidInput("Apple Music's playlist page did not carry its usual data; the playlist may be private".into()))?;
    from_playlist_page(&data, url)
}

fn from_playlist_page(data: &Value, url: &str) -> Result<MusicSource> {
    let inner = data
        .pointer("/data/0/data")
        .cloned()
        .ok_or_else(|| Error::InvalidInput("Apple Music's playlist page has an unexpected shape".into()))?;
    let sections = inner.get("sections").and_then(Value::as_array).cloned().unwrap_or_default();
    let section_of = |kind: &str| sections.iter().find(|s| json_str(s, "/itemKind").as_deref() == Some(kind));

    let header = section_of("containerDetailHeaderLockup").and_then(|s| s.pointer("/items/0")).cloned().unwrap_or(Value::Null);
    let title = json_str(&header, "/title")
        .or_else(|| json_str(&inner, "/seoData/pageTitle").map(|t| t.split(" - ").next().unwrap_or(&t).to_string()))
        .unwrap_or_else(|| "Apple Music playlist".into());
    let owner = json_str(&header, "/subtitleLinks/0/title");
    let cover = json_str(&header, "/artwork/dictionary/url").map(|u| fill_artwork_template(&u));
    let track_count = header.get("trackCount").and_then(Value::as_u64).map(|n| n as u32);

    let items = section_of("trackLockup").and_then(|s| s.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();
    let tracks: Vec<MusicTrack> = items
        .iter()
        .filter_map(|item| {
            let title = json_str(item, "/title")?;
            let id = json_str(item, "/contentDescriptor/identifiers/storeAdamID").unwrap_or_default();
            let artists = json_str(item, "/artistName")
                .map(|a| split_artists(&a))
                .or_else(|| {
                    item.get("subtitleLinks")
                        .and_then(Value::as_array)
                        .map(|links| links.iter().filter_map(|l| json_str(l, "/title")).collect())
                })
                .unwrap_or_default();
            Some(MusicTrack {
                id: format!("apple:{id}"),
                title,
                artists,
                album: json_str(item, "/tertiaryLinks/0/title"),
                album_artist: None,
                track_number: None,
                track_total: None,
                disc_number: None,
                year: None,
                date: None,
                genre: None,
                duration: item.get("duration").and_then(Value::as_f64).map(|ms| ms / 1000.0).filter(|s| *s > 0.0),
                cover_url: json_str(item, "/artwork/dictionary/url").map(|u| fill_artwork_template(&u)),
                isrc: None,
                source_url: json_str(item, "/contentDescriptor/url").unwrap_or_else(|| url.to_string()),
                service: Service::AppleMusic,
                source: AudioSource::Match,
                explicit: item.get("showExplicitBadge").and_then(Value::as_bool).unwrap_or(false),
                unavailable: None,
            })
        })
        .collect();

    if tracks.is_empty() {
        return Err(Error::InvalidInput("no tracks could be read from this Apple Music playlist".into()));
    }
    let listed = tracks.len() as u32;
    Ok(MusicSource {
        kind: SourceKind::Playlist,
        service: Service::AppleMusic,
        title,
        owner,
        cover_url: cover,
        source_url: url.to_string(),
        total: track_count,
        note: track_count.filter(|n| *n > listed).map(|n| format!("Apple Music's page lists {listed} of the playlist's {n} tracks")),
        tracks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_are_parsed_with_and_without_a_storefront() {
        assert_eq!(
            parse("https://music.apple.com/us/album/better-together/1440857781?i=1440857786&uo=4"),
            Some(Link::Song { storefront: "us".into(), id: "1440857786".into() })
        );
        assert_eq!(
            parse("https://music.apple.com/gb/album/random-access-memories/617154241"),
            Some(Link::Album { storefront: "gb".into(), id: "617154241".into() })
        );
        assert_eq!(parse("https://music.apple.com/album/x/617154241"), Some(Link::Album { storefront: "us".into(), id: "617154241".into() }));
        assert_eq!(parse("https://music.apple.com/us/song/get-lucky/617154421"), Some(Link::Song { storefront: "us".into(), id: "617154421".into() }));
        assert_eq!(parse("https://music.apple.com/us/artist/daft-punk/5468295"), Some(Link::Artist { storefront: "us".into(), id: "5468295".into() }));
        assert!(matches!(parse("https://music.apple.com/us/playlist/todays-hits/pl.f4d106fed2bd41149aaacabb233eb5eb"), Some(Link::Playlist { .. })));
        assert_eq!(parse("https://music.apple.com/us/music-video/x/123"), None);
        assert_eq!(parse("https://example.com/us/album/x/1"), None);
    }

    #[test]
    fn artwork_is_asked_for_at_embed_size() {
        assert_eq!(
            upsize_artwork("https://is1-ssl.mzstatic.com/image/thumb/Music115/v4/a/b.jpg/100x100bb.jpg"),
            "https://is1-ssl.mzstatic.com/image/thumb/Music115/v4/a/b.jpg/1400x1400bb.jpg"
        );
        assert_eq!(fill_artwork_template("https://x/{w}x{h}bb.{f}"), "https://x/1400x1400bb.jpg");
    }

    #[test]
    fn an_album_lookup_yields_numbered_tagged_tracks() {
        let results = vec![
            json!({"wrapperType": "collection", "collectionType": "Album", "collectionName": "In Between Dreams", "artistName": "Jack Johnson",
                   "artworkUrl100": "https://a/b.jpg/100x100bb.jpg", "trackCount": 2, "collectionViewUrl": "https://music.apple.com/us/album/x/1?uo=4"}),
            json!({"wrapperType": "track", "kind": "song", "trackId": 11, "trackName": "Better Together", "artistName": "Jack Johnson", "collectionName": "In Between Dreams",
                   "trackNumber": 1, "trackCount": 2, "discNumber": 1, "trackTimeMillis": 207679, "releaseDate": "2005-01-01T12:00:00Z", "primaryGenreName": "Rock",
                   "artworkUrl100": "https://a/b.jpg/100x100bb.jpg", "trackViewUrl": "https://music.apple.com/us/album/x/1?i=11&uo=4", "trackExplicitness": "notExplicit"}),
            json!({"wrapperType": "track", "kind": "song", "trackId": 12, "trackName": "Never Know", "artistName": "Jack Johnson & Friends", "collectionName": "In Between Dreams",
                   "trackNumber": 2, "trackCount": 2, "trackTimeMillis": 100000, "releaseDate": "2005-01-01T12:00:00Z", "trackExplicitness": "explicit"}),
        ];
        let source = from_lookup(&results, SourceKind::Album, "us", "1");
        assert_eq!(source.title, "In Between Dreams");
        assert_eq!(source.owner.as_deref(), Some("Jack Johnson"));
        assert_eq!(source.total, Some(2));
        assert_eq!(source.source_url, "https://music.apple.com/us/album/x/1");
        assert_eq!(source.cover_url.as_deref(), Some("https://a/b.jpg/1400x1400bb.jpg"));
        let t = &source.tracks[0];
        assert_eq!(t.track_number, Some(1));
        assert_eq!(t.year, Some(2005));
        assert_eq!(t.genre.as_deref(), Some("Rock"));
        assert_eq!(t.duration, Some(207.679));
        assert_eq!(t.source_url, "https://music.apple.com/us/album/x/1?i=11");
        assert_eq!(source.tracks[1].artists, vec!["Jack Johnson", "Friends"]);
        assert!(source.tracks[1].explicit);
    }

    #[test]
    fn a_single_song_lookup_is_a_track_source() {
        let results = vec![json!({"wrapperType": "track", "kind": "song", "trackId": 5, "trackName": "Song", "artistName": "Band", "collectionName": "LP", "trackTimeMillis": 1000})];
        let source = from_lookup(&results, SourceKind::Track, "us", "5");
        assert_eq!(source.kind, SourceKind::Track);
        assert_eq!(source.title, "Song");
        assert_eq!(source.owner.as_deref(), Some("Band"));
    }

    #[test]
    fn a_playlist_page_is_read_from_its_serialized_data() {
        let data = json!({"data": [{"data": {
            "seoData": {"pageTitle": "Today’s Hits - Playlist - Apple Music"},
            "sections": [
                {"itemKind": "containerDetailHeaderLockup", "items": [{"title": "Today’s Hits", "subtitleLinks": [{"title": "Apple Music Hits"}],
                    "artwork": {"dictionary": {"url": "https://art/{w}x{h}bb.{f}"}}, "trackCount": 3}]},
                {"itemKind": "trackLockup", "items": [
                    {"title": "stupid song", "artistName": "Olivia Rodrigo", "duration": 209680, "tertiaryLinks": [{"title": "some album"}],
                     "contentDescriptor": {"identifiers": {"storeAdamID": "1889992115"}, "url": "https://music.apple.com/us/album/s/1?i=1889992115"},
                     "artwork": {"dictionary": {"url": "https://art2/{w}x{h}bb.{f}"}}, "showExplicitBadge": true},
                    {"title": "other", "subtitleLinks": [{"title": "A"}, {"title": "B"}], "duration": 1000}
                ]}
            ]
        }}]});
        let source = from_playlist_page(&data, "https://music.apple.com/us/playlist/x/pl.1").unwrap();
        assert_eq!(source.title, "Today’s Hits");
        assert_eq!(source.owner.as_deref(), Some("Apple Music Hits"));
        assert_eq!(source.cover_url.as_deref(), Some("https://art/1400x1400bb.jpg"));
        assert_eq!(source.tracks.len(), 2);
        assert_eq!(source.tracks[0].artists, vec!["Olivia Rodrigo"]);
        assert_eq!(source.tracks[0].album.as_deref(), Some("some album"));
        assert_eq!(source.tracks[0].duration, Some(209.68));
        assert!(source.tracks[0].explicit);
        assert_eq!(source.tracks[1].artists, vec!["A", "B"]);
        assert!(source.note.as_deref().unwrap().contains("2 of the playlist's 3"));
    }
}
