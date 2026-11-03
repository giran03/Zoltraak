//! Mux a separately-downloaded video stream and audio stream into one file
//! with a local `ffmpeg` binary. Streams are stream-copied, never
//! re-encoded — the extractor only ever pairs formats drawn from the same
//! manifest, so the codecs already match what the destination container
//! supports.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Turn a failure to *launch* ffmpeg into an error that says what to do.
///
/// "No such file or directory (os error 2)" for a path the user never typed is
/// not a diagnosis. Sites like Facebook, Instagram and X deliver picture and
/// sound as separate streams, so on those a missing ffmpeg is the difference
/// between a finished video and a failed job, and the message has to name the
/// thing to install.
pub(crate) fn missing_ffmpeg(what: &str) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source: std::io::Error| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::InvalidInput(format!(
                "ffmpeg is needed to {what}, and was not found on PATH. Install it from https://ffmpeg.org/download.html, then retry this download."
            ))
        } else {
            Error::Io {
                path: PathBuf::from("ffmpeg"),
                source,
            }
        }
    }
}

pub async fn mux(video: &Path, audio: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let output = crate::media_tools::ffmpeg_command()
        .kill_on_drop(true)
        .arg("-y")
        .arg("-i")
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args(["-c", "copy", "-map", "0:v:0", "-map", "1:a:0"])
        .arg(dest)
        .output()
        .await
        .map_err(missing_ffmpeg("combine this video's picture and sound"))?;

    if !output.status.success() {
        return Err(Error::InvalidInput(format!(
            "ffmpeg failed to merge streams: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}

/// Repackage a single already-muxed stream (typically concatenated MPEG-TS
/// HLS segments) into an `.mp4` container without re-encoding. Lets an HLS
/// download land as a universally playable `.mp4` instead of a raw `.ts`.
///
/// `+faststart` moves the moov atom to the front so the file is seekable and
/// starts playing before it is fully read.
pub async fn remux(input: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let output = crate::media_tools::ffmpeg_command()
        .kill_on_drop(true)
        .arg("-y")
        .args(["-protocol_whitelist", "file,pipe"])
        .arg("-i")
        .arg(input)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(dest)
        .output()
        .await
        .map_err(missing_ffmpeg("repackage this stream"))?;

    if !output.status.success() {
        return Err(Error::InvalidInput(format!(
            "ffmpeg failed to remux stream: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(())
}

/// Join locally normalized periods without copying timestamp resets into output.
/// Stream-copy requires consistent track layouts and codec parameters.
pub(crate) async fn join(inputs: &[PathBuf], dest: &Path) -> Result<()> {
    let mut expected = None;
    for input in inputs {
        let output = tokio::time::timeout(std::time::Duration::from_secs(15), crate::media_tools::ffprobe_command()
            .kill_on_drop(true)
            .args(["-v", "error", "-protocol_whitelist", "file,pipe", "-show_entries",
                "stream=codec_type,codec_name,codec_tag_string,width,height,sample_rate,channels,channel_layout,extradata_hash", "-show_data_hash", "sha256", "-of", "json"])
            .arg(input).output()).await
            .map_err(|_| Error::InvalidInput("Continuity group inspection timed out".into()))?
            .map_err(|source| Error::Io { path: input.clone(), source })?;
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| Error::InvalidInput("Could not inspect continuity group tracks".into()))?;
        let tracks = value
            .get("streams")
            .cloned()
            .filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
            .ok_or_else(|| {
                Error::InvalidInput("Continuity group contains no readable tracks".into())
            })?;
        if !output.status.success() || expected.as_ref().is_some_and(|e| e != &tracks) {
            return Err(Error::InvalidInput("Track or codec changes across periods/discontinuities require transcoding, which is not supported yet".into()));
        }
        expected = Some(tracks);
    }
    let parent = inputs
        .first()
        .and_then(|p| p.parent())
        .ok_or_else(|| Error::InvalidInput("No continuity groups to join".into()))?;
    let list = parent.join("concat.txt");
    // These are generated numeric filenames in the same directory, never URLs.
    let text: String = (0..inputs.len())
        .map(|i| format!("file '{i}.mp4'\n"))
        .collect();
    tokio::fs::write(&list, text)
        .await
        .map_err(|source| Error::Io {
            path: list.clone(),
            source,
        })?;
    let output = crate::media_tools::ffmpeg_command()
        .kill_on_drop(true)
        .args([
            "-y",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "concat",
            "-safe",
            "1",
            "-i",
        ])
        .arg(&list)
        .args([
            "-map",
            "0",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(dest)
        .output()
        .await
        .map_err(missing_ffmpeg("join stream continuity groups"))?;
    if !output.status.success() {
        return Err(Error::InvalidInput(format!(
            "Could not join stream continuity groups: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
