//! The music pipeline end to end, against a local origin: a resolved source
//! becomes queued jobs, the audio is fetched, converted and tagged, and the
//! file lands where the options said it would.
//!
//! The catalogue extractors are covered by unit tests against captured
//! payloads (they parse, they do not download). What needs a real run is the
//! part those cannot reach: the worker's concurrency, the skip-existing
//! decision, pause and resume, the collection folder, and — where a local
//! `ffmpeg` exists — that the tags and the container actually come out right.


use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt;

use zoltraak_core::music::manager::{MusicJobState, MusicManager, NewMusicDownload, WAITING_FOR_YOUTUBE_SESSION};
use zoltraak_core::music::{AudioFormat, AudioSource, FolderMode, MusicSource, MusicTrack, OutputOptions, Service, SourceKind};
use zoltraak_core::{Queue, SessionVault, SiteSession};

/// A silent MP3 the converter and the tagger can both work on, made once with
/// the local ffmpeg. `None` when ffmpeg is absent, which is what makes every
/// test here skip rather than fail on a machine without it.
fn sample_mp3(dir: &Path) -> Option<Vec<u8>> {
    let path = dir.join("sample.mp3");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo", "-t", "2", "-c:a", "libmp3lame", "-b:a", "128k"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::read(&path).ok()
}

fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Origin {
    audio: Vec<u8>,
    cover: Vec<u8>,
    /// The album's own cover, deliberately different bytes from the track's.
    album_cover: Vec<u8>,
    requests: AtomicUsize,
    /// Milliseconds between body pieces, so a pause has somewhere to land.
    piece_delay: u64,
    /// Refuse this many requests before serving, so a test can watch a job
    /// recover from a source that was unavailable and then was not.
    refuse_first: AtomicUsize,
}

async fn serve_audio(State(origin): State<Arc<Origin>>, headers: HeaderMap) -> Response {
    origin.requests.fetch_add(1, Ordering::SeqCst);
    if origin.refuse_first.load(Ordering::SeqCst) > 0 {
        origin.refuse_first.fetch_sub(1, Ordering::SeqCst);
        return StatusCode::FORBIDDEN.into_response();
    }
    let start = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.trim_start_matches("bytes=").split('-').next()?.parse::<usize>().ok())
        .unwrap_or(0);
    let tail = origin.audio[start.min(origin.audio.len())..].to_vec();
    let status = if start > 0 { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    let delay = origin.piece_delay;

    let chunks: Vec<Vec<u8>> = tail.chunks(4096).map(|c| c.to_vec()).collect();
    let stream = futures_util::stream::iter(chunks.into_iter().map(move |chunk| async move {
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        Ok::<_, std::io::Error>(chunk)
    }))
    .then(|f| f);

    (status, [(axum::http::header::CONTENT_TYPE, "audio/mpeg")], Body::from_stream(stream)).into_response()
}

async fn serve_cover(State(origin): State<Arc<Origin>>) -> Response {
    ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], origin.cover.clone()).into_response()
}

async fn serve_album_cover(State(origin): State<Arc<Origin>>) -> Response {
    ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], origin.album_cover.clone()).into_response()
}

async fn spawn_origin(origin: Arc<Origin>) -> SocketAddr {
    let app = Router::new()
        .route("/track.mp3", get(serve_audio))
        .route("/cover.jpg", get(serve_cover))
        .route("/album.jpg", get(serve_album_cover))
        .with_state(origin);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// A real, decodable JPEG for the cover-art path — deliberately 16:9, since
/// that is the shape a video thumbnail arrives in and the shape the crop
/// exists to square.
fn sample_jpeg(dir: &Path) -> Option<Vec<u8>> {
    coloured_jpeg(dir, "blue", "640x360", "sample.jpg")
}

/// A second picture, square and a different colour, standing in for the
/// album's own cover so a test can tell the two apart by their bytes.
fn album_jpeg(dir: &Path) -> Option<Vec<u8>> {
    coloured_jpeg(dir, "red", "600x600", "album.jpg")
}

fn coloured_jpeg(dir: &Path, colour: &str, size: &str, name: &str) -> Option<Vec<u8>> {
    let path = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", &format!("color=c={colour}:s={size}"), "-frames:v", "1"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    status.success().then(|| std::fs::read(&path).ok())?
}

fn track(title: &str, addr: SocketAddr, position: u32) -> MusicTrack {
    MusicTrack {
        id: format!("test:{title}"),
        title: title.into(),
        artists: vec!["Test Artist".into()],
        album: Some("Test Album".into()),
        album_artist: Some("Test Artist".into()),
        track_number: Some(position),
        track_total: Some(2),
        disc_number: Some(1),
        year: Some(2024),
        date: Some("2024-03-01".into()),
        genre: Some("Electronic".into()),
        duration: Some(2.0),
        cover_url: Some(format!("http://{addr}/cover.jpg")),
        isrc: None,
        source_url: format!("http://{addr}/page/{title}"),
        service: Service::Direct,
        source: AudioSource::Direct {
            url: format!("http://{addr}/track.mp3"),
            headers: Vec::new(),
            container: Some("mp3".into()),
        },
        explicit: false,
        unavailable: None,
    }
}

fn source(kind: SourceKind, tracks: Vec<MusicTrack>) -> MusicSource {
    source_with_cover(kind, tracks, None)
}

fn source_with_cover(kind: SourceKind, tracks: Vec<MusicTrack>, cover_url: Option<String>) -> MusicSource {
    MusicSource {
        kind,
        service: Service::Direct,
        title: "Test Album".into(),
        owner: Some("Test Artist".into()),
        cover_url,
        source_url: "http://example/album".into(),
        tracks,
        total: None,
        note: None,
    }
}

async fn manager(dir: &Path) -> (MusicManager, Queue) {
    let (manager, queue, _) = manager_with_vault(dir).await;
    (manager, queue)
}

async fn manager_with_vault(dir: &Path) -> (MusicManager, Queue, Arc<SessionVault>) {
    let queue = Queue::open(&dir.join("zoltraak.db")).await.expect("open queue");
    let vault = Arc::new(SessionVault::in_memory());
    let manager = MusicManager::new(reqwest::Client::new(), queue.pool()).with_session_vault(Arc::clone(&vault));
    manager.recover().await.expect("recover");
    (manager, queue, vault)
}

/// A cookie that reads as a YouTube login, which is what lifts the CDN's
/// preview cap on a matched track.
const YOUTUBE_SIGN_IN: &str = "LOGIN_INFO=abc; SAPISID=def";

/// Wait until every job of a run has settled, or give up.
async fn settle(manager: &MusicManager, ids: &[zoltraak_core::DownloadId], timeout: Duration) -> Vec<zoltraak_core::MusicJob> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let jobs: Vec<_> = manager.list().await.into_iter().filter(|j| ids.contains(&j.id)).collect();
        if jobs.len() == ids.len() && jobs.iter().all(|j| j.state.is_terminal()) {
            return jobs;
        }
        if std::time::Instant::now() > deadline {
            for job in &jobs {
                eprintln!("unsettled: {} {:?} stage={:?} error={:?}", job.track.title, job.state, job.stage, job.error);
            }
            return jobs;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn options(dir: &Path, format: AudioFormat) -> OutputOptions {
    OutputOptions {
        format,
        directory: Some(dir.to_path_buf()),
        folder_mode: FolderMode::Collection,
        fetch_lyrics: false,
        ..OutputOptions::default()
    }
}

#[tokio::test]
async fn an_album_lands_as_numbered_tagged_files_in_its_own_folder() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 0, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Album, vec![track("First Song", addr, 1), track("Second Song", addr, 2)]),
            options: options(&out, AudioFormat::Mp3),
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");

    assert_eq!(enqueued.job_ids.len(), 2);
    assert_eq!(enqueued.skipped, 0);
    assert_eq!(
        enqueued.directory,
        out.join("Test Album"),
        "an album gets a folder named after itself"
    );

    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(60)).await;
    for job in &jobs {
        assert_eq!(job.state, MusicJobState::Finished, "{}: {:?}", job.track.title, job.error);
    }

    // Numbered, named from the template, and in the album's folder.
    let first = out.join("Test Album").join("01 - Test Artist - First Song.mp3");
    let second = out.join("Test Album").join("02 - Test Artist - Second Song.mp3");
    assert!(first.exists(), "expected {}", first.display());
    assert!(second.exists(), "expected {}", second.display());
    assert!(std::fs::metadata(&first).unwrap().len() > 0);

    // The tags the catalogue supplied are actually in the file.
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format_tags", "-of", "default=nw=1"])
        .arg(&first)
        .output()
        .expect("ffprobe");
    let tags = String::from_utf8_lossy(&probe.stdout).to_ascii_lowercase();
    assert!(tags.contains("first song"), "title tag missing: {tags}");
    assert!(tags.contains("test artist"), "artist tag missing: {tags}");
    assert!(tags.contains("test album"), "album tag missing: {tags}");
    assert!(tags.contains("electronic"), "genre tag missing: {tags}");
    assert!(tags.contains("2024"), "date tag missing: {tags}");

    // The cover was fetched and attached, so the file carries a video stream
    // that is a still picture rather than a movie.
    let streams = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=codec_name", "-of", "default=nw=1:nk=1"])
        .arg(&first)
        .output()
        .expect("ffprobe");
    assert!(
        String::from_utf8_lossy(&streams.stdout).trim().contains("mjpeg"),
        "the cover art should be embedded as an attached picture"
    );

    // No temporary files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(out.join("Test Album"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("zoltraak-") || n.contains("opendm-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left behind: {leftovers:?}");
}

#[tokio::test]
async fn best_keeps_the_source_container_and_a_second_run_skips_what_is_there() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 0, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let single = || source(SourceKind::Track, vec![track("Lone Song", addr, 1)]);

    let first = music
        .enqueue(NewMusicDownload {
            source: single(),
            options: options(&out, AudioFormat::Best),
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    // A single track never gets a folder of its own.
    assert_eq!(first.directory, out);

    let jobs = settle(&music, &first.job_ids, Duration::from_secs(45)).await;
    assert_eq!(jobs[0].state, MusicJobState::Finished, "{:?}", jobs[0].error);
    let landed = jobs[0].path.clone().expect("a finished job has a path");
    assert_eq!(
        landed.extension().and_then(|e| e.to_str()),
        Some("mp3"),
        "\"best\" keeps an MP3 source as an MP3 rather than re-encoding it"
    );

    let after_first = origin.requests.load(Ordering::SeqCst);

    // The same track again: the file is there, so nothing is fetched.
    let second = music
        .enqueue(NewMusicDownload {
            source: single(),
            options: options(&out, AudioFormat::Best),
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    assert_eq!(second.skipped, 1, "the existing file should be recognised at enqueue");
    let jobs = settle(&music, &second.job_ids, Duration::from_secs(20)).await;
    assert_eq!(jobs[0].state, MusicJobState::Skipped);
    assert_eq!(jobs[0].path.as_deref(), Some(landed.as_path()), "the skip points at the file that is there");
    assert_eq!(
        origin.requests.load(Ordering::SeqCst),
        after_first,
        "a skipped track must not touch the origin at all"
    );

    // Asking for it anyway downloads a numbered copy and leaves the first alone.
    music.resume(jobs[0].id, true).await.expect("force resume");
    let jobs = settle(&music, &second.job_ids, Duration::from_secs(45)).await;
    assert_eq!(jobs[0].state, MusicJobState::Finished, "{:?}", jobs[0].error);
    let copy = jobs[0].path.clone().expect("path");
    assert_ne!(copy, landed, "a forced re-download must not overwrite the original");
    assert!(landed.exists() && copy.exists());
    // Missing-file recovery reuses the completed entry and re-resolves its source.
    let id = jobs[0].id;
    assert!(music.restart_missing(id).await.is_err(), "an existing output cannot restart");
    std::fs::remove_file(&copy).unwrap();
    music.restart_missing(id).await.expect("restart missing output");
    assert!(music.restart_missing(id).await.is_err(), "a second restart cannot claim the same completed job");
    let recovered = settle(&music, &[id], Duration::from_secs(45)).await;
    assert_eq!(recovered[0].state, MusicJobState::Finished, "{:?}", recovered[0].error);
    assert!(recovered[0].path.as_ref().unwrap().is_file());
    assert!(landed.is_file(), "recovery must keep the original track from the first run");
}

#[tokio::test]
async fn a_held_track_stops_and_continues_when_resumed() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    // Dribbled out, so the pause lands mid-transfer rather than after it.
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 40, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Track, vec![track("Slow Song", addr, 1)]),
            options: OutputOptions { skip_existing: false, ..options(&out, AudioFormat::Best) },
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    let id = enqueued.job_ids[0];

    // Wait until it is actually moving before holding it.
    let mut running = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if music.get(id).await.is_some_and(|j| j.state == MusicJobState::Downloading) {
            running = true;
            break;
        }
    }
    assert!(running, "the job never reached the downloading state");

    assert!(music.pause(id).await, "a running job reports that it was held");
    let mut paused = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let job = music.get(id).await.expect("job exists");
        if job.state == MusicJobState::Paused {
            paused = Some(job);
            break;
        }
    }
    let job = paused.expect("the job should come to rest as paused");
    assert!(job.error.is_none(), "holding a track is not a failure");
    assert!(job.is_resumable());

    // The partial file does not stay behind pretending to be a track.
    let leftovers: Vec<_> = std::fs::read_dir(&out)
        .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    assert!(
        !leftovers.iter().any(|n| n.ends_with(".mp3")),
        "a held track must not leave a playable-looking file: {leftovers:?}"
    );

    music.resume(id, false).await.expect("resume");
    let jobs = settle(&music, &[id], Duration::from_secs(60)).await;
    assert_eq!(jobs[0].state, MusicJobState::Finished, "{:?}", jobs[0].error);
    assert!(jobs[0].path.as_ref().is_some_and(|p| p.exists()));
}

#[tokio::test]
async fn a_job_left_mid_flight_by_a_crash_is_requeued_not_failed() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 40, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let out = dir.path().join("library");
    let ids;
    {
        // A crash is simulated in the row rather than by abandoning a live
        // manager: dropping one only drops the handle — its spawned task
        // keeps running and keeps writing, which a killed process would not.
        // What has to be tested is what the *database* is left holding.
        let (music, queue) = manager(dir.path()).await;
        let enqueued = music
            .enqueue(NewMusicDownload {
                source: source(SourceKind::Track, vec![track("Interrupted", addr, 1)]),
                options: OutputOptions { skip_existing: false, ..options(&out, AudioFormat::Best) },
                base_directory: out.clone(),
            })
            .await
            .expect("enqueue");
        ids = enqueued.job_ids.clone();

        // Stop it cleanly, then stamp the row the way a kill mid-transfer
        // would leave it.
        music.pause(ids[0]).await;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if music.get(ids[0]).await.is_some_and(|j| !j.state.is_active()) {
                break;
            }
        }
        sqlx::query("UPDATE music_jobs SET state = 'downloading', downloaded_bytes = 4096 WHERE id = ?")
            .bind(ids[0].to_string())
            .execute(&queue.pool())
            .await
            .expect("stamp the row as interrupted");
    }

    // A second manager over the same database: recovery requeues the row
    // rather than calling an interrupted transfer a failure.
    let (music, _queue) = manager(dir.path()).await;
    let job = music.get(ids[0]).await.expect("the job survived the restart");
    assert!(
        matches!(job.state, MusicJobState::Queued | MusicJobState::Resolving | MusicJobState::Downloading),
        "an interrupted job comes back runnable, not failed: {:?}",
        job.state
    );
    assert!(job.error.is_none(), "an interruption is not an error to report");

    let jobs = settle(&music, &ids, Duration::from_secs(60)).await;
    assert_eq!(jobs[0].state, MusicJobState::Finished, "{:?}", jobs[0].error);
    assert!(jobs[0].path.as_ref().is_some_and(|p| p.exists()));
}

#[tokio::test]
async fn a_dead_source_fails_that_track_alone() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 0, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");

    let mut broken = track("Missing Song", addr, 2);
    broken.source = AudioSource::Direct {
        url: format!("http://{addr}/nowhere.mp3"),
        headers: Vec::new(),
        container: Some("mp3".into()),
    };
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Album, vec![track("Good Song", addr, 1), broken]),
            options: options(&out, AudioFormat::Best),
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");

    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(60)).await;
    let good = jobs.iter().find(|j| j.track.title == "Good Song").expect("the good track");
    let bad = jobs.iter().find(|j| j.track.title == "Missing Song").expect("the bad track");
    assert_eq!(good.state, MusicJobState::Finished, "one dead link must not cost the rest: {:?}", good.error);
    assert_eq!(bad.state, MusicJobState::Failed);
    assert!(bad.error.is_some(), "a failure has to say what went wrong");
    assert!(good.path.as_ref().is_some_and(|p| p.exists()));

    // And it can be retried on its own once the reason is gone.
    let failed_id = bad.id;
    music.resume(failed_id, false).await.expect("retry");
    let job = music.get(failed_id).await.expect("job");
    assert!(
        matches!(job.state, MusicJobState::Queued | MusicJobState::Resolving | MusicJobState::Downloading | MusicJobState::Failed),
        "a retried job goes back to work: {:?}",
        job.state
    );
}

#[tokio::test]
async fn a_track_waiting_for_a_youtube_sign_in_requeues_itself_when_one_arrives() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    // Refuse the first request, which is what an anonymous fetch of a matched
    // track meets: the CDN says no before a byte is served. The next one is
    // served, standing in for the same fetch once a sign-in is behind it.
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 0, refuse_first: AtomicUsize::new(1) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, queue, vault) = manager_with_vault(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Track, vec![track("Gated Song", addr, 1)]),
            options: OutputOptions { skip_existing: false, ..options(&out, AudioFormat::Best) },
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    let id = enqueued.job_ids[0];

    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(45)).await;
    assert_eq!(jobs[0].state, MusicJobState::Failed, "the refused fetch fails first time round");

    // Stand in for the YouTube refusal's own marker. The 403 path that writes
    // it needs a real googlevideo host to recognise, which a local origin
    // cannot be; what this test is for is everything that happens *after* the
    // marker is on the row.
    sqlx::query("UPDATE music_jobs SET stage = ? WHERE id = ?")
        .bind(WAITING_FOR_YOUTUBE_SESSION)
        .bind(id.to_string())
        .execute(&queue.pool())
        .await
        .expect("mark the row as waiting for a sign-in");

    // The user signs in and the extension shares the session. Nothing else
    // happens: no retry is clicked.
    vault
        .set("youtube.com", SiteSession { cookie: Some(YOUTUBE_SIGN_IN.into()), ..Default::default() })
        .await
        .expect("store the session");

    // Nothing else is done: no retry is clicked. The track reaching a
    // finished file is itself the proof that the watcher requeued it — and it
    // is a steadier thing to assert than a Queued state that a fast retry can
    // pass through between two polls.
    let mut jobs = Vec::new();
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        jobs = music.list().await.into_iter().filter(|j| j.id == id).collect();
        if jobs.first().is_some_and(|j| j.state == MusicJobState::Finished) {
            break;
        }
    }
    assert_eq!(
        jobs.first().map(|j| j.state),
        Some(MusicJobState::Finished),
        "a shared YouTube sign-in should requeue the waiting track and see it through: {:?}",
        jobs.first().and_then(|j| j.error.clone())
    );
    assert!(jobs[0].path.as_ref().is_some_and(|p| p.exists()));
    assert_eq!(jobs[0].stage, None, "the marker is cleared once it is no longer waiting");
}

#[tokio::test]
async fn a_session_for_another_site_leaves_the_waiting_tracks_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (music, queue, vault) = manager_with_vault(dir.path()).await;
    let addr: SocketAddr = "127.0.0.1:9".parse().expect("addr");
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Track, vec![track("Gated Song", addr, 1)]),
            options: OutputOptions { skip_existing: false, ..options(&out, AudioFormat::Best) },
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    let id = enqueued.job_ids[0];
    settle(&music, &enqueued.job_ids, Duration::from_secs(30)).await;
    sqlx::query("UPDATE music_jobs SET state = 'failed', stage = ? WHERE id = ?")
        .bind(WAITING_FOR_YOUTUBE_SESSION)
        .bind(id.to_string())
        .execute(&queue.pool())
        .await
        .expect("mark");

    // Instagram signing in says nothing about YouTube's preview cap.
    vault
        .set("instagram.com", SiteSession { cookie: Some("sessionid=x".into()), ..Default::default() })
        .await
        .expect("store");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let row: (String,) = sqlx::query_as("SELECT state FROM music_jobs WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&queue.pool())
        .await
        .expect("read back");
    assert_eq!(row.0, "failed", "only a YouTube sign-in lifts a YouTube refusal");

    // And a YouTube cookie that is not a login must not either — YouTube
    // keeps a stale APISID after a logout.
    vault
        .set("youtube.com", SiteSession { cookie: Some("SAPISID=def".into()), ..Default::default() })
        .await
        .expect("store");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let row: (String,) = sqlx::query_as("SELECT state FROM music_jobs WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&queue.pool())
        .await
        .expect("read back");
    assert_eq!(row.0, "failed", "a signed-out cookie is not a sign-in");
}

#[tokio::test]
async fn a_collection_saves_the_albums_own_cover_beside_its_files() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("track jpeg");
    let album_cover = album_jpeg(dir.path()).expect("album jpeg");
    assert_ne!(cover, album_cover, "the two pictures must be distinguishable");
    let origin = Arc::new(Origin {
        audio,
        cover,
        album_cover: album_cover.clone(),
        requests: AtomicUsize::new(0),
        piece_delay: 0,
        refuse_first: AtomicUsize::new(0),
    });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source_with_cover(
                SourceKind::Album,
                vec![track("First Song", addr, 1), track("Second Song", addr, 2)],
                Some(format!("http://{addr}/album.jpg")),
            ),
            options: OutputOptions { save_cover_file: true, ..options(&out, AudioFormat::Best) },
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");

    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(60)).await;
    for job in &jobs {
        assert_eq!(job.state, MusicJobState::Finished, "{}: {:?}", job.track.title, job.error);
    }

    let folder = out.join("Test Album");
    let cover_file = folder.join("cover.jpg");
    assert!(cover_file.exists(), "a collection should leave its cover beside the files");
    assert_eq!(
        std::fs::read(&cover_file).expect("read cover"),
        album_cover,
        "the album's own cover is written, not whichever track happened to finish first"
    );
    // Windows reads folder.jpg for a folder's thumbnail; players read cover.jpg.
    let folder_file = folder.join("folder.jpg");
    assert!(folder_file.exists(), "folder.jpg is what Windows shows on the folder");
    assert_eq!(std::fs::read(&folder_file).expect("read folder.jpg"), album_cover);

    // Written once for the whole collection, not once per track.
    let covers: Vec<String> = std::fs::read_dir(&folder)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("cover") || n.starts_with("folder"))
        .collect();
    assert_eq!(covers.len(), 2, "exactly cover.jpg and folder.jpg: {covers:?}");
    assert!(
        !std::fs::read_dir(&folder).unwrap().filter_map(|e| e.ok()).any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.contains("zoltraak-") || name.contains("opendm-")
        }),
        "no scratch files left behind"
    );
}

#[tokio::test]
async fn a_single_track_keeps_its_picture_beside_the_file() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("jpeg");
    let origin = Arc::new(Origin {
        audio,
        cover: cover.clone(),
        album_cover: cover.clone(),
        requests: AtomicUsize::new(0),
        piece_delay: 0,
        refuse_first: AtomicUsize::new(0),
    });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Track, vec![track("Lone Song", addr, 1)]),
            options: OutputOptions { save_cover_file: true, ..options(&out, AudioFormat::Best) },
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");

    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(45)).await;
    assert_eq!(jobs[0].state, MusicJobState::Finished, "{:?}", jobs[0].error);

    // A single has no folder to speak for, so the picture takes the file's name
    // rather than claiming to be the cover of everything around it.
    let audio_path = jobs[0].path.clone().expect("path");
    let picture = audio_path.with_extension("jpg");
    assert!(picture.exists(), "expected {}", picture.display());
    assert_eq!(std::fs::read(&picture).expect("read"), cover);
    assert!(!out.join("cover.jpg").exists(), "one track must not claim the whole folder's cover");
}

#[tokio::test]
async fn removing_a_collection_clears_its_rows_but_never_the_files() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(audio) = sample_mp3(dir.path()) else {
        eprintln!("skipping: ffmpeg could not produce a sample");
        return;
    };
    let cover = sample_jpeg(dir.path()).expect("ffmpeg can make a jpeg");
    let origin = Arc::new(Origin { audio, album_cover: cover.clone(), cover, requests: AtomicUsize::new(0), piece_delay: 0, refuse_first: AtomicUsize::new(0) });
    let addr = spawn_origin(Arc::clone(&origin)).await;

    let (music, _queue) = manager(dir.path()).await;
    let out = dir.path().join("library");
    let enqueued = music
        .enqueue(NewMusicDownload {
            source: source(SourceKind::Album, vec![track("Keep One", addr, 1), track("Keep Two", addr, 2)]),
            options: options(&out, AudioFormat::Best),
            base_directory: out.clone(),
        })
        .await
        .expect("enqueue");
    let jobs = settle(&music, &enqueued.job_ids, Duration::from_secs(60)).await;
    let files: Vec<PathBuf> = jobs.iter().filter_map(|j| j.path.clone()).collect();
    assert_eq!(files.len(), 2);

    let collection = enqueued.collection_id.expect("an album is a collection");
    let removed = music.remove_collection(&collection).await.expect("remove");
    assert_eq!(removed, 2);
    assert!(music.list().await.is_empty(), "the rows are gone");
    for file in &files {
        assert!(file.exists(), "removing a row must never delete the finished file: {}", file.display());
    }
}
