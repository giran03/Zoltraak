use axum::{Router, http::Uri, response::IntoResponse};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn page_hls_fallback_handles_relative_escaped_signed_and_extensionless_streams() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let app = Router::new().fallback(move |uri: Uri| {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push(uri.to_string());
            let body = match uri.path() {
                "/normal" => r#"<video src="/clip.mp4"></video><script>const ad='/ad.m3u8';</script>"#,
                "/relative" => r#"<video src="blob:page"></video><div data-config="/stream.m3u8?sig=a%2Fb+z&amp;x=1"></div>"#,
                "/escaped" => r#"<script>const config={src:"\u002fstream.m3u8\u003fsig=a%2Fb+z\u0026x=1"};</script>"#,
                "/broken" => r#"<video src="/invalid.m3u8"></video>"#,
                "/empty" => "<video src='blob:page'></video>",
                "/invalid.m3u8" => "<html>Please sign in</html>",
                "/stream.m3u8" | "/extensionless" => "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n",
                _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
            };
            body.into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let direct = zoltraak_core::video::analyze(&client, &format!("{base}/normal"), &vec![]).await.unwrap();
    assert_eq!(direct.formats[0].video_url.as_deref(), Some(format!("{base}/clip.mp4").as_str()));
    assert!(!requests.lock().unwrap().iter().any(|url| url.contains("ad.m3u8")));
    for path in ["relative", "escaped"] {
        let result = zoltraak_core::video::analyze(&client, &format!("{base}/{path}"), &vec![]).await.unwrap();
        assert_eq!(result.formats.len(), 1);
        assert_eq!(result.formats[0].container, "mp4");
        assert_eq!(result.formats[0].video_url.as_deref(), Some(format!("{base}/stream.m3u8?sig=a%2Fb+z&x=1").as_str()));
    }
    let result = zoltraak_core::video::analyze(&client, &format!("{base}/extensionless"), &vec![]).await.unwrap();
    assert_eq!(result.formats[0].container, "mp4");
    let broken = zoltraak_core::video::analyze(&client, &format!("{base}/broken"), &vec![]).await.unwrap_err();
    assert!(broken.to_string().contains("Stream detected but could not be resolved"));
    let empty = zoltraak_core::video::analyze(&client, &format!("{base}/empty"), &vec![]).await.unwrap_err();
    assert!(empty.to_string().contains("No stream detected"));
    assert!(requests.lock().unwrap().iter().any(|url| url == "/stream.m3u8?sig=a%2Fb+z&x=1"));
    server.abort();
}
