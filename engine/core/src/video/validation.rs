//! Evidence about tracks, and bounded checks before a file is called finished.
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::VideoFormat;
use crate::error::{Error, Result};
use crate::headers::Headers;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    #[default]
    Unknown,
    Inferred,
    Verified,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoValidation {
    pub status: ValidationStatus,
    pub has_video: Option<bool>,
    pub has_audio: Option<bool>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub video_duration: Option<f64>,
    #[serde(default)]
    pub audio_duration: Option<f64>,
    pub content_id: Option<String>,
    /// Whether representative samples decoded; this is not a full-file audit.
    #[serde(default)]
    pub samples_decoded: bool,
}

/// Retain content-bearing queries. Only known Google Video signature/transport
/// fields can be excluded. This identity is never used as a fetch URL.
pub fn source_identity(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return value.to_string();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return value.to_string();
    }
    url.set_fragment(None);
    let host = url.host_str().unwrap_or_default();
    let google = (host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
        && url.query_pairs().any(|(key, _)| key == "id");
    let mut pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| {
            !google
                || !matches!(
                    key.as_ref(),
                    "expire"
                        | "sig"
                        | "signature"
                        | "lsig"
                        | "n"
                        | "pot"
                        | "range"
                        | "bytestart"
                        | "byteend"
                )
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    pairs.sort();
    url.set_query(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    url.to_string()
}

pub fn content_identity(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .trim_start_matches("m.");
    let path: Vec<_> = url.path_segments()?.filter(|p| !p.is_empty()).collect();
    let query_id = || {
        url.query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, id)| id.into_owned())
    };
    let (site, id) = match host {
        "youtube.com" => (
            "youtube",
            query_id().or_else(|| {
                matches!(path.first().copied(), Some("shorts" | "embed" | "live"))
                    .then(|| path.get(1).map(|s| s.to_string()))
                    .flatten()
            })?,
        ),
        "youtu.be" => ("youtube", path.first()?.to_string()),
        "facebook.com" => (
            "facebook",
            query_id().or_else(|| {
                path.windows(2)
                    .find(|p| matches!(p[0], "reel" | "videos"))
                    .map(|p| p[1].to_string())
            })?,
        ),
        "instagram.com" => (
            "instagram",
            path.windows(2)
                .find(|p| matches!(p[0], "p" | "reel" | "reels" | "tv"))?[1]
                .to_string(),
        ),
        "x.com" | "twitter.com" => (
            "x",
            path.windows(2).find(|p| p[0] == "status")?[1].to_string(),
        ),
        "tiktok.com" => (
            "tiktok",
            path.windows(2).find(|p| p[0] == "video")?[1].to_string(),
        ),
        "reddit.com" => (
            "reddit",
            path.windows(2).find(|p| p[0] == "comments")?[1].to_string(),
        ),
        "vimeo.com" | "player.vimeo.com" => (
            "vimeo",
            path.iter()
                .find(|p| p.chars().all(|c| c.is_ascii_digit()))?
                .to_string(),
        ),
        _ => return None,
    };
    Some(format!("{site}:{id}"))
}

pub async fn inspect_format(client: &reqwest::Client, format: &mut VideoFormat, headers: &Headers) {
    let mut validation = format.validation.clone().unwrap_or_default();
    validation.samples_decoded = false;
    validation.has_audio = if format.requires_merge {
        Some(true)
    } else {
        format.has_audio
    };
    validation.has_video = if format.is_audio_only() {
        Some(false)
    } else if format.video_codec.is_some() || format.width.is_some() || format.requires_merge {
        Some(true)
    } else {
        None
    };
    validation.status = if validation.has_audio.is_some() || validation.has_video.is_some() {
        ValidationStatus::Inferred
    } else {
        ValidationStatus::Unknown
    };
    let progressive = format.video_url.as_deref().and_then(|s| reqwest::Url::parse(s).ok())
        .is_some_and(|url| ["http", "https"].contains(&url.scheme())
            && ![".m3u8", ".mpd"].iter().any(|suffix| url.path().to_ascii_lowercase().ends_with(suffix)));
    if !progressive || !["mp4", "m4a", "m4v", "mov"].contains(&format.container.as_str()) {
        format.validation = Some(validation);
        return;
    }
    let mut effective = headers.clone();
    for (name, value) in &format.headers {
        effective.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
        effective.push((name.clone(), value.clone()));
    }
    effective.retain(|(key, _)| !key.to_ascii_lowercase().starts_with("x-youtube-"));
    let probe = async {
        let tracks =
            super::mp4probe::probe(client, format.video_url.as_deref()?, &effective).await?;
        let audio = if format.requires_merge {
            Some(super::mp4probe::probe(client, format.audio_url.as_deref()?, &effective).await?)
        } else {
            None
        };
        Some((tracks, audio))
    };
    if let Ok(Some((tracks, audio))) = tokio::time::timeout(Duration::from_secs(4), probe).await {
        validation.status = ValidationStatus::Verified;
        validation.has_video = Some(tracks.video);
        validation.has_audio = Some(audio.map_or(tracks.audio, |t| t.audio));
        format.has_audio = Some(tracks.audio);
        if tracks.is_video_only() && !format.requires_merge && format.id == "direct" {
            format.id = "video-only".into();
        }
        if tracks.is_audio_only() && !format.requires_merge {
            format.container = "m4a".into();
            format.id = "audio".into();
            format.width = None;
            format.height = None;
        }
    }
    format.validation = Some(validation);
}

pub fn check_refresh(old: &VideoFormat, new: &VideoFormat, page: &str) -> Result<()> {
    let reject = || {
        Error::InvalidInput("The source changed its content or track configuration. Analyze it again before restarting.".into())
    };
    if old.id != new.id
        || old.container != new.container
        || old.requires_merge != new.requires_merge
        || old.audio_url.is_some() != new.audio_url.is_some()
    {
        return Err(reject());
    }
    for (a, b) in [
        (&old.video_codec, &new.video_codec),
        (&old.audio_codec, &new.audio_codec),
    ] {
        if a.is_some() && b.is_some() && a != b {
            return Err(reject());
        }
    }
    for (a, b) in [(old.width, new.width), (old.height, new.height)] {
        if a.is_some() && b.is_some() && a != b {
            return Err(reject());
        }
    }
    let expected = old
        .validation
        .as_ref()
        .and_then(|v| v.content_id.clone())
        .or_else(|| content_identity(page));
    let actual = new.validation.as_ref().and_then(|v| v.content_id.clone());
    if expected.is_some() && actual.is_some() && expected != actual {
        return Err(reject());
    }
    if let (Some(a), Some(b)) = (
        old.validation.as_ref().and_then(|v| v.duration),
        new.validation.as_ref().and_then(|v| v.duration),
    ) {
        if (a - b).abs() > 3.0 && (a - b).abs() / a.max(1.0) > 0.1 {
            return Err(reject());
        }
    }
    Ok(())
}

/// Strict about an invalid file, honest about a missing inspection tool.
pub async fn inspect_output(path: &Path, expected: &VideoFormat) -> Result<VideoValidation> {
    if super::direct::is_animated_image_container(&expected.container) {
        return inspect_animated_image(path).await;
    }
    let output = tokio::time::timeout(Duration::from_secs(15), crate::media_tools::ffprobe_command()
        .kill_on_drop(true)
        .args(["-v", "error", "-protocol_whitelist", "file,pipe", "-probesize", "5242880", "-analyzeduration", "5000000",
            "-show_entries", "format=duration:stream=codec_type,codec_name,width,height,duration:stream_disposition=attached_pic", "-of", "json"])
        .arg(path).output()).await
        .map_err(|_| Error::InvalidInput("Video verification timed out. The downloaded file was retained for inspection.".into()))?;
    let output = match output {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(VideoValidation::default()),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(output) => output,
    };
    if !output.status.success() {
        return Err(Error::InvalidInput("Downloaded media is not a readable video/audio container. The file was retained for inspection.".into()));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        Error::InvalidInput("Media verification returned unreadable metadata.".into())
    })?;
    let mut report = report_from_json(&value);
    report.content_id = expected
        .validation
        .as_ref()
        .and_then(|v| v.content_id.clone());
    validate_report(&report, expected)?;
    let duration = report.duration.unwrap_or(0.0);
    let mut samples = Vec::new();
    if report.has_video == Some(true) {
        samples.push((0.0, "0:V:0"));
        if duration > 2.0 {
            samples.extend([
                (duration / 2.0, "0:V:0"),
                ((duration - 1.0).max(0.0), "0:V:0"),
            ]);
        }
    }
    // Decode sound independently: video progress cannot prove audio samples
    // exist, and audio progress cannot prove that any picture decoded.
    if report.has_audio == Some(true) {
        samples.push((report.audio_duration.unwrap_or(duration) / 2.0, "0:a:0"));
    }
    for (position, track) in samples {
        let decoded = tokio::time::timeout(
            Duration::from_secs(15),
            crate::media_tools::ffmpeg_command()
                .kill_on_drop(true)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-v",
                    "error",
                    "-xerror",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-ss",
                ])
                .arg(position.to_string())
                .arg("-i")
                .arg(path)
                .args(["-t", "1", "-map"])
                .arg(track)
                .args(["-progress", "pipe:1", "-nostats", "-f", "null", "-"])
                .output(),
        )
        .await
        .map_err(|_| {
            Error::InvalidInput(
                "Video sample verification timed out. The downloaded file was retained.".into(),
            )
        })?;
        match decoded {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(source) => return Err(Error::Io { path: path.to_path_buf(), source }),
            Ok(output) if !output.status.success() => return Err(Error::InvalidInput("Downloaded media contains an undecodable sample. The file was retained for inspection.".into())),
            Ok(output) => {
                let progress = String::from_utf8_lossy(&output.stdout);
                let metric = if track == "0:V:0" { "frame=" } else { "out_time_us=" };
                let decoded = progress.lines().any(|line| line.strip_prefix(metric)
                    .and_then(|value| value.trim().parse::<u64>().ok()).is_some_and(|v| v > 0));
                if !decoded { return Err(Error::InvalidInput("Media sample contained no decodable frames or audio.".into())); }
            }
        }
    }
    report.samples_decoded = true;
    Ok(report)
}

/// An animated picture has no audio track and ffprobe cannot be trusted with
/// every animated WebP/APNG, so the file's magic number decides instead. An
/// HTML error page saved under a `.gif` name still fails here.
async fn inspect_animated_image(path: &Path) -> Result<VideoValidation> {
    use tokio::io::AsyncReadExt;
    let io_error = |source| Error::Io { path: path.to_path_buf(), source };
    let mut head = [0u8; crate::image::sniff::SNIFF_LEN];
    let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
    let read = file.read(&mut head).await.map_err(io_error)?;
    match crate::image::sniff::identify(&head[..read]) {
        Some(kind) if kind != crate::image::sniff::ImageKind::Svg => Ok(VideoValidation {
            status: ValidationStatus::Verified,
            has_video: Some(true),
            has_audio: Some(false),
            ..Default::default()
        }),
        _ => Err(Error::InvalidInput(
            "Downloaded file is not a readable image or animation. The file was retained for inspection.".into(),
        )),
    }
}

pub fn report_from_json(value: &serde_json::Value) -> VideoValidation {
    let streams = value["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .any(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1);
    let audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0);
    let track_duration = |kind: &str| {
        streams
            .iter()
            .find(|s| s["codec_type"] == kind)
            .and_then(|s| s["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    VideoValidation {
        status: ValidationStatus::Verified,
        has_video: Some(video),
        has_audio: Some(audio),
        duration,
        video_duration: track_duration("video"),
        audio_duration: track_duration("audio"),
        ..Default::default()
    }
}

pub fn validate_report(report: &VideoValidation, expected: &VideoFormat) -> Result<()> {
    if report.has_video != Some(true) && report.has_audio != Some(true) {
        return Err(Error::InvalidInput(
            "Downloaded file has no usable picture or sound tracks.".into(),
        ));
    }
    let audio_only = expected.is_audio_only();
    if !audio_only && report.has_video != Some(true) {
        return Err(Error::InvalidInput(
            "Downloaded file contains no video track.".into(),
        ));
    }
    let expects_audio = audio_only || expected.requires_merge || expected.has_audio == Some(true);
    if expects_audio && report.has_audio != Some(true) {
        return Err(Error::InvalidInput(
            "Downloaded file is missing its expected audio track.".into(),
        ));
    }
    if let (Some(video), Some(audio)) = (report.video_duration, report.audio_duration) {
        if (video - audio).abs() > 3.0 && (video - audio).abs() / video.max(1.0) > 0.1 {
            return Err(Error::InvalidInput("Video and audio durations do not match; the merged file was retained for inspection.".into()));
        }
    }
    if let (Some(a), Some(b)) = (
        expected.validation.as_ref().and_then(|v| v.duration),
        report.duration,
    ) {
        if (a - b).abs() > 3.0 && (a - b).abs() / a.max(1.0) > 0.1 {
            return Err(Error::InvalidInput(
                "Downloaded file duration does not match the selected video.".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_query_changes_and_google_content_ids_are_not_interchangeable() {
        assert_ne!(
            source_identity("https://cdn.test/seg?id=A"),
            source_identity("https://cdn.test/seg?id=B")
        );
        assert_eq!(
            source_identity(
                "https://r.googlevideo.com/videoplayback?id=A&itag=137&expire=1&sig=old"
            ),
            source_identity(
                "https://r.googlevideo.com/videoplayback?sig=new&expire=2&itag=137&id=A"
            )
        );
        assert_ne!(
            source_identity("https://r.googlevideo.com/videoplayback?id=A&itag=137"),
            source_identity("https://r.googlevideo.com/videoplayback?id=B&itag=137")
        );
        assert_eq!(
            content_identity("https://youtu.be/A"),
            content_identity("https://youtube.com/watch?v=A")
        );
    }

    #[test]
    fn same_format_id_cannot_hide_changed_content_codec_or_dimensions() {
        let old = VideoFormat {
            id: "720p".into(),
            video_codec: Some("h264".into()),
            height: Some(720),
            validation: Some(VideoValidation {
                content_id: Some("youtube:A".into()),
                duration: Some(60.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut new = old.clone();
        assert!(check_refresh(&old, &new, "https://youtube.com/watch?v=A").is_ok());
        new.validation.as_mut().unwrap().content_id = Some("youtube:B".into());
        assert!(check_refresh(&old, &new, "https://youtube.com/watch?v=A").is_err());
        new = old.clone();
        new.video_codec = Some("av1".into());
        assert!(check_refresh(&old, &new, "https://youtube.com/watch?v=A").is_err());
        new = old.clone();
        new.height = Some(1080);
        assert!(check_refresh(&old, &new, "https://youtube.com/watch?v=A").is_err());
        new = old.clone();
        new.validation.as_mut().unwrap().duration = Some(10.0);
        assert!(check_refresh(&old, &new, "https://youtube.com/watch?v=A").is_err());
    }

    #[test]
    fn audio_cover_art_does_not_count_as_video_and_silence_can_be_intentional() {
        let audio = report_from_json(&serde_json::json!({"streams": [
            {"codec_type":"video","disposition":{"attached_pic":1}}, {"codec_type":"audio"}
        ], "format":{"duration":"10"}}));
        assert_eq!(audio.has_video, Some(false));
        assert!(validate_report(&audio, &VideoFormat::default()).is_err());
        assert!(
            validate_report(
                &audio,
                &VideoFormat {
                    container: "m4a".into(),
                    ..Default::default()
                }
            )
            .is_ok()
        );
        let silent = VideoValidation {
            has_video: Some(true),
            has_audio: Some(false),
            ..Default::default()
        };
        assert!(
            validate_report(
                &silent,
                &VideoFormat {
                    has_audio: Some(false),
                    ..Default::default()
                }
            )
            .is_ok()
        );
        assert!(
            validate_report(
                &silent,
                &VideoFormat {
                    has_audio: Some(true),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn mismatched_merge_timelines_are_rejected() {
        let report = VideoValidation {
            has_video: Some(true),
            has_audio: Some(true),
            video_duration: Some(60.0),
            audio_duration: Some(10.0),
            ..Default::default()
        };
        assert!(
            validate_report(
                &report,
                &VideoFormat {
                    requires_merge: true,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn real_media_is_verified_and_invalid_bytes_are_rejected() {
        if crate::media_tools::ffprobe_command()
            .arg("-version")
            .output()
            .await
            .is_err()
        {
            eprintln!("ffprobe unavailable; real-media verification fixture not run");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        tokio::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/verified-av.mp4"),
        )
        .await
        .unwrap();
        let expected = VideoFormat {
            has_audio: Some(true),
            ..Default::default()
        };
        let report = inspect_output(&path, &expected).await.unwrap();
        assert_eq!(report.status, ValidationStatus::Verified);
        assert_eq!(
            (report.has_video, report.has_audio),
            (Some(true), Some(true))
        );
        if crate::media_tools::ffmpeg_command()
            .arg("-version")
            .output()
            .await
            .is_ok()
        {
            assert!(report.samples_decoded);
        }
        let wrong_duration = VideoFormat {
            validation: Some(VideoValidation {
                duration: Some(120.0),
                ..Default::default()
            }),
            ..expected
        };
        assert!(inspect_output(&path, &wrong_duration).await.is_err());
        tokio::fs::write(&path, b"<html>login required</html>")
            .await
            .unwrap();
        assert!(
            inspect_output(&path, &VideoFormat::default())
                .await
                .is_err()
        );
        assert!(
            path.exists(),
            "a failed validation retains the downloaded artifact"
        );
    }
}
