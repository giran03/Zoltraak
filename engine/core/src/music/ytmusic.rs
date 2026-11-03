//! YouTube and YouTube Music: playlists, albums, single videos, search — and
//! the matcher that finds a catalogue track's recording on YouTube.
//!
//! Everything here talks to InnerTube, YouTube's own client API, as one of
//! two web clients: `WEB` (www.youtube.com) for playlists, and `WEB_REMIX`
//! (music.youtube.com) for albums and for search. YouTube Music's search is
//! the one that matters for matching: filtered to *songs*, its results are
//! recordings — the album version, from the label's channel — rather than
//! the lyric videos, live takes and reuploads a general video search ranks
//! first. Plain YouTube search is the fallback for what YouTube Music does
//! not have.
//!
//! The audio itself is fetched by the existing YouTube extractor
//! ([`crate::video::youtube`]) at download time; nothing here touches
//! streams. What this module resolves is *which video*.

use std::collections::HashSet;

use reqwest::Client;
use serde_json::{Value, json};

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, parse_clock, year_of};
use crate::error::{Error, Result};
use crate::headers::Headers;
use crate::video::youtube::formats::query_param;
use crate::video::youtube::{self, innertube};

/// A playlist longer than this is cut, with a note. One paste should not be
/// able to queue a channel's entire upload history.
const MAX_PLAYLIST_ITEMS: usize = 5000;

// ─── InnerTube plumbing ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Web {
    /// www.youtube.com
    Video,
    /// music.youtube.com
    Music,
}

impl Web {
    fn origin(self) -> &'static str {
        match self {
            Self::Video => "https://www.youtube.com",
            Self::Music => "https://music.youtube.com",
        }
    }

    fn client(self) -> (&'static str, &'static str, u32) {
        match self {
            Self::Video => ("WEB", "2.20250101.00.00", 1),
            Self::Music => ("WEB_REMIX", "1.20250101.01.00", 67),
        }
    }
}

/// The cookie header in `headers`, if any. YouTube's own session lets a
/// private playlist be read; without one the request is anonymous.
fn cookie_of(headers: &Headers) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .map(|(_, v)| v.clone())
        .and_then(|c| youtube::minimize_cookie(&c))
}

/// `POST /youtubei/v1/{endpoint}` as a web client.
async fn innertube_post(client: &Client, web: Web, endpoint: &str, body: Value, cookie: Option<&str>) -> Result<Value> {
    let (name, version, id) = web.client();
    let mut payload = json!({
        "context": { "client": { "clientName": name, "clientVersion": version, "hl": "en", "gl": "US" } }
    });
    if let Value::Object(fields) = body {
        for (k, v) in fields {
            payload[k] = v;
        }
    }

    let url = format!("{}/youtubei/v1/{endpoint}?prettyPrint=false", web.origin());
    let mut request = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Origin", web.origin())
        .header("Referer", format!("{}/", web.origin()))
        .header("User-Agent", youtube::WEB_USER_AGENT)
        .header("X-YouTube-Client-Name", id.to_string())
        .header("X-YouTube-Client-Version", version);

    // The consent cookie keeps the EU interstitial out of the way; the
    // session cookie, with its proof-of-possession header, is what makes a
    // private playlist readable.
    let mut cookie_header = String::from("SOCS=CAI");
    if let Some(cookie) = cookie {
        cookie_header.push_str("; ");
        cookie_header.push_str(cookie);
        if let Some(auth) = innertube::sid_authorization(cookie, web.origin(), None, chrono::Utc::now().timestamp()) {
            request = request.header("Authorization", auth).header("X-Origin", web.origin());
        }
    }
    request = request.header("Cookie", cookie_header);

    let response = request.body(payload.to_string()).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus { status: status.as_u16(), url: format!("youtubei/v1/{endpoint} ({name})") });
    }
    let text = response.text().await?;
    serde_json::from_str(&text).map_err(|e| Error::InvalidInput(format!("YouTube's {endpoint} answer is not JSON: {e}")))
}

/// Every value under `key`, anywhere in the tree, in document order.
fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == key {
                    out.push(child);
                }
                find_all(child, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                find_all(item, key, out);
            }
        }
        _ => {}
    }
}

fn first<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut out = Vec::new();
    find_all(v, key, &mut out);
    out.into_iter().next()
}

fn runs_text(v: &Value) -> Option<String> {
    let runs = v.get("runs")?.as_array()?;
    let text: String = runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect();
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn text_of(v: &Value) -> Option<String> {
    v.get("simpleText")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| runs_text(v))
        .or_else(|| v.get("content").and_then(Value::as_str).map(str::to_string))
}

/// The best thumbnail of a `thumbnails` array: the last one is the largest.
fn largest_thumbnail(v: &Value) -> Option<String> {
    first(v, "thumbnails")
        .and_then(Value::as_array)
        .and_then(|t| t.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(upscale_thumbnail)
}

/// YouTube Music's square art is served at whatever size the URL asks for.
fn upscale_thumbnail(url: &str) -> String {
    if url.contains("googleusercontent.com") || url.contains("ggpht.com") {
        if let Some(i) = url.rfind("=w") {
            return format!("{}=w1200-h1200-l90-rj", &url[..i]);
        }
    }
    url.to_string()
}

fn video_thumbnail(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")
}

fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

// ─── Titles ──────────────────────────────────────────────────────────────────

/// Words in brackets that describe the upload, not the song.
const NOISE: &[&str] = &[
    "official video",
    "official music video",
    "official audio",
    "official lyric video",
    "official visualizer",
    "official",
    "music video",
    "lyric video",
    "lyrics video",
    "lyrics",
    "audio",
    "visualizer",
    "visualiser",
    "hd",
    "hq",
    "4k",
    "full hd",
    "remastered",
    "video oficial",
    "audio oficial",
    "clip officiel",
    "letra",
    "hd video",
    "official hd video",
];

/// Strip `(Official Video)`-style suffixes and surrounding whitespace.
pub fn clean_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest.as_bytes()[open] == b'(' { ')' } else { ']' };
        match rest[open + 1..].find(close_char) {
            Some(close) => {
                let inner = rest[open + 1..open + 1 + close].trim().to_ascii_lowercase();
                out.push_str(&rest[..open]);
                if !NOISE.contains(&inner.as_str()) {
                    out.push_str(&rest[open..open + close + 2]);
                }
                rest = &rest[open + close + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    // A trailing `| Official Video` or ` - Official Audio` is the same noise
    // without brackets.
    let lower = out.to_ascii_lowercase();
    for sep in [" | ", " - ", " – ", " — "] {
        if let Some(i) = lower.rfind(sep) {
            let tail = lower[i + sep.len()..].trim();
            if NOISE.contains(&tail) {
                out.truncate(i);
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Artist - Title` from a video's title and channel, the way a listener
/// would read it. A `- Topic` channel is YouTube's auto-generated artist
/// channel, and names the artist exactly.
pub fn split_video_title(title: &str, channel: Option<&str>) -> (Vec<String>, String) {
    let clean = clean_title(title);
    let topic_artist = channel.and_then(|c| c.strip_suffix(" - Topic")).map(|c| c.trim().to_string());

    let separators = [" - ", " – ", " — ", " | ", ": "];
    let split = separators.iter().find_map(|sep| clean.split_once(sep));

    match (topic_artist, split) {
        (Some(artist), Some((left, right))) if left.eq_ignore_ascii_case(&artist) => (vec![artist], right.trim().to_string()),
        (Some(artist), _) => (vec![artist], clean),
        (None, Some((left, right))) if !left.trim().is_empty() && !right.trim().is_empty() => {
            let artists = left.split(&[',', '&'][..]).map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
            (artists, right.trim().to_string())
        }
        (None, _) => (channel.map(|c| vec![c.trim().to_string()]).unwrap_or_default(), clean),
    }
}

// ─── Single video ────────────────────────────────────────────────────────────

async fn video_track(client: &Client, id: &str, cookie: Option<&str>) -> Result<MusicTrack> {
    // One `player` call as an app client: no streams needed here, only
    // `videoDetails`, which every client answers with even for a video it
    // will not serve.
    let ctx = innertube::RequestContext { video_id: id.to_string(), cookie: cookie.map(str::to_string), ..Default::default() };
    let details = match innertube::player_response(client, innertube::ClientKind::AndroidVr, &ctx).await {
        Ok(response) => response.get("videoDetails").cloned().unwrap_or(Value::Null),
        Err(e) => {
            tracing::debug!(error = %e, "YouTube player details unavailable; using oEmbed");
            Value::Null
        }
    };

    let mut title = details.get("title").and_then(Value::as_str).map(str::to_string);
    let mut channel = details.get("author").and_then(Value::as_str).map(str::to_string);
    let duration = details.get("lengthSeconds").and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).filter(|d| *d > 0.0);
    if title.is_none() {
        let oembed = format!("https://www.youtube.com/oembed?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3D{id}&format=json");
        if let Ok(v) = super::get_json(client, &oembed).await {
            title = v.get("title").and_then(Value::as_str).map(str::to_string);
            channel = channel.or_else(|| v.get("author_name").and_then(Value::as_str).map(str::to_string));
        }
    }
    let raw_title = title.ok_or_else(|| Error::InvalidInput(format!("YouTube has no video at {}", watch_url(id))))?;
    let (artists, song) = split_video_title(&raw_title, channel.as_deref());

    Ok(MusicTrack {
        id: format!("yt:{id}"),
        title: song,
        artists,
        album: None,
        album_artist: None,
        track_number: None,
        track_total: None,
        disc_number: None,
        year: None,
        date: None,
        genre: None,
        duration,
        cover_url: Some(video_thumbnail(id)),
        isrc: None,
        source_url: watch_url(id),
        service: Service::YouTube,
        source: AudioSource::YouTube { video_id: id.to_string() },
        explicit: false,
        unavailable: None,
    })
}

// ─── Playlists (www) ─────────────────────────────────────────────────────────

struct PlaylistPage {
    tracks: Vec<MusicTrack>,
    continuation: Option<String>,
}

fn parse_playlist_items(response: &Value) -> PlaylistPage {
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();

    // The 2025 layout: `lockupViewModel` per video.
    let mut lockups = Vec::new();
    find_all(response, "lockupViewModel", &mut lockups);
    for lockup in lockups {
        let Some(id) = lockup.get("contentId").and_then(Value::as_str) else { continue };
        if lockup.get("contentType").and_then(Value::as_str) != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
            continue;
        }
        if !seen.insert(id.to_string()) {
            continue;
        }
        let meta = lockup.pointer("/metadata/lockupMetadataViewModel");
        let title = meta.and_then(|m| m.pointer("/title/content")).and_then(Value::as_str).unwrap_or(id);
        let channel = meta
            .and_then(|m| m.pointer("/metadata/contentMetadataViewModel/metadataRows/0/metadataParts/0/text/content"))
            .and_then(Value::as_str);
        let duration = {
            let mut badges = Vec::new();
            find_all(lockup, "thumbnailBadgeViewModel", &mut badges);
            badges.iter().find_map(|b| b.get("text").and_then(Value::as_str).and_then(parse_clock))
        };
        // A deleted or private video shows up as a lockup with no channel and
        // a placeholder title; it cannot be fetched, so it is not offered.
        if matches!(title, "[Deleted video]" | "[Private video]") {
            continue;
        }
        tracks.push(playlist_track(id, title, channel, duration));
    }

    // The older layout, still served to some sessions.
    let mut renderers = Vec::new();
    find_all(response, "playlistVideoRenderer", &mut renderers);
    for r in renderers {
        let Some(id) = r.get("videoId").and_then(Value::as_str) else { continue };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let title = r.get("title").and_then(text_of).unwrap_or_else(|| id.to_string());
        if matches!(title.as_str(), "[Deleted video]" | "[Private video]") {
            continue;
        }
        let channel = r.get("shortBylineText").and_then(text_of);
        let duration = r.get("lengthSeconds").and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).filter(|d| *d > 0.0);
        tracks.push(playlist_track(id, &title, channel.as_deref(), duration));
    }

    let continuation = continuation_token(response);
    PlaylistPage { tracks, continuation }
}

fn playlist_track(id: &str, title: &str, channel: Option<&str>, duration: Option<f64>) -> MusicTrack {
    let (artists, song) = split_video_title(title, channel);
    MusicTrack {
        id: format!("yt:{id}"),
        title: song,
        artists,
        album: None,
        album_artist: None,
        track_number: None,
        track_total: None,
        disc_number: None,
        year: None,
        date: None,
        genre: None,
        duration,
        cover_url: Some(video_thumbnail(id)),
        isrc: None,
        source_url: watch_url(id),
        service: Service::YouTube,
        source: AudioSource::YouTube { video_id: id.to_string() },
        explicit: false,
        unavailable: None,
    }
}

/// The token to fetch the next page with, in either continuation shape.
fn continuation_token(response: &Value) -> Option<String> {
    let mut items = Vec::new();
    find_all(response, "continuationItemViewModel", &mut items);
    find_all(response, "continuationItemRenderer", &mut items);
    for item in items {
        let mut tokens = Vec::new();
        find_all(item, "token", &mut tokens);
        if let Some(token) = tokens.into_iter().find_map(Value::as_str) {
            return Some(token.to_string());
        }
    }
    let mut legacy = Vec::new();
    find_all(response, "nextContinuationData", &mut legacy);
    legacy.into_iter().find_map(|n| n.get("continuation").and_then(Value::as_str).map(str::to_string))
}

fn playlist_header(response: &Value) -> (Option<String>, Option<String>) {
    let title = first(response, "playlistMetadataRenderer")
        .and_then(|m| m.get("title"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| first(response, "pageHeaderViewModel").and_then(|h| h.pointer("/title/dynamicTextViewModel/text/content")).and_then(Value::as_str).map(str::to_string))
        .or_else(|| first(response, "playlistHeaderRenderer").and_then(|h| h.get("title")).and_then(text_of))
        .or_else(|| first(response, "microformatDataRenderer").and_then(|m| m.get("title")).and_then(Value::as_str).map(str::to_string));

    let owner = first(response, "pageHeaderViewModel")
        .and_then(|h| {
            let mut stacks = Vec::new();
            find_all(h, "avatarStackViewModel", &mut stacks);
            stacks.first().and_then(|s| s.pointer("/text/content")).and_then(Value::as_str).map(|t| t.trim_start_matches("by ").to_string())
        })
        .or_else(|| first(response, "playlistHeaderRenderer").and_then(|h| h.get("ownerText")).and_then(text_of));
    (title, owner)
}

async fn playlist(client: &Client, list_id: &str, cookie: Option<&str>) -> Result<MusicSource> {
    let browse_id = if list_id.starts_with("VL") { list_id.to_string() } else { format!("VL{list_id}") };
    let response = innertube_post(client, Web::Video, "browse", json!({ "browseId": browse_id }), cookie).await?;

    let (title, owner) = playlist_header(&response);
    let mut page = parse_playlist_items(&response);
    let mut tracks = std::mem::take(&mut page.tracks);
    let mut continuation = page.continuation;
    let mut pages = 0;
    while let Some(token) = continuation.take() {
        if tracks.len() >= MAX_PLAYLIST_ITEMS || pages > 100 {
            break;
        }
        pages += 1;
        let next = innertube_post(client, Web::Video, "browse", json!({ "continuation": token }), cookie).await?;
        let mut more = parse_playlist_items(&next);
        if more.tracks.is_empty() {
            break;
        }
        tracks.append(&mut more.tracks);
        continuation = more.continuation;
    }

    if tracks.is_empty() {
        let mut alerts = Vec::new();
        find_all(&response, "alertRenderer", &mut alerts);
        let reason = alerts.first().and_then(|a| a.get("text")).and_then(text_of);
        // The mix check comes before YouTube own words: for an `RD` list it
        // answers "This playlist type is unviewable", which is true and tells
        // the user nothing they can act on.
        return Err(Error::InvalidInput(if list_id.starts_with("RD") {
            "this is a YouTube mix — it is generated per viewer and has no fixed track list, so there is nothing to download as one. Open a track from it and paste that instead".into()
        } else {
            match reason {
                Some(r) => format!("YouTube: {r}"),
                None => "no videos could be listed for this playlist — it may be private (share your YouTube session from the extension) or empty".into(),
            }
        }));
    }

    let cover = tracks.first().and_then(|t| t.cover_url.clone());
    let listed = tracks.len();
    Ok(MusicSource {
        kind: SourceKind::Playlist,
        service: Service::YouTube,
        title: title.unwrap_or_else(|| format!("YouTube playlist {list_id}")),
        owner,
        cover_url: cover,
        source_url: format!("https://www.youtube.com/playlist?list={list_id}"),
        tracks,
        total: None,
        note: (listed >= MAX_PLAYLIST_ITEMS).then(|| format!("only the first {MAX_PLAYLIST_ITEMS} videos were listed")),
    })
}

// ─── YouTube Music albums and playlists ──────────────────────────────────────

/// One row of a YouTube Music list: `musicResponsiveListItemRenderer`.
fn music_item(item: &Value, album: Option<&str>, album_artist: Option<&str>, year: Option<u32>, cover: Option<&str>) -> Option<MusicTrack> {
    let video_id = item
        .pointer("/playlistItemData/videoId")
        .and_then(Value::as_str)
        .or_else(|| first(item, "watchEndpoint").and_then(|w| w.get("videoId")).and_then(Value::as_str))?;

    let columns: Vec<&Value> = item
        .get("flexColumns")
        .and_then(Value::as_array)
        .map(|cols| cols.iter().filter_map(|c| c.get("musicResponsiveListItemFlexColumnRenderer")).collect())
        .unwrap_or_default();
    let title = columns.first().and_then(|c| c.get("text")).and_then(runs_text)?;

    // The second column's runs are artists (linking to a `UC…` channel) and,
    // on a playlist row, the album (`MPREb…`) and the duration.
    let mut artists = Vec::new();
    let mut row_album = None;
    let mut row_duration = None;
    for column in columns.iter().skip(1) {
        let Some(runs) = column.get("text").and_then(|t| t.get("runs")).and_then(Value::as_array) else { continue };
        for run in runs {
            let Some(text) = run.get("text").and_then(Value::as_str) else { continue };
            let browse = run.pointer("/navigationEndpoint/browseEndpoint/browseId").and_then(Value::as_str);
            match browse {
                Some(id) if id.starts_with("UC") => artists.push(text.trim().to_string()),
                Some(id) if id.starts_with("MPREb") => row_album = Some(text.trim().to_string()),
                _ => {
                    if row_duration.is_none() {
                        row_duration = parse_clock(text);
                    }
                }
            }
        }
    }
    let duration = item
        .get("fixedColumns")
        .and_then(Value::as_array)
        .and_then(|cols| cols.first())
        .and_then(|c| c.get("musicResponsiveListItemFixedColumnRenderer"))
        .and_then(|c| c.get("text"))
        .and_then(runs_text)
        .and_then(|t| parse_clock(&t))
        .or(row_duration);
    let track_number = item.get("index").and_then(runs_text).and_then(|n| n.parse::<u32>().ok());
    let row_cover = largest_thumbnail(item);
    if artists.is_empty() {
        artists.extend(album_artist.map(str::to_string));
    }

    Some(MusicTrack {
        id: format!("yt:{video_id}"),
        title,
        artists,
        album: row_album.or_else(|| album.map(str::to_string)),
        album_artist: album_artist.map(str::to_string),
        track_number,
        track_total: None,
        disc_number: None,
        year,
        date: None,
        genre: None,
        duration,
        cover_url: row_cover.or_else(|| cover.map(str::to_string)).or_else(|| Some(video_thumbnail(video_id))),
        isrc: None,
        source_url: format!("https://music.youtube.com/watch?v={video_id}"),
        service: Service::YouTubeMusic,
        source: AudioSource::YouTube { video_id: video_id.to_string() },
        explicit: {
            let mut badges = Vec::new();
            find_all(item, "musicInlineBadgeRenderer", &mut badges);
            badges.iter().any(|b| b.pointer("/icon/iconType").and_then(Value::as_str) == Some("MUSIC_EXPLICIT_BADGE"))
        },
        unavailable: None,
    })
}

/// What the page header says: title, artist/owner, kind, year, cover.
struct MusicHeader {
    title: Option<String>,
    owner: Option<String>,
    is_album: bool,
    year: Option<u32>,
    cover: Option<String>,
}

fn music_header(response: &Value) -> MusicHeader {
    let header = first(response, "musicResponsiveHeaderRenderer")
        .or_else(|| first(response, "musicDetailHeaderRenderer"))
        .or_else(|| first(response, "musicEditablePlaylistDetailHeaderRenderer").and_then(|h| first(h, "musicDetailHeaderRenderer")));
    let Some(header) = header else {
        let micro = first(response, "microformatDataRenderer");
        return MusicHeader {
            title: micro.and_then(|m| m.get("title")).and_then(Value::as_str).map(str::to_string),
            owner: None,
            is_album: micro.and_then(|m| m.get("description")).and_then(Value::as_str).is_some_and(|d| d.starts_with("Album")),
            year: None,
            cover: micro.and_then(largest_thumbnail),
        };
    };
    let title = header.get("title").and_then(runs_text);
    let owner = header
        .get("straplineTextOne")
        .and_then(runs_text)
        .or_else(|| {
            // The detail header's subtitle reads `Album • Artist • 2001`.
            header.get("subtitle").and_then(|s| s.get("runs")).and_then(Value::as_array).and_then(|runs| {
                runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).map(str::trim).filter(|t| !t.is_empty() && *t != "•").nth(1).map(str::to_string)
            })
        });
    let subtitle_runs: Vec<String> = header
        .get("subtitle")
        .and_then(|s| s.get("runs"))
        .and_then(Value::as_array)
        .map(|runs| runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();
    let is_album = subtitle_runs.iter().any(|t| matches!(t.as_str(), "Album" | "Single" | "EP"));
    let year = subtitle_runs.iter().find_map(|t| (t.len() == 4).then(|| t.parse::<u32>().ok()).flatten()).filter(|y| *y > 1000);
    MusicHeader { title, owner, is_album, year, cover: largest_thumbnail(header) }
}

async fn music_browse(client: &Client, browse_id: &str, url: &str, cookie: Option<&str>) -> Result<MusicSource> {
    let response = innertube_post(client, Web::Music, "browse", json!({ "browseId": browse_id }), cookie).await?;
    let header = music_header(&response);
    let album_name = header.is_album.then(|| header.title.clone()).flatten();

    let mut items = Vec::new();
    find_all(&response, "musicResponsiveListItemRenderer", &mut items);
    let mut tracks: Vec<MusicTrack> = items
        .iter()
        .filter_map(|i| music_item(i, album_name.as_deref(), header.owner.as_deref(), header.year, header.cover.as_deref()))
        .collect();

    let mut continuation = continuation_token(&response);
    let mut pages = 0;
    while let Some(token) = continuation.take() {
        if tracks.len() >= MAX_PLAYLIST_ITEMS || pages > 100 {
            break;
        }
        pages += 1;
        let next = innertube_post(client, Web::Music, "browse", json!({ "continuation": token }), cookie).await?;
        let mut more = Vec::new();
        find_all(&next, "musicResponsiveListItemRenderer", &mut more);
        let before = tracks.len();
        tracks.extend(more.iter().filter_map(|i| music_item(i, album_name.as_deref(), header.owner.as_deref(), header.year, header.cover.as_deref())));
        if tracks.len() == before {
            break;
        }
        continuation = continuation_token(&next);
    }

    // De-duplicate on video id: a browse answer can repeat a row in a
    // "related" shelf under the list itself.
    let mut seen = HashSet::new();
    tracks.retain(|t| seen.insert(t.id.clone()));
    if header.is_album {
        let total = tracks.len() as u32;
        for (i, t) in tracks.iter_mut().enumerate() {
            t.track_number = t.track_number.or(Some(i as u32 + 1));
            t.track_total = Some(total);
        }
    }

    if tracks.is_empty() {
        return Err(Error::InvalidInput(
            "no tracks could be listed for this YouTube Music link — it may be private (share your YouTube session from the extension) or empty".into(),
        ));
    }
    Ok(MusicSource {
        kind: if header.is_album { SourceKind::Album } else { SourceKind::Playlist },
        service: Service::YouTubeMusic,
        title: header.title.unwrap_or_else(|| browse_id.to_string()),
        owner: header.owner,
        cover_url: header.cover.or_else(|| tracks.first().and_then(|t| t.cover_url.clone())),
        source_url: url.to_string(),
        tracks,
        total: None,
        note: None,
    })
}

// ─── Entry point ─────────────────────────────────────────────────────────────

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<MusicSource> {
    let cookie = cookie_of(headers);
    let host = super::host_of(url);
    let is_music = host.ends_with("music.youtube.com");
    let path = url.trim_start_matches("https://").trim_start_matches("http://").split_once('/').map(|(_, p)| p).unwrap_or("");
    let path_only = path.split(['?', '#']).next().unwrap_or(path);

    // `music.youtube.com/browse/MPREb_…` is an album page.
    if let Some(id) = path_only.strip_prefix("browse/") {
        let id = id.split('/').next().unwrap_or(id);
        if id.starts_with("MPREb") || id.starts_with("VL") || id.starts_with("MPLA") {
            return music_browse(client, id, url, cookie.as_deref()).await;
        }
    }

    let list = query_param(url, "list").filter(|l| !l.is_empty());
    let video = youtube::video_id(url);

    match (video, list) {
        // A watch link inside a playlist means the video; a user who wanted
        // the whole playlist has its own link.
        (Some(id), _) => {
            let mut track = video_track(client, &id, cookie.as_deref()).await?;
            if is_music {
                track.service = Service::YouTubeMusic;
                track.source_url = format!("https://music.youtube.com/watch?v={id}");
            }
            Ok(MusicSource {
                kind: SourceKind::Track,
                service: track.service,
                title: track.title.clone(),
                owner: track.primary_artist().map(str::to_string),
                cover_url: track.cover_url.clone(),
                source_url: track.source_url.clone(),
                tracks: vec![track],
                total: None,
                note: None,
            })
        }
        (None, Some(list)) if is_music || list.starts_with("OLAK5uy_") => {
            music_browse(client, &format!("VL{list}"), url, cookie.as_deref()).await
        }
        (None, Some(list)) => playlist(client, &list, cookie.as_deref()).await,
        (None, None) => Err(Error::InvalidInput(
            "this YouTube link is neither a video nor a playlist — channel pages and search pages cannot be downloaded as one".into(),
        )),
    }
}

// ─── Search and matching ─────────────────────────────────────────────────────

/// A search hit, with what is known about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub cover: Option<String>,
    /// From YouTube Music's song search (a recording), as opposed to a video.
    pub is_song: bool,
    /// A `- Topic` channel or an "official" badge: the label's own upload.
    pub official: bool,
}

/// YouTube Music, songs only.
const SONGS_FILTER: &str = "EgWKAQIIAWoKEAoQAxAEEAkQBQ%3D%3D";
/// Plain YouTube, videos only.
const VIDEOS_FILTER: &str = "EgIQAQ%3D%3D";

pub async fn search_music(client: &Client, query: &str) -> Result<Vec<Candidate>> {
    let response = innertube_post(client, Web::Music, "search", json!({ "query": query, "params": SONGS_FILTER }), None).await?;
    let mut items = Vec::new();
    find_all(&response, "musicResponsiveListItemRenderer", &mut items);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        let Some(track) = music_item(item, None, None, None, None) else { continue };
        let AudioSource::YouTube { video_id } = &track.source else { continue };
        if !seen.insert(video_id.clone()) {
            continue;
        }
        out.push(Candidate {
            video_id: video_id.clone(),
            title: track.title,
            artists: track.artists,
            album: track.album,
            duration: track.duration,
            cover: track.cover_url,
            is_song: true,
            official: true,
        });
    }
    Ok(out)
}

pub async fn search_videos(client: &Client, query: &str) -> Result<Vec<Candidate>> {
    let response = innertube_post(client, Web::Video, "search", json!({ "query": query, "params": VIDEOS_FILTER }), None).await?;
    let mut items = Vec::new();
    find_all(&response, "videoRenderer", &mut items);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        let Some(id) = item.get("videoId").and_then(Value::as_str) else { continue };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let raw_title = item.get("title").and_then(text_of).unwrap_or_default();
        let channel = item.get("ownerText").and_then(text_of);
        let duration = item.get("lengthText").and_then(text_of).and_then(|t| parse_clock(&t));
        let official = channel.as_deref().is_some_and(|c| c.ends_with(" - Topic")) || {
            let mut badges = Vec::new();
            find_all(item, "metadataBadgeRenderer", &mut badges);
            badges.iter().any(|b| b.get("style").and_then(Value::as_str) == Some("BADGE_STYLE_TYPE_VERIFIED_ARTIST"))
        };
        let (artists, title) = split_video_title(&raw_title, channel.as_deref());
        out.push(Candidate {
            video_id: id.to_string(),
            title,
            artists,
            album: None,
            duration,
            cover: Some(video_thumbnail(id)),
            is_song: false,
            official,
        });
    }
    Ok(out)
}

/// Lowercased, accents stripped, punctuation removed.
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
            '&' => ' ',
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

fn tokens(s: &str) -> HashSet<String> {
    normalize(s)
        .split_whitespace()
        .filter(|t| !matches!(*t, "the" | "a" | "an" | "feat" | "ft" | "featuring" | "and" | "with" | "prod"))
        .map(str::to_string)
        .collect()
}

/// `|A ∩ B| / |A|`: how much of the wanted title the candidate covers.
fn coverage(wanted: &HashSet<String>, got: &HashSet<String>) -> f64 {
    if wanted.is_empty() {
        return 0.0;
    }
    wanted.intersection(got).count() as f64 / wanted.len() as f64
}

/// Words that mark a different recording of the same song.
const VARIANT_WORDS: &[&str] = &[
    "live", "cover", "remix", "karaoke", "instrumental", "acoustic", "sped", "slowed", "reverb", "nightcore", "8d", "reaction",
    "tutorial", "lesson", "mashup", "edit", "extended", "demo", "rehearsal", "parody", "acapella", "concert", "unplugged",
    "reversed", "loop", "1 hour", "hour", "mix", "medley", "session",
];

/// How well a candidate fits the wanted track, in `0.0..=1.0`.
pub fn score(track: &MusicTrack, candidate: &Candidate) -> f64 {
    let wanted_title = tokens(&clean_title(&track.title));
    let got_title = tokens(&candidate.title);
    let title = {
        let c = coverage(&wanted_title, &got_title);
        let back = coverage(&got_title, &wanted_title);
        // Both directions: a candidate whose title is the wanted title plus a
        // paragraph of extras is not the song.
        0.7 * c + 0.3 * back
    };

    let wanted_artists: Vec<String> = track.artists.iter().map(|a| normalize(a)).filter(|a| !a.is_empty()).collect();
    let got_artist_line = normalize(&candidate.artists.join(" "));
    let got_all = format!("{got_artist_line} {}", normalize(&candidate.title));
    let artist = if wanted_artists.is_empty() {
        0.6
    } else if wanted_artists.iter().any(|a| got_all.contains(a.as_str())) {
        1.0
    } else {
        let wanted: HashSet<String> = wanted_artists.iter().flat_map(|a| a.split_whitespace().map(str::to_string)).collect();
        let got: HashSet<String> = got_all.split_whitespace().map(str::to_string).collect();
        coverage(&wanted, &got) * 0.8
    };

    let duration = match (track.duration, candidate.duration) {
        (Some(want), Some(got)) => {
            let diff = (want - got).abs();
            if diff <= 3.0 {
                1.0
            } else if diff <= 10.0 {
                0.85
            } else if diff <= 20.0 {
                0.55
            } else if diff <= 45.0 {
                0.2
            } else {
                0.0
            }
        }
        _ => 0.5,
    };

    let mut total = 0.45 * title + 0.30 * artist + 0.25 * duration;

    // A recording of a different kind than the one asked for.
    let wanted_norm = normalize(&track.title);
    let got_norm = normalize(&candidate.title);
    for word in VARIANT_WORDS {
        if got_norm.split_whitespace().any(|t| t == *word) && !wanted_norm.split_whitespace().any(|t| t == *word) {
            total -= 0.25;
        }
    }
    if candidate.is_song {
        total += 0.05;
    }
    if candidate.official {
        total += 0.03;
    }
    total.clamp(0.0, 1.0)
}

/// Below this, the best hit is not trusted to be the song.
pub const ACCEPT_SCORE: f64 = 0.55;

/// What a match settled on.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub score: f64,
}

impl Match {
    pub fn url(&self) -> String {
        watch_url(&self.video_id)
    }
}

fn search_query(track: &MusicTrack) -> String {
    let title = clean_title(&track.title);
    match track.primary_artist() {
        Some(artist) => format!("{artist} {title}"),
        None => title,
    }
}

/// Find the track's recording on YouTube.
///
/// YouTube Music's song search first (when preferred), plain YouTube second;
/// the best-scoring hit across both wins, but a good YouTube Music hit is
/// taken without asking YouTube at all.
pub async fn find_match(client: &Client, track: &MusicTrack, prefer_music: bool) -> Result<Match> {
    let query = search_query(track);
    let mut best: Option<(f64, Candidate)> = None;
    fn consider(best: &mut Option<(f64, Candidate)>, track: &MusicTrack, candidates: Vec<Candidate>) {
        for c in candidates {
            let s = score(track, &c);
            if best.as_ref().is_none_or(|(b, _)| s > *b) {
                *best = Some((s, c));
            }
        }
    }

    let order: [bool; 2] = if prefer_music { [true, false] } else { [false, true] };
    for use_music in order {
        let result = if use_music { search_music(client, &query).await } else { search_videos(client, &query).await };
        match result {
            Ok(candidates) => consider(&mut best, track, candidates),
            Err(e) => tracing::debug!(error = %e, music = use_music, "YouTube search failed"),
        }
        if best.as_ref().is_some_and(|(s, _)| *s >= 0.85) {
            break;
        }
    }

    match best {
        Some((s, c)) if s >= ACCEPT_SCORE => Ok(Match { video_id: c.video_id, title: c.title, artists: c.artists, album: c.album, score: s }),
        Some((s, c)) => Err(Error::InvalidInput(format!(
            "no confident match on YouTube for \"{}\" by {} — the closest was \"{}\" ({:.0}% fit)",
            track.title,
            track.artist_line(),
            c.title,
            s * 100.0
        ))),
        None => Err(Error::InvalidInput(format!("nothing on YouTube matched \"{}\" by {}", track.title, track.artist_line()))),
    }
}

/// A free-text search as a source: the top hits, each bound to its video.
pub async fn search_source(client: &Client, query: &str, prefer_music: bool) -> Result<MusicSource> {
    let mut candidates = if prefer_music { search_music(client, query).await.unwrap_or_default() } else { Vec::new() };
    if candidates.len() < 5 {
        if let Ok(mut videos) = search_videos(client, query).await {
            let seen: HashSet<String> = candidates.iter().map(|c| c.video_id.clone()).collect();
            videos.retain(|v| !seen.contains(&v.video_id));
            candidates.append(&mut videos);
        }
    }
    candidates.truncate(15);
    if candidates.is_empty() {
        return Err(Error::InvalidInput(format!("nothing on YouTube matched \"{query}\"")));
    }
    let tracks: Vec<MusicTrack> = candidates
        .into_iter()
        .map(|c| MusicTrack {
            id: format!("yt:{}", c.video_id),
            title: c.title,
            artists: c.artists,
            album: c.album,
            album_artist: None,
            track_number: None,
            track_total: None,
            disc_number: None,
            year: None,
            date: None,
            genre: None,
            duration: c.duration,
            cover_url: c.cover.or_else(|| Some(video_thumbnail(&c.video_id))),
            isrc: None,
            source_url: if c.is_song { format!("https://music.youtube.com/watch?v={}", c.video_id) } else { watch_url(&c.video_id) },
            service: if c.is_song { Service::YouTubeMusic } else { Service::YouTube },
            source: AudioSource::YouTube { video_id: c.video_id },
            explicit: false,
            unavailable: None,
        })
        .collect();
    Ok(MusicSource {
        kind: SourceKind::Search,
        service: Service::Search,
        title: query.to_string(),
        owner: None,
        cover_url: tracks.first().and_then(|t| t.cover_url.clone()),
        source_url: format!("https://music.youtube.com/search?q={}", urlencode(query)),
        tracks,
        total: None,
        note: Some("search results are YouTube Music songs first, then YouTube videos".into()),
    })
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

#[allow(dead_code)]
fn _year_helper(d: &str) -> Option<u32> {
    year_of(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artists: &[&str], duration: Option<f64>) -> MusicTrack {
        MusicTrack {
            id: "t".into(),
            title: title.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album: None,
            album_artist: None,
            track_number: None,
            track_total: None,
            disc_number: None,
            year: None,
            date: None,
            genre: None,
            duration,
            cover_url: None,
            isrc: None,
            source_url: String::new(),
            service: Service::Spotify,
            source: AudioSource::Match,
            explicit: false,
            unavailable: None,
        }
    }

    fn candidate(title: &str, artists: &[&str], duration: Option<f64>, is_song: bool) -> Candidate {
        Candidate {
            video_id: "x".into(),
            title: title.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album: None,
            duration,
            cover: None,
            is_song,
            official: is_song,
        }
    }

    #[test]
    fn upload_noise_is_stripped_from_titles() {
        assert_eq!(clean_title("Daft Punk - One More Time (Official Video)"), "Daft Punk - One More Time");
        assert_eq!(clean_title("Song [Official Audio] (Lyrics)"), "Song");
        assert_eq!(clean_title("Song (feat. Someone) [HD]"), "Song (feat. Someone)");
        assert_eq!(clean_title("Song | Official Video"), "Song");
        assert_eq!(clean_title("Song (Radio Edit)"), "Song (Radio Edit)");
    }

    #[test]
    fn video_titles_split_into_artist_and_song() {
        assert_eq!(split_video_title("Daft Punk - One More Time (Official Video)", Some("Daft Punk")), (vec!["Daft Punk".to_string()], "One More Time".to_string()));
        assert_eq!(split_video_title("One More Time", Some("Daft Punk - Topic")), (vec!["Daft Punk".to_string()], "One More Time".to_string()));
        assert_eq!(split_video_title("Daft Punk - One More Time", Some("Daft Punk - Topic")), (vec!["Daft Punk".to_string()], "One More Time".to_string()));
        assert_eq!(split_video_title("A & B - Song", None), (vec!["A".to_string(), "B".to_string()], "Song".to_string()));
        assert_eq!(split_video_title("Just A Title", Some("Channel")), (vec!["Channel".to_string()], "Just A Title".to_string()));
    }

    #[test]
    fn the_studio_recording_outscores_a_live_take_and_a_cover() {
        let want = track("One More Time", &["Daft Punk"], Some(320.0));
        let studio = candidate("One More Time", &["Daft Punk"], Some(320.0), true);
        let live = candidate("One More Time (Live at Coachella)", &["Daft Punk"], Some(340.0), false);
        let cover = candidate("One More Time - Daft Punk (piano cover)", &["Some Pianist"], Some(200.0), false);
        let radio = candidate("One More Time (Radio Edit)", &["Daft Punk"], Some(236.0), true);
        let s_studio = score(&want, &studio);
        assert!(s_studio > 0.9, "{s_studio}");
        assert!(s_studio > score(&want, &live));
        assert!(s_studio > score(&want, &cover));
        assert!(s_studio > score(&want, &radio), "the album length wins over the radio edit");
        assert!(score(&want, &cover) < ACCEPT_SCORE, "a cover with the wrong artist and length is not accepted");
    }

    #[test]
    fn a_wanted_live_version_is_not_penalised_for_being_live() {
        let want = track("Song (Live)", &["Band"], None);
        let live = candidate("Song (Live)", &["Band"], None, false);
        assert!(score(&want, &live) >= ACCEPT_SCORE);
    }

    #[test]
    fn matching_tolerates_accents_and_featured_credits() {
        let want = track("Déjà Vu (feat. Someone)", &["Beyoncé"], Some(240.0));
        let hit = candidate("Deja Vu", &["Beyonce", "Someone"], Some(241.0), true);
        assert!(score(&want, &hit) > 0.85);
    }

    #[test]
    fn playlist_items_are_read_from_both_layouts() {
        let response = json!({
            "contents": [
                {"lockupViewModel": {"contentId": "fOT0BUpITw8", "contentType": "LOCKUP_CONTENT_TYPE_VIDEO",
                    "contentImage": {"thumbnailViewModel": {"overlays": [{"thumbnailBottomOverlayViewModel": {"badges": [{"thumbnailBadgeViewModel": {"text": "3:55"}}]}}]}},
                    "metadata": {"lockupMetadataViewModel": {"title": {"content": "BELLAKEO - Peso Pluma, Anitta"},
                        "metadata": {"contentMetadataViewModel": {"metadataRows": [{"metadataParts": [{"text": {"content": "Peso Pluma"}}]}]}}}}}},
                {"lockupViewModel": {"contentId": "PLxyz", "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST", "metadata": {}}},
                {"playlistVideoRenderer": {"videoId": "abc12345678", "title": {"runs": [{"text": "Old Layout - Song"}]}, "lengthSeconds": "200", "shortBylineText": {"runs": [{"text": "Old Channel"}]}}},
                {"playlistVideoRenderer": {"videoId": "deleted0000", "title": {"simpleText": "[Deleted video]"}}},
                {"continuationItemViewModel": {"continuationCommand": {"innertubeCommand": {"continuationCommand": {"token": "NEXT"}}}}}
            ],
            "metadata": {"playlistMetadataRenderer": {"title": "Popular Music Videos"}},
            "header": {"pageHeaderRenderer": {"content": {"pageHeaderViewModel": {"metadata": {"contentMetadataViewModel": {"metadataRows": [{"metadataParts": [{"avatarStack": {"avatarStackViewModel": {"text": {"content": "by Music"}}}}]}]}}}}}}
        });
        let page = parse_playlist_items(&response);
        assert_eq!(page.tracks.len(), 2, "the playlist lockup and the deleted video are skipped");
        assert_eq!(page.tracks[0].source, AudioSource::YouTube { video_id: "fOT0BUpITw8".into() });
        assert_eq!(page.tracks[0].duration, Some(235.0));
        assert_eq!(page.tracks[0].artists, vec!["BELLAKEO"], "a dash-separated title splits on the dash");
        assert_eq!(page.tracks[1].artists, vec!["Old Layout"]);
        assert_eq!(page.tracks[1].duration, Some(200.0));
        assert_eq!(page.continuation.as_deref(), Some("NEXT"));
        let (title, owner) = playlist_header(&response);
        assert_eq!(title.as_deref(), Some("Popular Music Videos"));
        assert_eq!(owner.as_deref(), Some("Music"));
    }

    #[test]
    fn music_rows_carry_artist_album_duration_and_index() {
        let item = json!({
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "One More Time", "navigationEndpoint": {"watchEndpoint": {"videoId": "FGBhQbmPwH8"}}}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Daft Punk", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCRr1xG_2WIDs18a6cIiCxeA"}}}, {"text": " • "},
                    {"text": "Discovery", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_7ltM34kr0mH"}}}, {"text": " • "}, {"text": "5:21"}]}}}
            ],
            "playlistItemData": {"videoId": "FGBhQbmPwH8"},
            "index": {"runs": [{"text": "1"}]},
            "badges": [{"musicInlineBadgeRenderer": {"icon": {"iconType": "MUSIC_EXPLICIT_BADGE"}}}],
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://lh3.googleusercontent.com/abc=w60-h60-l90-rj"}, {"url": "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"}]}}}
        });
        let t = music_item(&item, None, None, Some(2001), None).unwrap();
        assert_eq!(t.title, "One More Time");
        assert_eq!(t.artists, vec!["Daft Punk"]);
        assert_eq!(t.album.as_deref(), Some("Discovery"));
        assert_eq!(t.duration, Some(321.0));
        assert_eq!(t.track_number, Some(1));
        assert_eq!(t.year, Some(2001));
        assert!(t.explicit);
        assert_eq!(t.cover_url.as_deref(), Some("https://lh3.googleusercontent.com/abc=w1200-h1200-l90-rj"));
        assert_eq!(t.source, AudioSource::YouTube { video_id: "FGBhQbmPwH8".into() });

        // An album row has no artist column of its own; it inherits the album's.
        let bare = json!({
            "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Aerodynamic"}]}}}],
            "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "3:33"}]}}}],
            "playlistItemData": {"videoId": "L93-7vRfxNs"}, "index": {"runs": [{"text": "2"}]}
        });
        let t = music_item(&bare, Some("Discovery"), Some("Daft Punk"), None, Some("https://cover")).unwrap();
        assert_eq!(t.artists, vec!["Daft Punk"]);
        assert_eq!(t.album.as_deref(), Some("Discovery"));
        assert_eq!(t.duration, Some(213.0));
        assert_eq!(t.cover_url.as_deref(), Some("https://cover"));
    }

    #[test]
    fn the_music_header_knows_an_album_from_a_playlist() {
        let album = json!({"header": {"musicResponsiveHeaderRenderer": {"title": {"runs": [{"text": "Discovery"}]}, "straplineTextOne": {"runs": [{"text": "Daft Punk"}]},
            "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "2001"}]}, "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://lh3.googleusercontent.com/c=w544-h544-l90-rj"}]}}}}}});
        let h = music_header(&album);
        assert!(h.is_album);
        assert_eq!(h.title.as_deref(), Some("Discovery"));
        assert_eq!(h.owner.as_deref(), Some("Daft Punk"));
        assert_eq!(h.year, Some(2001));
        assert_eq!(h.cover.as_deref(), Some("https://lh3.googleusercontent.com/c=w1200-h1200-l90-rj"));

        let playlist = json!({"header": {"musicDetailHeaderRenderer": {"title": {"runs": [{"text": "Mix"}]}, "subtitle": {"runs": [{"text": "Playlist"}, {"text": " • "}, {"text": "Someone"}, {"text": " • "}, {"text": "2024"}]}}}});
        let h = music_header(&playlist);
        assert!(!h.is_album);
        assert_eq!(h.owner.as_deref(), Some("Someone"));
    }
}
