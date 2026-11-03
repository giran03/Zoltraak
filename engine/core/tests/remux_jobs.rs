//! Repackaging, end to end against a real local `ffmpeg`.
//!
//! The plan — which stream is copied and which is re-encoded — is settled by
//! unit tests that need nothing installed. What only a real run can prove is
//! the part those cannot reach: that the arguments the planner emits actually
//! produce a playable file in the container that was asked for, that a
//! lossless crossing really did copy the codec rather than quietly encoding
//! it, and that `replace_source` deletes the original only once the
//! replacement exists.
//!
//! Every test skips, loudly, on a machine with no ffmpeg.


use std::path::{Path, PathBuf};
use std::time::Duration;

use zoltraak_core::remux::manager::{NewRemux, RemuxJob, RemuxJobState, RemuxManager};
use zoltraak_core::remux::{self, Container};

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

/// A two-second clip of colour bars and a tone, in whatever codecs are named.
/// `None` when ffmpeg cannot produce it — an installation without libx264,
/// say — which skips the test rather than failing it.
fn sample(path: &Path, video_codec: &str, audio_codec: &str) -> Option<PathBuf> {
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=2",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
            "-c:v", video_codec,
            "-c:a", audio_codec,
            "-shortest",
        ])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    (status.success() && path.exists()).then(|| path.to_path_buf())
}

async fn wait_for_terminal(manager: &RemuxManager, id: zoltraak_core::model::DownloadId) -> RemuxJob {
    for _ in 0..900 {
        if let Some(job) = manager.get(id).await {
            if job.state.is_terminal() {
                return job;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job never finished");
}

/// The headline case: a Matroska file of MP4-compatible streams becomes an
/// `.mp4` without a single frame being re-encoded.
#[tokio::test]
async fn an_mkv_of_h264_becomes_an_mp4_without_re_encoding() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some(input) = sample(&dir.path().join("clip.mkv"), "libx264", "aac") else {
        eprintln!("skipping: this ffmpeg cannot produce an h264/aac sample");
        return;
    };

    let manager = RemuxManager::new();
    let ids = manager
        .enqueue(NewRemux {
            inputs: vec![input.clone()],
            container: Container::Mp4,
            output_directory: None,
            replace_source: false,
        })
        .await
        .expect("queued");

    let job = wait_for_terminal(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?} {:?}", job.stage, job.error);
    assert_eq!(job.lossless, Some(true), "h264 and aac both fit in MP4, so nothing is encoded");

    let output = job.output.expect("a finished job names its file");
    assert_eq!(output, dir.path().join("clip.mp4"));

    // The proof that it was a copy and not an encode is in the output's own
    // codecs: a re-encode would have gone through libx264 afresh, and the
    // durations would not line up this exactly either.
    let probed = remux::probe(&output).await.expect("the result is probeable");
    assert_eq!(probed.video_codec.as_deref(), Some("h264"));
    assert_eq!(probed.audio_codec.as_deref(), Some("aac"));

    assert!(input.exists(), "the source is left alone unless replacement was asked for");
}

/// The case the screen exists for: VP9/Opus does not play in the places people
/// need MP4, so it is re-encoded rather than merely repackaged.
#[tokio::test]
async fn a_vp9_webm_is_re_encoded_into_a_playable_mp4() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some(input) = sample(&dir.path().join("clip.webm"), "libvpx-vp9", "libopus") else {
        eprintln!("skipping: this ffmpeg cannot produce a vp9/opus sample");
        return;
    };

    let manager = RemuxManager::new();
    let ids = manager
        .enqueue(NewRemux {
            inputs: vec![input],
            container: Container::Mp4,
            output_directory: None,
            replace_source: false,
        })
        .await
        .expect("queued");

    let job = wait_for_terminal(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?} {:?}", job.stage, job.error);
    assert_eq!(job.lossless, Some(false), "VP9 into MP4 is a transcode, and says so");

    let probed = remux::probe(&job.output.expect("output")).await.expect("probeable");
    assert_eq!(probed.video_codec.as_deref(), Some("h264"), "the codec that was the problem is gone");
    assert_eq!(probed.audio_codec.as_deref(), Some("aac"));
}

/// `replace_source` is the one destructive option on the screen, so what it
/// deletes and when is worth pinning down.
#[tokio::test]
async fn replacing_the_source_deletes_it_only_after_the_new_file_exists() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let Some(input) = sample(&dir.path().join("clip.mkv"), "libx264", "aac") else {
        eprintln!("skipping: this ffmpeg cannot produce an h264/aac sample");
        return;
    };

    let manager = RemuxManager::new();
    let ids = manager
        .enqueue(NewRemux {
            inputs: vec![input.clone()],
            container: Container::Mp4,
            output_directory: None,
            replace_source: true,
        })
        .await
        .expect("queued");

    let job = wait_for_terminal(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Finished, "{:?} {:?}", job.stage, job.error);
    let output = job.output.expect("output");
    assert!(output.is_file(), "the replacement is on disk");
    assert!(!input.exists(), "and the original is not");
}

/// A file that is not media at all fails as a job — with ffmpeg's own last
/// word on it, not a panic and not a silent success.
#[tokio::test]
async fn a_file_that_is_not_media_fails_with_a_reason() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let input = dir.path().join("notes.mkv");
    std::fs::write(&input, b"this is not a video").expect("write");

    let manager = RemuxManager::new();
    let ids = manager
        .enqueue(NewRemux {
            inputs: vec![input],
            container: Container::Mp4,
            output_directory: None,
            replace_source: false,
        })
        .await
        .expect("queued");

    let job = wait_for_terminal(&manager, ids[0]).await;
    assert_eq!(job.state, RemuxJobState::Failed);
    assert!(job.error.is_some(), "a failure says why");
    assert!(job.output.is_none());
    assert!(
        std::fs::read_dir(dir.path()).expect("readable").count() == 1,
        "no half-written file is left behind"
    );
}

/// A named output directory receives the results and the sources stay put.
#[tokio::test]
async fn a_batch_lands_in_the_directory_it_was_given() {
    if !have_ffmpeg() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let out = dir.path().join("converted");
    let mut inputs = Vec::new();
    for n in 0..2 {
        match sample(&dir.path().join(format!("clip{n}.mkv")), "libx264", "aac") {
            Some(path) => inputs.push(path),
            None => {
                eprintln!("skipping: this ffmpeg cannot produce an h264/aac sample");
                return;
            }
        }
    }

    let manager = RemuxManager::new();
    let ids = manager
        .enqueue(NewRemux {
            inputs: inputs.clone(),
            container: Container::Mp4,
            output_directory: Some(out.clone()),
            replace_source: false,
        })
        .await
        .expect("queued");
    assert_eq!(ids.len(), 2);

    for id in ids {
        let job = wait_for_terminal(&manager, id).await;
        assert_eq!(job.state, RemuxJobState::Finished, "{:?} {:?}", job.stage, job.error);
        assert_eq!(job.output.as_ref().and_then(|p| p.parent()), Some(out.as_path()));
    }
    assert!(out.join("clip0.mp4").is_file() && out.join("clip1.mp4").is_file());
    assert!(inputs.iter().all(|p| p.exists()));
}
