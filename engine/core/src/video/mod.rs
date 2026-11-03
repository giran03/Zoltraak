//! Video capture: turning a page URL or a directly-linked media URL into a
//! set of downloadable [`VideoFormat`]s, then fetching and (when needed)
//! muxing them into a single file.
//!
//! Direct media links, HLS, and DASH manifests reachable from the page are
//! handled generically; a handful of platforms get a dedicated extractor in
//! [`site`], and YouTube — whose player API and URL obfuscation are a problem
//! of their own — gets [`youtube`]. All of them produce the same
//! [`VideoFormat`]/[`VideoMetadata`] shape, so the download and merge steps
//! never know which one they are serving.

pub mod dash;
pub mod extractor;
pub mod fallback;
pub mod subtitles;
pub mod live;
pub mod direct;
pub mod hls;
pub mod groups;
pub mod manager;
pub mod merge;
pub mod mp4probe;
pub mod validation;
pub mod site;
pub(crate) mod tiktok;
pub mod title;
pub mod youtube;
pub(crate) mod net;
// Public because its options are part of how a caller configures a video
// download: segment concurrency, per-segment retry, and the shared bandwidth
// cap all live here (see [`stream::StreamOptions`]).
pub mod stream;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::headers::Headers;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFormat {
    pub id: String,
    pub container: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f32>,
    pub bitrate: Option<u64>,
    pub filesize: Option<u64>,
    /// A fetchable source for the video track — a direct file, an HLS media
    /// playlist, or an encoded DASH segment list.
    pub video_url: Option<String>,
    /// Set only when the audio track is a separate stream from `video_url`.
    pub audio_url: Option<String>,
    pub requires_merge: bool,
    /// Whether `video_url`'s own stream carries audio, when the extractor
    /// could establish it. `None` means "not known" — the ordinary state for a
    /// progressive file whose bytes nobody has looked at.
    ///
    /// This exists because the alternative is silence. Facebook, Instagram and
    /// X all serve DASH, where the video and the audio of one clip are two
    /// separate `.mp4` files that look identical from the outside; handing
    /// either one to the downloader on its own produces a video with no sound
    /// or a sound file with no picture. A format that is honest about carrying
    /// no audio can be paired with one that does, ranked below one that is
    /// complete, or labelled as what it is.
    #[serde(default)]
    pub has_audio: Option<bool>,
    /// Request headers the stream must be fetched with — the client identity a
    /// CDN checks against the URL it signed, most often. YouTube's CDN refuses
    /// a URL minted for its iOS app when a desktop browser asks for it.
    /// Carried on the format because analysis is stateless: the client hands
    /// the chosen format back verbatim, headers included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Headers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<validation::VideoValidation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_options: Option<DownloadOptions>,
}

impl Default for VideoFormat {
    fn default() -> Self {
        Self {
            id: String::new(),
            container: "mp4".into(),
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            filesize: None,
            video_url: None,
            audio_url: None,
            requires_merge: false,
            has_audio: None,
            headers: Vec::new(),
            validation: None,
            download_options: None,
        }
    }
}

impl VideoFormat {
    pub fn is_audio_only(&self) -> bool {
        ["m4a", "mp3", "aac", "ogg", "oga", "opus", "wav", "flac"].contains(&self.container.as_str())
            || self.id == "audio" || self.id.starts_with("dash-audio-")
            || self.validation.as_ref().is_some_and(|v| v.has_video == Some(false) && v.has_audio == Some(true))
    }

    /// Whether picking this format yields a file with sound.
    ///
    /// A separate audio stream counts, since the download muxes it back in. An
    /// unknown `has_audio` counts as playable: most progressive files do carry
    /// audio, and refusing to offer every unprobed format would leave the
    /// picker empty far more often than it would prevent a silent download.
    pub fn plays_with_audio(&self) -> bool {
        self.requires_merge || self.has_audio != Some(false)
    }

    /// A short human label for a format picker: `"1080p"`, `"320kbps"`, or
    /// the raw id when neither dimension nor bitrate is known.
    pub fn label(&self) -> String {
        let quality = match (self.height, self.bitrate) {
            (Some(h), _) => format!("{h}p"),
            (None, Some(b)) => format!("{}kbps", b / 1000),
            (None, None) => self.id.clone(),
        };
        if let Some((clip, _)) = self.id.strip_prefix("clip-").and_then(|value| value.split_once('-')) {
            if !clip.is_empty() && clip.chars().all(|character| character.is_ascii_digit()) {
                return format!("Clip {clip} · {quality}");
            }
        }
        quality
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<Subtitle>,
    pub title: String,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub webpage_url: String,
    pub formats: Vec<VideoFormat>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subtitle {
    pub language: String,
    pub label: String,
    pub url: String,
    pub extension: String,
    #[serde(default)]
    pub automatic: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOptions {
    /// Source-provided poster, persisted alongside the chosen format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub subtitle: Option<Subtitle>,
    pub live_seconds: Option<u32>,
}

/// Rank formats best-first, so that the head of the list is what a one-click
/// "just download it" should take.
///
/// Sound comes before resolution. A 1080p video-only rendition is not a better
/// download than a 720p one with audio — it is a broken one, and picking it is
/// exactly how "Download this video" ended up producing silent files. Only
/// among formats that will play with sound does height, then bitrate, then
/// size decide.
///
/// `requires_merge` is deliberately *not* a ranking term any more. It used to
/// sit at the top, which meant a merge pair always outranked a complete muxed
/// file of the same height even though the two produce the same result and the
/// muxed one needs no ffmpeg.
pub fn sort_formats(formats: &mut [VideoFormat]) {
    let audio_rank = |format: &VideoFormat| {
        if format.requires_merge || format.has_audio == Some(true) { 2 }
        else if format.has_audio.is_none() { 1 } else { 0 }
    };
    formats.sort_by(|a, b| {
        audio_rank(b)
            .cmp(&audio_rank(a))
            .then_with(|| b.height.unwrap_or(0).cmp(&a.height.unwrap_or(0)))
            .then_with(|| b.bitrate.unwrap_or(0).cmp(&a.bitrate.unwrap_or(0)))
            .then_with(|| b.filesize.unwrap_or(0).cmp(&a.filesize.unwrap_or(0)))
    });
}

/// Analyze a URL — a page or a direct media/manifest link — and return the
/// formats found. Known platforms are routed through site-specific extractors
/// first; everything else falls through to the generic page scraper.
pub async fn analyze(client: &reqwest::Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    analyze_with_fallback(client, url, headers, false).await
}

pub async fn analyze_with_fallback(client: &reqwest::Client, url: &str, headers: &Headers, use_fallback: bool) -> Result<VideoMetadata> {
    analyze_source(client, url, headers, use_fallback, None, None).await
}

pub async fn analyze_source(client: &reqwest::Client, url: &str, headers: &Headers, use_fallback: bool, source_kind: Option<&str>, media_type: Option<&str>) -> Result<VideoMetadata> {
    let canonical = if matches!(site::matches_platform(url), Some(site::Platform::Instagram)) {
        Some(crate::instagram::canonical_url(url)?)
    } else { None };
    let source = canonical.as_deref().unwrap_or(url);
    let mut meta = if use_fallback && canonical.is_some() {
        // The optional tool has no shared session. For Instagram, exhaust
        // the authenticated shared resolver before falling back to that tool.
        match site::analyze(client, source, headers, site::Platform::Instagram).await {
            Ok(meta) => meta,
            Err(primary) if primary.to_string().contains("rate-limited") => return Err(primary),
            Err(primary) => fallback::analyze(source).await.map_err(|optional|
                crate::error::Error::InvalidInput(format!("{primary} Optional fallback: {optional}")))?,
        }
    } else if use_fallback { fallback::analyze(source).await? }
        else if source_kind == Some("file") { direct::analyze_file(client, url, headers).await? }
        else {
            let adapter = match source_kind {
                Some("hls") => extractor::Extractor::Hls,
                Some("dash") => extractor::Extractor::Dash,
                Some("page") | None => extractor::Extractor::for_url(url),
                Some(_) => return Err(crate::error::Error::InvalidInput("Unsupported media source kind".into())),
            };
            adapter.extract(client, url, headers).await?
        };

    // An audio element can expose a bare HLS media playlist without codec
    // metadata. Keep the hint inferred; actual MP4 track inspection wins below.
    if media_type == Some("audio") && source_kind == Some("hls") {
        for format in &mut meta.formats {
            if format.video_codec.is_none() && format.width.is_none() && !format.requires_merge {
                format.id = "audio".into();
                format.container = "m4a".into();
                format.has_audio = Some(true);
            }
        }
    }

    sort_formats(&mut meta.formats);
    let content_id = validation::content_identity(&meta.webpage_url);
    for format in &mut meta.formats {
        if let Some(thumbnail) = &meta.thumbnail {
            format.download_options.get_or_insert_with(Default::default).thumbnail.get_or_insert(thumbnail.clone());
        }
        let report = format.validation.get_or_insert_with(Default::default);
        if report.duration.is_none() { report.duration = meta.duration; }
        report.content_id = content_id.clone();
    }
    // Analysis remains bounded even when a page exposes hundreds of formats.
    // The selected format is inspected again at start if it was not verified.
    for format in meta.formats.iter_mut().take(3) {
        validation::inspect_format(client, format, headers).await;
    }
    sort_formats(&mut meta.formats);
    meta.title = title::ensure_valid_title(&meta.title, url);
    Ok(meta)
}

/// CDN classification shared with image extraction. Video analysis also
/// inspects ordinary progressive sources through `validation::inspect_format`.
pub(crate) fn serves_split_tracks(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else { return false };
    let host = url.host_str().unwrap_or_default();
    ["fbcdn.net", "cdninstagram.com", "cdninstagram.net"].iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(id: &str, height: Option<u32>) -> VideoFormat {
        VideoFormat {
            id: id.into(),
            height,
            ..Default::default()
        }
    }

    #[test]
    fn a_silent_rendition_never_outranks_one_that_plays() {
        // The bug this ordering exists to prevent: a 1080p video-only track
        // sitting at the top of the list, so the one-click download picked it
        // and produced a file with no sound.
        let mut formats = vec![
            VideoFormat { has_audio: Some(false), ..format("1080p-silent", Some(1080)) },
            VideoFormat { has_audio: Some(true), ..format("720p-muxed", Some(720)) },
        ];

        sort_formats(&mut formats);

        assert_eq!(formats[0].id, "720p-muxed");
        assert_eq!(formats[1].id, "1080p-silent");
    }

    #[test]
    fn a_merge_pair_counts_as_playable_and_ranks_by_height() {
        // A video track with its audio alongside is complete once muxed, so it
        // competes on resolution — and no longer wins automatically over an
        // equally complete progressive file, which is what `requires_merge`
        // being the top sort key used to do.
        let mut formats = vec![
            VideoFormat { has_audio: Some(true), ..format("720p-muxed", Some(720)) },
            VideoFormat {
                requires_merge: true,
                has_audio: Some(false),
                audio_url: Some("https://cdn.example/audio.mp4".into()),
                ..format("1080p-merge", Some(1080))
            },
        ];

        sort_formats(&mut formats);

        assert_eq!(formats[0].id, "1080p-merge");
        assert!(formats[0].plays_with_audio());
    }

    #[test]
    fn an_unprobed_format_is_assumed_playable() {
        // Most progressive files carry audio and nothing has looked at them.
        // Treating "unknown" as silent would bury every format we never probed.
        let unknown = format("mp4", Some(480));
        assert_eq!(unknown.has_audio, None);
        assert!(unknown.plays_with_audio());
    }

    #[test]
    fn known_audio_ranks_above_an_unprobed_higher_resolution() {
        let mut formats = vec![format("unknown", Some(1080)),
            VideoFormat { has_audio: Some(true), ..format("known", Some(720)) }];
        sort_formats(&mut formats);
        assert_eq!(formats[0].id, "known");
    }

    #[test]
    fn shared_cdn_classification_rejects_lookalike_domains() {
        assert!(serves_split_tracks("https://video.fbcdn.net/clip.mp4"));
        assert!(serves_split_tracks("https://cdninstagram.com/clip.mp4"));
        assert!(!serves_split_tracks("https://evilfbcdn.net/clip.mp4"));
        assert!(!serves_split_tracks("https://fbcdn.net.evil.test/clip.mp4"));
        assert!(!serves_split_tracks("https://example.com/clip.mp4"));
    }


}
