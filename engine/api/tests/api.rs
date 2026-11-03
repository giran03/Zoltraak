//! End-to-end tests over a real socket.
//!
//! These go through `reqwest` rather than calling handlers directly, because
//! most of what is being asserted lives in the layers a direct call skips:
//! header parsing, status codes, and the auth middleware.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use zoltraak_api::{ApiState, ApiToken};
use zoltraak_core::downloader::Options;
use zoltraak_core::runner::RunnerConfig;
use zoltraak_core::{Downloader, Queue, Supervisor};
use serde_json::json;

const TOKEN: &str = "test-token-0123456789";

#[tokio::test]
async fn guide_theme_is_public_but_only_exposes_valid_palette_values() {
    let h = Harness::start().await;
    *h.state.guide_theme.write().unwrap() = json!({
        "colorScheme": "light",
        "variables": {"--panel": "#abcdef", "--route": "url(bad)", "unexpected": "hidden"},
        "geometry": {"radius": "rounded", "density": "<bad>"},
        "extra": "not exposed"
    });
    let response = h.get("/guide/theme").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let palette: serde_json::Value = response.json().await.unwrap();
    assert_eq!(palette, json!({
        "colorScheme": "light",
        "variables": {"--panel": "#abcdef"},
        "geometry": {"radius": "rounded", "density": "default"}
    }));
    let html = h.get("/guide?view=changelog").send().await.unwrap().text().await.unwrap();
    assert!(html.contains("changelogPage"));
    assert!(html.contains("--c-panel:#abcdef"));
    assert!(html.contains(r#"data-theme="light" data-motion="full""#));
}

struct Harness {
    base: String,
    client: reqwest::Client,
    supervisor: Supervisor,
    state: ApiState,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = Supervisor::start(
            Queue::open_in_memory().await.unwrap(),
            Downloader::new(reqwest::Client::new(), Options::default()),
            RunnerConfig {
                // The queue must stay put: these tests assert on rows, and a
                // worker racing off to fetch a nonexistent host would rewrite
                // state underneath them.
                max_concurrent: 1,
                idle_poll: Duration::from_secs(3600),
                ..RunnerConfig::default()
            },
        );

        let video = zoltraak_core::VideoManager::new(reqwest::Client::new(), supervisor.queue().pool())
        .with_rate_limiter(supervisor.rate_limiter());

        let state = ApiState {
            extension_theme: Default::default(),
            guide_theme: Default::default(),
            token: Arc::new(ApiToken::from_existing(TOKEN)),
            supervisor: supervisor.clone(),
            video,
            default_directory: Arc::new(dir.path().to_path_buf()),
            download_prefill_tx: None,
            image_prefill_tx: None,
            prefill_tx: None,
            folder_picker_tx: None,
            music: None,
            music_prefill_tx: None,
            remux: zoltraak_core::RemuxManager::new(),
            // Discord delivery is exercised in zoltraak-core's own tests; these
            // routes only need to answer 503 here.
            discord: None,
            auto_send: None,
            pairing_window: Arc::new(std::sync::RwLock::new(Some(
                std::time::Instant::now() + std::time::Duration::from_secs(120),
            ))),
            reload_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            reload_tx: Arc::new(tokio::sync::watch::channel(0u64).0),
            extension_sources: Default::default(),
            extension_source_switch: Default::default(),
            app_navigate_tx: None,
        };

        // Port 0: the OS picks a free one, so tests never collide with a real
        // engine on 1450 or with each other.
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let state_for_test = state.clone();
        tokio::spawn(zoltraak_api::serve(listener, state));

        Self {
            base: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            supervisor,
            state: state_for_test,
            _dir: dir,
        }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.post(format!("{}{path}", self.base))
    }

    /// An authenticated request, as the extension would make it.
    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder.bearer_auth(TOKEN)
    }
}

const CLIENT_A: &str = "00000000-0000-4000-8000-000000000001";
const CLIENT_B: &str = "00000000-0000-4000-8000-000000000002";

async fn heartbeat(h: &Harness, client_id: &str, browser_id: &str) -> serde_json::Value {
    h.authed(h.get("/extension/status?reloadVersion=0&sourceRevision=0"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", client_id)
        .header("X-Zoltraak-Browser-Id", browser_id)
        .header("X-Zoltraak-Extension-Version", "1.2.3")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn health_needs_no_token() {
    // The extension has to be able to tell "engine not running" apart from
    // "engine running, we are not paired with it".
    let h = Harness::start().await;
    let response = h.get("/health").send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<serde_json::Value>().await.unwrap()["status"], "ok");
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn everything_else_needs_a_token() {
    let h = Harness::start().await;

    for (method, path) in [
        ("GET", "/downloads"),
        ("GET", "/extension/theme"),
        ("POST", "/download"),
        ("POST", "/downloads/whatever/pause"),
    ] {
        let builder = match method {
            "GET" => h.get(path),
            _ => h.post(path).json(&json!({ "url": "https://example.com/f" })),
        };
        assert_eq!(
            builder.send().await.unwrap().status(),
            401,
            "{method} {path} answered an unauthenticated caller"
        );
    }

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_wrong_token_is_rejected() {
    let h = Harness::start().await;
    let response = h
        .get("/downloads")
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    h.supervisor.shutdown().await;
}

/// The single most important test in this file: a web page must not be able to
/// drive the engine even if it somehow learns the token.
#[tokio::test]
async fn a_web_origin_is_refused_even_with_a_valid_token() {
    let h = Harness::start().await;

    for origin in [
        "https://evil.example",
        "http://localhost:3000",
        "null",
        "https://chrome-extension://spoofed",
    ] {
        let response = h
            .authed(h.get("/downloads"))
            .header("Origin", origin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "origin {origin} was let through");
    }

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn an_extension_origin_is_accepted() {
    let h = Harness::start().await;
    let response = h
        .authed(h.get("/downloads"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    h.supervisor.shutdown().await;
}

/// No route may answer with CORS headers. Without them a page cannot pass a
/// preflight, so it can never attach an `Authorization` header in the first
/// place — this is the barrier that sits in front of the origin check.
#[tokio::test]
async fn no_route_hands_out_cors_headers() {
    let h = Harness::start().await;

    for response in [
        h.get("/health").send().await.unwrap(),
        h.authed(h.get("/downloads")).send().await.unwrap(),
    ] {
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "a CORS header would let a page read the response"
        );
    }

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_download_can_be_added_listed_and_removed() {
    let h = Harness::start().await;

    let created = h
        .authed(h.post("/download"))
        .json(&json!({
            "url": "https://example.com/big.iso",
            "filename": "big.iso",
            "priority": 5,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let id = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let listed = h
        .authed(h.get("/downloads"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["filename"], "big.iso");
    // camelCase, because the same shape feeds the desktop UI.
    assert!(listed[0]["downloadedBytes"].is_number());

    let removed = h
        .authed(h.client.delete(format!("{}/downloads/{id}", h.base)))
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), 204);

    assert!(
        h.authed(h.get("/downloads"))
            .send()
            .await
            .unwrap()
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap()
            .is_empty()
    );

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn non_http_schemes_are_refused() {
    let h = Harness::start().await;

    // `file://` is the one that matters: without this check, anything holding
    // the token could copy arbitrary local files into the download directory.
    for url in ["file:///etc/passwd", "ftp://example.com/f", "javascript:0"] {
        let response = h
            .authed(h.post("/download"))
            .json(&json!({ "url": url }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{url} was accepted");
    }

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn facebook_browser_metadata_prefill_validates_post_and_profile_ownership() {
    let h = Harness::start().await;
    heartbeat(&h, CLIENT_A, "chrome").await;
    heartbeat(&h, CLIENT_B, "firefox").await;
    let body = json!({"url":"https://www.facebook.com/user/posts/pfbidSelected", "browserDocuments":[{
        "__typename":"Story", "permalink_url":"https://www.facebook.com/user/posts/pfbidSelected",
        "post_id":"12345", "attachments":[{"all_subattachments":{"count":1,"nodes":[{
            "__typename":"Photo", "id":"1", "image":{"uri":"https://scontent.fbcdn.net/photo.jpg"}
        }]}}]
    }]});
    let active = h.authed(h.post("/image/prefill")).timeout(Duration::from_secs(5)).header("X-Zoltraak-Client-Id", CLIENT_A)
        .json(&body).send().await.unwrap();
    assert_eq!(active.status(), 204);
    let inactive = h.authed(h.post("/image/prefill")).timeout(Duration::from_secs(5)).header("X-Zoltraak-Client-Id", CLIENT_B)
        .json(&body).send().await.unwrap();
    assert_eq!(inactive.status(), 423);
    let mut foreign = body.clone(); foreign["url"] = json!("https://www.facebook.com/user/posts/pfbidOther");
    assert_eq!(h.authed(h.post("/image/prefill")).timeout(Duration::from_secs(5)).header("X-Zoltraak-Client-Id", CLIENT_A)
        .json(&foreign).send().await.unwrap().status(), 400);
    let mut invalid = body.clone(); invalid["browserDocuments"][0]["attachments"][0]["all_subattachments"]["nodes"][0]["image"]["uri"] = json!("https://127.0.0.1/private.jpg");
    assert_eq!(h.authed(h.post("/image/prefill")).timeout(Duration::from_secs(5)).header("X-Zoltraak-Client-Id", CLIENT_A)
        .json(&invalid).send().await.unwrap().status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn image_prefill_accepts_posts_and_rejects_local_urls() {
    let h = Harness::start().await;

    let accepted = h
        .authed(h.post("/image/prefill"))
        .json(&json!({ "url": "https://www.instagram.com/p/selected/" }))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 204);

    let rejected = h
        .authed(h.post("/image/prefill"))
        .json(&json!({ "url": "file:///etc/passwd" }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_filename_cannot_escape_the_destination_directory() {
    let h = Harness::start().await;

    let created = h
        .authed(h.post("/download"))
        .json(&json!({
            "url": "https://example.com/f",
            "filename": "../../evil.exe",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    let listed = h
        .authed(h.get("/downloads"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    let filename = listed[0]["filename"].as_str().unwrap();
    assert!(
        !filename.contains('/') && !filename.contains('\\'),
        "traversal survived: {filename}"
    );
    // Containment comes from taking the last segment, not from mangling the
    // separators — so the name that survives is a name, not a flattened path.
    assert_eq!(filename, "evil.exe");

    h.supervisor.shutdown().await;
}

/// A captured browser download arrives as the absolute path the browser was
/// going to write to. The file has to keep the name the user saw, not a
/// flattened rendering of the whole path.
#[tokio::test]
async fn a_captured_download_keeps_the_name_the_browser_chose() {
    let h = Harness::start().await;

    let created = h
        .authed(h.post("/download"))
        .json(&json!({
            "url": "https://example.com/dl?id=91827",
            "filename": r"C:\Users\guill\Downloads\create-glowstone-dust-recipe-1.jar",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    let listed = h
        .authed(h.get("/downloads"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(listed[0]["filename"], "create-glowstone-dust-recipe-1.jar");

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_relative_directory_is_refused() {
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/download"))
        .json(&json!({ "url": "https://example.com/f", "directory": "downloads" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_duplicate_download_is_a_conflict_not_a_server_error() {
    let h = Harness::start().await;
    let body = json!({ "url": "https://example.com/f", "filename": "f" });

    assert_eq!(
        h.authed(h.post("/download")).json(&body).send().await.unwrap().status(),
        201
    );
    // A double-click in the extension should be a clear answer, not a 500.
    assert_eq!(
        h.authed(h.post("/download")).json(&body).send().await.unwrap().status(),
        409
    );

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_malformed_id_is_a_bad_request_and_an_unknown_one_is_not_found() {
    let h = Harness::start().await;

    assert_eq!(
        h.authed(h.post("/downloads/not-a-uuid/pause")).send().await.unwrap().status(),
        400
    );
    assert_eq!(
        h.authed(h.post(&format!("/downloads/{}/resume", uuid::Uuid::new_v4())))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_category_can_be_added_listed_and_removed() {
    let h = Harness::start().await;

    let created = h
        .authed(h.post("/categories"))
        .json(&json!({
            "name": "Archives",
            "kind": "extension",
            "pattern": "zip,rar",
            "directory": h._dir.path(),
            "priority": 5,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let id = created.json::<serde_json::Value>().await.unwrap()["id"].as_i64().unwrap();

    let listed = h
        .authed(h.get("/categories"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "Archives");
    assert_eq!(listed[0]["enabled"], true);

    assert_eq!(
        h.authed(h.post(&format!("/categories/{id}/disable")))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let listed = h
        .authed(h.get("/categories"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(listed[0]["enabled"], false);

    assert_eq!(
        h.authed(h.client.delete(format!("{}/categories/{id}", h.base)))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert!(
        h.authed(h.get("/categories"))
            .send()
            .await
            .unwrap()
            .json::<Vec<serde_json::Value>>()
            .await
            .unwrap()
            .is_empty()
    );

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_category_routes_a_matching_download_automatically() {
    let h = Harness::start().await;

    let target = h._dir.path().join("archives");
    h.authed(h.post("/categories"))
        .json(&json!({
            "name": "Archives",
            "kind": "extension",
            "pattern": "zip",
            "directory": target,
        }))
        .send()
        .await
        .unwrap();

    let created = h
        .authed(h.post("/download"))
        .json(&json!({ "url": "https://example.com/thing.zip" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    let listed = h
        .authed(h.get("/downloads"))
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();
    assert_eq!(listed[0]["directory"], target.to_string_lossy().as_ref());

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_relative_category_directory_is_refused() {
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/categories"))
        .json(&json!({
            "name": "Bad",
            "kind": "extension",
            "pattern": "zip",
            "directory": "relative/path",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn an_invalid_category_regex_is_rejected() {
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/categories"))
        .json(&json!({
            "name": "Bad",
            "kind": "regex",
            "pattern": "(unclosed",
            "directory": h._dir.path(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn settings_default_then_round_trip_through_put() {
    let h = Harness::start().await;

    let defaults = h
        .authed(h.get("/settings"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(defaults["bandwidthLimitBytesPerSec"], serde_json::Value::Null);

    let updated = json!({
        "bandwidthLimitBytesPerSec": 250000,
        "defaultConnections": 6,
        "defaultDirectory": h._dir.path(),
        "schedule": { "enabled": true, "startMinutes": 60, "stopMinutes": 120 },
        "onComplete": { "type": "run_command", "command": "echo hi" },
    });
    assert_eq!(
        h.authed(h.client.put(format!("{}/settings", h.base)))
            .json(&updated)
            .send()
            .await
            .unwrap()
            .status(),
        204
    );

    let got = h
        .authed(h.get("/settings"))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(got["bandwidthLimitBytesPerSec"], 250000);
    assert_eq!(got["defaultConnections"], 6);
    assert_eq!(got["schedule"]["startMinutes"], 60);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_relative_default_directory_in_settings_is_refused() {
    let h = Harness::start().await;
    let response = h
        .authed(h.client.put(format!("{}/settings", h.base)))
        .json(&json!({ "defaultDirectory": "relative/path" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

/// The reason the whole header path exists: a link behind a login is only
/// fetchable with the browser's own session attached.
#[tokio::test]
async fn forwarded_headers_are_stored_and_dangerous_ones_dropped() {
    let h = Harness::start().await;

    let created = h
        .authed(h.post("/download"))
        .json(&json!({
            "url": "https://example.com/private.zip",
            "headers": [["Cookie", "session=abc"], ["Range", "bytes=0-10"]],
            "referrer": "https://example.com/page",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    let id: zoltraak_core::DownloadId = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let item = h.supervisor.queue().get(id).await.unwrap();
    // Sanitizing preserves the caller's casing; HTTP header names are
    // case-insensitive, so compare that way rather than pinning a convention.
    let names: Vec<String> = item
        .headers
        .iter()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();

    assert!(names.iter().any(|n| n == "cookie"), "the session cookie was lost: {names:?}");
    assert!(names.iter().any(|n| n == "referer"), "referrer was not mapped: {names:?}");
    assert!(
        !names.iter().any(|n| n == "range"),
        "a caller-supplied Range would corrupt segmented writes"
    );

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn inactive_extension_actions_are_kept_but_browser_credentials_are_removed() {
    let h = Harness::start().await;
    heartbeat(&h, CLIENT_A, "chrome").await;
    heartbeat(&h, CLIENT_B, "firefox").await;

    let created = h
        .authed(h.post("/download"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", CLIENT_B)
        .json(&json!({
            "url": "https://example.com/from-inactive.zip",
            "headers": [["Cookie", "session=must-not-cross"], ["X-Ordinary", "kept"]]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let id: zoltraak_core::DownloadId = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let item = h.supervisor.queue().get(id).await.unwrap();
    assert!(!item.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("cookie")));
    assert!(item.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("x-ordinary")));

    let active = h
        .authed(h.post("/download"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", CLIENT_A)
        .json(&json!({
            "url": "https://example.com/from-active.zip",
            "headers": [["Cookie", "session=allowed"]]
        }))
        .send()
        .await
        .unwrap();
    let active_id: zoltraak_core::DownloadId = active.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let active_item = h.supervisor.queue().get(active_id).await.unwrap();
    assert!(active_item.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("cookie")));

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn analyzing_a_direct_media_url_tolerates_unavailable_probing_and_returns_one_format() {
    // A direct link is recognized by extension alone, so this never actually
    // reaches the (nonexistent) host — exactly what makes it a good route
    // test rather than an integration test of the extractor.
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/video/analyze"))
        .json(&json!({ "url": "https://example.invalid/clip.mp4" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    let formats = body["formats"].as_array().unwrap();
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0]["container"], "mp4");
    assert_eq!(formats[0]["requiresMerge"], false);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn video_analyze_rejects_non_http_schemes() {
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/video/analyze"))
        .json(&json!({ "url": "file:///etc/passwd" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_video_job_can_be_started_listed_and_removed() {
    let h = Harness::start().await;

    let analyzed = h
        .authed(h.post("/video/analyze"))
        .json(&json!({ "url": "https://example.invalid/clip.mp4" }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let format = analyzed["formats"][0].clone();

    let created = h
        .authed(h.post("/video/download"))
        .json(&json!({
            "title": "clip",
            "webpageUrl": "https://example.invalid/clip.mp4",
            "format": format,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let id: zoltraak_core::DownloadId = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let listed = h.authed(h.get("/video/jobs")).send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    assert!(listed.as_array().unwrap().iter().any(|j| j["id"] == id.to_string()));

    let removed = h.authed(h.client.delete(format!("{}/video/jobs/{id}", h.base))).send().await.unwrap();
    assert_eq!(removed.status(), 204);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_shared_session_is_minimised_listed_without_values_and_forgettable() {
    let h = Harness::start().await;

    // The extension shares a YouTube session with a full cookie jar.
    let res = h
        .authed(h.post("/sessions"))
        .json(&json!({
            "domain": "youtube.com",
            "cookie": "SAPISID=secret-value; LOGIN_INFO=li; tracking=junk",
            "poToken": "pot-value",
            "visitorData": "vis"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    let listed: serde_json::Value = h.authed(h.get("/sessions")).send().await.unwrap().json().await.unwrap();
    let rendered = listed.to_string();
    assert!(!rendered.contains("secret-value"), "cookie value leaked: {rendered}");
    assert!(!rendered.contains("pot-value"), "token leaked: {rendered}");
    let entry = &listed[0];
    assert_eq!(entry["domain"], "youtube.com");
    assert_eq!(entry["signedIn"], true);
    assert_eq!(entry["hasPoToken"], true);
    assert_eq!(entry["cookieNames"], json!(["SAPISID", "LOGIN_INFO"]), "tracking cookies are dropped on arrival");

    // The legacy header-list shape still works, for older extensions.
    let res = h
        .authed(h.post("/site-cookies"))
        .json(&json!({ "domain": "x.com", "headers": [["Cookie", "auth_token=abc"]] }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let listed: serde_json::Value = h.authed(h.get("/sessions")).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2);

    // A signed-out browser (nothing usable) removes the session.
    let res = h
        .authed(h.post("/sessions"))
        .json(&json!({ "domain": "youtube.com", "cookie": "tracking=junk" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let listed: serde_json::Value = h.authed(h.get("/sessions")).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["domain"], "x.com");

    // Forget one, then everything.
    let res = h.authed(h.client.delete(format!("{}/sessions/x.com", h.base))).send().await.unwrap();
    assert_eq!(res.status(), 204);
    let res = h.authed(h.client.delete(format!("{}/sessions/x.com", h.base))).send().await.unwrap();
    assert_eq!(res.status(), 404);
    let res = h.authed(h.client.delete(format!("{}/sessions", h.base))).send().await.unwrap();
    assert_eq!(res.status(), 204);
    let listed: serde_json::Value = h.authed(h.get("/sessions")).send().await.unwrap().json().await.unwrap();
    assert!(listed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn only_the_active_extension_profile_may_share_sessions() {
    let h = Harness::start().await;
    assert_eq!(heartbeat(&h, CLIENT_A, "chrome").await["activeCookieSource"], true);
    assert_eq!(heartbeat(&h, CLIENT_B, "firefox").await["activeCookieSource"], false);

    let extension_post = |client_id: Option<&str>| {
        let mut request = h
            .authed(h.post("/sessions"))
            .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop");
        if let Some(client_id) = client_id {
            request = request.header("X-Zoltraak-Client-Id", client_id);
        }
        request.json(&json!({ "domain": "x.com", "cookie": "auth_token=secret" }))
    };

    assert_eq!(extension_post(None).send().await.unwrap().status(), 428);
    assert_eq!(extension_post(Some(CLIENT_B)).send().await.unwrap().status(), 423);
    assert_eq!(extension_post(Some(CLIENT_A)).send().await.unwrap().status(), 204);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn selecting_another_profile_clears_shared_sessions_before_activation() {
    let h = Harness::start().await;
    heartbeat(&h, CLIENT_A, "chrome").await;
    heartbeat(&h, CLIENT_B, "firefox").await;
    let stored = h
        .authed(h.post("/sessions"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", CLIENT_A)
        .json(&json!({ "domain": "x.com", "cookie": "auth_token=secret" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stored.status(), 204);

    let state = h.state.select_extension_source(CLIENT_B).await.unwrap();
    assert_eq!(state.active_client_id.as_deref(), Some(CLIENT_B));
    let sessions: serde_json::Value = h.authed(h.get("/sessions")).send().await.unwrap().json().await.unwrap();
    assert!(sessions.as_array().unwrap().is_empty());

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn selecting_a_profile_wakes_its_status_long_poll() {
    let h = Harness::start().await;
    heartbeat(&h, CLIENT_A, "chrome").await;
    heartbeat(&h, CLIENT_B, "firefox").await;
    let request = h
        .authed(h.get("/extension/status?reloadVersion=0&sourceRevision=1"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", CLIENT_B)
        .header("X-Zoltraak-Browser-Id", "firefox")
        .header("X-Zoltraak-Extension-Version", "1.2.3");
    let waiting = tokio::spawn(async move { request.send().await.unwrap().json::<serde_json::Value>().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(20)).await;

    h.state.select_extension_source(CLIENT_B).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .expect("source selection did not wake the long poll")
        .unwrap();
    assert_eq!(response["sourceChanged"], true);
    assert_eq!(response["activeCookieSource"], true);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn pair_endpoint_rejects_web_origins() {
    let h = Harness::start().await;
    // Web page origin is forbidden from receiving the token
    let res = h
        .post("/pair")
        .header("Origin", "https://malicious.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    // Missing origin is also rejected for pairing
    let res = h.post("/pair").send().await.unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn pairing_needs_an_open_window_even_for_the_official_extension_and_is_single_use() {
    let h = Harness::start().await;
    let pair = |origin: &'static str| h.post("/pair").header("Origin", origin).send();
    const OFFICIAL: &str = "chrome-extension://anilnadppolgahpagmgjnogppicckamg";

    h.state.close_pairing_window();
    assert_eq!(pair(OFFICIAL).await.unwrap().status(), 403, "a known ID is just a header a script can set");
    assert_eq!(pair("moz-extension://11111111-2222-3333-4444-555555555555").await.unwrap().status(), 403);

    h.state.enable_pairing_window(std::time::Duration::from_secs(60));
    let res = pair("moz-extension://11111111-2222-3333-4444-555555555555").await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["token"], TOKEN);
    assert_eq!(pair(OFFICIAL).await.unwrap().status(), 403, "the window closes after one pairing");
    assert_eq!(pair("moz-extension://aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").await.unwrap().status(), 403);
}

#[tokio::test]
async fn pair_endpoint_allows_known_extension_origin() {
    let h = Harness::start().await;
    let res = h
        .post("/pair")
        .header("Origin", "chrome-extension://anilnadppolgahpagmgjnogppicckamg")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()
            .get("Access-Control-Allow-Origin")
            .unwrap()
            .to_str()
            .unwrap(),
        "chrome-extension://anilnadppolgahpagmgjnogppicckamg"
    );
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["token"], TOKEN);
    assert!(body["port"].is_number());
}

#[tokio::test]
async fn pair_endpoint_handles_options_preflight() {
    let h = Harness::start().await;
    let res = h
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/pair", h.base))
        .header("Origin", "chrome-extension://anilnadppolgahpagmgjnogppicckamg")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(
        res.headers()
            .get("Access-Control-Allow-Origin")
            .unwrap()
            .to_str()
            .unwrap(),
        "chrome-extension://anilnadppolgahpagmgjnogppicckamg"
    );
}


// ---- Remux ---------------------------------------------------------------

#[tokio::test]
async fn remuxing_refuses_a_container_it_does_not_know() {
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/remux/jobs"))
        .json(&json!({ "paths": ["C:/clips/holiday.mkv"], "container": "avi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("mp4"),
        "the refusal names what it will take: {body}"
    );
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn intercepted_download_later_is_paused_and_keeps_the_selected_folder() {
    let h = Harness::start().await;
    let chosen = h._dir.path().join("chosen");
    let category = h.authed(h.post("/categories")).json(&json!({
        "name": "Archives", "kind": "extension", "pattern": "zip",
        "directory": h._dir.path().join("archives"), "priority": 1
    })).send().await.unwrap();
    assert_eq!(category.status(), 201);
    let response = h.authed(h.post("/download")).json(&json!({
        "url": "https://example.com/archive.zip", "filename": "edited.zip",
        "directory": chosen, "paused": true
    })).send().await.unwrap();
    assert_eq!(response.status(), 201);
    let jobs = h.supervisor.snapshot().await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(serde_json::to_value(&jobs[0]).unwrap()["state"], "paused");
    assert_eq!(jobs[0].filename, "edited.zip");
    assert_eq!(jobs[0].directory, chosen.to_string_lossy());
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn remuxing_refuses_a_path_that_is_not_there() {
    // The client has just read these paths off a drop or a picker, so a typo
    // is answered at the point it was made rather than as a job that fails a
    // second later for reasons the user has to go and read.
    let h = Harness::start().await;
    let response = h
        .authed(h.post("/remux/jobs"))
        .json(&json!({ "paths": ["C:/definitely/not/here.mkv"], "container": "mp4" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn a_file_already_in_the_target_container_becomes_a_skipped_job() {
    let h = Harness::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    std::fs::write(&path, b"not really an mp4").unwrap();

    let response = h
        .authed(h.post("/remux/jobs"))
        .json(&json!({ "paths": [path], "container": "mp4" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let ids = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(ids["ids"].as_array().unwrap().len(), 1);

    // The skip is decided from the name, so it lands without ffmpeg being
    // launched — which is what makes this assertable anywhere.
    let job = loop {
        let jobs: Vec<serde_json::Value> =
            h.authed(h.get("/remux/jobs")).send().await.unwrap().json().await.unwrap();
        let job = jobs.into_iter().next().expect("the job is listed");
        if job["state"] != "queued" && job["state"] != "probing" {
            break job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(job["state"], "skipped");
    assert_eq!(job["stage"], "already a .mp4");
    assert!(path.exists(), "a skipped job never touches the file");

    // And it can be cleared away again.
    let cleared: serde_json::Value = h
        .client
        .delete(format!("{}/remux/jobs", h.base))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["removed"], 1);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn probing_describes_a_local_file_without_starting_anything() {
    let h = Harness::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mkv");
    std::fs::write(&path, b"0123456789").unwrap();

    let body: Vec<serde_json::Value> = h
        .authed(h.post("/remux/probe"))
        .json(&json!({ "paths": [path] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["extension"], "mkv");
    assert_eq!(body[0]["bytes"], 10);

    let jobs: Vec<serde_json::Value> =
        h.authed(h.get("/remux/jobs")).send().await.unwrap().json().await.unwrap();
    assert!(jobs.is_empty(), "probing is a question, not an order");
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn gecko_clients_never_receive_reload_signal() {
    let h = Harness::start().await;
    h.state.trigger_extension_reload();
    assert_eq!(h.state.current_reload_version(), 1);

    // 1. A Chromium client receives reload: true
    let chrome_res = h
        .authed(h.get("/extension/status?reloadVersion=0&sourceRevision=0"))
        .header("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop")
        .header("X-Zoltraak-Client-Id", CLIENT_A)
        .header("X-Zoltraak-Browser-Id", "brave")
        .header("X-Zoltraak-Extension-Version", "0.1.40")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(chrome_res["reload"], true);
    assert_eq!(chrome_res["reloadVersion"], 1);

    // 2. A Zen / Gecko client receives reload: false
    let zen_res = h
        .authed(h.get("/extension/status?reloadVersion=0&sourceRevision=0"))
        .header("Origin", "moz-extension://12345678-1234-1234-1234-123456789abc")
        .header("X-Zoltraak-Client-Id", CLIENT_B)
        .header("X-Zoltraak-Browser-Id", "zen")
        .header("X-Zoltraak-Extension-Version", "0.1.40")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(zen_res["reload"], false, "Zen extension must never receive reload: true");

    // 3. check-reload endpoint also protects Gecko origins
    let gecko_check = h
        .get("/extension/check-reload?v=0")
        .header("Origin", "moz-extension://12345678-1234-1234-1234-123456789abc")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(gecko_check["reload"], false);

    // 4. reset_reload_version resets it to 0
    h.state.reset_reload_version();
    assert_eq!(h.state.current_reload_version(), 0);

    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn discord_reconnect_requires_authentication_and_reports_unavailable_service() {
    let h = Harness::start().await;
    assert_eq!(h.post("/discord/reconnect").send().await.unwrap().status(), 401);
    let response = h.authed(h.post("/discord/reconnect")).send().await.unwrap();
    assert_eq!(response.status(), 503);
    assert!(response.text().await.unwrap().contains("Discord delivery is not enabled"));
}

#[tokio::test]
async fn discord_bot_profile_routes_require_auth_and_validate_scope() {
    let h = Harness::start().await;
    for path in ["/discord/bot/profile?scope=global", "/discord/bot/diagnostics"] {
        assert_eq!(h.get(path).send().await.unwrap().status(), 401);
        assert_eq!(h.authed(h.get(path)).send().await.unwrap().status(), 503);
    }
    let profile = || h.client.patch(format!("{}/discord/bot/profile", h.base)).json(&json!({"scope":"global","fields":{"avatar":null}}));
    assert_eq!(profile().send().await.unwrap().status(), 401);
    assert_eq!(h.authed(profile()).send().await.unwrap().status(), 503);
    assert_eq!(h.authed(h.get("/discord/bot/profile?scope=unknown")).send().await.unwrap().status(), 400);
    h.supervisor.shutdown().await;
}

#[tokio::test]
async fn standalone_builds_expose_no_licensing_routes_and_accept_work_with_only_the_install_token() {
    let h = Harness::start().await;
    for path in ["/api/auth/status", "/api/auth/redeem", "/api/auth/heartbeat", "/discord/avatars"] {
        let res = h.authed(h.post(path)).json(&json!({})).send().await.unwrap();
        assert!(matches!(res.status().as_u16(), 404 | 405), "{path}: {}", res.status());
    }
    for path in ["/download", "/video/analyze", "/remux/jobs"] {
        let status = h.authed(h.post(path)).json(&json!({})).send().await.unwrap().status();
        assert!(status != 403 && status != 503, "{path} must not be license-gated: {status}");
    }
    h.supervisor.shutdown().await;
}
