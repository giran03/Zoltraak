//! Bounded looks at a source before converting it: a still frame, a waveform,
//! and a short rendered sample made by the same recipe the real job would use.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::conversion::{Format, Settings};
use crate::error::{Error, Result};

/// Longest sample a preview renders.
pub const MAX_SAMPLE_SECONDS: f64 = 6.0;
/// Largest preview file read back into memory.
const MAX_SAMPLE_BYTES: u64 = 12_000_000;
const STILL_TIMEOUT: Duration = Duration::from_secs(20);
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(45);
/// Waveforms read at most this much audio, decoded to a low sample rate.
const WAVEFORM_SECONDS: u32 = 1800;
const WAVEFORM_RATE: u32 = 4000;
const MAX_STILL_WIDTH: u32 = 1280;
const MAX_WAVEFORM_BUCKETS: usize = 1024;
const MAX_CACHE_BYTES: usize = 48_000_000;
const MAX_CACHE_ENTRIES: usize = 8;

#[derive(Debug, Clone, PartialEq)]
struct SampleKey {
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
    settings: String,
    start: f64,
    length: f64,
    tool: PathBuf,
    tool_modified: Option<std::time::SystemTime>,
}
impl SampleKey {
    fn read(path: &Path, settings: &Settings, start: f64, length: f64) -> Option<Self> {
        let path = std::fs::canonicalize(path).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        let tool = crate::media_tools::find_tool("ffmpeg");
        Some(Self { path, size: meta.len(), modified: meta.modified().ok()?,
            settings: serde_json::to_string(settings).ok()?, start, length,
            tool_modified: std::fs::metadata(&tool).ok().and_then(|m| m.modified().ok()), tool })
    }
}

#[derive(Default)]
struct SampleCache { entries: VecDeque<(SampleKey, Sample)>, bytes: usize }
impl SampleCache {
    fn get(&mut self, key: &SampleKey) -> Option<Sample> {
        let index = self.entries.iter().position(|(candidate, _)| candidate == key)?;
        let entry = self.entries.remove(index)?;
        let sample = entry.1.clone();
        self.entries.push_back(entry);
        Some(sample)
    }
    fn insert(&mut self, key: SampleKey, sample: Sample) {
        if sample.bytes.len() > MAX_SAMPLE_BYTES as usize { return; }
        // Replacing a source or recipe must not keep old copies of that same request.
        self.entries.retain(|(candidate, _)| candidate != &key);
        self.bytes = self.entries.iter().map(|(_, sample)| sample.bytes.len()).sum();
        while self.bytes + sample.bytes.len() > MAX_CACHE_BYTES || self.entries.len() >= MAX_CACHE_ENTRIES {
            let Some((_, old)) = self.entries.pop_front() else { break; };
            self.bytes -= old.bytes.len();
        }
        self.bytes += sample.bytes.len();
        self.entries.push_back((key, sample));
    }
}
fn cache() -> &'static Mutex<SampleCache> {
    static CACHE: OnceLock<Mutex<SampleCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SampleCache::default()))
}

/// Own the temporary directory even if the render future is cancelled.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::fs::remove_dir_all(&self.0).is_err() {
            // Windows can retain the killed child's file handles briefly.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let path = self.0.clone();
                runtime.spawn(async move {
                    for _ in 0..20 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if std::fs::remove_dir_all(&path).is_ok() || !path.exists() { break; }
                    }
                });
            }
        }
    }
}

fn media_file(path: &Path) -> Result<()> {
    if !path.is_file() || !crate::remux::is_media_path(path) {
        return Err(Error::InvalidInput(format!(
            "{} is not a media file",
            path.display()
        )));
    }
    Ok(())
}

/// Run FFmpeg for a bounded time and return what it wrote to standard output.
async fn capture(args: Vec<String>, timeout: Duration) -> Result<Vec<u8>> {
    let output = tokio::time::timeout(
        timeout,
        crate::media_tools::ffmpeg_command()
            .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
            .args(["-protocol_whitelist", "file,pipe"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::InvalidInput("FFmpeg took too long to read this file".into()))?
    .map_err(crate::remux::launch_error)?;
    if !output.status.success() || output.stdout.is_empty() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(Error::InvalidInput(format!(
            "FFmpeg could not read this file: {}",
            reason.lines().next().unwrap_or("no output")
        )));
    }
    Ok(output.stdout)
}

/// One frame near `at_seconds` as JPEG bytes, no wider than `max_width`.
pub async fn thumbnail(path: &Path, at_seconds: f64, max_width: u32) -> Result<Vec<u8>> {
    media_file(path)?;
    if !at_seconds.is_finite() || at_seconds < 0.0 {
        return Err(Error::InvalidInput("Choose a time inside the file".into()));
    }
    let width = max_width.clamp(16, MAX_STILL_WIDTH);
    let mut args: Vec<String> = Vec::new();
    if at_seconds > 0.0 {
        args.extend(["-ss".into(), at_seconds.to_string()]);
    }
    args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
    args.extend([
        "-an".into(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale='min({width},iw)':-2"),
        "-c:v".into(),
        "mjpeg".into(),
        "-q:v".into(),
        "5".into(),
        "-f".into(),
        "image2pipe".into(),
        "pipe:1".into(),
    ]);
    capture(args, STILL_TIMEOUT).await
}

/// Peak loudness (0 to 1) of `buckets` equal slices of the first half hour of audio.
pub async fn waveform(path: &Path, buckets: usize) -> Result<Vec<f32>> {
    media_file(path)?;
    let buckets = buckets.clamp(8, MAX_WAVEFORM_BUCKETS);
    let bytes = capture(
        vec![
            "-t".into(),
            WAVEFORM_SECONDS.to_string(),
            "-i".into(),
            path.to_string_lossy().into_owned(),
            "-vn".into(),
            "-ac".into(),
            "1".into(),
            "-ar".into(),
            WAVEFORM_RATE.to_string(),
            "-f".into(),
            "s16le".into(),
            "pipe:1".into(),
        ],
        SAMPLE_TIMEOUT,
    )
    .await?;
    Ok(peaks(&bytes, buckets))
}

fn peaks(pcm: &[u8], buckets: usize) -> Vec<f32> {
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if samples.is_empty() {
        return vec![0.0; buckets];
    }
    (0..buckets)
        .map(|bucket| {
            let start = bucket * samples.len() / buckets;
            let end = ((bucket + 1) * samples.len() / buckets).max(start + 1).min(samples.len());
            let peak = samples[start..end]
                .iter()
                .map(|sample| i32::from(*sample).unsigned_abs())
                .max()
                .unwrap_or(0);
            (peak as f32 / f32::from(i16::MAX)).min(1.0)
        })
        .collect()
}

/// A short rendered sample, ready to show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    /// The format actually rendered; some outputs are shown in a viewable stand-in.
    pub format: Format,
    pub start_seconds: f64,
    pub sample_seconds: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Formats a browser cannot show are rendered as a close relative instead.
fn viewable(format: Format) -> Format {
    match format {
        Format::Mkv | Format::Mov => Format::Mp4,
        Format::Bmp | Format::Tiff => Format::Png,
        Format::Aiff => Format::Wav,
        other => other,
    }
}

fn mime(format: Format) -> &'static str {
    match format {
        Format::Mp4 => "video/mp4",
        Format::Webm => "video/webm",
        Format::Gif => "image/gif",
        Format::Webp => "image/webp",
        Format::Png => "image/png",
        Format::Jpg => "image/jpeg",
        Format::Mp3 => "audio/mpeg",
        Format::M4a | Format::Alac => "audio/mp4",
        Format::Flac => "audio/flac",
        Format::Wav => "audio/wav",
        Format::Opus | Format::Ogg => "audio/ogg",
        Format::Mkv | Format::Mov | Format::Bmp | Format::Tiff | Format::Aiff => {
            "application/octet-stream"
        }
    }
}

fn preview_bounds(info: &crate::remux::MediaInfo, settings: &Settings) -> (u32, u32) {
    let crop_w = f64::from(info.width.unwrap_or(16)) * settings.crop.map_or(1.0, |c| c.width);
    let crop_h = f64::from(info.height.unwrap_or(9)) * settings.crop.map_or(1.0, |c| c.height);
    let (oriented_w, oriented_h) = if matches!(settings.rotate, 90 | 270) {
        (crop_h, crop_w)
    } else {
        (crop_w, crop_h)
    };
    let (max_w, max_h) = if oriented_h > oriented_w { (540, 960) } else { (960, 540) };
    (
        settings.width.unwrap_or(max_w).min(max_w),
        settings.height.unwrap_or(max_h).min(max_h),
    )
}

fn scratch_directory() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("zoltraak-preview-{}", crate::model::DownloadId::new()));
    std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

async fn run_sample(mut args: Vec<String>, output: Option<&Path>) -> Result<String> {
    // The output path is always the last argument; cap its size just before it.
    if output.is_some() {
        let last = args.len().saturating_sub(1);
        args.splice(last..last, ["-fs".to_string(), MAX_SAMPLE_BYTES.to_string()]);
    }
    let result = tokio::time::timeout(
        SAMPLE_TIMEOUT,
        crate::media_tools::ffmpeg_command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::InvalidInput("The preview took too long to render".into()))?
    .map_err(crate::remux::launch_error)?;
    if !result.status.success() || output.is_some_and(|output| !output.is_file()) {
        let reason = String::from_utf8_lossy(&result.stderr);
        return Err(Error::InvalidInput(format!(
            "The preview could not be rendered: {}",
            reason.lines().next().unwrap_or("no output")
        )));
    }
    Ok(String::from_utf8_lossy(&result.stderr).into_owned())
}

/// Render `length_seconds` of `path` from `start_seconds` with the recipe `settings` describe.
pub async fn sample(
    path: &Path,
    settings: &Settings,
    start_seconds: f64,
    length_seconds: f64,
) -> Result<Sample> {
    media_file(path)?;
    if !start_seconds.is_finite() || start_seconds < 0.0 {
        return Err(Error::InvalidInput("Choose a start inside the file".into()));
    }
    let length = if length_seconds.is_finite() && length_seconds > 0.0 {
        length_seconds.min(MAX_SAMPLE_SECONDS)
    } else {
        return Err(Error::InvalidInput("Choose a sample length above zero".into()));
    };
    settings.validate()?;
    let key = SampleKey::read(path, settings, start_seconds, length);
    if let Some(sample) = key.as_ref().and_then(|key| cache().lock().ok()?.get(key)) {
        return Ok(sample);
    }
    let info = crate::remux::probe(path).await?;
    let original_plan = settings.plan(&info)?;
    let still = crate::conversion::still(&info);
    let mut recipe = settings.clone();
    recipe.format = viewable(settings.format);
    recipe.target_size_mb = None;
    recipe.mode = "encode".into();
    let (max_w, max_h) = preview_bounds(&info, settings);
    recipe.width = Some(max_w);
    recipe.height = Some(max_h);
    if !still {
        recipe.start_seconds = start_seconds;
        let recipe_end = original_plan.duration_seconds.map(|duration| settings.start_seconds + duration);
        let end = (start_seconds + length).min(recipe_end.unwrap_or(f64::MAX));
        recipe.end_seconds = Some(info.duration_seconds.map_or(end, |total| end.min(total)));
        recipe.duration_seconds = None;
    } else if !recipe.format.image() && !(recipe.format == Format::Webp && settings.duration_seconds.is_none() && settings.end_seconds.is_none()) {
        recipe.start_seconds = 0.0;
        recipe.end_seconds = None;
        recipe.duration_seconds = Some(original_plan.duration_seconds.unwrap_or(length).min(length));
    }
    let plan = recipe.plan(&info)?;
    let directory = Scratch(scratch_directory()?);
    let result = tokio::time::timeout(SAMPLE_TIMEOUT, render(&info, &recipe, &plan, &directory.0)).await
        .map_err(|_| Error::InvalidInput("The preview took too long to render".into()))?;
    let (bytes, width, height) = result?;
    let sample = Sample {
        bytes,
        mime: mime(recipe.format),
        format: recipe.format,
        start_seconds: if still { 0.0 } else { start_seconds },
        sample_seconds: plan.duration_seconds.unwrap_or(length),
        width,
        height,
    };
    // A source edited during rendering must never seed the cache.
    if let Some(key) = key {
        if SampleKey::read(path, settings, start_seconds, length).as_ref() == Some(&key) {
            if let Ok(mut cache) = cache().lock() { cache.insert(key, sample.clone()); }
        }
    }
    Ok(sample)
}

async fn render(
    info: &crate::remux::MediaInfo,
    recipe: &Settings,
    plan: &crate::conversion::ConversionPlan,
    directory: &Path,
) -> Result<(Vec<u8>, Option<u32>, Option<u32>)> {
    let output = directory.join(format!("preview.{}", recipe.format.extension()));
    if recipe.format == Format::Gif {
        run_sample(recipe.palette_args(info, &output, plan), Some(&output.with_extension("palette.png"))).await?;
    }
    let mut args = recipe.args(info, &output, plan);
    if recipe.normalize && recipe.normalize_two_pass && plan.streams.audio == crate::remux::StreamAction::Encode {
        let stderr = run_sample(recipe.loudness_analysis_args(info, plan), None).await?;
        recipe.apply_loudness_measurement(&mut args, &stderr);
    }
    run_sample(args, Some(&output)).await?;
    let metadata = std::fs::metadata(&output).map_err(|source| Error::Io { path: output.clone(), source })?;
    if metadata.len() > MAX_SAMPLE_BYTES {
        return Err(Error::InvalidInput("The preview exceeded its 12 MB limit; reduce its dimensions or duration".into()));
    }
    let bytes = std::fs::read(&output).map_err(|source| Error::Io {
        path: output.clone(),
        source,
    })?;
    let (width, height) = match crate::remux::probe(&output).await {
        Ok(rendered) => (rendered.width, rendered.height),
        Err(_) => (None, None),
    };
    Ok((bytes, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_tracks_source_recipe_range_and_evicts_old_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, b"source").unwrap();
        let settings = Settings::default();
        let key = SampleKey::read(&path, &settings, 0.0, 2.0).unwrap();
        let sample = Sample { bytes: vec![1, 2, 3], mime: "video/mp4", format: Format::Mp4,
            start_seconds: 0.0, sample_seconds: 2.0, width: Some(64), height: Some(64) };
        let mut cache = SampleCache::default();
        cache.insert(key.clone(), sample.clone());
        assert_eq!(cache.get(&key).unwrap().bytes, sample.bytes);
        assert!(cache.get(&SampleKey::read(&path, &settings, 1.0, 2.0).unwrap()).is_none());
        let other = Settings { quality: 50, ..Default::default() };
        assert!(cache.get(&SampleKey::read(&path, &other, 0.0, 2.0).unwrap()).is_none());
        std::fs::write(&path, b"new source, different length").unwrap();
        assert!(cache.get(&SampleKey::read(&path, &settings, 0.0, 2.0).unwrap()).is_none());
        for start in 1..=MAX_CACHE_ENTRIES {
            let mut next = key.clone(); next.start = start as f64;
            cache.insert(next, sample.clone());
        }
        assert!(cache.get(&key).is_none(), "least recently used sample evicted");
        assert_eq!(cache.entries.len(), MAX_CACHE_ENTRIES);
        assert_eq!(cache.bytes, MAX_CACHE_ENTRIES * sample.bytes.len());
        let large = Sample { bytes: vec![0; MAX_SAMPLE_BYTES as usize], ..sample };
        for start in 10..20 { let mut next = key.clone(); next.start = start as f64; cache.insert(next, large.clone()); }
        assert!(cache.bytes <= MAX_CACHE_BYTES);
        assert_eq!(cache.entries.len(), 4, "byte cap applies before entry cap");
    }

    #[test]
    fn peaks_take_the_loudest_sample_in_each_slice() {
        let mut pcm = Vec::new();
        for sample in [0_i16, 100, -16000, 50, 0, 0, 8000, -200] {
            pcm.extend(sample.to_le_bytes());
        }
        let result = peaks(&pcm, 4);
        assert_eq!(result.len(), 4);
        assert!((result[1] - 16000.0 / 32767.0).abs() < 1e-4);
        assert!(result[0] < 0.01 && result[2] < 0.01);
        assert!((result[3] - 8000.0 / 32767.0).abs() < 1e-4);
        assert_eq!(peaks(&[], 3), vec![0.0; 3]);
    }

    #[test]
    fn unviewable_formats_use_a_close_relative() {
        assert_eq!(viewable(Format::Mkv), Format::Mp4);
        assert_eq!(viewable(Format::Tiff), Format::Png);
        assert_eq!(viewable(Format::Mp3), Format::Mp3);
        assert_eq!(mime(viewable(Format::Aiff)), "audio/wav");
    }

    #[test]
    fn preview_bounds_follow_post_rotation_orientation() {
        let info = crate::remux::MediaInfo {
            path: PathBuf::from("clip.mp4"),
            extension: "mp4".into(),
            video_codec: Some("h264".into()),
            audio_codec: None,
            width: Some(1920),
            height: Some(1080),
            duration_seconds: Some(5.0),
            bytes: Some(1000),
            unprobed: false,
            unsupported_animation: false,
            audio_tracks: vec![],
        };
        assert_eq!(preview_bounds(&info, &Settings::default()), (960, 540));
        let rotated = Settings { rotate: 90, ..Default::default() };
        assert_eq!(preview_bounds(&info, &rotated), (540, 960));
    }
}
