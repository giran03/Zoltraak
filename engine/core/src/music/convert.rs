//! From a fetched stream to a tagged file in the format the user asked for,
//! with a local `ffmpeg`.
//!
//! Three things happen in one ffmpeg run: the audio is transcoded (or, when
//! the source already is what was asked for, copied bit-for-bit), the
//! catalogue's metadata is written as tags, and the cover art is attached.
//! One run rather than three because every pass over an MP3 is a chance to
//! re-encode it by accident, and because a file that exists on disk half-way
//! through is a file the user can open and be confused by — the output is
//! written to a temporary name and renamed into place only once complete.
//!
//! `ffmpeg` and `ffprobe` are expected on `PATH`, the same contract the video
//! merge and the Discord transcode already rely on. Without them the only
//! thing that can be produced is the source stream as it arrived, which the
//! manager handles by not calling this at all.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::AudioFormat;
use crate::error::{Error, Result};
use crate::video::merge::missing_ffmpeg;

/// What is written into the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    /// `YYYY` or `YYYY-MM-DD`.
    pub date: Option<String>,
    pub genre: Option<String>,
    /// The source page, so a file can be traced back to where it came from.
    pub comment: Option<String>,
    pub lyrics: Option<String>,
}

impl Tags {
    fn args(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut push = |key: &str, value: &Option<String>| {
            if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                out.push("-metadata".into());
                out.push(format!("{key}={v}"));
            }
        };
        push("title", &self.title);
        push("artist", &self.artist);
        push("album", &self.album);
        push("album_artist", &self.album_artist);
        push("date", &self.date);
        push("genre", &self.genre);
        push("comment", &self.comment);
        push("lyrics", &self.lyrics);
        if let Some(n) = self.track {
            let value = match self.track_total {
                Some(total) => format!("{n}/{total}"),
                None => n.to_string(),
            };
            out.push("-metadata".into());
            out.push(format!("track={value}"));
        }
        if let Some(d) = self.disc {
            out.push("-metadata".into());
            out.push(format!("disc={d}"));
        }
        out
    }
}

/// Everything one conversion needs.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub input: &'a Path,
    /// The container the input arrived in, when known. Decides what `Best`
    /// means without probing when ffprobe is missing.
    pub input_container: Option<&'a str>,
    /// Whether the input carries a picture the output must not.
    pub has_video: bool,
    pub output_dir: &'a Path,
    /// Without extension; the format decides it.
    pub stem: &'a str,
    pub format: AudioFormat,
    /// kbps, for lossy targets. Ignored for lossless and copy.
    pub bitrate: Option<u32>,
    pub normalize: bool,
    pub tags: Option<&'a Tags>,
    /// A JPEG/PNG/WebP file to attach, for the formats that can carry one.
    pub cover: Option<&'a Path>,
}

/// The codec of the first audio stream in a file, as ffprobe names it
/// (`aac`, `opus`, `mp3`, `vorbis`, `flac`, …).
pub async fn probe_codec(path: &Path) -> Option<String> {
    let output = crate::media_tools::ffprobe_command()
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let codec = String::from_utf8_lossy(&output.stdout).trim().to_ascii_lowercase();
    (!codec.is_empty()).then_some(codec)
}

/// The codec a container implies, for when nothing probed the bytes.
fn codec_of_container(container: &str) -> Option<&'static str> {
    match container.to_ascii_lowercase().as_str() {
        "mp3" => Some("mp3"),
        "m4a" | "mp4" | "aac" => Some("aac"),
        "webm" | "opus" => Some("opus"),
        "ogg" | "oga" => Some("vorbis"),
        "flac" => Some("flac"),
        "wav" => Some("pcm_s16le"),
        _ => None,
    }
}

/// The extension a `Best` output takes for a source codec: the codec's own
/// clean container, so nothing is re-encoded.
pub fn best_extension(codec: &str) -> &'static str {
    match codec {
        "aac" | "alac" => "m4a",
        "opus" => "opus",
        "vorbis" => "ogg",
        "mp3" => "mp3",
        "flac" => "flac",
        c if c.starts_with("pcm") => "wav",
        _ => "m4a",
    }
}

/// Whether a target extension can carry an attached picture the way ffmpeg
/// writes one. Ogg-family containers store pictures as a base64 comment
/// ffmpeg does not produce, so they get their tags and no cover.
fn carries_cover(extension: &str) -> bool {
    matches!(extension, "mp3" | "m4a" | "flac")
}

/// `ffmpeg -version`'s first line, when ffmpeg is on PATH.
pub async fn ffmpeg_version() -> Option<String> {
    let output = crate::media_tools::ffmpeg_command().arg("-version").stdin(Stdio::null()).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    // "ffmpeg version 8.1-full_build-www.gyan.dev Copyright …" → "8.1-full_build-www.gyan.dev"
    Some(line.split_whitespace().nth(2).unwrap_or(line).to_string())
}

pub async fn ffprobe_available() -> bool {
    crate::media_tools::ffprobe_command().arg("-version").stdin(Stdio::null()).output().await.map(|o| o.status.success()).unwrap_or(false)
}

/// The output's final name, numbered if something already holds it.
fn output_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let name = crate::probe::dedupe_filename(&format!("{stem}.{extension}"), |candidate| dir.join(candidate).exists());
    dir.join(name)
}

/// Run the conversion. Returns the finished file's path.
pub async fn convert(req: &Request<'_>, cancel: &CancellationToken) -> Result<PathBuf> {
    std::fs::create_dir_all(req.output_dir).map_err(|source| Error::Io { path: req.output_dir.to_path_buf(), source })?;

    let source_codec = match probe_codec(req.input).await {
        Some(c) => Some(c),
        None => req.input_container.and_then(codec_of_container).map(str::to_string),
    };
    let source_codec = source_codec.as_deref().unwrap_or("");

    // What to do with the audio: copy when the source already is the target
    // (and nothing asks for a filter), else encode.
    let mut audio_args: Vec<String> = Vec::new();
    let extension: &str;
    let copy_ok = !req.normalize;
    match req.format {
        AudioFormat::Best => {
            extension = best_extension(source_codec);
            if copy_ok && !source_codec.is_empty() {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                // Nothing known about the source: the safest generally
                // playable target.
                audio_args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "256k".into()]);
            }
        }
        AudioFormat::Mp3 => {
            extension = "mp3";
            if copy_ok && source_codec == "mp3" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                let k = req.bitrate.unwrap_or(320);
                audio_args.extend(["-c:a".into(), "libmp3lame".into(), "-b:a".into(), format!("{k}k")]);
            }
        }
        AudioFormat::M4a => {
            extension = "m4a";
            if copy_ok && source_codec == "aac" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                let k = req.bitrate.unwrap_or(256);
                audio_args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{k}k")]);
            }
        }
        AudioFormat::Opus => {
            extension = "opus";
            if copy_ok && source_codec == "opus" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                let k = req.bitrate.unwrap_or(160);
                audio_args.extend(["-c:a".into(), "libopus".into(), "-b:a".into(), format!("{k}k")]);
            }
        }
        AudioFormat::Ogg => {
            extension = "ogg";
            if copy_ok && source_codec == "vorbis" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                let k = req.bitrate.unwrap_or(192);
                audio_args.extend(["-c:a".into(), "libvorbis".into(), "-b:a".into(), format!("{k}k")]);
            }
        }
        AudioFormat::Flac => {
            extension = "flac";
            if copy_ok && source_codec == "flac" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                audio_args.extend(["-c:a".into(), "flac".into(), "-compression_level".into(), "8".into()]);
            }
        }
        AudioFormat::Wav => {
            extension = "wav";
            audio_args.extend(["-c:a".into(), "pcm_s16le".into()]);
        }
        AudioFormat::Alac => {
            extension = "m4a";
            if copy_ok && source_codec == "alac" {
                audio_args.extend(["-c:a".into(), "copy".into()]);
            } else {
                audio_args.extend(["-c:a".into(), "alac".into()]);
            }
        }
    }
    if req.normalize {
        // EBU R128 to the streaming loudness most players normalise to.
        audio_args.extend(["-af".into(), "loudnorm=I=-14:TP=-1.5:LRA=11".into()]);
    }

    let final_path = output_path(req.output_dir, req.stem, extension);
    let tmp_path = req.output_dir.join(format!("{}.zoltraak-convert.{extension}", req.stem));

    let cover = req.cover.filter(|c| c.exists()).filter(|_| carries_cover(extension));
    let what = match req.format {
        AudioFormat::Best => "package this track",
        _ => "convert this track",
    };

    // The cover is the one part of this that is allowed to fail.
    //
    // Cover art arrives from whatever a service points at: a wide video
    // thumbnail, a WebP some builds cannot decode, a URL that answered with
    // something that only looks like an image. Any of those makes ffmpeg
    // abort the *whole* run — and then a track that downloaded perfectly is
    // reported as a failure over its artwork. So the attempts step down:
    // crop-and-re-encode, then attach the image as it is, then no cover at
    // all. The audio is identical in all three.
    let attempts: &[CoverMode] = match cover {
        Some(_) => &[CoverMode::Fitted, CoverMode::AsIs, CoverMode::None],
        None => &[CoverMode::None],
    };

    let mut last_error = None;
    for (index, mode) in attempts.iter().enumerate() {
        let _ = std::fs::remove_file(&tmp_path);
        let command = build_command(req, &audio_args, extension, cover.filter(|_| *mode != CoverMode::None), *mode, &tmp_path);
        match run(command, what, cancel).await {
            Ok(()) => {
                if index > 0 {
                    tracing::warn!(stem = req.stem, "the cover art could not be embedded; the track was written without it");
                }
                last_error = None;
                break;
            }
            Err(e) => {
                // A cancellation is the user, not the cover: stop here rather
                // than working down the ladder against their wishes.
                if cancel.is_cancelled() {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(e);
                }
                last_error = Some(e);
            }
        }
    }
    if let Some(e) = last_error {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // The name was chosen before the encode; something may have claimed it
    // meanwhile, so the check is repeated at the moment of the rename.
    let final_path = if final_path.exists() {
        output_path(req.output_dir, req.stem, extension)
    } else {
        final_path
    };
    std::fs::rename(&tmp_path, &final_path).map_err(|source| Error::Io { path: final_path.clone(), source })?;
    Ok(final_path)
}

/// How the cover art is attached, from best to most forgiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverMode {
    /// Re-encoded to a square JPEG: a 16:9 video thumbnail makes poor album
    /// art, and every player draws it in a square.
    Fitted,
    /// Copied in as it arrived. Nothing to decode, so nothing to fail.
    AsIs,
    None,
}

/// One ffmpeg invocation for a given cover mode.
fn build_command(
    req: &Request<'_>,
    audio_args: &[String],
    extension: &str,
    cover: Option<&Path>,
    mode: CoverMode,
    dest: &Path,
) -> Command {
    let mut command = crate::media_tools::ffmpeg_command();
    command.arg("-y").args(crate::media_tools::LOCAL_INPUT).arg("-i").arg(req.input);
    if let Some(cover) = cover {
        command.args(crate::media_tools::LOCAL_INPUT).arg("-i").arg(cover);
    }

    // Only the audio from the source: a muxed video's picture, a source's
    // own embedded art, and any subtitle track are all left behind.
    command.args(["-map", "0:a:0"]);
    command.args(audio_args);

    match (cover, mode) {
        (Some(_), CoverMode::Fitted) => {
            command
                .args(["-map", "1:v:0"])
                .args(["-c:v", "mjpeg", "-q:v", "2"])
                // The comma is escaped rather than quoted: a filtergraph
                // splits on commas, and there is no shell here to strip
                // quotes for us.
                .args(["-filter:v:0", "crop='if(gt(iw/ih\\,1.2)\\,ih\\,iw)':'ih'"]);
            attach_cover_metadata(&mut command, extension);
        }
        (Some(_), CoverMode::AsIs) => {
            command.args(["-map", "1:v:0"]).args(["-c:v", "copy"]);
            attach_cover_metadata(&mut command, extension);
        }
        _ => {
            command.arg("-vn");
            if extension == "mp3" {
                command.args(["-id3v2_version", "3"]);
            }
        }
    }

    if extension == "m4a" {
        command.args(["-movflags", "+faststart"]);
    }
    if let Some(tags) = req.tags {
        command.args(tags.args());
    }
    // WAV cannot hold most of these tags; ffmpeg warns and continues.
    command.arg(dest);
    command
}

fn attach_cover_metadata(command: &mut Command, extension: &str) {
    if extension == "mp3" {
        command.args(["-id3v2_version", "3", "-metadata:s:v", "title=Album cover", "-metadata:s:v", "comment=Cover (front)"]);
    } else {
        command.args(["-disposition:v:0", "attached_pic"]);
    }
}

/// Run ffmpeg, aborting it if the job is cancelled.
async fn run(mut command: Command, what: &str, cancel: &CancellationToken) -> Result<()> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(missing_ffmpeg(what))?;

    let output = tokio::select! {
        result = child.wait_with_output() => result.map_err(|source| Error::Io { path: PathBuf::from("ffmpeg"), source })?,
        _ = cancel.cancelled() => {
            return Err(Error::InvalidInput("stopped while the file was being converted".into()));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).next_back().unwrap_or("no output");
        return Err(Error::InvalidInput(format!("ffmpeg could not {what}: {detail}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_become_metadata_arguments() {
        let tags = Tags {
            title: Some("One More Time".into()),
            artist: Some("Daft Punk".into()),
            album: Some("Discovery".into()),
            track: Some(1),
            track_total: Some(14),
            disc: Some(1),
            date: Some("2001-03-12".into()),
            comment: Some("https://example/t".into()),
            ..Default::default()
        };
        let args = tags.args();
        assert!(args.contains(&"title=One More Time".to_string()));
        assert!(args.contains(&"track=1/14".to_string()));
        assert!(args.contains(&"disc=1".to_string()));
        assert!(args.contains(&"date=2001-03-12".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("genre=")), "an absent tag is not written as empty");
    }

    #[test]
    fn best_keeps_the_source_codec_in_its_own_container() {
        assert_eq!(best_extension("aac"), "m4a");
        assert_eq!(best_extension("opus"), "opus");
        assert_eq!(best_extension("mp3"), "mp3");
        assert_eq!(best_extension("vorbis"), "ogg");
        assert_eq!(best_extension("pcm_s16le"), "wav");
        assert_eq!(codec_of_container("webm"), Some("opus"));
    }

    #[test]
    fn only_some_containers_carry_a_cover() {
        assert!(carries_cover("mp3") && carries_cover("m4a") && carries_cover("flac"));
        assert!(!carries_cover("opus") && !carries_cover("ogg") && !carries_cover("wav"));
    }

    #[test]
    fn the_crop_filter_escapes_its_commas_rather_than_quoting_them() {
        // A filtergraph splits on commas and there is no shell to strip
        // quotes, so an unescaped comma silently becomes a second filter.
        let req = Request {
            input: Path::new("in.mp3"),
            input_container: Some("mp3"),
            has_video: false,
            output_dir: Path::new("."),
            stem: "song",
            format: AudioFormat::Best,
            bitrate: None,
            normalize: false,
            tags: None,
            cover: None,
        };
        let command = build_command(&req, &["-c:a".into(), "copy".into()], "mp3", Some(Path::new("cover.jpg")), CoverMode::Fitted, Path::new("out.mp3"));
        let args: Vec<String> = command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let filter = args.iter().find(|a| a.starts_with("crop=")).expect("the crop filter is passed");
        let unescaped_comma = filter
            .char_indices()
            .any(|(i, c)| c == ',' && !filter[..i].ends_with('\\'));
        assert!(!unescaped_comma, "an unescaped comma would split the filtergraph: {filter}");
        assert!(filter.contains("\\,1.2"), "{filter}");

        // Falling back to AsIs copies the image instead of decoding it.
        let command = build_command(&req, &["-c:a".into(), "copy".into()], "mp3", Some(Path::new("cover.jpg")), CoverMode::AsIs, Path::new("out.mp3"));
        let args: Vec<String> = command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!args.iter().any(|a| a.starts_with("crop=")), "the fallback must not re-encode the image");
        assert!(args.windows(2).any(|w| w[0] == "-c:v" && w[1] == "copy"));

        // With no cover the video is dropped outright.
        let command = build_command(&req, &["-c:a".into(), "copy".into()], "mp3", None, CoverMode::None, Path::new("out.mp3"));
        let args: Vec<String> = command.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|a| a == "-vn"));
    }
}
