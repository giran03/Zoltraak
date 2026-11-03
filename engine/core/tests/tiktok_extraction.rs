//! Local HTTP fixtures reproduce modern TikTok pages without a live account.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::get,
};
use serde_json::json;
use zoltraak_core::{headers::Headers, image, video};

const ID: &str = "7671362008486251790";

async fn origin() -> (
    reqwest::Client,
    String,
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let post = format!("http://www.tiktok.com:{}/@creator/video/{ID}", addr.port());
    let media = format!("http://127.0.0.1:{}/clip.mp4?token=a%2Fb+z", addr.port());
    let requests = Arc::new(AtomicUsize::new(0));
    let pages = requests.clone();
    let selected = json!({"id": ID, "desc": "Selected TikTok", "video": {
        "playAddr": {"urlList": [media]}, "width": 160, "height": 90
    }});
    let payload = json!({"props": {"pageProps": {"itemList": [
        {"id": "1111111111111111111", "video": {"playAddr": "https://cdn.invalid/neighbor.mp4"}}, selected
    ]}}});
    let html = format!("<script id='__NEXT_DATA__'>{payload}</script>");
    let redirect = post.clone();
    let app = Router::new()
        .route(
            "/@creator/video/{id}",
            get(move |headers: HeaderMap| {
                let html = html.clone();
                let pages = pages.clone();
                async move {
                    pages.fetch_add(1, Ordering::Relaxed);
                    // Simulate the different page produced by invented client hints
                    // or the old forced Referer, even with the correct session.
                    if headers.contains_key("sec-ch-ua")
                        || headers.get("cookie").and_then(|v| v.to_str().ok()) != Some("session=ok")
                    {
                        return StatusCode::FORBIDDEN.into_response();
                    }
                    if headers.get("referer").and_then(|v| v.to_str().ok())
                        != Some("https://www.tiktok.com/foryou")
                    {
                        return "<title>Empty video page</title>".into_response();
                    }
                    ([("content-type", "text/html")], html).into_response()
                }
            }),
        )
        .route(
            "/t/shared",
            get(move || {
                let redirect = redirect.clone();
                async move { Redirect::temporary(&redirect) }
            }),
        )
        .route(
            "/clip.mp4",
            get(|| async {
                (
                    [("content-type", "video/mp4")],
                    include_bytes!("fixtures/verified-av.mp4").as_slice(),
                )
            }),
        );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .referer(false)
        .resolve("www.tiktok.com", addr)
        .resolve("vm.tiktok.com", addr)
        .build()
        .unwrap();
    (client, post, media, requests, handle)
}

fn headers() -> Headers {
    vec![
        ("Cookie".into(), "session=ok".into()),
        ("Referer".into(), "https://www.tiktok.com/foryou".into()),
    ]
}

#[tokio::test]
async fn video_and_images_use_the_same_item_and_each_fetch_the_page_once() {
    let (client, post, media, requests, server) = origin().await;
    let meta = video::analyze(&client, &post, &headers()).await.unwrap();
    assert_eq!(meta.title, "Selected TikTok");
    assert_eq!(meta.formats.len(), 1);
    assert_eq!(meta.formats[0].video_url.as_deref(), Some(media.as_str()));
    let album = image::analyze(&client, &post, &headers()).await.unwrap();
    assert_eq!(album.items.len(), 1);
    assert_eq!(album.items[0].kind, image::MediaKind::Video);
    assert_eq!(
        album.items[0].formats[0].video_url.as_deref(),
        Some(media.as_str())
    );
    assert_eq!(
        requests.load(Ordering::Relaxed),
        2,
        "Images should reuse its page instead of refetching it for Video"
    );
    server.abort();
}

#[tokio::test]
async fn shortened_share_links_resolve_the_post_before_selecting_feed_items() {
    let (client, post, media, requests, server) = origin().await;
    let mut shared = reqwest::Url::parse(&post).unwrap();
    shared.set_host(Some("vm.tiktok.com")).unwrap();
    shared.set_path("/t/shared");
    let meta = video::analyze(&client, shared.as_str(), &headers())
        .await
        .unwrap();
    assert_eq!(meta.webpage_url, post);
    assert_eq!(meta.formats[0].video_url.as_deref(), Some(media.as_str()));
    assert_eq!(
        requests.load(Ordering::Relaxed),
        2,
        "resolve anonymously, then authenticate the canonical post"
    );
    let album = image::analyze(&client, shared.as_str(), &headers())
        .await
        .unwrap();
    assert_eq!(
        album.items[0].formats[0].video_url.as_deref(),
        Some(media.as_str())
    );
    assert_eq!(requests.load(Ordering::Relaxed), 4);
    server.abort();
}

#[tokio::test]
#[ignore = "Needs live TikTok access and ZOLTRAAK_TIKTOK_TEST_URL"]
async fn live_tiktok_post_resolves() {
    let url = std::env::var("ZOLTRAAK_TIKTOK_TEST_URL").expect("provide a public TikTok post URL");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(40),
        video::analyze(&client, &url, &Headers::new()),
    )
    .await
    .expect("live analysis should finish within 40 seconds")
    .expect("live post should resolve");
    assert!(!result.formats.is_empty());
    assert!(result.formats.iter().all(|f| {
        f.video_url
            .as_deref()
            .is_some_and(|u| u.starts_with("http"))
    }));
    println!("Resolved {} TikTok renditions", result.formats.len());
    let selected = result
        .formats
        .iter()
        .find(|f| !f.requires_merge && f.has_audio != Some(false))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tiktok.mp4");
    let downloaded = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        video::stream::fetch(
            &client,
            selected.video_url.as_deref().unwrap(),
            &selected.headers,
            &path,
        ),
    )
    .await
    .expect("download should finish within 30 seconds")
    .expect("selected rendition should download");
    assert!(matches!(downloaded, video::stream::StreamOutcome::Completed { bytes } if bytes > 0));
    zoltraak_core::media_tools::register_search_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src-tauri/resources/bin"),
    );
    let info = zoltraak_core::remux::probe(&path).await.unwrap();
    assert!(!info.unprobed && info.video_codec.is_some() && info.audio_codec.is_some());
    println!(
        "Downloaded {} bytes; verified {}x{} with {:?} video and {:?} audio",
        downloaded.bytes(),
        info.width.unwrap_or(0),
        info.height.unwrap_or(0),
        info.video_codec,
        info.audio_codec
    );
    for format in result.formats {
        println!(
            "Rendition: {}x{}, validation={:?}, audio={:?}",
            format.width.unwrap_or(0),
            format.height.unwrap_or(0),
            format.validation.map(|v| v.status),
            format.has_audio
        );
    }
}
