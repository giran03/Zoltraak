//! Direct audio links, and any page the video extractor can find a stream on.
//!
//! A link ending in `.mp3` or `.flac` is the simplest case there is: one
//! file, named after its path. Everything else — a page with an HLS/DASH
//! manifest, a media element, an Open Graph audio tag — goes through the
//! generic video analyzer, and the best *audio* rendition it finds is taken.
//! A DASH source's separate audio track is exactly what a music download
//! wants, so it is preferred over a muxed file the converter would have to
//! strip the picture from.

use reqwest::Client;

use super::{AudioSource, MusicSource, MusicTrack, Service, SourceKind};
use crate::error::{Error, Result};
use crate::headers::Headers;
use crate::video::VideoFormat;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "aac", "flac", "wav", "ogg", "oga", "opus", "wma", "aiff", "aif", "ape", "mka", "alac", "m4b", "weba",
];

/// Whether a URL's path ends in an audio file extension.
pub fn is_audio_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    let path = parsed.path().to_ascii_lowercase();
    AUDIO_EXTENSIONS.iter().any(|ext| path.ends_with(&format!(".{ext}")))
}

/// The container a URL's extension implies.
fn container_of(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    AUDIO_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// A stream for a track's audio, picked out of a video analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    pub url: String,
    pub headers: Headers,
    /// Container the bytes arrive in, when it can be told from the format.
    pub container: Option<String>,
    /// Whether the stream also carries a picture the converter must drop.
    pub has_video: bool,
    pub filesize: Option<u64>,
}

/// The best audio the formats offer: an audio-only rendition first, then a
/// merge pair's separate audio track, then a muxed file as a last resort.
pub fn pick_audio_stream(formats: &[VideoFormat]) -> Option<AudioStream> {
    audio_stream_candidates(formats).into_iter().next()
}

/// Ordered alternatives, retaining the extractor's headers and actual
/// container. A broken high-bitrate rendition must not hide a working one.
pub(crate) fn audio_stream_candidates(formats: &[VideoFormat]) -> Vec<AudioStream> {
    let mut candidates = Vec::new();
    let mut audio: Vec<_> = formats.iter().filter(|f| f.video_codec.is_none() && f.height.is_none()
        && f.video_url.is_some() && f.has_audio != Some(false)).collect();
    audio.sort_by_key(|f| std::cmp::Reverse((f.bitrate.unwrap_or(0), f.filesize.unwrap_or(0))));
    for f in audio {
        candidates.push(AudioStream { url: f.video_url.clone().unwrap(), headers: f.headers.clone(),
            container: Some(audio_container(f)), has_video: false, filesize: f.filesize });
    }
    let mut split: Vec<_> = formats.iter().filter(|f| f.requires_merge && f.audio_url.is_some()).collect();
    split.sort_by_key(|f| std::cmp::Reverse((f.bitrate.unwrap_or(0), f.height.unwrap_or(0))));
    for f in split {
        let codec = f.audio_codec.as_deref().unwrap_or("").to_ascii_lowercase();
        let container = if codec.starts_with("opus") || codec.starts_with("vorbis") { "webm" }
            else if codec.starts_with("mp3") || codec.starts_with("mp4a.6b") { "mp3" } else { "m4a" };
        candidates.push(AudioStream { url: f.audio_url.clone().unwrap(), headers: f.headers.clone(),
            container: Some(container.into()), has_video: false, filesize: None });
    }
    let mut muxed: Vec<_> = formats.iter().filter(|f| f.video_url.is_some() && f.plays_with_audio()).collect();
    muxed.sort_by_key(|f| (f.height.unwrap_or(u32::MAX), f.filesize.unwrap_or(u64::MAX)));
    for f in muxed {
        candidates.push(AudioStream { url: f.video_url.clone().unwrap(), headers: f.headers.clone(),
            container: Some(f.container.clone()), has_video: true, filesize: f.filesize });
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|stream| !stream.url.trim().is_empty() && seen.insert(stream.url.clone()));
    candidates
}

pub(crate) async fn streams_from_page(client: &Client, page: &str, headers: &Headers) -> Result<Vec<AudioStream>> {
    let meta = crate::video::analyze(client, page, headers).await?;
    let mut candidates = audio_stream_candidates(&meta.formats);
    for stream in &mut candidates {
        for (name, value) in headers {
            if !stream.headers.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(name)) {
                stream.headers.push((name.clone(), value.clone()));
            }
        }
    }
    Ok(candidates)
}

fn audio_container(f: &VideoFormat) -> String {
    match f.container.as_str() {
        "webm" => "webm".into(),
        "mp4" => "m4a".into(),
        other => other.into(),
    }
}

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<MusicSource> {
    if is_audio_url(url) {
        let filename = crate::probe::filename_from_url(url);
        let stem = std::path::Path::new(&filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(filename.clone());
        let title = stem.replace(['_', '+'], " ").trim().to_string();
        let title = if title.is_empty() { "audio".to_string() } else { title };
        // `Artist - Title.mp3` is the convention most files follow.
        let (artists, title) = match title.split_once(" - ") {
            Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => (vec![a.trim().to_string()], t.trim().to_string()),
            _ => (Vec::new(), title),
        };
        let track = MusicTrack {
            id: format!("direct:{url}"),
            title: title.clone(),
            artists: artists.clone(),
            album: None,
            album_artist: None,
            track_number: None,
            track_total: None,
            disc_number: None,
            year: None,
            date: None,
            genre: None,
            duration: None,
            cover_url: None,
            isrc: None,
            source_url: url.to_string(),
            service: Service::Direct,
            source: AudioSource::Direct { url: url.to_string(), headers: headers.clone(), container: container_of(url) },
            explicit: false,
            unavailable: None,
        };
        return Ok(MusicSource {
            kind: SourceKind::Track,
            service: Service::Direct,
            title,
            owner: artists.first().cloned(),
            cover_url: None,
            source_url: url.to_string(),
            tracks: vec![track],
            total: None,
            note: None,
        });
    }

    let meta = crate::video::analyze(client, url, headers).await?;
    let stream = pick_audio_stream(&meta.formats)
        .ok_or_else(|| Error::InvalidInput(format!("no audio stream could be found at {url}")))?;
    let mut all_headers = stream.headers.clone();
    for (name, value) in headers {
        if !all_headers.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(name)) {
            all_headers.push((name.clone(), value.clone()));
        }
    }
    let track = MusicTrack {
        id: format!("direct:{}", meta.webpage_url),
        title: meta.title.clone(),
        artists: meta.uploader.clone().into_iter().collect(),
        album: None,
        album_artist: None,
        track_number: None,
        track_total: None,
        disc_number: None,
        year: None,
        date: None,
        genre: None,
        duration: meta.duration,
        cover_url: meta.thumbnail.clone(),
        isrc: None,
        source_url: meta.webpage_url.clone(),
        service: Service::Direct,
        source: AudioSource::Direct { url: stream.url, headers: all_headers, container: stream.container },
        explicit: false,
        unavailable: None,
    };
    Ok(MusicSource {
        kind: SourceKind::Track,
        service: Service::Direct,
        title: meta.title,
        owner: meta.uploader,
        cover_url: meta.thumbnail,
        source_url: meta.webpage_url,
        tracks: vec![track],
        total: None,
        note: stream.has_video.then(|| "this source has no separate audio track; the picture will be dropped when the file is converted".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(id: &str, container: &str, video_codec: Option<&str>, height: Option<u32>, bitrate: u64) -> VideoFormat {
        VideoFormat {
            id: id.into(),
            container: container.into(),
            video_codec: video_codec.map(str::to_string),
            audio_codec: Some("mp4a.40.2".into()),
            height,
            bitrate: Some(bitrate),
            video_url: Some(format!("https://cdn/{id}")),
            ..Default::default()
        }
    }

    #[test]
    fn fallback_candidates_preserve_quality_order_headers_and_containers() {
        let formats = vec![
            VideoFormat { headers: vec![("Referer".into(),"https://source/page".into())], ..format("high", "webm", None, None, 192_000) },
            format("low", "m4a", None, None, 128_000),
            format("low", "m4a", None, None, 128_000),
            format("muxed", "mp4", Some("avc1"), Some(360), 500_000),
        ];
        let streams = audio_stream_candidates(&formats);
        assert_eq!(streams.iter().map(|s|s.url.as_str()).collect::<Vec<_>>(),vec!["https://cdn/high","https://cdn/low","https://cdn/muxed"]);
        assert_eq!(streams[0].headers,formats[0].headers);
        assert_eq!(streams[1].container.as_deref(),Some("m4a"));
        assert!(streams[2].has_video);
        assert!(!is_audio_url("https://example.mp3"),"a hostname is not an audio file path");
    }

    #[test]
    fn audio_urls_are_recognised_by_extension() {
        assert!(is_audio_url("https://x/song.MP3?token=1"));
        assert!(is_audio_url("https://x/a/b.flac"));
        assert!(!is_audio_url("https://x/clip.mp4"));
        assert!(!is_audio_url("https://x/page"));
        assert_eq!(container_of("https://x/song.OPUS").as_deref(), Some("opus"));
    }

    #[test]
    fn an_audio_only_rendition_is_preferred_over_a_muxed_file() {
        let formats = vec![
            format("720", "mp4", Some("avc1"), Some(720), 2000),
            VideoFormat { has_audio: Some(true), ..format("140", "m4a", None, None, 128_000) },
            VideoFormat { has_audio: Some(true), ..format("251", "webm", None, None, 160_000) },
        ];
        let s = pick_audio_stream(&formats).unwrap();
        assert_eq!(s.url, "https://cdn/251", "the higher bitrate audio-only track");
        assert_eq!(s.container.as_deref(), Some("webm"));
        assert!(!s.has_video);
    }

    #[test]
    fn a_merge_pairs_audio_track_beats_a_muxed_file() {
        let formats = vec![
            format("360", "mp4", Some("avc1"), Some(360), 500),
            VideoFormat { requires_merge: true, audio_url: Some("https://cdn/audio".into()), audio_codec: Some("opus".into()), ..format("1080", "webm", Some("vp9"), Some(1080), 4000) },
        ];
        let s = pick_audio_stream(&formats).unwrap();
        assert_eq!(s.url, "https://cdn/audio");
        assert_eq!(s.container.as_deref(), Some("webm"));
    }

    #[test]
    fn a_muxed_file_is_the_last_resort_and_the_smallest_wins() {
        let formats = vec![format("1080", "mp4", Some("avc1"), Some(1080), 4000), format("360", "mp4", Some("avc1"), Some(360), 500)];
        let s = pick_audio_stream(&formats).unwrap();
        assert_eq!(s.url, "https://cdn/360");
        assert!(s.has_video);
        assert!(pick_audio_stream(&[]).is_none());
    }
}
