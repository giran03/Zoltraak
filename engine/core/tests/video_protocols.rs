//! Real HTTP and FFmpeg fixtures for continuity boundaries and finite periods.

use axum::{
    Router,
    extract::{Path as RoutePath, State},
    http::StatusCode,
    routing::get,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
use zoltraak_core::video::{
    self, groups,
    stream::{Segment, StreamOptions, StreamOutcome},
};

#[derive(Clone)]
struct Origin {
    files: Arc<HashMap<String, Vec<u8>>>,
    requests: Arc<AtomicUsize>,
}
async fn serve(
    State(origin): State<Origin>,
    RoutePath(name): RoutePath<String>,
) -> (StatusCode, Vec<u8>) {
    origin.requests.fetch_add(1, Ordering::SeqCst);
    match origin.files.get(&name) {
        Some(b) => (StatusCode::OK, b.clone()),
        None => (StatusCode::NOT_FOUND, vec![]),
    }
}
async fn origin(
    files: HashMap<String, Vec<u8>>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let state = Origin {
        files: Arc::new(files),
        requests: Arc::new(AtomicUsize::new(0)),
    };
    let requests = state.requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/{name}", get(serve)).with_state(state),
        )
        .await
        .unwrap();
    });
    (base, requests, task)
}
async fn ffmpeg_available() -> bool {
    let okay = zoltraak_core::media_tools::ffmpeg_command()
        .arg("-version")
        .output()
        .await
        .is_ok()
        && zoltraak_core::media_tools::ffprobe_command()
            .arg("-version")
            .output()
            .await
            .is_ok();
    if !okay {
        eprintln!("Skipping real protocol fixture: FFmpeg/ffprobe unavailable");
    }
    okay
}
async fn make_track(dest: &Path, args: &[&str]) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verified-av.mp4");
    let output = zoltraak_core::media_tools::ffmpeg_command()
        .args(["-y", "-v", "error", "-i"])
        .current_dir(dest.parent().unwrap())
        .arg(source)
        .args(args)
        .arg(dest)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
async fn duration(path: &Path) -> f64 {
    let out = zoltraak_core::media_tools::ffprobe_command()
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(path)
        .output()
        .await
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn hls_fmp4_discontinuities_and_init_changes_join_with_valid_timestamps() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let playlist = temp.path().join("source.m3u8");
    make_track(
        &playlist,
        &[
            "-c",
            "copy",
            "-f",
            "hls",
            "-hls_segment_type",
            "fmp4",
            "-hls_list_size",
            "0",
        ],
    )
    .await;
    let source = std::fs::read_to_string(&playlist).unwrap();
    let segment = source
        .lines()
        .find(|l| !l.starts_with('#') && !l.is_empty())
        .unwrap();
    let init = std::fs::read(temp.path().join("init.mp4")).unwrap();
    let media = std::fs::read(temp.path().join(segment)).unwrap();
    let files = HashMap::from([
        ("init1.mp4".into(), init.clone()), ("init2.mp4".into(), init), ("one.m4s".into(), media.clone()), ("two.m4s".into(), media),
        ("video.m3u8".into(), b"#EXTM3U\n#EXT-X-MAP:URI=\"init1.mp4\"\n#EXTINF:1,\none.m4s\n#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"init2.mp4\"\n#EXTINF:1,\ntwo.m4s\n#EXT-X-ENDLIST".to_vec())]);
    let (base, _, server) = origin(files).await;
    let dest = temp.path().join("joined.mp4");
    let outcome = video::hls::download_media_playlist(
        &reqwest::Client::new(),
        &format!("{base}video.m3u8"),
        &vec![],
        &dest,
    )
    .await
    .unwrap();
    assert!(matches!(outcome, StreamOutcome::Completed { .. }));
    assert!((duration(&dest).await - 2.0).abs() < 0.2);
    let format = video::VideoFormat {
        has_audio: Some(true),
        ..Default::default()
    };
    assert!(
        video::validation::inspect_output(&dest, &format)
            .await
            .unwrap()
            .samples_decoded
    );
    server.abort();
}

#[tokio::test]
async fn dash_periods_download_and_mux_both_tracks_for_the_entire_presentation() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let v = temp.path().join("video.mp4");
    let a = temp.path().join("audio.mp4");
    make_track(&v, &["-an", "-c", "copy"]).await;
    make_track(&a, &["-vn", "-c", "copy"]).await;
    let files = HashMap::from([
        ("v.mp4".into(), std::fs::read(&v).unwrap()),
        ("a.mp4".into(), std::fs::read(&a).unwrap()),
    ]);
    let (base, _, server) = origin(files).await;
    let period = r#"<Period duration="PT1S"><AdaptationSet contentType="video"><Representation id="v" codecs="avc1" width="64" height="64"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet><AdaptationSet contentType="audio"><Representation id="a" codecs="mp4a"><BaseURL>a.mp4</BaseURL></Representation></AdaptationSet></Period>"#;
    let mut f = video::dash::formats_from_manifest(
        &format!(r#"<MPD mediaPresentationDuration="PT2S">{period}{period}</MPD>"#),
        &base,
    )
    .unwrap()
    .remove(0);
    let client = reqwest::Client::new();
    for (url, path) in [
        (f.video_url.as_ref().unwrap(), &v),
        (f.audio_url.as_ref().unwrap(), &a),
    ] {
        assert!(matches!(
            video::stream::fetch(&client, url, &vec![], path)
                .await
                .unwrap(),
            StreamOutcome::Completed { .. }
        ));
    }
    let dest = temp.path().join("complete.mp4");
    video::merge::mux(&v, &a, &dest).await.unwrap();
    f.validation = Some(video::validation::VideoValidation {
        duration: Some(2.0),
        ..Default::default()
    });
    let report = video::validation::inspect_output(&dest, &f).await.unwrap();
    assert!(
        report.samples_decoded && report.has_video == Some(true) && report.has_audio == Some(true)
    );
    assert!((duration(&dest).await - 2.0).abs() < 0.2);
    server.abort();
}

#[tokio::test]
async fn hls_transport_stream_timestamp_resets_are_joined_locally() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let ts = temp.path().join("segment.ts");
    make_track(&ts, &["-c", "copy", "-f", "mpegts"]).await;
    let media = std::fs::read(ts).unwrap();
    let (base, _, server) = origin(HashMap::from([
        ("one.ts".into(), media.clone()), ("two.ts".into(), media),
        ("video.m3u8".into(), b"#EXTM3U\n#EXTINF:1,\none.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\ntwo.ts\n#EXT-X-ENDLIST".to_vec())])).await;
    let dest = temp.path().join("transport.mp4");
    video::hls::download_media_playlist(
        &reqwest::Client::new(),
        &format!("{base}video.m3u8"),
        &vec![],
        &dest,
    )
    .await
    .unwrap();
    assert!((duration(&dest).await - 2.0).abs() < 0.2);
    assert!(
        video::validation::inspect_output(
            &dest,
            &video::VideoFormat {
                has_audio: Some(true),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .samples_decoded
    );
    server.abort();
}

#[tokio::test]
async fn grouped_resume_reuses_completed_groups_and_changed_identity_restarts() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let fixture = include_bytes!("fixtures/verified-av.mp4").to_vec();
    let size = fixture.len() as u64;
    let (base, requests, server) = origin(HashMap::from([
        ("one.mp4".into(), fixture.clone()),
        ("two.mp4".into(), fixture.clone()),
        ("new.mp4".into(), fixture),
    ]))
    .await;
    let groups = vec![
        vec![Segment::whole(format!("{base}one.mp4"))],
        vec![Segment::whole(format!("{base}two.mp4"))],
    ];
    let dest = temp.path().join("out.mp4");
    let cancel = CancellationToken::new();
    let report = |tick: video::stream::StreamTick| {
        if tick.bytes > size {
            cancel.cancel();
        }
    };
    let client = reqwest::Client::new();
    let options = StreamOptions {
        cancel: cancel.clone(),
        ..Default::default()
    };
    assert!(matches!(
        groups::download(&client, &groups, &vec![], &dest, Some(&report), &options)
            .await
            .unwrap(),
        StreamOutcome::Paused(_)
    ));
    assert!(
        temp.path().join("out.mp4.groups/0.mp4").exists(),
        "first group survives pause"
    );
    let options = StreamOptions::default();
    groups::download(&client, &groups, &vec![], &dest, None, &options)
        .await
        .unwrap();
    let before = requests.load(Ordering::SeqCst);
    // Both completed groups survive process-like recreation of StreamOptions.
    groups::download(&client, &groups, &vec![], &dest, None, &options)
        .await
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), before);
    let mut changed = groups.clone();
    changed[1][0].url = format!("{base}new.mp4");
    groups::download(&client, &changed, &vec![], &dest, None, &options)
        .await
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), before + 2);
    server.abort();
}

#[tokio::test]
async fn encrypted_hls_fails_before_fetching_media_or_keys() {
    let (base, requests, server) = origin(HashMap::from([(
        "encrypted.m3u8".into(),
        b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:1,\nmedia.ts\n#EXT-X-ENDLIST"
            .to_vec(),
    )]))
    .await;
    let temp = tempfile::tempdir().unwrap();
    assert!(
        video::hls::download_media_playlist(
            &reqwest::Client::new(),
            &format!("{base}encrypted.m3u8"),
            &vec![],
            &temp.path().join("out.ts")
        )
        .await
        .is_err()
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn actual_codec_parameter_changes_fail_and_preserve_groups_for_inspection() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let scaled = temp.path().join("scaled.mp4");
    make_track(
        &scaled,
        &["-vf", "scale=128:128", "-c:v", "libx264", "-c:a", "copy"],
    )
    .await;
    let (base, _, server) = origin(HashMap::from([
        (
            "old.mp4".into(),
            include_bytes!("fixtures/verified-av.mp4").to_vec(),
        ),
        ("new.mp4".into(), std::fs::read(scaled).unwrap()),
    ]))
    .await;
    let groups = vec![
        vec![Segment::whole(format!("{base}old.mp4"))],
        vec![Segment::whole(format!("{base}new.mp4"))],
    ];
    let dest = temp.path().join("bad.mp4");
    let error = groups::download(
        &reqwest::Client::new(),
        &groups,
        &vec![],
        &dest,
        None,
        &StreamOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("require transcoding"));
    assert!(temp.path().join("bad.mp4.groups/0.mp4").exists());
    assert!(temp.path().join("bad.mp4.groups/1.mp4").exists());
    assert!(!dest.exists());
    server.abort();
}

#[tokio::test]
async fn live_capture_resumes_sequence_without_replay_and_rejects_a_window_gap() {
    if !ffmpeg_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let ts = temp.path().join("segment.ts");
    make_track(&ts, &["-c", "copy", "-f", "mpegts"]).await;
    let media = Arc::new(std::fs::read(ts).unwrap());
    let size = media.len() as u64;
    let polls = Arc::new(AtomicUsize::new(0));
    let served = Arc::new(AtomicUsize::new(0));
    let polls_route = polls.clone();
    let served_route = served.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let router = Router::new().route("/live.m3u8", get(move || {
        let step = polls_route.fetch_add(1, Ordering::SeqCst);
        async move { match step {
            0 => "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:1,\none.ts\n",
            1 | 2 => "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:1,\none.ts\n#EXTINF:1,\ntwo.ts\n#EXTINF:1,\nthree.ts\n#EXT-X-ENDLIST",
            _ => "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:110\n#EXTINF:1,\nlater.ts\n",
        }}
    })).route("/{name}", get(move || {
        served_route.fetch_add(1, Ordering::SeqCst); let bytes = media.clone(); async move { (*bytes).clone() }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let source = format!("{base}live.m3u8");
    let client = reqwest::Client::new();
    let capture = temp.path().join("capture.mp4");
    let cancel = CancellationToken::new();
    let cb = |tick: video::stream::StreamTick| {
        if tick.bytes > size {
            cancel.cancel();
        }
    };
    let options = StreamOptions {
        live_seconds: Some(30),
        cancel: cancel.clone(),
        ..Default::default()
    };
    assert!(matches!(
        video::stream::fetch_with_progress(
            &client,
            &source,
            &vec![],
            &capture,
            Some(&cb),
            &options
        )
        .await
        .unwrap(),
        StreamOutcome::Paused(_)
    ));
    let ledger_path = temp.path().join("capture.mp4.live/progress.json");
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger["count"], 1);
    assert_eq!(ledger["next"], 101);
    let options = StreamOptions {
        live_seconds: Some(30),
        ..Default::default()
    };
    video::stream::fetch_with_progress(&client, &source, &vec![], &capture, None, &options)
        .await
        .unwrap();
    assert!((duration(&capture).await - 3.0).abs() < 0.2);
    assert_eq!(
        served.load(Ordering::SeqCst),
        4,
        "first completed segment is not fetched again; paused segment restarts"
    );
    assert!(
        video::validation::inspect_output(
            &capture,
            &video::VideoFormat {
                has_audio: Some(true),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .samples_decoded
    );
    // Restore the pause ledger and force a rolled window. A gap must fail.
    std::fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    polls.store(3, Ordering::SeqCst);
    let error =
        video::stream::fetch_with_progress(&client, &source, &vec![], &capture, None, &options)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("missing segment"));
    // An elapsed deadline finalizes saved footage without fetching the window.
    let mut expired = ledger;
    expired["deadline"] = serde_json::json!(0);
    std::fs::write(&ledger_path, serde_json::to_vec(&expired).unwrap()).unwrap();
    let before = polls.load(Ordering::SeqCst);
    video::stream::fetch_with_progress(&client, &source, &vec![], &capture, None, &options)
        .await
        .unwrap();
    assert_eq!(polls.load(Ordering::SeqCst), before);
    assert!((duration(&capture).await - 1.0).abs() < 0.2);
    // The public job path validates recorded duration and retires its ledger.
    polls.store(0, Ordering::SeqCst);
    let queue = zoltraak_core::Queue::open(&temp.path().join("live.db"))
        .await
        .unwrap();
    let manager = zoltraak_core::VideoManager::new(client.clone(), queue.pool());
    let id = manager
        .start(zoltraak_core::NewVideoDownload {
            title: "Managed live".into(),
            webpage_url: format!("{base}watch"),
            directory: temp.path().into(),
            headers: Vec::new(),
            format: video::VideoFormat {
                id: "live".into(),
                video_url: Some(source.clone()),
                has_audio: Some(true),
                download_options: Some(video::DownloadOptions {
                    thumbnail: None,
                    live_seconds: Some(30),
                    subtitle: None,
                }),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    for _ in 0..250 {
        if matches!(
            manager.get(id).await.unwrap().state,
            zoltraak_core::VideoJobState::Finished | zoltraak_core::VideoJobState::Failed
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let job = manager.get(id).await.unwrap();
    assert_eq!(
        job.state,
        zoltraak_core::VideoJobState::Finished,
        "{:?}",
        job.error
    );
    assert!(job.validation.unwrap().samples_decoded);
    assert!(!temp.path().join("Managed live.ts.part.live").exists());
    server.abort();
}

#[tokio::test]
async fn subtitle_sidecars_are_bounded_validated_and_do_not_receive_cross_origin_credentials() {
    use axum::http::HeaderMap;
    let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen_route = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/caption.vtt",
                    get(move |headers: HeaderMap| {
                        seen_route.store(
                            headers.contains_key("cookie") || headers.contains_key("authorization"),
                            Ordering::SeqCst,
                        );
                        async { "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nFixture\n" }
                    }),
                )
                .route("/bad.vtt", get(|| async { "<html>Login required</html>" }))
                .route(
                    "/huge.vtt",
                    get(|| async { vec![b'x'; 2 * 1024 * 1024 + 1] }),
                ),
        )
        .await
        .unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let video = temp.path().join("video.mp4");
    let subtitle = video::Subtitle {
        language: "en".into(),
        label: "English".into(),
        url: format!("{base}caption.vtt"),
        extension: "vtt".into(),
        automatic: false,
    };
    let headers = vec![
        ("Cookie".into(), "secret".into()),
        ("Authorization".into(), "secret".into()),
    ];
    let client = reqwest::Client::new();
    let saved = video::subtitles::save(
        &client,
        &subtitle,
        "https://other.example/video",
        &headers,
        &video,
    )
    .await
    .unwrap();
    assert!(!seen.load(Ordering::SeqCst));
    assert!(saved.exists());
    video::subtitles::save(
        &client,
        &subtitle,
        "https://other.example/video",
        &headers,
        &video,
    )
    .await
    .unwrap();
    video::subtitles::save(
        &client,
        &subtitle,
        &format!("{base}video"),
        &headers,
        &video,
    )
    .await
    .unwrap();
    assert!(
        seen.load(Ordering::SeqCst),
        "same-origin captions retain the authorized session"
    );
    for endpoint in ["bad.vtt", "huge.vtt"] {
        let changed = video::Subtitle {
            url: format!("{base}{endpoint}"),
            ..subtitle.clone()
        };
        assert!(
            video::subtitles::save(
                &client,
                &changed,
                "https://other.example/video",
                &headers,
                &video
            )
            .await
            .is_err()
        );
    }
    std::fs::write(&saved, b"existing user caption").unwrap();
    assert!(
        video::subtitles::save(
            &client,
            &subtitle,
            "https://other.example/video",
            &headers,
            &video
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(&saved).unwrap(), b"existing user caption");
    server.abort();
}

#[tokio::test]
async fn optional_local_ytdlp_extracts_a_controlled_single_video_without_downloading_it() {
    let mut tool = tokio::process::Command::new("yt-dlp");
    #[cfg(windows)]
    tool.creation_flags(0x08000000);
    if !tool
        .arg("--version")
        .output()
        .await
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("Skipping optional executable fixture: local yt-dlp unavailable");
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let fetched = Arc::new(AtomicUsize::new(0));
    let fetched_route = fetched.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/watch.html", get(|| async {
            axum::response::Html("<html><head><title>Controlled video fixture</title></head><body><video src=\"/video.mp4\"></video></body></html>")
        })).route("/video.mp4", get(move || {
            fetched_route.fetch_add(1, Ordering::SeqCst);
            async { include_bytes!("fixtures/verified-av.mp4").to_vec() }
        }))).await.unwrap();
    });
    let meta = video::fallback::analyze(&format!("{base}watch.html"))
        .await
        .unwrap();
    assert_eq!(meta.formats.len(), 1);
    assert_eq!(
        meta.formats[0].video_url.as_deref(),
        Some(format!("{base}video.mp4").as_str())
    );
    assert!(meta.formats[0].id.starts_with("yt-dlp-"));
    assert_eq!(
        fetched.load(Ordering::SeqCst),
        0,
        "fallback performs extraction only"
    );
    server.abort();
}
