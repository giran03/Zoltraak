//! Local HTTP regressions for media responses that do not have a file extension.
use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::get,
};
use zoltraak_core::video;

async fn origin() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/audio", get(|| async { ([("content-type", "audio/mpeg")], b"audio fixture".as_slice()) }))
        .route("/playlist", get(|| async { ([("content-type", "text/plain")], "#EXTM3U\n#EXTINF:4,\naudio.aac\n#EXT-X-ENDLIST\n") }))
        .route("/dash-audio", get(|| async { ([("content-type", "application/octet-stream")],
            r#"<MPD mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="audio/mp4"><Representation id="a" bandwidth="128000" codecs="mp4a.40.2"><BaseURL>audio.m4a</BaseURL></Representation></AdaptationSet></Period></MPD>"#) }))
        .route(
            "/media",
            get(|| async {
                (
                    [("content-type", "video/mp4")],
                    include_bytes!("fixtures/verified-av.mp4").as_slice(),
                )
            }),
        )
        .route(
            "/webm",
            get(|| async { ([("content-type", "video/webm")], b"webm fixture".as_slice()) }),
        )
        .route(
            "/attachment",
            get(|| async {
                (
                    [
                        ("content-type", "application/octet-stream"),
                        ("content-disposition", "attachment; filename=clip.webm"),
                    ],
                    b"webm fixture".as_slice(),
                )
            }),
        )
        .route(
            "/protected",
            get(|headers: HeaderMap| async move {
                if headers.get("cookie").and_then(|v| v.to_str().ok()) == Some("session=ok") {
                    (
                        [("content-type", "video/mp4")],
                        include_bytes!("fixtures/verified-av.mp4").as_slice(),
                    )
                        .into_response()
                } else {
                    StatusCode::FORBIDDEN.into_response()
                }
            }),
        )
        .route(
            "/redirect",
            get(|| async { Redirect::temporary("/pages/watch") }),
        )
        .route(
            "/pages/watch",
            get(|| async {
                (
                    [("content-type", "text/html")],
                    "<title>Relative clip</title><video src='media?token=signed'></video>",
                )
            }),
        )
        .route(
            "/pages/media",
            get(|headers: HeaderMap| async move {
                if headers
                    .get("referer")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.ends_with("/pages/watch"))
                {
                    (
                        [("content-type", "video/mp4")],
                        include_bytes!("fixtures/verified-av.mp4").as_slice(),
                    )
                        .into_response()
                } else {
                    StatusCode::FORBIDDEN.into_response()
                }
            }),
        )
        .route(
            "/poster",
            get(|| async { ([("content-type", "image/jpeg")], b"poster".as_slice()) }),
        )
        .route(
            "/html-login",
            get(|| async { ([("content-type", "text/html")], "<title>Sign in</title>") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, task)
}

#[tokio::test]
async fn extensionless_mp4_and_webm_are_files_instead_of_html_pages() {
    let (base, task) = origin().await;
    let client = reqwest::Client::new();
    for (path, container) in [
        ("media?token=signed", "mp4"),
        ("webm", "webm"),
        ("attachment", "webm"),
    ] {
        let url = format!("{base}/{path}");
        let meta = video::analyze(&client, &url, &vec![]).await.unwrap();
        assert_eq!(meta.formats.len(), 1);
        assert_eq!(meta.formats[0].container, container);
        assert_eq!(meta.formats[0].video_url.as_deref(), Some(url.as_str()));
    }
    task.abort();
}

#[tokio::test]
async fn generic_audio_and_extensionless_manifest_hints_use_the_shared_pipeline() {
    let (base, task) = origin().await;
    let client = reqwest::Client::new();
    let audio = video::analyze(&client, &format!("{base}/audio?sig=a%2Fb+z"), &vec![]).await.unwrap();
    assert_eq!(audio.formats[0].container, "mp3");
    assert_eq!(audio.formats[0].id, "audio");
    assert_eq!(audio.formats[0].has_audio, Some(true));
    assert_eq!(audio.formats[0].validation.as_ref().unwrap().has_video, Some(false));
    let hls = video::analyze_source(&client, &format!("{base}/playlist"), &vec![], false, Some("hls"), Some("audio")).await.unwrap();
    assert_eq!(hls.formats[0].container, "m4a");
    assert_eq!(hls.formats[0].validation.as_ref().unwrap().has_video, Some(false));
    let dash = video::analyze_source(&client, &format!("{base}/dash-audio"), &vec![], false, Some("dash"), Some("audio")).await.unwrap();
    assert_eq!(dash.formats[0].container, "m4a");
    assert!(!dash.formats[0].requires_merge);
    assert!(video::analyze_source(&client, &format!("{base}/playlist"), &vec![], false, Some("invalid"), None).await.is_err());
    task.abort();
}

#[tokio::test]
async fn extensionless_media_keeps_the_request_session() {
    let (base, task) = origin().await;
    let url = format!("{base}/protected");
    let headers = vec![("Cookie".into(), "session=ok".into())];
    let meta = video::analyze(&reqwest::Client::new(), &url, &headers)
        .await
        .unwrap();
    assert_eq!(meta.formats[0].video_url.as_deref(), Some(url.as_str()));
    assert_eq!(meta.formats[0].has_audio, Some(true));
    task.abort();
}

#[tokio::test]
async fn redirected_pages_resolve_media_against_the_final_page_url() {
    let (base, task) = origin().await;
    let url = format!("{base}/redirect");
    let meta = video::analyze(&reqwest::Client::new(), &url, &vec![])
        .await
        .unwrap();
    assert_eq!(meta.webpage_url, url);
    assert_eq!(
        meta.formats[0].video_url.as_deref(),
        Some(format!("{base}/pages/media?token=signed").as_str())
    );
    assert_eq!(
        meta.formats[0].has_audio,
        Some(true),
        "the redirected page's referrer must reach the track probe"
    );
    task.abort();
}

#[tokio::test]
async fn posters_and_login_pages_are_not_video_files() {
    let (base, task) = origin().await;
    for path in ["poster", "html-login"] {
        assert!(
            video::analyze(&reqwest::Client::new(), &format!("{base}/{path}"), &vec![])
                .await
                .is_err()
        );
    }
    task.abort();
}
