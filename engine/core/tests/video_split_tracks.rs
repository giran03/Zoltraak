//! End to end, against real media: a video served the way Facebook and
//! Instagram serve it — picture in one `.mp4`, sound in another, described by
//! a DASH manifest — comes out as one file that plays with both.
//!
//! The tracks are produced by ffmpeg, which is also what the engine merges
//! with, so this suite needs it on PATH; it skips (loudly) rather than fails
//! when it is missing, because a machine without ffmpeg says nothing about
//! this code. Everything else is local: a range-honouring origin on 127.0.0.1.


use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use zoltraak_core::video::mp4probe;
use zoltraak_core::{NewVideoDownload, Queue, VideoJobState, VideoManager};

fn have_ffmpeg() -> bool {
    let ok = |bin: &str| {
        std::process::Command::new(bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    ok("ffmpeg") && ok("ffprobe")
}

/// One second of synthetic media. `frag` writes fragmented mp4 with the
/// `moov` up front, which is the DASH shape Meta serves; without it ffmpeg
/// writes a classic file with `moov` at the end, which is what the probe's
/// tail-read path exists for.
fn render(dir: &Path, name: &str, video: bool, audio: bool, frag: bool) -> PathBuf {
    let out = dir.join(name);
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error"]);
    if video {
        cmd.args(["-f", "lavfi", "-i", "testsrc=duration=1:size=64x64:rate=10"]);
    }
    if audio {
        cmd.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"]);
    }
    if video {
        cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast"]);
    }
    if audio {
        cmd.args(["-c:a", "aac"]);
    }
    if frag {
        cmd.args(["-movflags", "frag_keyframe+empty_moov"]);
    }
    cmd.arg(&out);
    let status = cmd.status().expect("run ffmpeg");
    assert!(status.success(), "ffmpeg could not render {name}");
    out
}

/// `(video streams, audio streams)` in a finished file, per ffprobe.
fn stream_counts(path: &Path) -> (usize, usize) {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    let text = String::from_utf8_lossy(&out.stdout);
    (
        text.lines().filter(|l| l.trim() == "video").count(),
        text.lines().filter(|l| l.trim() == "audio").count(),
    )
}

struct Origin {
    dir: PathBuf,
}

/// Serve a file from the origin's directory, honouring `Range` the way a CDN
/// does — the probe reads heads and tails, and the downloader resumes.
async fn serve(State(origin): State<Arc<Origin>>, AxumPath(name): AxumPath<String>, headers: HeaderMap) -> Response {
    let Ok(bytes) = std::fs::read(origin.dir.join(&name)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = if name.ends_with(".mpd") { "application/dash+xml" } else { "video/mp4" };

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("bytes="))
        .map(|spec| {
            let len = bytes.len();
            match spec.split_once('-') {
                // A suffix range: the last N bytes.
                Some(("", n)) => {
                    let n: usize = n.parse().unwrap_or(len);
                    (len.saturating_sub(n), len - 1)
                }
                Some((s, "")) => (s.parse().unwrap_or(0), len - 1),
                Some((s, e)) => (s.parse().unwrap_or(0), e.parse::<usize>().unwrap_or(len - 1).min(len - 1)),
                None => (0, len - 1),
            }
        });

    match range {
        Some((start, end)) if start < bytes.len() => (
            StatusCode::PARTIAL_CONTENT,
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CONTENT_RANGE, format!("bytes {start}-{end}/{}", bytes.len())),
            ],
            bytes[start..=end].to_vec(),
        )
            .into_response(),
        _ => (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], bytes).into_response(),
    }
}

async fn spawn_origin(dir: PathBuf) -> SocketAddr {
    let app = Router::new().route("/{name}", get(serve)).with_state(Arc::new(Origin { dir }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// The manifest shape both Meta platforms inline: whole-file `BaseURL`s under
/// `SegmentBase`, video and audio in separate adaptation sets.
fn meta_style_mpd(addr: SocketAddr) -> String {
    format!(
        r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT1S" type="static">
  <Period>
    <AdaptationSet contentType="video" segmentAlignment="true">
      <Representation id="v" mimeType="video/mp4" codecs="avc1.42c00a" bandwidth="200000" width="64" height="64">
        <BaseURL>http://{addr}/video.mp4</BaseURL>
        <SegmentBase indexRange="0-0"><Initialization range="0-0"/></SegmentBase>
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio" segmentAlignment="true">
      <Representation id="a" mimeType="audio/mp4" codecs="mp4a.40.2" bandwidth="64000">
        <BaseURL>http://{addr}/audio.mp4</BaseURL>
        <SegmentBase indexRange="0-0"><Initialization range="0-0"/></SegmentBase>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#
    )
}

#[tokio::test]
async fn the_probe_tells_a_video_track_from_an_audio_track_on_real_files() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    render(dir.path(), "video.mp4", true, false, true);
    render(dir.path(), "audio.mp4", false, true, true);
    render(dir.path(), "muxed.mp4", true, true, true);
    // Classic layout: moov at the end, so the head alone does not answer.
    render(dir.path(), "classic.mp4", true, true, false);
    let addr = spawn_origin(dir.path().to_path_buf()).await;
    let client = reqwest::Client::new();

    let video = mp4probe::probe(&client, &format!("http://{addr}/video.mp4"), &vec![]).await;
    let audio = mp4probe::probe(&client, &format!("http://{addr}/audio.mp4"), &vec![]).await;
    let muxed = mp4probe::probe(&client, &format!("http://{addr}/muxed.mp4"), &vec![]).await;
    let classic = mp4probe::probe(&client, &format!("http://{addr}/classic.mp4"), &vec![]).await;

    assert!(video.expect("video probed").is_video_only(), "a DASH video track has no sound");
    assert!(audio.expect("audio probed").is_audio_only(), "a DASH audio track has no picture");
    let muxed = muxed.expect("muxed probed");
    assert!(muxed.video && muxed.audio, "a whole file has both");
    let classic = classic.expect("classic probed via its tail");
    assert!(classic.video && classic.audio, "moov-at-the-end still classifies");
}

#[tokio::test]
async fn a_split_track_manifest_downloads_as_one_file_with_picture_and_sound() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        return;
    }
    let media = tempfile::tempdir().expect("tempdir");
    render(media.path(), "video.mp4", true, false, true);
    render(media.path(), "audio.mp4", false, true, true);
    let addr = spawn_origin(media.path().to_path_buf()).await;
    std::fs::write(media.path().join("clip.mpd"), meta_style_mpd(addr)).expect("write mpd");

    let client = reqwest::Client::new();
    let meta = zoltraak_core::video::analyze(&client, &format!("http://{addr}/clip.mpd"), &vec![])
        .await
        .expect("manifest analyzes");

    // What the picker sees: one rendition, and it knows it needs its audio.
    assert_eq!(meta.formats.len(), 1);
    let format = meta.formats[0].clone();
    assert!(format.requires_merge, "picture and sound are separate, so the job merges");
    assert_eq!(format.video_url.as_deref(), Some(format!("http://{addr}/video.mp4").as_str()));
    assert_eq!(format.audio_url.as_deref(), Some(format!("http://{addr}/audio.mp4").as_str()));
    assert_eq!(format.height, Some(64));
    assert!(format.plays_with_audio());

    // Run it for real.
    let out = tempfile::tempdir().expect("tempdir");
    let queue = Queue::open(&out.path().join("zoltraak.db")).await.expect("open queue");
    let manager = VideoManager::new(client, queue.pool());
    let id = manager
        .start(NewVideoDownload {
            title: "Split Clip".into(),
            webpage_url: format!("http://{addr}/watch"),
            format,
            directory: out.path().to_path_buf(),
            headers: vec![],
        })
        .await
        .expect("job starts");

    let mut job = None;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let j = manager.get(id).await.expect("job exists");
        if matches!(j.state, VideoJobState::Finished | VideoJobState::Failed) {
            job = Some(j);
            break;
        }
    }
    let job = job.expect("job finished within ten seconds");
    assert_eq!(job.state, VideoJobState::Finished, "job failed: {:?}", job.error);

    let path = job.path.expect("finished job has a path");
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("Split Clip.mp4"));
    assert_eq!(stream_counts(&path), (1, 1), "the result carries exactly one video and one audio stream");

    // The intermediates are gone; only the merged file remains.
    let leftovers: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp") || n.ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
}

/// Wait for a job to stop moving, then report it.
async fn settle(manager: &VideoManager, id: zoltraak_core::DownloadId) -> zoltraak_core::VideoJob {
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let job = manager.get(id).await.expect("job exists");
        if matches!(job.state, VideoJobState::Finished | VideoJobState::Failed) {
            return job;
        }
    }
    panic!("job did not settle within ten seconds");
}

/// The `.tmp` files a merge job leaves in a directory, by name.
fn temp_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

/// A merge that ffmpeg refuses keeps the streams that were already fetched.
///
/// Deleting them regardless used to strand the job: the row still said both
/// streams were finished, so the retry the user was offered went straight to a
/// merge whose inputs were gone, and reported a missing file instead of the
/// fault that actually stopped it.
#[tokio::test]
async fn a_failed_merge_keeps_the_streams_it_downloaded() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        return;
    }
    let media = tempfile::tempdir().expect("tempdir");
    render(media.path(), "video.mp4", true, false, true);
    // Sound that is not sound: enough bytes to download, nothing ffmpeg can mux.
    std::fs::write(media.path().join("audio.mp4"), vec![b'x'; 4096]).expect("write junk");
    let addr = spawn_origin(media.path().to_path_buf()).await;

    let out = tempfile::tempdir().expect("tempdir");
    let queue = Queue::open(&out.path().join("zoltraak.db")).await.expect("open queue");
    let manager = VideoManager::new(reqwest::Client::new(), queue.pool());
    let id = manager
        .start(NewVideoDownload {
            title: "Broken Merge".into(),
            webpage_url: format!("http://{addr}/watch"),
            format: zoltraak_core::VideoFormat {
                id: "split".into(),
                video_url: Some(format!("http://{addr}/video.mp4")),
                audio_url: Some(format!("http://{addr}/audio.mp4")),
                requires_merge: true,
                ..Default::default()
            },
            directory: out.path().to_path_buf(),
            headers: vec![],
        })
        .await
        .expect("job starts");

    let job = settle(&manager, id).await;
    assert_eq!(job.state, VideoJobState::Failed, "junk audio cannot be muxed");
    assert_eq!(
        temp_files(out.path()),
        vec!["Broken Merge.audio.tmp".to_string(), "Broken Merge.video.tmp".to_string()],
        "both fetched streams survive a merge that failed"
    );
}

/// A resumed job whose temporary streams are gone fetches them again.
///
/// The done-flags live in the row and the bytes live in the user's download
/// folder, so the two drift apart — a cleaned folder, or an older build that
/// deleted its merge inputs before failing. Trusting the flags then handed
/// ffmpeg two paths that did not exist.
#[tokio::test]
async fn a_resume_whose_streams_vanished_fetches_them_again() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        return;
    }
    let media = tempfile::tempdir().expect("tempdir");
    render(media.path(), "video.mp4", true, false, true);
    render(media.path(), "audio.mp4", false, true, true);
    let addr = spawn_origin(media.path().to_path_buf()).await;

    let out = tempfile::tempdir().expect("tempdir");
    let queue = Queue::open(&out.path().join("zoltraak.db")).await.expect("open queue");
    let manager = VideoManager::new(reqwest::Client::new(), queue.pool());
    let id = manager
        .start(NewVideoDownload {
            title: "Vanished Streams".into(),
            webpage_url: format!("http://{addr}/watch"),
            format: zoltraak_core::VideoFormat {
                id: "split".into(),
                video_url: Some(format!("http://{addr}/video.mp4")),
                audio_url: Some(format!("http://{addr}/audio.mp4")),
                requires_merge: true,
                ..Default::default()
            },
            directory: out.path().to_path_buf(),
            headers: vec![],
        })
        .await
        .expect("job starts");
    assert_eq!(settle(&manager, id).await.state, VideoJobState::Finished);

    // The state a stranded job is in: both streams marked finished, neither on
    // disk, and the merged file gone too.
    let dest = out.path().join("Vanished Streams.mp4");
    std::fs::remove_file(&dest).expect("remove the merged file");
    assert!(temp_files(out.path()).is_empty(), "a successful merge cleaned up after itself");
    let resume = r#"{"video":{"segmentsDone":0,"bytes":0,"fingerprint":null},"videoDone":true,
                     "audio":{"segmentsDone":0,"bytes":0,"fingerprint":null},"audioDone":true}"#;
    sqlx::query("UPDATE video_jobs SET state = 'failed', resume_state = ? WHERE id = ?")
        .bind(resume)
        .bind(id.to_string())
        .execute(&queue.pool())
        .await
        .expect("rewrite the resume state");

    manager.resume(id).await.expect("job resumes");
    let job = settle(&manager, id).await;
    assert_eq!(job.state, VideoJobState::Finished, "job failed: {:?}", job.error);
    assert_eq!(stream_counts(&dest), (1, 1), "the streams were fetched again and merged again");
}
