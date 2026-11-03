//! Repackaging a file that is already on disk into another container.
//!
//! This is the *local* counterpart to everything else in the engine: nothing
//! is fetched, nothing is resolved, and the only input is a path the user
//! already has. A `.webm` from YouTube, a `.ts` left by an HLS capture and a
//! `.mkv` from anywhere else all play badly in the places people actually put
//! video — a phone, a Discord embed, a video editor — and the fix is almost
//! always a container change rather than a re-encode.
//!
//! So the rule here is: **copy the streams unless the container cannot hold
//! them.** A stream copy is lossless and runs at disk speed; a re-encode costs
//! minutes and quality. [`plan`] decides that per stream, and a job says which
//! of the two it did ([`manager::RemuxJob::lossless`]) so nobody has to guess
//! afterwards why a file took four minutes or why it looks softer.
//!
//! `ffmpeg`/`ffprobe` on `PATH` is the same contract the video merge and the
//! music conversion already rely on.

pub mod manager;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::video::merge::missing_ffmpeg;

/// A container a file can be repackaged into.
///
/// Deliberately short. These are the four that answer a real question —
/// "will this play", "will my editor open it", "will it fit in a web page" —
/// and every extra entry is another compatibility matrix to keep honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Mp4,
    Mkv,
    Mov,
    Webm,
}

impl Container {
    pub const ALL: [Container; 4] = [
        Container::Mp4,
        Container::Mkv,
        Container::Mov,
        Container::Webm,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::Webm => "webm",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "mp4" | "m4v" => Some(Self::Mp4),
            "mkv" | "matroska" => Some(Self::Mkv),
            "mov" | "qt" => Some(Self::Mov),
            "webm" => Some(Self::Webm),
            _ => None,
        }
    }

    /// Whether a file that already has this extension is this container.
    ///
    /// `.m4v` is an MP4 by another name, so asking for MP4 on one is not work
    /// worth doing; `.ts`, `.flv` and the rest are not, however much of an MP4
    /// their contents may be.
    fn holds_extension(self, extension: &str) -> bool {
        Self::parse(extension) == Some(self)
    }

    /// Whether this container can carry `codec`'s video as it stands.
    ///
    /// MP4 is stricter here than the specification is. It *can* legally hold
    /// VP9 or VP8, and ffmpeg will happily write one — but a `.mp4` that
    /// QuickTime, an iPhone and half the editors on the market refuse to open
    /// is not what someone who asked for an MP4 wanted, and the whole reason
    /// they are on this screen is that their VP9 `.webm` did not play.
    fn accepts_video(self, codec: &str) -> bool {
        match self {
            Self::Mkv => true,
            Self::Mp4 => matches!(codec, "h264" | "hevc" | "av1" | "mpeg4"),
            Self::Mov => matches!(codec, "h264" | "hevc" | "prores" | "mpeg4" | "mjpeg"),
            Self::Webm => matches!(codec, "vp8" | "vp9" | "av1"),
        }
    }

    fn accepts_audio(self, codec: &str) -> bool {
        match self {
            Self::Mkv => true,
            Self::Mp4 => matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "alac"),
            Self::Mov => matches!(codec, "aac" | "mp3" | "alac") || codec.starts_with("pcm"),
            Self::Webm => matches!(codec, "opus" | "vorbis"),
        }
    }

    /// What a stream that cannot be copied is encoded to instead.
    fn video_encoder(self) -> &'static str {
        match self {
            Self::Webm => "libvpx-vp9",
            // Matroska takes anything, so this is only ever a fallback for a
            // source ffprobe could not read at all.
            _ => "libx264",
        }
    }

    fn audio_encoder(self) -> &'static str {
        match self {
            Self::Webm => "libopus",
            _ => "aac",
        }
    }

    /// Whether the container wants its index at the front of the file.
    fn wants_faststart(self) -> bool {
        matches!(self, Self::Mp4 | Self::Mov)
    }
}

impl std::fmt::Display for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

/// What a file turned out to be.
///
/// Every field is optional because every field can be absent in practice: a
/// file with no audio, a stream with no declared duration, an `ffprobe` that
/// is not installed. Nothing here is load-bearing — it decides the *plan* and
/// the caption, and an unknown simply means "assume it needs encoding".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: PathBuf,
    /// The file's own extension, lowercased — what the name claims it is.
    pub extension: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrack>,
    /// Some FFmpeg builds encode animated WebP but cannot decode it.
    #[serde(default)]
    pub unsupported_animation: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub bytes: Option<u64>,
    /// Set when `ffprobe` was not available to read the file, so the codecs
    /// above are unknown rather than absent. The UI says so instead of
    /// claiming a file has no audio.
    #[serde(default)]
    pub unprobed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: u32,
    pub codec: String,
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
    pub language: Option<String>,
}

impl MediaInfo {
    pub fn filename(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Whether repackaging into `container` would be a no-op.
    pub fn already_is(&self, container: Container) -> bool {
        container.holds_extension(&self.extension)
    }
}

/// The extensions this screen will offer to work on.
///
/// Not a security boundary — ffmpeg is handed the path either way — but a
/// list this long stops the file list filling with `.txt` and `.exe` when
/// someone drops a whole folder on it.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "wmv",
    "3gp", "ogv", "vob", "asf", "rm", "rmvb", "divx", "f4v", "m4a", "mp3", "aac", "flac", "opus",
    "ogg", "wav", "wma", "alac", "aiff", "jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff",
    "apng", "m4b",
];

/// Whether a path looks like something worth offering to remux.
pub fn is_media_path(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            MEDIA_EXTENSIONS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// Read a local file with `ffprobe`.
///
/// A missing `ffprobe` is not an error: the file is still described by its
/// name and size, flagged [`MediaInfo::unprobed`], and a job over it simply
/// plans conservatively. Refusing to show a list because one of two binaries
/// is absent would be worse than showing it with fewer columns filled in.
pub async fn probe(path: &Path) -> Result<MediaInfo> {
    let metadata = std::fs::metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(Error::InvalidInput(format!(
            "{} is not a file",
            path.display()
        )));
    }

    let mut info = MediaInfo {
        path: path.to_path_buf(),
        extension: path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase(),
        bytes: Some(metadata.len()),
        ..Default::default()
    };

    let output = tokio::time::timeout(std::time::Duration::from_secs(15), crate::media_tools::ffprobe_command()
        .args([
            "-v", "error",
            "-protocol_whitelist", "file,pipe",
            "-format_whitelist", "mov,matroska,webm,avi,flv,mpegts,mpeg,asf,mp3,aac,flac,wav,aiff,ogg,image2,png_pipe,jpeg_pipe,webp_pipe,bmp_pipe,tiff_pipe,gif,apng",
            "-show_entries", "format=duration",
            "-show_entries", "stream=codec_type,codec_name,width,height,channels,sample_rate,duration:stream_tags=language:stream_disposition=attached_pic",
            "-of", "json",
        ])
        .arg(path)
        .stdin(Stdio::null()).kill_on_drop(true)
        .output()).await;

    let output = match output {
        Ok(Ok(output)) if output.status.success() => output,
        // Either ffprobe is missing or it could not make sense of the file.
        // Both leave the caller with a name and a size, which is enough to
        // list it and enough to try.
        _ => {
            info.unprobed = true;
            return Ok(info);
        }
    };

    apply_probe_json(&mut info, &String::from_utf8_lossy(&output.stdout));
    if info.extension == "webp" {
        use std::io::Read;
        let mut header = [0_u8; 30];
        if std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut header))
            .is_ok()
            && &header[..4] == b"RIFF"
            && &header[8..12] == b"WEBP"
            && &header[12..16] == b"VP8X"
        {
            let animated = header[20] & 2 != 0;
            info.unsupported_animation =
                animated && (info.width.unwrap_or(0) == 0 || info.duration_seconds.is_none());
            let dimension =
                |bytes: &[u8]| 1 + u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
            if info.width.unwrap_or(0) == 0 {
                info.width = Some(dimension(&header[24..27]));
            }
            if info.height.unwrap_or(0) == 0 {
                info.height = Some(dimension(&header[27..30]));
            }
        }
    }
    Ok(info)
}

/// Fill `info` from `ffprobe -of json` output. Split out so the parsing is
/// testable without ffprobe on the machine running the tests.
fn apply_probe_json(info: &mut MediaInfo, json: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        info.unprobed = true;
        return;
    };

    info.duration_seconds = value
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);

    for stream in value
        .get("streams")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        let kind = stream
            .get("codec_type")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        let codec = stream
            .get("codec_name")
            .and_then(|c| c.as_str())
            .map(|c| c.to_ascii_lowercase());
        match kind {
            "video"
                if info.video_codec.is_none()
                    && stream
                        .pointer("/disposition/attached_pic")
                        .and_then(|v| v.as_u64())
                        != Some(1) =>
            {
                info.video_codec = codec;
                info.width = stream
                    .get("width")
                    .and_then(|w| w.as_u64())
                    .map(|w| w as u32);
                info.height = stream
                    .get("height")
                    .and_then(|h| h.as_u64())
                    .map(|h| h as u32);
            }
            "audio" => {
                if info.audio_codec.is_none() {
                    info.audio_codec = codec.clone();
                }
                info.audio_tracks.push(AudioTrack {
                    index: info.audio_tracks.len() as u32,
                    codec: codec.unwrap_or_default(),
                    channels: stream
                        .get("channels")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32),
                    sample_rate: stream
                        .get("sample_rate")
                        .and_then(|v| v.as_str())
                        .and_then(|v| v.parse().ok()),
                    language: stream
                        .pointer("/tags/language")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
            }
            _ => {}
        }
    }
}

/// What happens to one stream on the way across.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamAction {
    /// Byte-for-byte. Fast and lossless.
    Copy,
    /// The container cannot hold this codec, so it is encoded to one it can.
    Encode,
    /// There is no such stream in the source.
    Absent,
}

/// How one file crosses into one container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub video: StreamAction,
    pub audio: StreamAction,
}

impl Plan {
    /// Whether the whole job is a straight repackage — no pixels or samples
    /// touched. This is the difference between seconds and minutes, and
    /// between the original quality and a generation of loss, so it is the one
    /// thing a job reports about itself.
    pub fn is_lossless(self) -> bool {
        self.video != StreamAction::Encode && self.audio != StreamAction::Encode
    }

    /// One line naming what is about to happen, for the job's caption.
    pub fn describe(self, container: Container) -> String {
        match (self.video, self.audio) {
            (StreamAction::Encode, StreamAction::Encode) => {
                format!("re-encoding picture and sound for .{container}")
            }
            (StreamAction::Encode, _) => format!("re-encoding the picture for .{container}"),
            (_, StreamAction::Encode) => format!("re-encoding the sound for .{container}"),
            _ => format!("copying the streams into .{container}"),
        }
    }
}

/// Decide what has to happen to get `info` into `container`.
///
/// A stream whose codec is unknown — an unprobed file — is planned as a
/// re-encode. That is the conservative reading: an encode of something that
/// could have been copied wastes time, whereas a copy of something the
/// container cannot hold produces a file that does not play, which is the
/// exact complaint that brought the user here.
pub fn plan(info: &MediaInfo, container: Container) -> Plan {
    let video = match info.video_codec.as_deref() {
        Some(codec) if container.accepts_video(codec) => StreamAction::Copy,
        Some(_) => StreamAction::Encode,
        // Unprobed files have no codecs *and* no proof they have no streams,
        // so they are planned as a full transcode of whatever is in there.
        None if info.unprobed => StreamAction::Encode,
        None => StreamAction::Absent,
    };
    let audio = match info.audio_codec.as_deref() {
        Some(codec) if container.accepts_audio(codec) => StreamAction::Copy,
        Some(_) => StreamAction::Encode,
        None if info.unprobed => StreamAction::Encode,
        None => StreamAction::Absent,
    };
    Plan { video, audio }
}

/// The ffmpeg invocation for one job, as a plain argument list.
///
/// Built as data rather than by poking at a [`Command`] so the decisions above
/// can be asserted in a unit test without ffmpeg being installed.
///
/// The mapping keeps the first video stream and *every* audio stream, and
/// drops subtitles and data streams: a subtitle track that MP4 cannot hold
/// aborts the entire run, and losing a soft subtitle is a smaller surprise
/// than a job that fails on a file that plays fine.
pub fn ffmpeg_args(input: &Path, dest: &Path, container: Container, plan: Plan) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        // Progress on stdout, parsed for the percentage.
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        "-protocol_whitelist".into(),
        crate::media_tools::LOCAL_PROTOCOLS.into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
    ];

    // `?` makes each mapping optional, so an audio-only or picture-only source
    // is not a failure.
    if plan.video != StreamAction::Absent {
        args.extend(["-map".into(), "0:v:0?".into()]);
    }
    if plan.audio != StreamAction::Absent {
        args.extend(["-map".into(), "0:a?".into()]);
    }
    args.extend(["-sn".into(), "-dn".into()]);

    match plan.video {
        StreamAction::Copy => args.extend(["-c:v".into(), "copy".into()]),
        StreamAction::Encode => {
            args.extend(["-c:v".into(), container.video_encoder().into()]);
            match container {
                // Constant-quality rather than a bitrate: the source's own
                // bitrate is the wrong target for a different codec, and CRF
                // is the setting that keeps "looks the same" true.
                Container::Webm => {
                    args.extend(["-crf".into(), "31".into(), "-b:v".into(), "0".into()])
                }
                _ => args.extend([
                    "-crf".into(),
                    "20".into(),
                    "-preset".into(),
                    "veryfast".into(),
                    // 4:2:0 is the only chroma layout every hardware decoder
                    // agrees on; a 4:4:4 source would otherwise produce an
                    // MP4 that phones refuse.
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ]),
            }
        }
        StreamAction::Absent => {}
    }

    match plan.audio {
        StreamAction::Copy => args.extend(["-c:a".into(), "copy".into()]),
        StreamAction::Encode => {
            args.extend(["-c:a".into(), container.audio_encoder().into()]);
            args.extend(["-b:a".into(), "192k".into()]);
        }
        StreamAction::Absent => {}
    }

    if container.wants_faststart() {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }

    args.push(dest.to_string_lossy().into_owned());
    args
}

/// Build the command itself. `missing_ffmpeg` turns a failure to launch into
/// the sentence that names the thing to install.
/// The error a caller gets when ffmpeg is not there at all.
pub(crate) fn launch_error(source: std::io::Error) -> Error {
    missing_ffmpeg("repackage this file")(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_encoders_are_in_the_capability_manifest() {
        let caps = crate::media_tools::required_capabilities();
        for container in Container::ALL {
            for encoder in [container.video_encoder(), container.audio_encoder()] {
                assert!(caps.encoders.iter().any(|e| e == encoder), "{encoder} not in media_capabilities.json");
            }
        }
    }

    fn info(video: Option<&str>, audio: Option<&str>, extension: &str) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("clip.{extension}")),
            extension: extension.into(),
            video_codec: video.map(str::to_string),
            audio_codec: audio.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_webm_of_vp9_and_opus_is_re_encoded_into_mp4() {
        // The whole reason someone asks for MP4: their VP9 file does not play
        // where they need it to. Copying it into an .mp4 container would keep
        // the codec that was the problem.
        let plan = plan(&info(Some("vp9"), Some("opus"), "webm"), Container::Mp4);
        assert_eq!(plan.video, StreamAction::Encode);
        assert_eq!(plan.audio, StreamAction::Encode);
        assert!(!plan.is_lossless());
    }

    #[test]
    fn an_h264_source_crosses_into_mp4_untouched() {
        for extension in ["mkv", "ts", "avi", "flv"] {
            let plan = plan(&info(Some("h264"), Some("aac"), extension), Container::Mp4);
            assert_eq!(plan.video, StreamAction::Copy, "{extension}");
            assert_eq!(plan.audio, StreamAction::Copy, "{extension}");
            assert!(plan.is_lossless(), "{extension}");
        }
    }

    #[test]
    fn matroska_takes_anything_as_it_is() {
        let plan = plan(&info(Some("vp9"), Some("opus"), "webm"), Container::Mkv);
        assert!(plan.is_lossless());
    }

    #[test]
    fn an_absent_stream_is_not_an_encode() {
        let plan = plan(&info(Some("h264"), None, "mp4"), Container::Mov);
        assert_eq!(plan.audio, StreamAction::Absent);
        assert!(plan.is_lossless());

        let args = ffmpeg_args(
            Path::new("in.mp4"),
            Path::new("out.mov"),
            Container::Mov,
            plan,
        );
        assert!(
            !args.iter().any(|a| a == "0:a?"),
            "no audio stream is mapped: {args:?}"
        );
        assert!(!args.iter().any(|a| a == "-c:a"));
    }

    #[test]
    fn an_unprobed_file_is_planned_conservatively() {
        // Nothing is known about it, so the plan must not promise a copy that
        // would produce an unplayable file.
        let mut unknown = info(None, None, "ts");
        unknown.unprobed = true;
        let plan = plan(&unknown, Container::Mp4);
        assert_eq!(plan.video, StreamAction::Encode);
        assert!(!plan.is_lossless());
    }

    #[test]
    fn a_copy_job_asks_ffmpeg_to_copy_and_faststart() {
        let plan = Plan {
            video: StreamAction::Copy,
            audio: StreamAction::Copy,
        };
        let args = ffmpeg_args(
            Path::new("in.mkv"),
            Path::new("out.mp4"),
            Container::Mp4,
            plan,
        );
        assert!(
            args.windows(2).any(|w| w[0] == "-c:v" && w[1] == "copy"),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "copy"),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-movflags" && w[1] == "+faststart"),
            "{args:?}"
        );
        // Subtitles and data are dropped rather than allowed to abort the run.
        assert!(args.iter().any(|a| a == "-sn") && args.iter().any(|a| a == "-dn"));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn an_encode_into_webm_uses_the_containers_own_codecs() {
        let plan = Plan {
            video: StreamAction::Encode,
            audio: StreamAction::Encode,
        };
        let args = ffmpeg_args(
            Path::new("in.mp4"),
            Path::new("out.webm"),
            Container::Webm,
            plan,
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-c:v" && w[1] == "libvpx-vp9"),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w[0] == "-c:a" && w[1] == "libopus"),
            "{args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "-movflags"),
            "webm has no moov atom: {args:?}"
        );
    }

    #[test]
    fn m4v_already_counts_as_mp4() {
        assert!(info(None, None, "m4v").already_is(Container::Mp4));
        assert!(!info(None, None, "ts").already_is(Container::Mp4));
        assert!(!info(None, None, "mov").already_is(Container::Mp4));
    }

    #[test]
    fn probe_json_is_read_into_the_first_stream_of_each_kind() {
        let mut parsed = MediaInfo::default();
        apply_probe_json(
            &mut parsed,
            r#"{
                "streams": [
                    {"codec_type": "video", "codec_name": "VP9", "width": 1920, "height": 1080},
                    {"codec_type": "audio", "codec_name": "opus"},
                    {"codec_type": "audio", "codec_name": "aac"},
                    {"codec_type": "subtitle", "codec_name": "subrip"}
                ],
                "format": {"duration": "212.480000"}
            }"#,
        );
        assert_eq!(
            parsed.video_codec.as_deref(),
            Some("vp9"),
            "codec names are normalised"
        );
        assert_eq!(parsed.audio_codec.as_deref(), Some("opus"));
        assert_eq!(parsed.width, Some(1920));
        assert_eq!(parsed.duration_seconds, Some(212.48));
        assert!(!parsed.unprobed);
    }

    #[test]
    fn a_zero_duration_is_treated_as_unknown() {
        // A raw MPEG-TS often reports "N/A" or 0; a percentage computed from
        // that would be a division by zero pretending to be progress.
        let mut parsed = MediaInfo::default();
        apply_probe_json(
            &mut parsed,
            r#"{"streams": [], "format": {"duration": "0.000000"}}"#,
        );
        assert_eq!(parsed.duration_seconds, None);
    }

    #[test]
    fn media_paths_are_recognised_by_extension() {
        assert!(is_media_path(Path::new("C:/clips/holiday.MKV")));
        assert!(is_media_path(Path::new("/home/a/song.flac")));
        assert!(!is_media_path(Path::new("notes.txt")));
        assert!(!is_media_path(Path::new("no-extension")));
    }
}
