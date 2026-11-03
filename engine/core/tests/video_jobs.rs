//! `VideoManager` pause, resume, and removal.
//!
//! The stream-level tests in `video_segments.rs` cover *how* a download stops
//! and continues. These cover the part that has to survive the job outliving
//! the call that started it: the chosen format, the session it was resolved
//! with, and the resolved filename all have to come back off the row, because
//! analysis is stateless and there is nobody left to hand them over again.


use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use zoltraak_core::{
    NewVideoDownload, Queue, VideoFormat, VideoJobState, VideoManager,
};

const TOTAL: usize = 300_000;

fn payload() -> Vec<u8> {
    let mut bytes = include_bytes!("fixtures/verified-av.mp4").to_vec();
    let padding = TOTAL - bytes.len();
    bytes.extend_from_slice(&(padding as u32).to_be_bytes());
    bytes.extend_from_slice(b"free");
    bytes.resize(TOTAL, 0);
    bytes
}

#[derive(Default)]
struct Origin {
    requests: AtomicUsize,
    rate_requests: AtomicUsize,
    /// Cookie header seen on the most recent request, so a resumed job can be
    /// checked for carrying the same session as the original.
    saw_cookie: std::sync::Mutex<Option<String>>,
}

/// A slow, range-honouring `.mp4`, so a pause lands mid-transfer.
async fn serve_clip(State(origin): State<Arc<Origin>>, headers: HeaderMap) -> Response {
    origin.requests.fetch_add(1, Ordering::SeqCst);
    if let Some(cookie) = headers.get(axum::http::header::COOKIE).and_then(|v| v.to_str().ok()) {
        *origin.saw_cookie.lock().unwrap() = Some(cookie.to_string());
    }

    let body = payload();
    let start = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.trim_start_matches("bytes=").trim_end_matches('-').parse::<usize>().ok());

    // Dribble the body out so a cancel has somewhere to land.
    let tail = match start {
        Some(offset) => body[offset..].to_vec(),
        None => body,
    };
    let chunks = tail
        .chunks(16 * 1024)
        .map(|c| c.to_vec())
        .collect::<Vec<_>>();
    let stream = futures_util::stream::iter(chunks.into_iter().map(|chunk| async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        Ok::<_, std::io::Error>(chunk)
    }))
    .then(|f| f);

    let status = if start.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    let mut response = (status, Body::from_stream(stream)).into_response();
    response.headers_mut().insert(axum::http::header::ETAG, "\"fixture-v1\"".parse().unwrap());
    if let Some(offset) = start {
        response.headers_mut().insert(axum::http::header::CONTENT_RANGE,
            format!("bytes {offset}-{}/{TOTAL}", TOTAL - 1).parse().unwrap());
    }
    response
}

use futures_util::StreamExt;

async fn spawn_origin(origin: Arc<Origin>) -> SocketAddr {
    let app = Router::new().route("/clip.mp4", get(serve_clip))
        .route("/caption.vtt", get(|| async { "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nCaption fixture\n" }))
        .route("/bad-caption.vtt", get(|| async { "<html>Sign in</html>" }))
        .route("/bad.mp4", get(|| async { ([ (axum::http::header::CONTENT_TYPE, "video/mp4") ], "invalid media bytes") }))
        .route("/limited.mp4", get(|State(origin): State<Arc<Origin>>| async move {
            if origin.rate_requests.fetch_add(1, Ordering::SeqCst) < 2 {
                return StatusCode::TOO_MANY_REQUESTS.into_response();
            }
            ([(axum::http::header::CONTENT_TYPE, "video/mp4")], payload()).into_response()
        }))
        .with_state(origin);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[tokio::test]
async fn a_transient_rate_limit_recovers_without_reextracting_the_source() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(origin.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (video, _) = manager(dir.path()).await;
    let mut format = direct_format(addr);
    format.video_url = Some(format!("http://{addr}/limited.mp4"));
    let id = video.start(NewVideoDownload { title: "Limited clip".into(), webpage_url: format!("http://{addr}/watch"),
        format, directory: dir.path().into(), headers: Vec::new() }).await.unwrap();
    for _ in 0..250 {
        if matches!(video.get(id).await.unwrap().state, VideoJobState::Finished | VideoJobState::Failed) { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let job = video.get(id).await.unwrap();
    assert_eq!(job.state, VideoJobState::Finished, "{:?}", job.error);
    assert_eq!(origin.rate_requests.load(Ordering::SeqCst), 3);
    assert_eq!(std::fs::read(job.path.unwrap()).unwrap(), payload());
}

#[tokio::test]
async fn invalid_media_fails_verification_and_remains_available_for_inspection() {
    if zoltraak_core::media_tools::ffprobe_command().arg("-version").output().await.is_err() {
        eprintln!("ffprobe unavailable; invalid-container fixture not run"); return;
    }
    let addr = spawn_origin(Arc::new(Origin::default())).await;
    let dir = tempfile::tempdir().unwrap();
    let (video, _) = manager(dir.path()).await;
    let mut format = direct_format(addr);
    format.video_url = Some(format!("http://{addr}/bad.mp4"));
    let id = video.start(NewVideoDownload { title: "Bad clip".into(), webpage_url: format!("http://{addr}/bad.mp4"),
        format, directory: dir.path().into(), headers: Vec::new() }).await.unwrap();
    for _ in 0..100 {
        if video.get(id).await.unwrap().state == VideoJobState::Failed { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let job = video.get(id).await.unwrap();
    assert_eq!(job.state, VideoJobState::Failed);
    assert!(job.error.unwrap().contains("readable"));
    assert!(job.path.unwrap().exists());
}

fn direct_format(addr: SocketAddr) -> VideoFormat {
    VideoFormat {
        validation: None,
        download_options: None,
        id: "direct".into(),
        container: "mp4".into(),
        video_codec: None,
        audio_codec: None,
        width: None,
        height: Some(720),
        fps: None,
        bitrate: None,
        filesize: Some(TOTAL as u64),
        video_url: Some(format!("http://{addr}/clip.mp4")),
        audio_url: None,
        has_audio: None,
        headers: Vec::new(),
        requires_merge: false,
    }
}

#[tokio::test]
async fn selected_captions_are_persisted_and_required_before_completion() {
    let addr = spawn_origin(Arc::new(Origin::default())).await;
    let dir = tempfile::tempdir().unwrap();
    let (video, queue) = manager(dir.path()).await;
    for (endpoint, finished) in [("caption.vtt", true), ("bad-caption.vtt", false)] {
        let mut format = direct_format(addr);
        format.download_options = Some(zoltraak_core::video::DownloadOptions { subtitle: Some(zoltraak_core::video::Subtitle {
            language: "en".into(), label: "English".into(), extension: "vtt".into(), url: format!("http://{addr}/{endpoint}"), automatic: false,
        }), live_seconds: None, thumbnail: None });
        let id = video.start(NewVideoDownload { title: endpoint.into(), webpage_url: format!("http://{addr}/watch"), format, directory: dir.path().into(), headers: Vec::new() }).await.unwrap();
        for _ in 0..200 {
            if matches!(video.get(id).await.unwrap().state, VideoJobState::Finished | VideoJobState::Failed) { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let job = video.get(id).await.unwrap();
        assert_eq!(job.state, if finished { VideoJobState::Finished } else { VideoJobState::Failed });
        let path = job.path.unwrap(); assert!(path.exists(), "video is retained even when captions fail");
        if finished {
            let caption = path.with_file_name(format!("{}.en.vtt", path.file_name().unwrap().to_string_lossy()));
            assert!(caption.exists());
            assert!(job.validation.unwrap().samples_decoded);
        } else { assert!(job.error.unwrap().contains("caption text")); }
        let stored: String = sqlx::query_scalar("SELECT format FROM video_jobs WHERE id = ?").bind(id.to_string()).fetch_one(&queue.pool()).await.unwrap();
        let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(stored["downloadOptions"]["subtitle"]["language"], "en");
    }
}

/// A manager over a temporary on-disk database, so a job survives being
/// reloaded the way it would across an app restart.
async fn manager(dir: &std::path::Path) -> (VideoManager, Queue) {
    let queue = Queue::open(&dir.join("zoltraak.db")).await.expect("open queue");
    let manager = VideoManager::new(reqwest::Client::new(), queue.pool());
    (manager, queue)
}

#[tokio::test]
async fn pausing_a_job_stops_it_and_leaves_it_resumable() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let (video, _queue) = manager(dir.path()).await;

    let id = video
        .start(NewVideoDownload {
            title: "A Clip".into(),
            webpage_url: format!("http://{addr}/watch"),
            format: direct_format(addr),
            directory: dir.path().to_path_buf(),
            headers: vec![("cookie".into(), "session=abc".into())],
        })
        .await
        .expect("job starts");

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(video.cancel(id).await, "a running job reports that it was cancelled");

    // The task needs a moment to unwind to its boundary and persist.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let job = video.get(id).await.expect("job still exists");
    assert_eq!(job.state, VideoJobState::Paused, "a user pausing is not a failure");
    assert!(job.error.is_none(), "a pause must not leave an error on the row");
    assert!(job.downloaded_bytes > 0, "the bytes already fetched are kept");
    assert!(
        (job.downloaded_bytes as usize) < TOTAL,
        "the job must have actually been cut short"
    );
    assert!(job.is_resumable());

    // Cancelling again is a no-op rather than an error.
    assert!(!video.cancel(id).await);
}

#[tokio::test]
async fn a_running_job_reports_a_rate_and_an_eta_and_drops_them_when_it_stops() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let (video, _queue) = manager(dir.path()).await;

    let id = video
        .start(NewVideoDownload {
            title: "Metered".into(),
            webpage_url: format!("http://{addr}/watch"),
            format: direct_format(addr),
            directory: dir.path().to_path_buf(),
            headers: Vec::new(),
        })
        .await
        .expect("job starts");

    // Sample until the meter has something to say. The first tick has no
    // interval to measure against, so a rate needs at least two.
    let mut running = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let job = video.get(id).await.expect("job exists");
        if job.bytes_per_second > 0 {
            running = Some(job);
            break;
        }
    }

    let job = running.expect("a running download should report a throughput");
    assert!(job.bytes_per_second > 0, "rate must be live, not zero");
    assert_eq!(
        job.total_bytes,
        Some(TOTAL as u64),
        "a single file's total comes from Content-Length, so the bar is determinate"
    );
    assert!(
        job.eta_seconds.is_some(),
        "with a rate and a total there is nothing left to stop an ETA being computed"
    );

    video.cancel(id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let job = video.get(id).await.expect("job exists");
    assert_eq!(job.state, VideoJobState::Paused);
    assert_eq!(
        job.bytes_per_second, 0,
        "a stopped job showing its last speed is worse than showing none — the number looks live and never moves"
    );
    assert_eq!(job.eta_seconds, None, "no rate means no ETA");
}

#[tokio::test]
async fn a_resumed_job_finishes_the_same_file_it_started() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let (video, _queue) = manager(dir.path()).await;
    let poster = "https://media.example.test/poster.jpg";
    let mut format = direct_format(addr);
    format.download_options = Some(zoltraak_core::video::DownloadOptions {
        thumbnail: Some(poster.into()), ..Default::default()
    });

    let id = video
        .start(NewVideoDownload {
            title: "A Clip".into(),
            webpage_url: format!("http://{addr}/watch"),
            format,
            directory: dir.path().to_path_buf(),
            headers: vec![("cookie".into(), "session=abc".into())],
        })
        .await
        .expect("job starts");

    tokio::time::sleep(Duration::from_millis(150)).await;
    video.cancel(id).await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let paused_at = video.get(id).await.expect("job exists").downloaded_bytes;
    assert!(paused_at > 0 && (paused_at as usize) < TOTAL);

    video.resume(id).await.expect("resume is accepted");

    // Let it run to completion.
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if video.get(id).await.map(|j| j.state) == Some(VideoJobState::Finished) {
            break;
        }
    }

    let job = video.get(id).await.expect("job exists");
    assert_eq!(job.state, VideoJobState::Finished, "resumed job should finish");
    assert_eq!(job.thumbnail.as_deref(), Some(poster));
    let recorded_validation = job.validation.clone().expect("completion records verification status");
    if recorded_validation.status == zoltraak_core::video::validation::ValidationStatus::Verified {
        assert_eq!((recorded_validation.has_video, recorded_validation.has_audio), (Some(true), Some(true)));
    }
    let (reloaded, _) = manager(dir.path()).await;
    reloaded.recover().await.unwrap();
    assert_eq!(reloaded.get(id).await.unwrap().validation, Some(recorded_validation));
    assert_eq!(reloaded.get(id).await.unwrap().thumbnail.as_deref(), Some(poster), "poster survives a restart");

    let path = job.path.expect("a finished job has a path");
    assert_eq!(
        std::fs::read(&path).expect("read output"),
        payload(),
        "a resumed file must be byte-identical to one downloaded in one go"
    );

    // The resumed request has to carry the same session the format was
    // resolved with, or a login-gated stream 401s halfway through.
    assert_eq!(
        origin.saw_cookie.lock().unwrap().as_deref(),
        Some("session=abc"),
        "stored headers must be replayed on resume"
    );

    // The filename must not have drifted: re-deriving it would have numbered
    // around this job's own partial file and orphaned the bytes it had.
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        "A Clip.mp4",
        "a resumed job keeps the name it was given, not a deduped variant"
    );
}

#[tokio::test]
async fn removing_a_running_job_stops_it_and_cleans_up() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");
    let (video, _queue) = manager(dir.path()).await;

    let id = video
        .start(NewVideoDownload {
            title: "Doomed".into(),
            webpage_url: format!("http://{addr}/watch"),
            format: direct_format(addr),
            directory: dir.path().to_path_buf(),
            headers: Vec::new(),
        })
        .await
        .expect("job starts");

    tokio::time::sleep(Duration::from_millis(150)).await;
    let dest = dir.path().join("Doomed.mp4");
    assert!(dest.exists(), "the job should be mid-write before it is removed");

    assert!(video.remove(id).await.expect("remove succeeds"));

    // The half-written file must be gone, and must stay gone. If removal only
    // dropped the row, the still-running task would recreate it a moment later
    // — which is precisely the bug this covers.
    assert!(
        !dest.exists(),
        "removing an unfinished job must delete its partial file, not leave a truncated .mp4 behind"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !dest.exists(),
        "removing a job must stop the writer, not just drop the row while it keeps downloading"
    );
    assert!(video.get(id).await.is_none(), "the job is gone from the list");
}

#[tokio::test]
async fn an_interrupted_job_comes_back_paused_and_can_be_continued() {
    let origin = Arc::new(Origin::default());
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let dir = tempfile::tempdir().expect("tempdir");

    // First "run" of the app: start a job and abandon it mid-flight, the way a
    // crash or a kill would.
    let id = {
        let (video, _queue) = manager(dir.path()).await;
        let id = video
            .start(NewVideoDownload {
                title: "Survivor".into(),
                webpage_url: format!("http://{addr}/watch"),
                format: direct_format(addr),
                directory: dir.path().to_path_buf(),
                headers: Vec::new(),
            })
            .await
            .expect("job starts");
        tokio::time::sleep(Duration::from_millis(150)).await;
        video.cancel(id).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        id
    };

    // Second "run": a fresh manager over the same database.
    let (video, _queue) = manager(dir.path()).await;
    video.recover().await.expect("recovery runs");

    let job = video.get(id).await.expect("the job survived the restart");
    assert_eq!(
        job.state,
        VideoJobState::Paused,
        "an interrupted job is stopped, not lost — calling it failed would push the user to restart a large download for no reason"
    );

    video.resume(id).await.expect("a recovered job can be resumed");

    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if video.get(id).await.map(|j| j.state) == Some(VideoJobState::Finished) {
            break;
        }
    }

    let job = video.get(id).await.expect("job exists");
    assert_eq!(job.state, VideoJobState::Finished);
    assert_eq!(
        std::fs::read(job.path.expect("path")).expect("read"),
        payload(),
        "a job resumed in a later process must still produce the right bytes"
    );
}

/// Hits YouTube for real: analyze, download the top-ranked format through the
/// manager (a ranged, parallel fetch and an ffmpeg merge), and check a whole
/// file lands. Run on request with `--ignored`.
#[tokio::test]
#[ignore]
async fn live_youtube_download_lands_a_whole_file() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "zoltraak_core=debug".into()))
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().expect("tempdir");
    let (video, _queue) = manager(dir.path()).await;
    // `ZOLTRAAK_LIVE_VIDEO` picks another video; `ZOLTRAAK_LIVE_MAX_HEIGHT` caps
    // the rung so a 4K test does not pull a gigabyte.
    let url = std::env::var("ZOLTRAAK_LIVE_VIDEO")
        .or_else(|_| std::env::var("OPENDM_LIVE_VIDEO"))
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
    let max_height: u32 = std::env::var("ZOLTRAAK_LIVE_MAX_HEIGHT")
        .or_else(|_| std::env::var("OPENDM_LIVE_MAX_HEIGHT"))
        .ok()
        .and_then(|h| h.parse().ok())
        .unwrap_or(u32::MAX);
    let meta = video.analyze(&url, &Vec::new()).await.expect("analyze");
    for f in &meta.formats {
        println!("  format {} {:?}p {}fps {} bytes merge={}", f.id, f.height, f.fps.unwrap_or(0.0), f.filesize.unwrap_or(0), f.requires_merge);
    }
    let format = meta
        .formats
        .iter()
        .find(|f| f.height.unwrap_or(0) <= max_height && f.height.is_some())
        .cloned()
        .unwrap_or_else(|| meta.formats[0].clone());
    println!(
        "downloading {} {:?}p {} bytes merge={} url={}",
        format.id,
        format.height,
        format.filesize.unwrap_or(0),
        format.requires_merge,
        format.video_url.as_deref().unwrap_or("")
    );
    let id = video
        .start(NewVideoDownload {
            title: meta.title,
            webpage_url: meta.webpage_url,
            format: format.clone(),
            directory: dir.path().to_path_buf(),
            headers: Vec::new(),
        })
        .await
        .expect("job starts");

    let started = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let job = video.get(id).await.expect("job exists");
        println!("  {:?} {} bytes {} B/s", job.state, job.downloaded_bytes, job.bytes_per_second);
        if matches!(job.state, VideoJobState::Finished | VideoJobState::Failed) || started.elapsed() > Duration::from_secs(300) {
            break;
        }
    }
    let job = video.get(id).await.expect("job exists");
    assert_eq!(job.state, VideoJobState::Finished, "job error: {:?}", job.error);
    let path = job.path.expect("a finished job has a path");
    let len = std::fs::metadata(&path).expect("file").len();
    println!("landed {} ({len} bytes) in {:?}", path.display(), started.elapsed());
    assert!(len > 0);
}

#[tokio::test]
async fn finished_video_removal_deletes_only_when_requested_and_keeps_failed_entries() {
    let addr = spawn_origin(Arc::new(Origin::default())).await;
    for mode in ["keep", "delete", "missing", "blocked"] {
        let dir = tempfile::tempdir().unwrap();
        let (video, queue) = manager(dir.path()).await;
        let id = video.start(NewVideoDownload {
            title: "Removal fixture".into(), webpage_url: format!("http://{addr}/watch"),
            format: direct_format(addr), directory: dir.path().into(), headers: Vec::new(),
        }).await.unwrap();
        for _ in 0..400 {
            if matches!(video.get(id).await.unwrap().state, VideoJobState::Finished | VideoJobState::Failed) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let job = video.get(id).await.unwrap();
        assert_eq!(job.state, VideoJobState::Finished, "{:?}", job.error);
        let path = job.path.unwrap();
        if mode == "missing" || mode == "blocked" { std::fs::remove_file(&path).unwrap(); }
        if mode == "blocked" {
            std::fs::create_dir(&path).unwrap();
            assert!(video.remove_with_file(id, true).await.is_err());
            assert!(video.get(id).await.is_some());
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_jobs WHERE id = ?").bind(id.to_string()).fetch_one(&queue.pool()).await.unwrap();
            assert_eq!(count, 1);
            assert!(path.is_dir());
        } else {
            assert!(video.remove_with_file(id, mode != "keep").await.unwrap());
            assert!(video.get(id).await.is_none());
            assert_eq!(path.exists(), mode == "keep");
        }
    }
}

#[tokio::test]
async fn located_video_path_survives_recovery_and_missing_output_restarts() {
    let addr = spawn_origin(Arc::new(Origin::default())).await;
    let dir = tempfile::tempdir().unwrap();
    let (video, queue) = manager(dir.path()).await;
    let id = video.start(NewVideoDownload { title: "Locate fixture".into(), webpage_url: format!("http://{addr}/watch"), format: direct_format(addr), directory: dir.path().into(), headers: Vec::new() }).await.unwrap();
    for _ in 0..400 { if video.get(id).await.unwrap().state == VideoJobState::Finished { break; } tokio::time::sleep(Duration::from_millis(20)).await; }
    let job = video.get(id).await.unwrap();
    assert_eq!(job.state, VideoJobState::Finished, "{:?}", job.error);
    let original = job.path.unwrap();
    assert!(video.restart_missing(id).await.is_err());
    let moved = dir.path().join("moved.mp4");
    std::fs::rename(&original, &moved).unwrap();
    video.locate_file(id, &moved).await.unwrap();
    assert!(video.locate_file(id, dir.path()).await.is_err());
    let restored = VideoManager::new(reqwest::Client::new(), queue.pool());
    restored.recover().await.unwrap();
    assert_eq!(restored.get(id).await.unwrap().path, Some(moved.clone()));
    std::fs::remove_file(moved).unwrap();
    restored.restart_missing(id).await.unwrap();
    assert!(restored.restart_missing(id).await.is_err(), "a second restart cannot launch another writer");
    for _ in 0..400 { if matches!(restored.get(id).await.unwrap().state, VideoJobState::Finished | VideoJobState::Failed) { break; } tokio::time::sleep(Duration::from_millis(20)).await; }
    let job = restored.get(id).await.unwrap();
    assert_eq!(job.state, VideoJobState::Finished, "{:?}", job.error);
    assert_eq!(std::fs::read(job.path.unwrap()).unwrap(), payload());
}
