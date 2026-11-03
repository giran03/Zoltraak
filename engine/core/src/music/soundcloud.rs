//! SoundCloud: tracks and sets, through the API its own web player uses.
//!
//! `api-v2.soundcloud.com` needs a `client_id`, which SoundCloud does not
//! hand out — but its web player has one baked into its JavaScript, and the
//! player is public. So the id is scraped from the player's scripts, cached,
//! and re-scraped when the API starts refusing it (it rotates every few
//! weeks). This is what every SoundCloud downloader does, and it is the
//! reason none of them need an account.
//!
//! A track's page also embeds the player's initial state (`__sc_hydration`),
//! which for a single track carries everything needed without an API call.
//!
//! Streams are minted per request and expire in minutes, so a track resolves
//! to [`AudioSource::SoundCloud`] and the manager asks [`stream_url`] for a
//! fresh link when it is ready to fetch.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::Value;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind, get_html, get_json, json_str, resolve_redirects, year_of};
use crate::error::{Error, Result};

const API: &str = "https://api-v2.soundcloud.com";

// ─── client_id ───────────────────────────────────────────────────────────────

static CLIENT_ID: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();
const CLIENT_ID_TTL: Duration = Duration::from_secs(6 * 60 * 60);

fn cached_client_id() -> Option<String> {
    let cache = CLIENT_ID.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;
    guard.as_ref().filter(|(_, at)| at.elapsed() < CLIENT_ID_TTL).map(|(id, _)| id.clone())
}

fn remember_client_id(id: &str) {
    let cache = CLIENT_ID.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((id.to_string(), Instant::now()));
    }
}

fn forget_client_id() {
    let cache = CLIENT_ID.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        *guard = None;
    }
}

/// The player's script URLs in a SoundCloud page, in document order.
fn script_urls(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("https://a-v2.sndcdn.com/assets/") {
        let tail = &rest[i..];
        let end = tail.find(['"', '\'', ' ', '>']).unwrap_or(tail.len());
        let url = &tail[..end];
        if url.ends_with(".js") && !out.iter().any(|u| u == url) {
            out.push(url.to_string());
        }
        rest = &tail[end..];
    }
    out
}

/// `client_id:"…"` inside a script.
fn client_id_in(script: &str) -> Option<String> {
    let mut rest = script;
    while let Some(i) = rest.find("client_id") {
        let tail = &rest[i + "client_id".len()..];
        let tail = tail.trim_start_matches([':', '=', ' ', '"', '\'']);
        let id: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if id.len() >= 24 {
            return Some(id);
        }
        rest = &rest[i + "client_id".len()..];
    }
    None
}

async fn client_id(client: &Client) -> Result<String> {
    if let Some(id) = cached_client_id() {
        return Ok(id);
    }
    let home = get_html(client, "https://soundcloud.com/").await?;
    // The id lives in one of the last few bundles; walk from the end.
    for url in script_urls(&home).into_iter().rev().take(6) {
        if let Ok(script) = get_html(client, &url).await {
            if let Some(id) = client_id_in(&script) {
                remember_client_id(&id);
                return Ok(id);
            }
        }
    }
    Err(Error::InvalidInput("could not read SoundCloud's client id from its player; try again later".into()))
}

/// An API call with the client id, retried once with a fresh id when the
/// current one is refused.
async fn api(client: &Client, path_and_query: &str) -> Result<Value> {
    for attempt in 0..2 {
        let id = client_id(client).await?;
        let sep = if path_and_query.contains('?') { '&' } else { '?' };
        let url = format!("{API}{path_and_query}{sep}client_id={id}");
        match get_json(client, &url).await {
            Ok(v) => return Ok(v),
            Err(Error::UnexpectedStatus { status: 401 | 403, .. }) if attempt == 0 => {
                forget_client_id();
                continue;
            }
            Err(Error::UnexpectedStatus { status: 404, .. }) => {
                return Err(Error::InvalidInput("SoundCloud has nothing at this link — it may be private or removed".into()));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("the loop returns on its second pass")
}

// ─── Tracks ──────────────────────────────────────────────────────────────────

fn artwork(url: Option<&str>, user_avatar: Option<&str>) -> Option<String> {
    url.or(user_avatar).map(|u| u.replace("-large.", "-t500x500."))
}

fn track_of(t: &Value, set: Option<&SetContext>) -> Option<MusicTrack> {
    let id = t.get("id").and_then(Value::as_u64)?;
    let title = json_str(t, "/title")?;
    let user = json_str(t, "/user/username");
    let publisher_artist = json_str(t, "/publisher_metadata/artist");
    let artists: Vec<String> = publisher_artist.or(user.clone()).into_iter().collect();
    let date = json_str(t, "/release_date").or_else(|| json_str(t, "/created_at"));
    let streamable = t.get("streamable").and_then(Value::as_bool).unwrap_or(true);
    let blocked = json_str(t, "/policy").as_deref() == Some("BLOCK");
    Some(MusicTrack {
        id: format!("sc:{id}"),
        title,
        artists,
        album: json_str(t, "/publisher_metadata/album_title").or_else(|| set.filter(|s| s.is_album).map(|s| s.title.clone())),
        album_artist: set.filter(|s| s.is_album).and_then(|s| s.owner.clone()),
        track_number: None,
        track_total: set.filter(|s| s.is_album).map(|s| s.total),
        disc_number: None,
        year: date.as_deref().and_then(year_of),
        date: date.as_deref().map(|d| d.chars().take(10).collect()),
        genre: json_str(t, "/genre"),
        duration: t.get("duration").and_then(Value::as_f64).map(|ms| ms / 1000.0).filter(|s| *s > 0.0),
        cover_url: artwork(json_str(t, "/artwork_url").as_deref(), json_str(t, "/user/avatar_url").as_deref())
            .or_else(|| set.and_then(|s| s.cover.clone())),
        isrc: json_str(t, "/publisher_metadata/isrc"),
        source_url: json_str(t, "/permalink_url").unwrap_or_else(|| format!("https://api.soundcloud.com/tracks/{id}")),
        service: Service::SoundCloud,
        source: AudioSource::SoundCloud { track_id: id },
        explicit: json_str(t, "/publisher_metadata/explicit").is_some(),
        unavailable: if blocked {
            Some("SoundCloud blocks this track in your region".into())
        } else if !streamable {
            Some("the uploader has not made this track streamable".into())
        } else {
            None
        },
    })
}

struct SetContext {
    title: String,
    owner: Option<String>,
    cover: Option<String>,
    is_album: bool,
    total: u32,
}

fn single(track: MusicTrack) -> MusicSource {
    MusicSource {
        kind: SourceKind::Track,
        service: Service::SoundCloud,
        title: track.title.clone(),
        owner: track.primary_artist().map(str::to_string),
        cover_url: track.cover_url.clone(),
        source_url: track.source_url.clone(),
        tracks: vec![track],
        total: None,
        note: None,
    }
}

/// The player's initial state in a page: the entry whose `hydratable` is
/// `kind`.
fn hydration(html: &str, kind: &str) -> Option<Value> {
    let start = html.find("window.__sc_hydration")?;
    let open = html[start..].find('[')? + start;
    let close = html[open..].find("];</script>").or_else(|| html[open..].find("]\n</script>"))? + open + 1;
    let list: Value = serde_json::from_str(&html[open..close]).ok()?;
    list.as_array()?.iter().find(|e| e.get("hydratable").and_then(Value::as_str) == Some(kind)).and_then(|e| e.get("data")).cloned()
}

pub async fn analyze(client: &Client, url: &str) -> Result<MusicSource> {
    let mut url = url.trim().to_string();
    let host = super::host_of(&url);
    if host.ends_with("on.soundcloud.com") || host.ends_with("snd.sc") || host.ends_with("soundcloud.app.goo.gl") {
        url = resolve_redirects(client, &url).await?;
    }
    let url = url.split(['?', '#']).next().unwrap_or(&url).trim_end_matches('/').to_string();

    // The page first: a track page carries its own data, and even a set page
    // says what kind of thing it is without spending an API call.
    let html = get_html(client, &url).await.ok();
    if let Some(html) = &html {
        if let Some(sound) = hydration(html, "sound") {
            if let Some(track) = track_of(&sound, None) {
                return Ok(single(track));
            }
        }
        if let Some(playlist) = hydration(html, "playlist") {
            return set_source(client, playlist).await;
        }
    }

    let resolved = api(client, &format!("/resolve?url={}", urlencode(&url))).await?;
    match json_str(&resolved, "/kind").as_deref() {
        Some("track") => track_of(&resolved, None).map(single).ok_or_else(|| Error::InvalidInput("SoundCloud's track data was incomplete".into())),
        Some("playlist") => set_source(client, resolved).await,
        Some("user") => Err(Error::InvalidInput("this is a SoundCloud profile — paste a track or a set (playlist/album) instead".into())),
        _ => Err(Error::InvalidInput("this SoundCloud link is not a track or a set".into())),
    }
}

async fn set_source(client: &Client, set: Value) -> Result<MusicSource> {
    let title = json_str(&set, "/title").unwrap_or_else(|| "SoundCloud set".into());
    let owner = json_str(&set, "/user/username");
    let is_album = set.get("is_album").and_then(Value::as_bool).unwrap_or(false)
        || matches!(json_str(&set, "/set_type").as_deref(), Some("album") | Some("ep") | Some("single"));
    let cover = artwork(json_str(&set, "/artwork_url").as_deref(), json_str(&set, "/user/avatar_url").as_deref());
    let items = set.get("tracks").and_then(Value::as_array).cloned().unwrap_or_default();
    let total = set.get("track_count").and_then(Value::as_u64).map(|n| n as u32).unwrap_or(items.len() as u32);
    let context = SetContext { title: title.clone(), owner: owner.clone(), cover: cover.clone(), is_album, total };

    // Only the first few tracks of a set come with their data; the rest are
    // `{id}` stubs to be looked up in batches, and handed back in the set's
    // order rather than the API's.
    let order: Vec<u64> = items.iter().filter_map(|t| t.get("id").and_then(Value::as_u64)).collect();
    let mut full: std::collections::HashMap<u64, Value> = items
        .iter()
        .filter(|t| t.get("title").is_some())
        .filter_map(|t| Some((t.get("id")?.as_u64()?, t.clone())))
        .collect();
    let missing: Vec<u64> = order.iter().copied().filter(|id| !full.contains_key(id)).collect();
    for batch in missing.chunks(50) {
        let ids: Vec<String> = batch.iter().map(u64::to_string).collect();
        let page = api(client, &format!("/tracks?ids={}", ids.join("%2C"))).await?;
        if let Some(list) = page.as_array() {
            for t in list {
                if let Some(id) = t.get("id").and_then(Value::as_u64) {
                    full.insert(id, t.clone());
                }
            }
        }
    }

    let mut tracks: Vec<MusicTrack> = order.iter().filter_map(|id| full.get(id)).filter_map(|t| track_of(t, Some(&context))).collect();
    if is_album {
        for (i, t) in tracks.iter_mut().enumerate() {
            t.track_number = Some(i as u32 + 1);
        }
    }
    if tracks.is_empty() {
        return Err(Error::InvalidInput("this SoundCloud set has no tracks that can be listed".into()));
    }
    Ok(MusicSource {
        kind: if is_album { SourceKind::Album } else { SourceKind::Playlist },
        service: Service::SoundCloud,
        title,
        owner,
        cover_url: cover,
        source_url: json_str(&set, "/permalink_url").unwrap_or_default(),
        tracks,
        total: Some(total),
        note: None,
    })
}

// ─── Streams ─────────────────────────────────────────────────────────────────

/// A fresh stream URL, trying every supported full-length rendition before
/// refreshing stale player metadata once. Never expose signed query strings
/// in the terminal error.
pub async fn stream_url(client: &Client, track_id: u64) -> Result<(String, &'static str)> {
    stream_url_excluding(client, track_id, &[]).await
}

pub(crate) async fn stream_url_excluding(client: &Client, track_id: u64, excluded: &[String]) -> Result<(String, &'static str)> {
    let mut last = Error::InvalidInput("SoundCloud offers no usable full-length stream for this track".into());
    for attempt in 0..2 {
        if attempt > 0 {
            forget_client_id();
        }
        let track = api(client, &format!("/tracks/{track_id}")).await?;
        if json_str(&track, "/policy").as_deref() == Some("BLOCK") {
            return Err(Error::InvalidInput("SoundCloud blocks this track in your region".into()));
        }
        if json_str(&track, "/policy").as_deref() == Some("SNIP") {
            return Err(Error::InvalidInput("SoundCloud only offers a preview for this track".into()));
        }
        if track.get("streamable").and_then(Value::as_bool) == Some(false) {
            return Err(Error::InvalidInput("the uploader has not made this SoundCloud track streamable".into()));
        }
        let id = client_id(client).await?;
        match mint_stream(client, &track, &id, excluded).await {
            Ok(stream) => return Ok(stream),
            Err(error @ Error::ApiRejected { status: 429, .. }) => return Err(error),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn transcoding_rank(t: &Value) -> Option<(u8, &'static str)> {
    if t.get("snipped").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let protocol = json_str(t, "/format/protocol")?;
    let mime = json_str(t, "/format/mime_type")?.to_ascii_lowercase();
    let media_type = mime.split(';').next()?.trim();
    match (protocol.as_str(), media_type) {
        ("progressive", "audio/mpeg") => Some((0, "mp3")),
        ("hls", "audio/mpeg") => Some((1, "mp3")),
        ("hls", "audio/mp4" | "audio/aac" | "audio/x-m4a") => Some((2, "m4a")),
        // This describes a playlist, not an MP3 codec. Prefer explicit audio
        // formats, but retain legacy MPEG HLS as a last resort.
        ("hls", "audio/mpegurl" | "application/x-mpegurl" | "application/vnd.apple.mpegurl") => Some((3, "mp3")),
        _ => None,
    }
}

async fn mint_stream(client: &Client, track: &Value, id: &str, excluded: &[String]) -> Result<(String, &'static str)> {
    let mut candidates: Vec<_> = track.pointer("/media/transcodings").and_then(Value::as_array)
        .into_iter().flatten().filter_map(|t| transcoding_rank(t).map(|(rank, container)| (rank, container, t))).collect();
    candidates.sort_by_key(|(rank, _, _)| *rank);
    let mut seen = std::collections::HashSet::new();
    let mut failures = Vec::new();
    for (_, container, transcoding) in candidates {
        let Some(base) = json_str(transcoding, "/url") else { continue };
        let Ok(mut endpoint) = reqwest::Url::parse(&base) else { continue };
        if !matches!(endpoint.scheme(), "http" | "https") || !seen.insert(base) { continue; }
        // Replace auth fields rather than concatenating a second '?' or
        // treating '+' and '&' inside a token as query separators.
        let query: Vec<_> = endpoint.query_pairs().filter(|(key, _)| key != "client_id" && key != "track_authorization")
            .map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
        endpoint.set_query(None);
        endpoint.query_pairs_mut().extend_pairs(query).append_pair("client_id", id);
        if let Some(auth) = json_str(track, "/track_authorization") {
            endpoint.query_pairs_mut().append_pair("track_authorization", &auth);
        }
        match get_json(client, endpoint.as_str()).await {
            Ok(minted) => {
                if let Some(stream) = json_str(&minted, "/url") {
                    if reqwest::Url::parse(&stream).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) && !excluded.contains(&stream) {
                        return Ok((stream, container));
                    }
                }
                failures.push("missing or unusable stream URL".to_string());
            }
            Err(Error::UnexpectedStatus { status, .. }) => {
                // A rate limit affects the client, not one rendition. Stop
                // instead of multiplying requests across every alternative.
                if status == 429 {
                    return Err(Error::ApiRejected { service: "SoundCloud", status, message: "stream requests are rate-limited; wait before retrying".into() });
                }
                failures.push(format!("HTTP {status}"));
            }
            Err(_) => failures.push("stream resolver unavailable".into()),
        }
    }
    failures.sort();
    failures.dedup();
    let reason = if failures.is_empty() { "no supported full-length rendition".to_string() } else { failures.join(", ") };
    Err(Error::InvalidInput(format!("SoundCloud could not provide a playable stream ({reason}). The track may be unavailable; try again later.")))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn fixture(handler: impl Fn(axum::http::Uri) -> axum::response::Response + Clone + Send + Sync + 'static) -> (String, tokio::task::JoinHandle<()>) {
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let handler = handler.clone();
            async move { handler(uri) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        (base, server)
    }

    fn renditions(base: &str, paths: &[(&str, &str)]) -> Value {
        json!({"track_authorization": "secret+with&query=value", "media": {"transcodings": paths.iter().map(|(path,mime)|
            json!({"url":format!("{base}{path}"), "format":{"protocol":"hls", "mime_type":mime}, "snipped":false})
        ).collect::<Vec<_>>()}})
    }

    #[tokio::test]
    async fn a_withdrawn_mpeg_endpoint_falls_back_to_working_aac() {
        use axum::response::IntoResponse;
        let (base, server) = fixture(|uri| match uri.path() {
            "/old" => axum::http::StatusCode::NOT_FOUND.into_response(),
            "/aac" => axum::Json(json!({"url":"https://cdn.example/audio.m3u8"})).into_response(),
            _ => panic!("unexpected rendition"),
        }).await;
        let track = renditions(&base, &[("/old", "audio/mpeg"), ("/aac", "audio/mp4; codecs=\"mp4a.40.2\"")]);
        let stream = mint_stream(&Client::new(), &track, "client", &[]).await.unwrap();
        assert_eq!(stream, ("https://cdn.example/audio.m3u8".into(), "m4a"));
        server.abort();
    }

    #[tokio::test]
    async fn authorization_query_is_encoded_and_bad_resolvers_do_not_hide_alternatives() {
        use axum::response::IntoResponse;
        let (base, server) = fixture(|uri| {
            let query: std::collections::HashMap<_,_> = reqwest::Url::parse(&format!("http://local{uri}")).unwrap().query_pairs()
                .map(|(k,v)|(k.into_owned(),v.into_owned())).collect();
            assert_eq!(query.get("client_id").map(String::as_str),Some("fresh"));
            assert_eq!(query.get("track_authorization").map(String::as_str),Some("secret+with&query=value"));
            match uri.path() {
                "/denied" => axum::http::StatusCode::UNAUTHORIZED.into_response(),
                "/bad-json" => "not JSON".into_response(),
                "/missing" => axum::Json(json!({})).into_response(),
                "/invalid" => axum::Json(json!({"url":"file:///not-a-stream"})).into_response(),
                "/working" => { assert_eq!(query.get("preset").map(String::as_str),Some("aac")); axum::Json(json!({"url":"https://cdn.example/fresh"})).into_response() },
                _ => panic!("unexpected endpoint"),
            }
        }).await;
        let track = renditions(&base, &[("/denied","audio/mpeg"),("/bad-json","audio/mpeg"),("/missing","audio/mpeg"),("/invalid","audio/mpeg"),("/working?preset=aac&client_id=old","audio/mp4")]);
        assert_eq!(mint_stream(&Client::new(), &track, "fresh", &[]).await.unwrap().1,"m4a");
        server.abort();
    }

    #[tokio::test]
    async fn rate_limits_stop_and_terminal_errors_do_not_expose_tokens() {
        use axum::response::IntoResponse;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let (base, server) = fixture(move |uri| {
            count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            if uri.path()=="/limit" { axum::http::StatusCode::TOO_MANY_REQUESTS.into_response() }
            else { axum::http::StatusCode::NOT_FOUND.into_response() }
        }).await;
        let track = renditions(&base,&[("/limit","audio/mpeg"),("/untried","audio/mp4")]);
        assert!(matches!(mint_stream(&Client::new(),&track,"secret-client",&[]).await,Err(Error::ApiRejected{status:429,..})));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),1);
        let track = renditions(&base,&[("/gone","audio/mpeg")]);
        let error = mint_stream(&Client::new(),&track,"secret-client",&[]).await.unwrap_err().to_string();
        assert!(error.contains("HTTP 404"));
        assert!(!error.contains("secret") && !error.contains("track_authorization") && !error.contains(&base));
        server.abort();
    }

    #[test]
    fn playlist_mime_is_not_misclassified_as_mp3_and_previews_are_excluded() {
        assert!(transcoding_rank(&json!({"snipped":true,"format":{"protocol":"hls","mime_type":"audio/mpeg"}})).is_none());
        let aac = transcoding_rank(&json!({"format":{"protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""}})).unwrap();
        let playlist = transcoding_rank(&json!({"format":{"protocol":"hls","mime_type":"audio/mpegurl"}})).unwrap();
        assert!(aac.0 < playlist.0);
        assert!(transcoding_rank(&json!({"format":{"protocol":"progressive","mime_type":"video/mp4"}})).is_none());
    }

    #[tokio::test]
    #[ignore = "requires live SoundCloud access; resolves metadata and manifest only"]
    async fn live_short_link_uses_a_working_full_length_rendition() {
        let client=Client::builder().timeout(Duration::from_secs(30)).build().unwrap();
        let source=analyze(&client,"https://on.soundcloud.com/oUKDDayY7aSAqt1WPM").await.unwrap();
        let AudioSource::SoundCloud{track_id}=source.tracks[0].source else{panic!("expected SoundCloud track")};
        let (url,container)=stream_url(&client,track_id).await.unwrap();
        assert_eq!(track_id,864460012);
        assert_eq!(container,"m4a");
        let response=client.get(&url).send().await.unwrap();
        assert!(response.status().is_success(),"minted manifest must be accessible");
        assert!(response.text().await.unwrap().trim_start().starts_with("#EXTM3U"));
    }

    #[test]
    fn the_client_id_is_found_in_a_bundle() {
        assert_eq!(client_id_in(r#"x={client_id:"Pb72ranhoyt6gw7hM7TkzUItXlMWSNSo",env:"production"}"#).as_deref(), Some("Pb72ranhoyt6gw7hM7TkzUItXlMWSNSo"));
        assert_eq!(client_id_in(r#"client_id: 'abcdefghijklmnopqrstuvwxyz012345'"#).as_deref(), Some("abcdefghijklmnopqrstuvwxyz012345"));
        assert_eq!(client_id_in("client_id=x"), None, "too short to be one");
        let html = r#"<script crossorigin src="https://a-v2.sndcdn.com/assets/0-a.js"></script><script crossorigin src="https://a-v2.sndcdn.com/assets/55-b.js"></script>"#;
        assert_eq!(script_urls(html), vec!["https://a-v2.sndcdn.com/assets/0-a.js", "https://a-v2.sndcdn.com/assets/55-b.js"]);
    }

    #[test]
    fn a_track_is_read_from_the_api_shape() {
        let t = json!({
            "id": 293, "title": "Flickermood", "duration": 213886, "genre": "Electronic", "release_date": "2003-06-02T00:00:00Z",
            "artwork_url": "https://i1.sndcdn.com/artworks-000067273316-smsiqx-large.jpg", "permalink_url": "https://soundcloud.com/forss/flickermood",
            "user": {"username": "Forss", "avatar_url": "https://i1.sndcdn.com/avatars-x-large.jpg"}, "streamable": true, "policy": "ALLOW",
            "publisher_metadata": {"artist": "Forss", "album_title": "Soulhack", "isrc": "XX123"}
        });
        let track = track_of(&t, None).unwrap();
        assert_eq!(track.artists, vec!["Forss"]);
        assert_eq!(track.album.as_deref(), Some("Soulhack"));
        assert_eq!(track.duration, Some(213.886));
        assert_eq!(track.year, Some(2003));
        assert_eq!(track.isrc.as_deref(), Some("XX123"));
        assert_eq!(track.cover_url.as_deref(), Some("https://i1.sndcdn.com/artworks-000067273316-smsiqx-t500x500.jpg"));
        assert_eq!(track.source, AudioSource::SoundCloud { track_id: 293 });
        assert!(track.unavailable.is_none());

        let blocked = json!({"id": 1, "title": "x", "user": {"username": "u"}, "policy": "BLOCK"});
        assert!(track_of(&blocked, None).unwrap().unavailable.is_some());
    }

    #[test]
    fn hydration_state_is_read_from_a_page() {
        let html = r#"<script>window.__sc_hydration = [{"hydratable":"anonymousId","data":"x"},{"hydratable":"sound","data":{"id":293,"title":"Flickermood","user":{"username":"Forss"}}}];</script>"#;
        let sound = hydration(html, "sound").unwrap();
        assert_eq!(sound["title"], "Flickermood");
        assert!(hydration(html, "playlist").is_none());
    }
}
