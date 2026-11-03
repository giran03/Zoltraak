//! One post resolver for both Images and Video. Only post addresses are
//! canonicalized; attachment URLs retain their signed query strings.
use crate::{
    error::{Error, Result},
    headers::Headers,
    image::{Completeness, ImageAlbum, ImageItem, MediaKind},
    video::{self, VideoMetadata},
};
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::{sync::LazyLock, time::{Duration, Instant}};

const MAX_BODY: usize = 8 * 1024 * 1024;
const APP_ID: &str = "936619743392459";

// Reuse a recently resolved post across Images, Video and overlay analysis.
// The session is part of the key: a different sign-in must resolve independently.
// Credentials and signed sources stay in memory, never in diagnostics or on disk.
type CachedPost = (String, Headers, Instant, Post);
static RECENT_POSTS: LazyLock<tokio::sync::Mutex<Vec<CachedPost>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));

pub(crate) fn canonical_url(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| Error::InvalidInput("Invalid Instagram post URL.".into()))?;
    let host = parsed.host_str().unwrap_or_default();
    if !matches!(parsed.scheme(), "http" | "https")
        || !(host == "instagram.com" || host.ends_with(".instagram.com"))
    {
        return Err(Error::InvalidInput("Not an Instagram post URL.".into()));
    }
    let path: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
    let kind =
        match path.first().copied() {
            Some("reel" | "reels") => "reel",
            Some("p") => "p",
            Some("tv") => "tv",
            _ => return Err(Error::InvalidInput(
                "Use an Instagram post or reel link, rather than an audio, profile, or feed link."
                    .into(),
            )),
        };
    let code = path.get(1).copied().unwrap_or_default();
    if matches!(code, "audio" | "liked" | "saved")
        || video::site::shortcode_to_media_id(code).is_none()
    {
        return Err(Error::InvalidInput(
            "The Instagram URL does not identify a post.".into(),
        ));
    }
    Ok(format!("https://www.instagram.com/{kind}/{code}/"))
}

#[derive(Clone)]
pub(crate) struct Post {
    pub album: ImageAlbum,
}

impl Post {
    pub fn into_video(self) -> Result<VideoMetadata> {
        let clips: Vec<_> = self
            .album
            .items
            .iter()
            .filter(|item| item.kind == MediaKind::Video)
            .collect();
        if clips.is_empty() {
            if self.album.completeness != Some(Completeness::Complete) {
                return Err(Error::InvalidInput("Instagram exposed only a preview image, not a video source. Open the post in your signed-in browser and refresh its shared sign-in before retrying.".into()));
            }
            return Err(Error::InvalidInput("Instagram returned this post's attachments, but none are videos. Use Images to view its photos.".into()));
        }
        let single = clips.len() == 1;
        let thumbnail = if single {
            clips[0].thumbnail.clone()
        } else {
            None
        };
        let duration = if single {
            clips[0]
                .formats
                .first()
                .and_then(|format| format.validation.as_ref()?.duration)
        } else {
            None
        };
        let mut formats = Vec::new();
        for clip in clips {
            for mut format in clip.formats.clone() {
                if !single {
                    format.id = format!("clip-{}-{}", clip.id, format.id);
                }
                format
                    .download_options
                    .get_or_insert_with(Default::default)
                    .thumbnail = clip.thumbnail.clone();
                formats.push(format);
            }
        }
        Ok(VideoMetadata {
            title: self.album.title,
            thumbnail,
            duration,
            uploader: self.album.uploader,
            webpage_url: self.album.source_url,
            formats,
            subtitles: Vec::new(),
        })
    }
}

fn parse_post(node: &Value, url: &str) -> Option<Post> {
    let mut items = Vec::new();
    let children = node
        .get("carousel_media")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            node.pointer("/edge_sidecar_to_children/edges")
                .and_then(Value::as_array)
                .map(|edges| {
                    edges
                        .iter()
                        .filter_map(|edge| edge.get("node").cloned())
                        .collect()
                })
        })
        .unwrap_or_else(|| vec![node.clone()]);
    for (index, child) in children.iter().enumerate() {
        if let Some(mut item) = crate::image::sites::ig_item(child, (index + 1).to_string()) {
            // Keep every clip's duration on its own formats, never on its neighbor.
            let duration = child
                .get("video_duration")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0);
            for format in &mut item.formats {
                format.validation = Some(video::validation::VideoValidation {
                    duration,
                    ..Default::default()
                });
            }
            items.push(item);
        }
    }
    if items.is_empty() {
        return None;
    }
    let title = node
        .pointer("/caption/text")
        .or_else(|| node.pointer("/edge_media_to_caption/edges/0/node/text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(160).collect())
        .unwrap_or_else(|| "Instagram post".into());
    let uploader = node
        .pointer("/user/username")
        .or_else(|| node.pointer("/owner/username"))
        .and_then(Value::as_str)
        .map(|name| format!("@{name}"));
    Some(Post {
        album: ImageAlbum {
            title,
            uploader,
            source_url: url.into(),
            completeness: Some(if items.len() == children.len() {
                Completeness::Complete
            } else {
                Completeness::Partial
            }),
            expected_count: Some(children.len()),
            items,
            ..Default::default()
        },
    })
}

fn targeted_node<'a>(
    json: &'a Value,
    code: &str,
    media_id: &str,
    depth: usize,
) -> Option<&'a Value> {
    if depth > 40 {
        return None;
    }
    let conflicting_code = ["shortcode", "code"].iter().any(|key| {
        json.get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| value != code)
    });
    let identified = ["shortcode", "code"]
        .iter()
        .any(|key| json.get(key).and_then(Value::as_str) == Some(code))
        || ["id", "pk"].iter().any(|key| {
            json.get(key).is_some_and(|value| {
                value
                    .as_str()
                    .is_some_and(|id| id.split('_').next() == Some(media_id))
                    || value.as_u64().is_some_and(|id| id.to_string() == media_id)
            })
        });
    if !conflicting_code
        && identified
        && (json.get("video_versions").is_some()
            || json.get("video_dash_manifest").is_some()
            || json.get("video_url").is_some()
            || json.get("carousel_media").is_some()
            || json.get("image_versions2").is_some()
            || json.get("display_url").is_some()
            || json.get("edge_sidecar_to_children").is_some())
    {
        return Some(json);
    }
    match json {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "comments" | "recommendations" | "related_stories"
                )
            })
            .find_map(|(_, value)| targeted_node(value, code, media_id, depth + 1)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| targeted_node(value, code, media_id, depth + 1)),
        _ => None,
    }
}

fn parse_api(
    body: &str,
    url: &str,
    code: &str,
    media_id: &str,
) -> std::result::Result<Post, &'static str> {
    let json: Value = serde_json::from_str(body)
        .map_err(|_| "returned an unsupported response instead of post JSON")?;
    let message = json
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["wait a few minutes", "rate limit", "too many requests"]
        .iter()
        .any(|reason| message.contains(reason))
    {
        return Err("was rate-limited; wait before retrying");
    }
    if [
        "login_required",
        "checkpoint_required",
        "challenge_required",
    ]
    .iter()
    .any(|message| json.get("message").and_then(Value::as_str) == Some(message))
    {
        return Err("requires a shared Instagram sign-in or a browser account check");
    }
    let node = targeted_node(&json, code, media_id, 0)
        .or_else(|| {
            let node = json.pointer("/items/0")?;
            // Older API payloads can omit identity fields. A conflicting ID is
            // never treated as an attachment of the requested post.
            if ["code", "shortcode", "id", "pk"]
                .iter()
                .any(|key| node.get(key).is_some())
            {
                None
            } else {
                Some(node)
            }
        })
        .ok_or("did not return the requested post object")?;
    parse_post(node, url).ok_or("returned the post but no supported attachment URLs")
}

fn contains_identified_post(value: &Value, depth: usize) -> bool {
    if depth > 40 {
        return false;
    }
    if ["code", "shortcode", "id", "pk"]
        .iter()
        .any(|key| value.get(key).is_some())
        && [
            "video_url",
            "video_versions",
            "video_dash_manifest",
            "carousel_media",
            "edge_sidecar_to_children",
            "image_versions2",
            "display_url",
        ]
        .iter()
        .any(|key| value.get(key).is_some())
    {
        return true;
    }
    match value {
        Value::Object(map) => map
            .values()
            .any(|child| contains_identified_post(child, depth + 1)),
        Value::Array(values) => values
            .iter()
            .any(|child| contains_identified_post(child, depth + 1)),
        _ => false,
    }
}

fn parse_page(body: &str, url: &str, code: &str, media_id: &str) -> Option<Post> {
    static SCRIPTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>(.*?)</script>").unwrap());
    let mut identified_posts = false;
    for script in SCRIPTS.captures_iter(body) {
        if let Ok(json) = serde_json::from_str::<Value>(script[1].trim()) {
            identified_posts |= contains_identified_post(&json, 0);
            if let Some(node) = targeted_node(&json, code, media_id, 0) {
                if let Some(post) = parse_post(node, url) {
                    return Some(post);
                }
            }
        }
    }
    // Structured payloads identify their posts explicitly. Do not let a
    // broad URL scan silently substitute a recommendation for the target.
    if identified_posts {
        return None;
    }
    let formats = video::site::extract_instagram_formats(body, url);
    let mut items = Vec::new();
    if let Some(item) = ImageItem::video("1", formats) {
        items.push(item.with_thumbnail(video::site::page_thumbnail(body)));
    } else {
        items.extend(crate::image::sites::og_items(body));
    }
    if items.is_empty() {
        return None;
    }
    Some(Post {
        album: ImageAlbum {
            title: video::site::page_title(body, url),
            source_url: url.into(),
            items,
            ..Default::default()
        },
    })
}

async fn read_stage(
    client: &Client,
    url: &str,
    headers: &Headers,
) -> std::result::Result<String, String> {
    let request = async {
        let mut response = video::net::fetch_response(client, url, headers).await.map_err(|error| match error {
            Error::UnexpectedStatus { status: 429, .. } => "was rate-limited (HTTP 429); wait before retrying".into(),
            Error::UnexpectedStatus { status: 401 | 403, .. } => "was denied access; refresh the shared sign-in or check the post in your browser".into(),
            Error::UnexpectedStatus { status, .. } => format!("returned HTTP {status}"),
            Error::Http(error) if error.is_timeout() => "request timed out".into(),
            Error::Http(error) if error.is_builder() => "request headers were invalid; refresh the shared sign-in".into(),
            Error::Http(error) if error.is_connect() => "connection failed; check network or proxy access to Instagram".into(),
            _ => "could not reach Instagram".into(),
        })?;
        if ["/accounts/login", "/challenge", "/checkpoint"]
            .iter()
            .any(|path| response.url().path().starts_with(path))
        {
            return Err("redirected to a sign-in or account-check page".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "could not read Instagram's response".to_string())?
        {
            if bytes.len() + chunk.len() > MAX_BODY {
                return Err("response exceeded the extraction limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    tokio::time::timeout(Duration::from_secs(12), request)
        .await
        .unwrap_or_else(|_| Err("timed out".into()))
}

pub(crate) async fn resolve(client: &Client, url: &str, headers: &Headers) -> Result<Post> {
    resolve_at(client, url, headers, "https://www.instagram.com").await
}

pub(crate) async fn invalidate(url: &str) {
    invalidate_at(url, "https://www.instagram.com").await;
}

async fn invalidate_at(url: &str, origin: &str) {
    if let Ok(canonical) = canonical_url(url) {
        let address = format!("{origin}{canonical}");
        RECENT_POSTS.lock().await.retain(|(key, _, _, _)| key != &address);
    }
}

async fn resolve_at(client: &Client, url: &str, headers: &Headers, origin: &str) -> Result<Post> {
    let canonical = canonical_url(url)?;
    let key = format!("{origin}{canonical}");
    let mut session: Headers = headers.iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("referer"))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())).collect();
    session.sort();
    {
        let mut cache = RECENT_POSTS.lock().await;
        cache.retain(|(_, _, when, _)| when.elapsed() < Duration::from_secs(30));
        if let Some((_, _, _, post)) = cache.iter().find(|(address, saved, _, _)| address == &key && saved == &session) {
            return Ok(post.clone());
        }
    }
    let post = resolve_uncached(client, &canonical, headers, origin).await?;
    if post.album.items.iter().any(|item| item.kind == MediaKind::Video) {
        let mut cache = RECENT_POSTS.lock().await;
        cache.retain(|(address, saved, when, _)| when.elapsed() < Duration::from_secs(30)
            && !(address == &key && saved == &session));
        if cache.len() >= 16 { cache.remove(0); }
        cache.push((key, session, Instant::now(), post.clone()));
    }
    Ok(post)
}

async fn resolve_uncached(client: &Client, url: &str, headers: &Headers, origin: &str) -> Result<Post> {
    let canonical = canonical_url(url)?;
    let (kind, code) = video::site::instagram_shortcode(&canonical).unwrap();
    let media_id = video::site::shortcode_to_media_id(code)
        .unwrap()
        .to_string();
    let mut api_headers = headers.clone();
    api_headers.retain(|(name, _)| !name.eq_ignore_ascii_case("x-ig-app-id"));
    api_headers.push(("X-IG-App-ID".into(), APP_ID.into()));
    let api = format!("{origin}/api/v1/media/{media_id}/info/");
    let mut failures = Vec::new();
    let mut preview = None;
    match read_stage(client, &api, &api_headers).await {
        Ok(body) => match parse_api(&body, &canonical, code, &media_id) {
            Ok(post) => return Ok(post),
            Err(reason) => failures.push(format!("media API {reason}")),
        },
        Err(reason) => failures.push(format!("media API {reason}")),
    }
    if failures
        .last()
        .is_some_and(|reason| reason.contains("rate-limited"))
    {
        return Err(Error::InvalidInput(format!(
            "Instagram extraction failed: {}.",
            failures.join("; ")
        )));
    }
    for (stage, address) in [
        ("post page", format!("{origin}/{kind}/{code}/")),
        (
            "embed page",
            format!("{origin}/{kind}/{code}/embed/captioned/"),
        ),
    ] {
        match read_stage(client, &address, headers).await {
            Ok(body) => {
                if let Some(post) = parse_page(&body, &canonical, code, &media_id) {
                    if post.album.completeness == Some(Completeness::Complete)
                        || post
                            .album
                            .items
                            .iter()
                            .any(|item| item.kind == MediaKind::Video)
                    {
                        return Ok(post);
                    }
                    // An og:image is often just a reel's poster. Keep it for
                    // Images, but do not let it prevent the embed video attempt.
                    preview.get_or_insert(post);
                    continue;
                }
                failures.push(format!("{stage} contained no supported post media"));
            }
            Err(reason) => {
                let limited = reason.contains("rate-limited");
                failures.push(format!("{stage} {reason}"));
                if limited {
                    break;
                }
            }
        }
    }
    if let Some(post) = preview {
        return Ok(post);
    }
    // Diagnostics name stages and classifications, never cookies, response
    // bodies, signed attachment URLs, or tracking parameters.
    Err(Error::InvalidInput(format!(
        "Instagram extraction failed: {}.",
        failures.join("; ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use std::sync::{Arc, Mutex};
    const CODE: &str = "Dd9M2ZgRDNs";
    const URL: &str = "https://www.instagram.com/reels/Dd9M2ZgRDNs/";
    const SHARED: &str = "https://www.instagram.com/reel/Dd9M2ZgRDNs/?utm_source=ig_web_copy_link&stkn=NTc4MTIwNjQ2YQ==";

    fn payload(base: &str) -> Value {
        serde_json::json!({"items":[{"code":CODE,"caption":{"text":"Requested post"},"user":{"username":"creator"},
        "carousel_media":[
            {"media_type":1,"image_versions2":{"candidates":[{"url":format!("{base}/poster.jpg"),"width":720,"height":1280}]}},
            {"media_type":2,"video_duration":1.0,"video_versions":[{"url":format!("{base}/clip?token=signed"),"width":720,"height":1280}],
                "image_versions2":{"candidates":[{"url":format!("{base}/poster.jpg"),"width":720,"height":1280}]}}
        ]}]})
    }

    #[derive(Clone)]
    struct Fixture {
        base: String,
        mode: &'static str,
        requests: Arc<Mutex<Vec<String>>>,
    }
    async fn api(
        State(state): State<Fixture>,
        Path(id): Path<String>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        state.requests.lock().unwrap().push("api".into());
        assert_eq!(
            id,
            video::site::shortcode_to_media_id(CODE)
                .unwrap()
                .to_string()
        );
        assert_eq!(headers.get("x-ig-app-id").unwrap(), APP_ID);
        if state.mode == "limited" {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        if matches!(state.mode, "embed" | "preview") {
            return StatusCode::BAD_GATEWAY.into_response();
        }
        if state.mode == "unsupported" {
            return (
                [("content-type", "application/json")],
                "{\"changed_response\":true}",
            )
                .into_response();
        }
        if headers
            .get("cookie")
            .and_then(|header| header.to_str().ok())
            != Some("session=ok")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        axum::Json(payload(&state.base)).into_response()
    }
    async fn page(State(state): State<Fixture>) -> axum::response::Response {
        state.requests.lock().unwrap().push("page".into());
        if state.mode == "preview" {
            return (
                [("content-type", "text/html")],
                format!(
                    "<meta property='og:image' content='{}/poster.jpg'>",
                    state.base
                ),
            )
                .into_response();
        }
        StatusCode::FORBIDDEN.into_response()
    }
    async fn embed(State(state): State<Fixture>) -> axum::response::Response {
        state.requests.lock().unwrap().push("embed".into());
        if matches!(state.mode, "embed" | "preview") {
            return ([("content-type", "text/html")], format!("<title>Embedded clip</title><meta property='og:video' content='{}/clip?token=signed'>", state.base)).into_response();
        }
        StatusCode::FORBIDDEN.into_response()
    }
    async fn origin(mode: &'static str) -> (Fixture, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture {
            base: format!("http://{}", listener.local_addr().unwrap()),
            mode,
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/api/v1/media/{id}/info/", get(api))
            .route("/reel/Dd9M2ZgRDNs/", get(page))
            .route("/reel/Dd9M2ZgRDNs/embed/captioned/", get(embed))
            .route(
                "/clip",
                get(|| async {
                    (
                        [("content-type", "video/mp4")],
                        include_bytes!("../tests/fixtures/verified-av.mp4").as_slice(),
                    )
                }),
            )
            .with_state(fixture.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (fixture, task)
    }

    #[test]
    fn exact_reported_urls_normalize_to_the_same_post_without_touching_media_urls() {
        assert_eq!(canonical_url(URL).unwrap(), canonical_url(SHARED).unwrap());
        assert_eq!(
            canonical_url(URL).unwrap(),
            format!("https://www.instagram.com/reel/{CODE}/")
        );
        for value in [
            "https://www.instagram.com/reels/audio/123/",
            "https://instagram.com.evil.test/reel/ABC/",
            "https://www.instagram.com/reels/",
        ] {
            assert!(canonical_url(value).is_err());
        }
    }

    #[tokio::test]
    async fn both_urls_and_tab_adapters_return_the_same_nested_clip_and_download_real_video() {
        let (fixture, task) = origin("success").await;
        let client = Client::new();
        let headers = vec![("Cookie".into(), "session=ok".into())];
        for url in [URL, SHARED] {
            let post = resolve_at(&client, url, &headers, &fixture.base)
                .await
                .unwrap();
            let album = post.album.clone();
            let video = post.into_video().unwrap();
            assert_eq!(album.items.len(), 2);
            assert_eq!(video.formats.len(), 1);
            assert_eq!(
                video.formats[0].video_url.as_deref(),
                Some(album.items[1].url.as_str())
            );
            assert_eq!(video.formats[0].height, Some(1280));
            assert_eq!(
                video.formats[0].validation.as_ref().unwrap().duration,
                Some(1.0)
            );
            assert_eq!(video.webpage_url, canonical_url(URL).unwrap());
            assert!(
                video.formats[0]
                    .video_url
                    .as_ref()
                    .unwrap()
                    .ends_with("?token=signed")
            );
            let directory = tempfile::tempdir().unwrap();
            let file = directory.path().join("clip.mp4");
            video::stream::fetch(
                &client,
                video.formats[0].video_url.as_ref().unwrap(),
                &headers,
                &file,
            )
            .await
            .unwrap();
            assert_eq!(
                tokio::fs::read(&file).await.unwrap(),
                include_bytes!("../tests/fixtures/verified-av.mp4")
            );
            let tracks = video::mp4probe::probe(
                &client,
                video.formats[0].video_url.as_ref().unwrap(),
                &headers,
            )
            .await
            .unwrap();
            assert!(tracks.video && tracks.audio);
            if crate::media_tools::ffprobe_command()
                .arg("-version")
                .output()
                .await
                .is_ok()
            {
                let report = video::validation::inspect_output(&file, &video.formats[0])
                    .await
                    .unwrap();
                assert_eq!(report.has_video, Some(true));
                assert_eq!(report.has_audio, Some(true));
            }
        }
        assert_eq!(*fixture.requests.lock().unwrap(), vec!["api"]);
        task.abort();
    }

    #[tokio::test]
    async fn recent_posts_are_session_scoped_and_refresh_can_bypass_the_cache() {
        let (fixture, task) = origin("success").await;
        let client = Client::new();
        let headers = vec![("Cookie".into(), "session=ok".into())];
        resolve_at(&client, URL, &headers, &fixture.base).await.unwrap();
        let mut browser_headers = headers.clone();
        browser_headers.push(("Referer".into(), "https://www.instagram.com/".into()));
        resolve_at(&client, SHARED, &browser_headers, &fixture.base).await.unwrap();
        assert_eq!(*fixture.requests.lock().unwrap(), vec!["api"]);
        assert!(resolve_at(&client, URL, &vec![], &fixture.base).await.is_err());
        invalidate_at(URL, &fixture.base).await;
        resolve_at(&client, URL, &headers, &fixture.base).await.unwrap();
        assert_eq!(fixture.requests.lock().unwrap().last().unwrap(), "api");
        task.abort();
    }

    #[test]
    fn distinct_carousel_clips_keep_their_formats_and_durations_separate() {
        let mut json = payload("https://cdn.example");
        let mut second = json["items"][0]["carousel_media"][1].clone();
        second["video_versions"][0]["url"] = "https://cdn.example/another?token=other".into();
        second["video_duration"] = 12.0.into();
        json["items"][0]["carousel_media"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let post = parse_api(
            &json.to_string(),
            URL,
            CODE,
            &video::site::shortcode_to_media_id(CODE)
                .unwrap()
                .to_string(),
        )
        .unwrap();
        let video = post.into_video().unwrap();
        assert_eq!(video.formats.len(), 2);
        assert_eq!(video.duration, None);
        assert!(video.formats[0].id.starts_with("clip-2-"));
        assert!(video.formats[1].id.starts_with("clip-3-"));
        assert_eq!(
            video.formats[1].validation.as_ref().unwrap().duration,
            Some(12.0)
        );
        assert!(
            video
                .formats
                .iter()
                .all(|format| format.audio_url.is_none() && !format.requires_merge)
        );
    }

    #[tokio::test]
    async fn a_denied_page_does_not_prevent_embed_recovery() {
        let (fixture, task) = origin("embed").await;
        let post = resolve_at(&Client::new(), SHARED, &vec![], &fixture.base)
            .await
            .unwrap();
        assert_eq!(post.into_video().unwrap().formats.len(), 1);
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec!["api", "page", "embed"]
        );
        task.abort();
    }

    #[tokio::test]
    async fn a_preview_image_does_not_prevent_embed_video_recovery() {
        let (fixture, task) = origin("preview").await;
        let post = resolve_at(&Client::new(), SHARED, &vec![], &fixture.base)
            .await
            .unwrap();
        assert_eq!(post.into_video().unwrap().formats.len(), 1);
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec!["api", "page", "embed"]
        );
        task.abort();
    }

    #[tokio::test]
    async fn failures_identify_the_stage_without_exposing_credentials_or_claiming_privacy() {
        for mode in ["limited", "unsupported", "success"] {
            let (fixture, task) = origin(mode).await;
            let error = resolve_at(&Client::new(), SHARED, &vec![], &fixture.base)
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("media API"));
            assert!(
                !error.contains("stkn")
                    && !error.contains("private")
                    && !error.contains("session=ok")
            );
            match mode {
                "limited" => assert!(error.contains("rate-limited")),
                "unsupported" => assert!(error.contains("post object")),
                _ => assert!(error.contains("denied access")),
            }
            if mode == "limited" {
                assert_eq!(*fixture.requests.lock().unwrap(), vec!["api"]);
            }
            task.abort();
        }
    }

    #[test]
    fn structured_recommendations_cannot_replace_the_requested_clip() {
        let mut json = payload("https://cdn.example");
        json["items"][0]["code"] = "OtherCode".into();
        let media_id = video::site::shortcode_to_media_id(CODE)
            .unwrap()
            .to_string();
        assert!(parse_api(&json.to_string(), URL, CODE, &media_id).is_err());
        let html = format!("<script type='application/json'>{json}</script>");
        assert!(parse_page(&html, URL, CODE, &media_id).is_none());
    }

    #[test]
    fn unrelated_configuration_does_not_disable_a_valid_embed_clip() {
        let html = r#"<script type="application/json">{"code":"SUCCESS"}</script><meta property="og:video" content="https://cdn.example/clip.mp4?token=signed">"#;
        let media_id = video::site::shortcode_to_media_id(CODE)
            .unwrap()
            .to_string();
        assert!(parse_page(html, URL, CODE, &media_id).is_some());
        assert!(
            parse_api(
                r#"{"message":"Please wait a few minutes before you try again."}"#,
                URL,
                CODE,
                &media_id
            )
            .err()
            .unwrap()
            .contains("rate-limited")
        );
    }

    #[test]
    fn an_authoritative_photo_post_reports_no_video_without_claiming_a_login_problem() {
        let mut json = payload("https://cdn.example");
        json["items"][0]["carousel_media"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        let post = parse_api(
            &json.to_string(),
            URL,
            CODE,
            &video::site::shortcode_to_media_id(CODE)
                .unwrap()
                .to_string(),
        )
        .unwrap();
        assert_eq!(post.album.items.len(), 1);
        let error = post.into_video().unwrap_err().to_string();
        assert!(error.contains("none are videos"));
        assert!(!error.contains("private") && !error.contains("login"));
    }
}
