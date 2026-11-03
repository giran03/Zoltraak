//! Typed local conversion recipes. Planning and execution use the same recipe;
//! the window never needs its own codec compatibility table.
use crate::{
    error::{Error, Result},
    remux::{Container, MediaInfo, Plan, StreamAction},
};

/// Conversion clients resolve their current working folder at enqueue time.
pub fn default_output_directory(working_directory: &Path) -> std::path::PathBuf {
    working_directory.join("Converted")
}

#[derive(Clone)]
struct CachedSource {
    bytes: u64,
    modified: Option<std::time::SystemTime>,
    tool: std::path::PathBuf,
    info: MediaInfo,
}

/// Cache inspection while editing settings; execution always probes afresh.
pub async fn inspect_for_plan(path: &Path) -> Result<MediaInfo> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, CachedSource>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let metadata = std::fs::metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let key = std::path::absolute(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let tool = crate::media_tools::find_ffprobe();
    let bytes = metadata.len();
    let modified = metadata.modified().ok();
    if let Ok(entries) = cache.lock() {
        if let Some(entry) = entries.get(&key) {
            if entry.bytes == bytes && entry.modified == modified && entry.tool == tool {
                let mut info = entry.info.clone();
                info.path = path.to_path_buf();
                return Ok(info);
            }
        }
    }
    let info = crate::remux::probe(path).await?;
    if !info.unprobed {
        if let Ok(mut entries) = cache.lock() {
            if entries.len() >= 500 {
                entries.clear();
            }
            entries.insert(
                key,
                CachedSource {
                    bytes,
                    modified,
                    tool,
                    info: info.clone(),
                },
            );
        }
    }
    Ok(info)
}
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Mp4,
    Mkv,
    Mov,
    Webm,
    Gif,
    Webp,
    Png,
    Jpg,
    Bmp,
    Tiff,
    Mp3,
    M4a,
    Alac,
    Flac,
    Wav,
    Aiff,
    Opus,
    Ogg,
}
impl Format {
    pub const ALL: [Self; 18] = [
        Self::Mp4,
        Self::Mkv,
        Self::Mov,
        Self::Webm,
        Self::Gif,
        Self::Webp,
        Self::Png,
        Self::Jpg,
        Self::Bmp,
        Self::Tiff,
        Self::Mp3,
        Self::M4a,
        Self::Alac,
        Self::Flac,
        Self::Wav,
        Self::Aiff,
        Self::Opus,
        Self::Ogg,
    ];
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Mp3 => "mp3",
            Self::M4a | Self::Alac => "m4a",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Opus => "opus",
            Self::Ogg => "ogg",
        }
    }
    pub fn audio(self) -> bool {
        matches!(
            self,
            Self::Mp3
                | Self::M4a
                | Self::Alac
                | Self::Flac
                | Self::Wav
                | Self::Aiff
                | Self::Opus
                | Self::Ogg
        )
    }
    pub fn image(self) -> bool {
        matches!(self, Self::Png | Self::Jpg | Self::Bmp | Self::Tiff)
    }
    pub fn animation(self) -> bool {
        matches!(self, Self::Gif | Self::Webp)
    }
    pub fn container(self) -> Option<Container> {
        match self {
            Self::Mp4 => Some(Container::Mp4),
            Self::Mkv => Some(Container::Mkv),
            Self::Mov => Some(Container::Mov),
            Self::Webm => Some(Container::Webm),
            _ => None,
        }
    }
    /// The FFmpeg muxer that writes this format.
    pub fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "matroska",
            Self::Mov => "mov",
            Self::Webm => "webm",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Png | Self::Jpg | Self::Bmp | Self::Tiff => "image2",
            Self::M4a | Self::Alac => "ipod",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Opus => "opus",
            Self::Ogg => "ogg",
        }
    }

    /// Filters a recipe for this format may use; a build without them
    /// leaves the format unavailable rather than failing mid-conversion.
    pub fn required_filters(self) -> &'static [&'static str] {
        if self.audio() {
            &["loudnorm", "afade"]
        } else if self == Self::Gif {
            &["scale", "fps", "palettegen", "paletteuse"]
        } else {
            &["scale", "fps", "pad", "overlay", "drawbox"]
        }
    }

    pub fn encoder(self) -> &'static str {
        match self {
            Self::Mp4 | Self::Mkv | Self::Mov => "libx264",
            Self::Webm => "libvpx-vp9",
            Self::Gif => "gif",
            Self::Webp => "libwebp_anim",
            Self::Png => "png",
            Self::Jpg => "mjpeg",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Mp3 => "libmp3lame",
            Self::M4a => "aac",
            Self::Alac => "alac",
            Self::Flac => "flac",
            Self::Wav => "pcm_s16le",
            Self::Aiff => "pcm_s16be",
            Self::Opus => "libopus",
            Self::Ogg => "libvorbis",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Settings {
    pub format: Format,
    /// Auto copies compatible streams only when no transformations were requested.
    pub mode: String,
    pub quality: u8,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub start_seconds: f64,
    pub end_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub mute: bool,
    pub audio_bitrate: u32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub audio_track: u32,
    pub normalize: bool,
    pub normalize_two_pass: bool,
    pub loudness_lufs: f64,
    pub true_peak_db: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub strip_metadata: bool,
    pub background: String,
    pub loop_count: u16,
    pub dither: String,
    pub lossless: bool,
    /// Approximate size budget for video and lossy-audio output, in MiB.
    pub target_size_mb: Option<f64>,
    /// Output filename pattern: `{name}`, `{format}`, `{date}` and `{time}`.
    pub name_template: String,
    /// The part of the picture to keep, as fractions of the source size.
    pub crop: Option<Crop>,
    /// Clockwise quarter turns in degrees: 0, 90, 180 or 270.
    pub rotate: u16,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

/// A rectangle as fractions (0 to 1) of the source picture, so it holds at any resolution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Crop {
    fn is_valid(&self) -> bool {
        const MIN_SIDE: f64 = 0.01;
        const SLACK: f64 = 1e-6;
        [self.x, self.y, self.width, self.height].iter().all(|n| n.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.width >= MIN_SIDE
            && self.height >= MIN_SIDE
            && self.x + self.width <= 1.0 + SLACK
            && self.y + self.height <= 1.0 + SLACK
    }
    /// Even sizes and offsets, as the video encoders require.
    fn filter(&self) -> String {
        let even = |axis: &str, fraction: f64| format!("trunc(i{axis}*{fraction:.6}/2)*2");
        format!(
            "crop={}:{}:{}:{}",
            even("w", self.width),
            even("h", self.height),
            even("w", self.x),
            even("h", self.y)
        )
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: Format::Mp4,
            mode: "auto".into(),
            quality: 80,
            width: None,
            height: None,
            fps: None,
            start_seconds: 0.0,
            end_seconds: None,
            duration_seconds: None,
            mute: false,
            audio_bitrate: 192,
            sample_rate: None,
            channels: None,
            audio_track: 0,
            normalize: false,
            normalize_two_pass: false,
            loudness_lufs: -16.0,
            true_peak_db: -1.5,
            fade_in: 0.0,
            fade_out: 0.0,
            strip_metadata: false,
            background: "#ffffff".into(),
            loop_count: 0,
            dither: "sierra2_4a".into(),
            lossless: false,
            target_size_mb: None,
            name_template: DEFAULT_NAME_TEMPLATE.into(),
            crop: None,
            rotate: 0,
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

pub const DEFAULT_NAME_TEMPLATE: &str = "{name}";
const NAME_TOKENS: [&str; 4] = ["name", "format", "date", "time"];
const RESERVED_NAMES: [&str; 8] = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "LPT1", "LPT2"];
/// Container overhead and rate-control drift are absorbed by aiming below the budget.
const SIZE_BUDGET_MARGIN: f64 = 0.95;
const MIN_VIDEO_KBPS: f64 = 30.0;

/// Bitrates that fit a size budget; `video` is `None` for audio-only output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeBudget {
    pub video_kbps: Option<u32>,
    pub audio_kbps: u32,
}

fn name_template_error(template: &str) -> Option<&'static str> {
    if template.trim().is_empty() || template.chars().count() > 120 {
        return Some("The filename pattern must be 1 to 120 characters");
    }
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        if !rest[open..].starts_with('{') {
            return Some("The filename pattern has an unmatched }");
        }
        let Some(close) = rest[open..].find('}') else {
            return Some("The filename pattern has an unmatched {");
        };
        if !NAME_TOKENS.contains(&&rest[open + 1..open + close]) {
            return Some("Use only {name}, {format}, {date} and {time} in the filename pattern");
        }
        rest = &rest[open + close + 1..];
    }
    None
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionPlan {
    pub streams: Plan,
    pub description: String,
    pub warnings: Vec<String>,
    pub duration_seconds: Option<f64>,
    pub output_extension: String,
}

pub fn still(info: &MediaInfo) -> bool {
    matches!(
        info.extension.as_str(),
        "jpg" | "jpeg" | "png" | "bmp" | "tif" | "tiff"
    ) || (info.extension == "webp" && info.duration_seconds.is_none())
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        let fail = |s: &str| Err(Error::InvalidInput(s.into()));
        if !matches!(self.mode.as_str(), "auto" | "encode" | "copy") {
            return fail("Choose automatic, encode, or stream copy mode");
        }
        if self.quality == 0 || self.quality > 100 {
            return fail("Quality must be between 1 and 100");
        }
        if [self.width, self.height]
            .into_iter()
            .flatten()
            .any(|n| !(1..=16384).contains(&n))
        {
            return fail("Dimensions must be between 1 and 16384 pixels");
        }
        if self
            .fps
            .is_some_and(|n| !n.is_finite() || !(1.0..=120.0).contains(&n))
        {
            return fail("Frame rate must be between 1 and 120");
        }
        if !self.start_seconds.is_finite()
            || self.start_seconds < 0.0
            || self
                .end_seconds
                .is_some_and(|n| !n.is_finite() || n <= self.start_seconds)
            || self
                .duration_seconds
                .is_some_and(|n| !n.is_finite() || n <= 0.0 || n > 86400.0)
        {
            return fail("Choose a valid time range and a duration up to 24 hours");
        }
        if !(6..=512).contains(&self.audio_bitrate) {
            return fail("Audio bitrate must be between 6 and 512 kbps");
        }
        if !self.loudness_lufs.is_finite() || !(-70.0..=-5.0).contains(&self.loudness_lufs)
            || !self.true_peak_db.is_finite() || !(-9.0..=0.0).contains(&self.true_peak_db) {
            return fail("Loudness must be -70 to -5 LUFS and true peak -9 to 0 dBTP");
        }
        if self.sample_rate.is_some_and(|n| {
            ![
                8000, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 192000,
            ]
            .contains(&n)
        }) || self.channels.is_some_and(|n| ![1, 2].contains(&n))
        {
            return fail("Choose a supported sample rate and mono or stereo channels");
        }
        if self.format == Format::Opus
            && self
                .sample_rate
                .is_some_and(|n| ![8000, 12000, 16000, 24000, 48000].contains(&n))
        {
            return fail("Opus requires 8000, 12000, 16000, 24000 or 48000 Hz");
        }
        if [self.fade_in, self.fade_out]
            .iter()
            .any(|n| !n.is_finite() || *n < 0.0 || *n > 3600.0)
        {
            return fail("Fades must be between zero and 3600 seconds");
        }
        if self.background.len() != 7
            || !self.background.starts_with('#')
            || !self.background[1..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return fail("Background must be a six-digit hex color");
        }
        if !matches!(self.dither.as_str(), "sierra2_4a" | "bayer" | "none") {
            return fail("Choose a supported dithering method");
        }
        if self.audio_track > 63 {
            return fail("Audio track index must be between 0 and 63");
        }
        if self.format.audio() && self.mute {
            return fail("Audio output cannot be muted");
        }
        if !matches!(self.rotate, 0 | 90 | 180 | 270) {
            return fail("Rotate by 0, 90, 180 or 270 degrees");
        }
        if self.crop.is_some_and(|crop| !crop.is_valid()) {
            return fail("The crop area must stay inside the picture");
        }
        if self.format.audio()
            && (self.crop.is_some()
                || self.rotate != 0
                || self.flip_horizontal
                || self.flip_vertical)
        {
            return fail("Crop, rotate and flip apply to pictures, not audio");
        }
        if let Some(message) = name_template_error(&self.name_template) {
            return fail(message);
        }
        if let Some(size) = self.target_size_mb {
            if !size.is_finite() || !(0.1..=100_000.0).contains(&size) {
                return fail("Target size must be between 0.1 and 100000 MB");
            }
            if !self.supports_target_size() {
                return fail("Target size works with video and lossy audio (MP3, M4A, Opus, Ogg)");
            }
        }
        Ok(())
    }
    fn supports_target_size(&self) -> bool {
        self.format.container().is_some()
            || matches!(
                self.format,
                Format::Mp3 | Format::M4a | Format::Opus | Format::Ogg
            )
    }
    /// The output filename without its extension, safe on every supported filesystem.
    pub fn output_stem(&self, source_stem: &str) -> String {
        let now = chrono::Local::now();
        let expanded = self
            .name_template
            .replace("{name}", source_stem)
            .replace("{format}", self.format.extension())
            .replace("{date}", &now.format("%Y-%m-%d").to_string())
            .replace("{time}", &now.format("%H-%M-%S").to_string());
        let cleaned: String = expanded
            .chars()
            .map(|c| {
                if c.is_control() || "<>:\"/\\|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let cleaned = cleaned.trim().trim_end_matches('.').trim().to_string();
        if cleaned.is_empty() {
            return source_stem.to_string();
        }
        if RESERVED_NAMES.contains(&cleaned.to_ascii_uppercase().as_str()) {
            return format!("_{cleaned}");
        }
        cleaned
    }
    /// Split a size budget between picture and sound for a known output length.
    pub fn size_budget(
        &self,
        duration: f64,
        has_video: bool,
        has_audio: bool,
    ) -> Result<Option<SizeBudget>> {
        let Some(size) = self.target_size_mb else {
            return Ok(None);
        };
        if !duration.is_finite() || duration <= 0.0 {
            return Err(Error::InvalidInput(
                "Target size needs a known output duration".into(),
            ));
        }
        let total_kbps = size * 8388.608 * SIZE_BUDGET_MARGIN / duration;
        if !has_video {
            if total_kbps < 6.0 {
                return Err(Error::InvalidInput(
                    "The target size is too small for this length; raise it or shorten the clip".into(),
                ));
            }
            return Ok(Some(SizeBudget {
                video_kbps: None,
                audio_kbps: total_kbps.min(512.0) as u32,
            }));
        }
        let audio = if has_audio {
            f64::from(self.audio_bitrate).min(total_kbps * 0.3).max(6.0)
        } else {
            0.0
        };
        let video = total_kbps - audio;
        if video < MIN_VIDEO_KBPS {
            return Err(Error::InvalidInput(
                "The target size is too small for this length; raise it or shorten the clip".into(),
            ));
        }
        Ok(Some(SizeBudget {
            video_kbps: Some(video as u32),
            audio_kbps: audio as u32,
        }))
    }
    fn transforms_video(&self) -> bool {
        self.crop.is_some()
            || self.rotate != 0
            || self.flip_horizontal
            || self.flip_vertical
            || self.width.is_some()
            || self.height.is_some()
            || self.fps.is_some()
            || self.start_seconds > 0.0
            || self.end_seconds.is_some()
            || self.duration_seconds.is_some()
    }
    fn transforms_audio(&self) -> bool {
        self.sample_rate.is_some()
            || self.channels.is_some()
            || self.normalize
            || self.fade_in > 0.0
            || self.fade_out > 0.0
            || self.start_seconds > 0.0
            || self.end_seconds.is_some()
            || self.duration_seconds.is_some()
    }
    pub fn plan(&self, info: &MediaInfo) -> Result<ConversionPlan> {
        self.validate()?;
        if info.unsupported_animation {
            return Err(Error::InvalidInput("This FFmpeg build cannot decode animated WebP inputs. Static WebP and animated WebP export are supported".into()));
        }
        let mut selected = info.clone();
        if let Some(track) = info.audio_tracks.get(self.audio_track as usize) {
            selected.audio_codec = Some(track.codec.clone());
        }
        if !info.audio_tracks.is_empty()
            && !self.mute
            && !self.format.image()
            && !self.format.animation()
            && self.audio_track as usize >= info.audio_tracks.len()
        {
            return Err(Error::InvalidInput(
                "The selected audio track does not exist".into(),
            ));
        }
        let info = &selected;
        if info.unprobed {
            return Err(Error::InvalidInput(
                "FFprobe must successfully read this file before conversion".into(),
            ));
        }
        let audio = info.audio_codec.is_some();
        let video = info.video_codec.is_some();
        if self.format.audio() && !audio {
            return Err(Error::InvalidInput("This source has no audio track".into()));
        }
        if !self.format.audio() && !video {
            return Err(Error::InvalidInput(
                "This output needs a picture or video source".into(),
            ));
        }
        if !still(info)
            && info
                .duration_seconds
                .is_some_and(|n| self.start_seconds >= n)
        {
            return Err(Error::InvalidInput(
                "The start time is past the end of this file".into(),
            ));
        }
        if !still(info)
            && self
                .end_seconds
                .zip(info.duration_seconds)
                .is_some_and(|(end, total)| end > total + 0.05)
        {
            return Err(Error::InvalidInput(
                "The end time is past the end of this file".into(),
            ));
        }
        if still(info)
            && !self.format.image()
            && self.format != Format::Webp
            && self.duration_seconds.is_none()
        {
            return Err(Error::InvalidInput(
                "Set a duration to turn a still image into a clip or animation".into(),
            ));
        }
        let mut streams = self
            .format
            .container()
            .map(|c| crate::remux::plan(info, c))
            .unwrap_or(Plan {
                video: StreamAction::Encode,
                audio: StreamAction::Encode,
            });
        if self.format.audio() {
            streams.video = StreamAction::Absent;
        }
        if self.format.image() || self.format.animation() || self.mute || !audio {
            streams.audio = StreamAction::Absent;
        }
        if self.mode == "encode" || self.transforms_video() || self.format.container().is_none() {
            if streams.video != StreamAction::Absent {
                streams.video = StreamAction::Encode;
            }
        }
        if self.mode == "encode" || self.transforms_audio() || self.format.container().is_none() {
            if streams.audio != StreamAction::Absent {
                streams.audio = StreamAction::Encode;
            }
        }
        // Audio extraction may preserve the original compressed or lossless stream.
        if self.format.audio() && self.mode != "encode" && !self.transforms_audio() {
            let compatible = matches!(
                (self.format, info.audio_codec.as_deref()),
                (Format::Mp3, Some("mp3"))
                    | (Format::M4a, Some("aac"))
                    | (Format::Alac, Some("alac"))
                    | (Format::Flac, Some("flac"))
                    | (Format::Opus, Some("opus"))
                    | (Format::Ogg, Some("vorbis"))
                    | (Format::Wav, Some("pcm_s16le"))
                    | (Format::Aiff, Some("pcm_s16be"))
            );
            if compatible {
                streams.audio = StreamAction::Copy;
            }
        }
        // A size budget is only met by re-encoding whatever it applies to.
        if self.target_size_mb.is_some() {
            if streams.video != StreamAction::Absent {
                streams.video = StreamAction::Encode;
            }
            if streams.audio != StreamAction::Absent {
                streams.audio = StreamAction::Encode;
            }
        }
        if self.mode == "copy" && !streams.is_lossless() {
            return Err(Error::InvalidInput("Stream copy cannot apply these settings or use this output format; choose automatic or encode".into()));
        }
        let duration = if self.format.image() {
            None
        } else {
            self.end_seconds
                .map(|n| n - self.start_seconds)
                .or(self.duration_seconds)
                .or(info
                    .duration_seconds
                    .map(|n| (n - self.start_seconds).max(0.0)))
                .map(|duration| {
                    if still(info) {
                        duration
                    } else {
                        duration.min(
                            info.duration_seconds
                                .map(|n| n - self.start_seconds)
                                .unwrap_or(duration),
                        )
                    }
                })
        };
        if self.fade_out > 0.0 && duration.is_none() {
            return Err(Error::InvalidInput(
                "Fade out needs a known output duration".into(),
            ));
        }
        if duration.is_some_and(|n| self.fade_in > n || self.fade_out > n) {
            return Err(Error::InvalidInput(
                "A fade cannot be longer than the output".into(),
            ));
        }
        let mut warnings = vec![];
        if self.normalize && self.normalize_two_pass && streams.audio == StreamAction::Encode {
            warnings.push("Measures the selected audio in a first pass, then normalizes in a second pass. Silence uses single-pass normalization; unattainable targets may change dynamics".into());
        }
        if let Some(budget) = self.size_budget(
            duration.unwrap_or(0.0),
            streams.video != StreamAction::Absent,
            streams.audio != StreamAction::Absent,
        )? {
            let size = self.target_size_mb.unwrap_or_default();
            warnings.push(match budget.video_kbps {
                Some(video) => format!("Aims for about {size} MB: {video} kbps video, {} kbps audio. The result can land slightly above or below", budget.audio_kbps),
                None => format!("Aims for about {size} MB at {} kbps. The result can land slightly above or below", budget.audio_kbps),
            });
        }
        if audio && streams.audio == StreamAction::Absent {
            warnings.push("Audio is removed".into());
        }
        if self.format.image() && !still(info) {
            warnings.push("Exports one frame at the selected start time".into());
        }
        if self.format == Format::Webp && !still(info) {
            warnings.push("Animated WebP export may not be readable by this FFmpeg build; originals are kept when output decoding cannot be verified".into());
        }
        if matches!(
            self.format,
            Format::Jpg | Format::Bmp | Format::Mp4 | Format::Mov | Format::Mkv | Format::Webm
        ) && streams.video == StreamAction::Encode
        {
            warnings.push(format!(
                "Transparency is flattened against {}",
                self.background
            ));
        }
        if streams.audio != StreamAction::Absent || streams.video != StreamAction::Absent {
            warnings.push("Only the selected audio track and first picture stream are exported; subtitles and attachments are omitted".into());
        }
        if matches!(
            self.format,
            Format::Flac | Format::Alac | Format::Wav | Format::Aiff
        ) && audio
        {
            warnings.push("Lossless output cannot restore detail lost in the source".into());
        }
        if streams.video == StreamAction::Encode {
            warnings.push(
                "HDR and color-profile preservation are not supported for encoded picture output"
                    .into(),
            );
        }
        let description = if streams.is_lossless() {
            "Copies selected streams without re-encoding"
        } else if self.format.image() {
            "Encodes a single image"
        } else if self.format.animation() {
            "Encodes an animation"
        } else if self.format.audio() {
            "Encodes the selected audio track"
        } else {
            "Encodes incompatible or customized streams"
        }
        .into();
        Ok(ConversionPlan {
            streams,
            description,
            warnings,
            duration_seconds: duration,
            output_extension: self.format.extension().into(),
        })
    }

    pub fn args(&self, info: &MediaInfo, dest: &Path, plan: &ConversionPlan) -> Vec<String> {
        let mut args = vec![
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-progress",
            "pipe:1",
            "-nostats",
            "-n",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        // Exclude network-backed inputs and playlist demuxers. Input paths are absolute.
        add(&mut args, "-protocol_whitelist", "file,pipe");
        add(
            &mut args,
            "-format_whitelist",
            "mov,matroska,webm,avi,flv,mpegts,mpeg,asf,mp3,aac,flac,wav,aiff,ogg,image2,png_pipe,jpeg_pipe,webp_pipe,bmp_pipe,tiff_pipe,gif,apng",
        );
        let budget = plan.duration_seconds.and_then(|duration| {
            self.size_budget(
                duration,
                plan.streams.video != StreamAction::Absent,
                plan.streams.audio != StreamAction::Absent,
            )
            .ok()
            .flatten()
        });
        let single_image = self.format.image()
            || (self.format == Format::Webp && still(info) && self.duration_seconds.is_none());
        if still(info) && !single_image {
            add(&mut args, "-loop", "1");
        }
        if matches!(info.extension.as_str(), "gif" | "apng") {
            add(&mut args, "-ignore_loop", "1");
        }
        if self.start_seconds > 0.0 {
            add(&mut args, "-ss", &self.start_seconds.to_string());
        }
        add(&mut args, "-protocol_whitelist", crate::media_tools::LOCAL_PROTOCOLS);
        add(&mut args, "-i", &info.path.to_string_lossy());
        if self.format == Format::Gif {
            add(&mut args, "-protocol_whitelist", crate::media_tools::LOCAL_PROTOCOLS);
            add(
                &mut args,
                "-i",
                &dest.with_extension("palette.png").to_string_lossy(),
            );
        }
        if let Some(duration) = plan.duration_seconds {
            add(&mut args, "-t", &duration.to_string());
        }
        if single_image {
            add(&mut args, "-frames:v", "1");
            if self.format.image() {
                add(&mut args, "-update", "1");
            }
        }
        args.extend(["-sn".into(), "-dn".into()]);
        if self.strip_metadata {
            add(&mut args, "-map_metadata", "-1");
        }
        if plan.streams.video != StreamAction::Absent {
            add(
                &mut args,
                "-map",
                if self.format == Format::Gif {
                    "[gifout]"
                } else {
                    "0:V:0"
                },
            );
            add(
                &mut args,
                "-c:v",
                if plan.streams.video == StreamAction::Copy {
                    "copy"
                } else {
                    self.format.encoder()
                },
            );
            if plan.streams.video == StreamAction::Encode {
                let mut filters = vec![];
                if let Some(crop) = &self.crop {
                    filters.push(crop.filter());
                }
                match self.rotate {
                    90 => filters.push("transpose=1".into()),
                    180 => filters.push("hflip,vflip".into()),
                    270 => filters.push("transpose=2".into()),
                    _ => {}
                }
                if self.flip_horizontal {
                    filters.push("hflip".into());
                }
                if self.flip_vertical {
                    filters.push("vflip".into());
                }
                if self.width.is_some() || self.height.is_some() {
                    filters.push(format!(
                        "scale={}:{}:force_original_aspect_ratio=decrease:flags=lanczos",
                        self.width
                            .map(|n| format!("min(iw\\,{n})"))
                            .unwrap_or("-2".into()),
                        self.height
                            .map(|n| format!("min(ih\\,{n})"))
                            .unwrap_or("-2".into())
                    ));
                }
                if let Some(fps) = self.fps {
                    filters.push(format!("fps={fps}"));
                }
                if self.format.container().is_some() {
                    filters.push("pad=ceil(iw/2)*2:ceil(ih/2)*2".into());
                }
                if matches!(
                    self.format,
                    Format::Jpg
                        | Format::Bmp
                        | Format::Mp4
                        | Format::Mov
                        | Format::Mkv
                        | Format::Webm
                ) {
                    filters.push(format!("split[fg][bg];[bg]drawbox=c={}:t=fill:replace=1[canvas];[canvas][fg]overlay=shortest=1:format=auto", self.background));
                }
                if self.format == Format::Gif {
                    if filters.is_empty() {
                        filters.push("null".into());
                    }
                    add(
                        &mut args,
                        "-filter_complex",
                        &format!(
                            "[0:V:0]{}[frames];[frames][1:v]paletteuse=dither={}[gifout]",
                            filters.join(","),
                            self.dither
                        ),
                    );
                } else if !filters.is_empty() {
                    add(&mut args, "-vf", &filters.join(","));
                }
                let target_video = budget.and_then(|b| b.video_kbps);
                match self.format {
                    Format::Mp4 | Format::Mov | Format::Mkv => {
                        if let Some(kbps) = target_video {
                            add(&mut args, "-b:v", &format!("{kbps}k"));
                            add(&mut args, "-maxrate", &format!("{}k", kbps * 3 / 2));
                            add(&mut args, "-bufsize", &format!("{}k", kbps * 2));
                        } else {
                            add(
                                &mut args,
                                "-crf",
                                &((100 - self.quality) as f32 * 0.45 + 5.0)
                                    .round()
                                    .to_string(),
                            );
                        }
                        add(&mut args, "-preset", "veryfast");
                        add(&mut args, "-pix_fmt", "yuv420p");
                    }
                    Format::Webm => {
                        if let Some(kbps) = target_video {
                            add(&mut args, "-b:v", &format!("{kbps}k"));
                            add(&mut args, "-maxrate", &format!("{}k", kbps * 3 / 2));
                        } else {
                            add(
                                &mut args,
                                "-crf",
                                &((100 - self.quality) as f32 * 0.5 + 10.0)
                                    .round()
                                    .to_string(),
                            );
                            add(&mut args, "-b:v", "0");
                        }
                        add(&mut args, "-pix_fmt", "yuv420p");
                    }
                    Format::Jpg => {
                        add(
                            &mut args,
                            "-q:v",
                            &(2 + (100 - self.quality) as u32 * 29 / 99).to_string(),
                        );
                    }
                    Format::Png => {
                        add(
                            &mut args,
                            "-compression_level",
                            &((self.quality as u32 * 9) / 100).to_string(),
                        );
                    }
                    Format::Webp => {
                        add(&mut args, "-quality", &self.quality.to_string());
                        add(
                            &mut args,
                            "-lossless",
                            if self.lossless { "1" } else { "0" },
                        );
                        add(&mut args, "-loop", &self.loop_count.to_string());
                    }
                    Format::Gif => add(
                        &mut args,
                        "-loop",
                        &if self.loop_count == 1 {
                            "-1".into()
                        } else if self.loop_count == 0 {
                            "0".into()
                        } else {
                            (self.loop_count - 1).to_string()
                        },
                    ),
                    _ => {}
                }
            }
        } else {
            args.push("-vn".into());
        }
        if plan.streams.audio != StreamAction::Absent {
            add(&mut args, "-map", &format!("0:a:{}", self.audio_track));
            let encoder = if self.format.audio() {
                self.format.encoder()
            } else if self.format == Format::Webm {
                "libopus"
            } else {
                "aac"
            };
            add(
                &mut args,
                "-c:a",
                if plan.streams.audio == StreamAction::Copy {
                    "copy"
                } else {
                    encoder
                },
            );
            if plan.streams.audio == StreamAction::Encode {
                if !matches!(
                    self.format,
                    Format::Flac | Format::Alac | Format::Wav | Format::Aiff
                ) {
                    let kbps = budget.map_or(self.audio_bitrate, |b| b.audio_kbps);
                    add(&mut args, "-b:a", &format!("{kbps}k"));
                }
                if let Some(rate) = self.sample_rate {
                    add(&mut args, "-ar", &rate.to_string());
                }
                if let Some(channels) = self.channels {
                    add(&mut args, "-ac", &channels.to_string());
                }
                let mut filters = vec![];
                if self.normalize {
                    if let Some(channels) = self.channels {
                        filters.push(format!("aformat=channel_layouts={}", if channels == 1 { "mono" } else { "stereo" }));
                    }
                    filters.push(self.loudness_filter());
                }
                if self.fade_in > 0.0 {
                    filters.push(format!("afade=t=in:d={}", self.fade_in));
                }
                if self.fade_out > 0.0 {
                    filters.push(format!(
                        "afade=t=out:st={}:d={}",
                        plan.duration_seconds.unwrap_or(0.0) - self.fade_out,
                        self.fade_out
                    ));
                }
                if !filters.is_empty() {
                    add(&mut args, "-af", &filters.join(","));
                }
            }
        } else {
            args.push("-an".into());
        }
        if matches!(
            self.format,
            Format::Mp4 | Format::Mov | Format::M4a | Format::Alac
        ) {
            add(&mut args, "-movflags", "+faststart");
        }
        args.push(dest.to_string_lossy().into_owned());
        args
    }

    pub fn loudness_filter(&self) -> String {
        format!("loudnorm=I={}:TP={}:LRA=11", self.loudness_lufs, self.true_peak_db)
    }

    /// Analyze exactly the selected time range/track, before output fades.
    pub fn loudness_analysis_args(&self, info: &MediaInfo, plan: &ConversionPlan) -> Vec<String> {
        let mut recipe = self.clone();
        recipe.format = Format::Wav;
        recipe.target_size_mb = None;
        recipe.fade_in = 0.0;
        recipe.fade_out = 0.0;
        recipe.normalize = true;
        let mut audio_plan = plan.clone();
        audio_plan.streams.video = StreamAction::Absent;
        audio_plan.streams.audio = StreamAction::Encode;
        let mut args = recipe.args(info, Path::new("unused.wav"), &audio_plan);
        args.pop();
        if let Some(index) = args.iter().position(|arg| arg == "-loglevel") { args[index + 1] = "info".into(); }
        if let Some(index) = args.iter().position(|arg| arg == "-af") {
            args[index + 1].push_str(":print_format=json");
        }
        args.extend(["-f".into(), "null".into(), "-".into()]);
        args
    }

    /// Replace only the loudnorm node; retain fades and all other recipe options.
    pub fn apply_loudness_measurement(&self, args: &mut [String], stderr: &str) -> bool {
        let Some(start) = stderr.rfind('{') else { return false; };
        let Some(end) = stderr[start..].find('}') else { return false; };
        let Ok(values) = serde_json::from_str::<serde_json::Value>(&stderr[start..=start + end]) else { return false; };
        let fields = [("input_i", "measured_I"), ("input_tp", "measured_TP"), ("input_lra", "measured_LRA"), ("input_thresh", "measured_thresh"), ("target_offset", "offset")];
        let mut filter = self.loudness_filter();
        for (key, option) in fields {
            let value = values[key].as_str().and_then(|v| v.parse::<f64>().ok()).or_else(|| values[key].as_f64());
            let Some(value) = value.filter(|v| v.is_finite()) else { return false; };
            filter.push_str(&format!(":{option}={value}"));
        }
        filter.push_str(":linear=true");
        if let Some(index) = args.iter().position(|arg| arg == "-af") {
            args[index + 1] = args[index + 1].replacen(&self.loudness_filter(), &filter, 1);
            return true;
        }
        false
    }

    /// Analyze the selected clip in a separate pass, keeping palette generation
    /// from buffering the whole animation in a split filter graph.
    pub fn palette_args(
        &self,
        info: &MediaInfo,
        dest: &Path,
        plan: &ConversionPlan,
    ) -> Vec<String> {
        let mut palette = self.clone();
        palette.format = Format::Png;
        let mut args = palette.args(info, &dest.with_extension("palette.png"), plan);
        let filter = args.iter().position(|arg| arg == "-vf");
        if let Some(index) = filter {
            args[index + 1].push_str(",palettegen");
        } else {
            let last = args.len() - 1;
            args.splice(last..last, ["-vf".into(), "palettegen".into()]);
        }
        args
    }
}
fn add(args: &mut Vec<String>, key: &str, value: &str) {
    args.extend([key.into(), value.into()]);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub formats: Vec<Format>,
    pub ffprobe: bool,
}
async fn listing(option: &str) -> Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        crate::media_tools::ffmpeg_command()
            .args(["-hide_banner", option])
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| Error::InvalidInput("FFmpeg capability check timed out".into()))?
    .map_err(crate::remux::launch_error)?;
    if !output.status.success() {
        return Err(Error::InvalidInput(format!(
            "FFmpeg could not report {option}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub async fn capabilities() -> Result<Capabilities> {
    let (encoders, muxers, filters) = tokio::try_join!(
        listing("-encoders"),
        listing("-muxers"),
        listing("-filters")
    )?;
    let encoders = encoders
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect::<Vec<_>>();
    let muxers = muxers
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .flat_map(|names| names.split(','))
        .collect::<Vec<_>>();
    let filters = filters
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .collect::<Vec<_>>();
    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        crate::media_tools::ffprobe_command()
            .arg("-version")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .output(),
    )
    .await;
    Ok(Capabilities {
        formats: Format::ALL
            .into_iter()
            .filter(|f| {
                encoders.contains(&f.encoder())
                    && muxers.contains(&f.muxer())
                    && f.required_filters().iter().all(|filter| filters.contains(filter))
                    && (f.container().is_none()
                        || encoders.contains(&if *f == Format::Webm { "libopus" } else { "aac" }))
            })
            .collect(),
        ffprobe: matches!(probe,Ok(Ok(ref p)) if p.status.success()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_only_needs_components_the_bundled_build_is_checked_for() {
        let caps = crate::media_tools::required_capabilities();
        for format in Format::ALL {
            assert!(caps.encoders.iter().any(|e| e == format.encoder()), "{format:?} encoder {} not in media_capabilities.json", format.encoder());
            assert!(caps.muxers.iter().any(|m| m == format.muxer()), "{format:?} muxer {} not in media_capabilities.json", format.muxer());
            for filter in format.required_filters() {
                assert!(caps.filters.iter().any(|f| f == filter), "{format:?} filter {filter} not in media_capabilities.json");
            }
        }
    }

    #[test]
    fn loudness_targets_and_measurements_are_validated_and_legacy_recipes_keep_defaults() {
        let legacy: Settings = serde_json::from_str(r#"{"normalize":true}"#).unwrap();
        assert!(!legacy.normalize_two_pass);
        assert_eq!(legacy.loudness_lufs, -16.0);
        assert_eq!(legacy.true_peak_db, -1.5);
        for (lufs, peak) in [(-71.0, -1.5), (-4.0, -1.5), (-16.0, 1.0), (f64::NAN, -1.5)] {
            assert!(Settings { loudness_lufs: lufs, true_peak_db: peak, ..Default::default() }.validate().is_err());
        }
        let settings = Settings { normalize: true, ..Default::default() };
        let original = format!("{},afade=t=in:d=1", settings.loudness_filter());
        let mut args = vec!["-af".into(), original.clone()];
        assert!(!settings.apply_loudness_measurement(&mut args, r#"{"input_i":"-inf"}"#));
        assert_eq!(args[1], original, "silence or malformed analysis keeps single-pass fallback");
        assert!(settings.apply_loudness_measurement(&mut args, r#"{"input_i":"-30","input_tp":"-15","input_lra":"1","input_thresh":"-40","target_offset":"0.1"}"#));
        assert!(args[1].contains("measured_I=-30"));
        assert!(args[1].ends_with(",afade=t=in:d=1"), "measured pass preserves fades");
    }

    fn with_template(template: &str) -> Settings {
        Settings {
            name_template: template.into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_template_expands_tokens_and_rejects_unknown_ones() {
        let settings = with_template("{name}-{format}");
        assert_eq!(settings.output_stem("clip"), "clip-mp4");
        assert!(settings.validate().is_ok());
        for bad in ["", "{nam}", "{name", "name}", "{}"] {
            assert!(with_template(bad).validate().is_err(), "{bad}");
        }
        assert!(with_template(&"a".repeat(121)).validate().is_err());
    }

    #[test]
    fn name_template_output_is_filesystem_safe() {
        assert_eq!(with_template("a/b:c").output_stem("x"), "a_b_c");
        assert_eq!(with_template("{name}...").output_stem("clip"), "clip");
        assert_eq!(with_template("con").output_stem("x"), "_con");
        assert_eq!(with_template("..").output_stem("fallback"), "fallback");
        let stamped = with_template("{date}_{time}").output_stem("x");
        assert_eq!(stamped.len(), "2026-01-01_00-00-00".len());
    }

    #[test]
    fn size_budget_splits_video_and_audio_within_the_target() {
        let settings = Settings {
            target_size_mb: Some(10.0),
            ..Default::default()
        };
        let budget = settings.size_budget(60.0, true, true).unwrap().unwrap();
        let total = f64::from(budget.video_kbps.unwrap() + budget.audio_kbps) * 60.0 / 8.0 / 1024.0;
        assert!(total < 10.0 && total > 8.5, "{total}");
        assert_eq!(budget.audio_kbps, 192);
        let silent = settings.size_budget(60.0, true, false).unwrap().unwrap();
        assert_eq!(silent.audio_kbps, 0);
    }

    #[test]
    fn size_budget_rejects_impossible_or_unsupported_targets() {
        let tiny = Settings {
            target_size_mb: Some(0.1),
            ..Default::default()
        };
        assert!(tiny.size_budget(3600.0, true, true).is_err());
        assert!(tiny.size_budget(0.0, true, true).is_err());
        let flac = Settings {
            format: Format::Flac,
            target_size_mb: Some(5.0),
            ..Default::default()
        };
        assert!(flac.validate().is_err());
        let mp3 = Settings {
            format: Format::Mp3,
            target_size_mb: Some(5.0),
            ..Default::default()
        };
        assert!(mp3.validate().is_ok());
        assert_eq!(
            mp3.size_budget(600.0, false, true).unwrap().unwrap().video_kbps,
            None
        );
    }
}
