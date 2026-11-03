//! Direct media links, and the fallback for an ordinary page: scan its HTML
//! for `<video>`/`<source>` tags, Open Graph video/image metadata, and
//! JSON-LD `contentUrl` — no JavaScript execution, so a player that only
//! sets `video.src` at runtime is out of reach here. That gap is exactly
//! what the browser extension's content script (which runs *after* the page
//! script does) fills in.

use std::sync::LazyLock;

use regex::Regex;
use reqwest::Client;

use super::net::fetch_response;
use super::{VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use crate::headers::Headers;

const MEDIA_EXTENSIONS: &[&str] = &["mp4", "webm", "mov", "mkv", "m4v", "avi", "flv", "3gp", "ogv", "ts", "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"];

fn is_audio_container(container: &str) -> bool {
    ["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"].contains(&container)
}

/// Extensions of files that are never a video stream. Posters, thumbnails, and
/// preview stills routinely appear in the same JSON blobs and meta tags the
/// video URLs do — a page's JSON-LD `ImageObject.contentUrl`, an `og:image`,
/// an `og:video` that is really a preview frame. Left unchecked, `guess_container`
/// coerces such a URL to `"mp4"` and the download becomes a single still frame
/// saved with a video extension — the classic "it only downloaded the thumbnail".
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "ico", "heic", "heif", "avif", "tif", "tiff",
];

/// Image containers that carry motion. A URL that names one of these directly
/// is a clip the user wants, not a poster, so it is accepted as a download even
/// though [`is_image_url`] keeps the same extensions out of *scraped* candidates.
pub const ANIMATED_IMAGE_EXTENSIONS: &[&str] = &["gif", "apng", "webp"];

pub fn is_animated_image_container(container: &str) -> bool {
    ANIMATED_IMAGE_EXTENSIONS.contains(&container)
}

/// Animated image extension implied by a `Content-Type` header value.
fn animated_image_from_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/gif" => Some("gif"),
        "image/apng" | "image/vnd.mozilla.apng" => Some("apng"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

pub fn is_direct_media(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    MEDIA_EXTENSIONS.iter().chain(ANIMATED_IMAGE_EXTENSIONS).any(|ext| path.ends_with(&format!(".{ext}")))
        || crate::probe::extension_from_url(url)
            .is_some_and(|ext| MEDIA_EXTENSIONS.contains(&ext.as_str()) || is_animated_image_container(&ext))
}

/// Whether a URL's path (ignoring query/fragment) ends in a known image
/// extension. Used to keep poster/thumbnail URLs out of the video candidate
/// list — they are the single most common reason a "download" produces a
/// still frame instead of the clip.
pub fn is_image_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| path.ends_with(&format!(".{ext}")))
}

/// Meta/fbcdn (Facebook, Instagram) serves video CDN links with `bytestart`/
/// `byteend` query params that the *player* uses to fetch a specific chunk
/// during scrubbing/prefetch — a URL captured mid-playback (by the extension's
/// network sniffer, or copied while the video was buffering) often carries
/// these. Downloading such a URL as-is only pulls that one byte range (a
/// truncated, unplayable file), not the full video. Stripping the two params
/// makes the CDN serve the complete file — every other query param (`oh`,
/// `oe`, `_nc_*`, …) is part of the signed-URL auth and must stay untouched.
fn strip_byte_range_params(url: &str) -> String {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    let host = parsed.host_str().unwrap_or_default();
    if !["fbcdn.net", "cdninstagram.com", "cdninstagram.net"].iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}"))) {
        return url.to_string();
    }
    let had_range_param = parsed
        .query_pairs()
        .any(|(k, _)| k.eq_ignore_ascii_case("bytestart") || k.eq_ignore_ascii_case("byteend"));
    if !had_range_param {
        return url.to_string();
    }
    let Some((prefix, query)) = url.split_once('?') else { return url.to_string() };
    let (query, fragment) = query.split_once('#').map_or((query, None), |(q, f)| (q, Some(f)));
    let kept = query.split('&').filter(|pair| {
        let key = pair.split('=').next().unwrap_or_default();
        !key.eq_ignore_ascii_case("bytestart") && !key.eq_ignore_ascii_case("byteend")
    }).collect::<Vec<_>>().join("&");
    format!("{prefix}{}{}", if kept.is_empty() { String::new() } else { format!("?{kept}") },
        fragment.map_or(String::new(), |f| format!("#{f}")))
}

pub fn single_format(url: &str) -> VideoMetadata {
    let url = strip_byte_range_params(url);
    let container = guess_container(&url);
    let audio = is_audio_container(&container);
    VideoMetadata {
        subtitles: Vec::new(),
        title: title_from_url(&url),
        thumbnail: None,
        duration: None,
        uploader: None,
        webpage_url: url.clone(),
        formats: vec![VideoFormat {
            validation: None,
            download_options: None,
            id: if audio { "audio" } else { "direct" }.into(),
            container,
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            fps: None,
            bitrate: None,
            filesize: None,
            video_url: Some(url),
            audio_url: None,
            has_audio: if audio { Some(true) } else { None },
            headers: Vec::new(),
            requires_merge: false,
        }],
    }
}

pub fn title_from_url(url: &str) -> String {
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    clean.rsplit('/').find(|s| !s.is_empty()).unwrap_or("video").to_string()
}

pub fn guess_container(url: &str) -> String {
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    let path = match clean.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map(|(_, p)| p).unwrap_or(""),
        None => clean,
    };
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if matches!(
            ext_lower.as_str(),
            "mp4" | "webm" | "mkv" | "mov" | "avi" | "flv" | "m4v" | "3gp" | "ogv" | "ts" | "m3u8" | "mpd" | "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "wav" | "flac" | "gif" | "apng" | "webp"
        ) {
            return ext_lower;
        }
    }
    if let Some(ext) = crate::probe::extension_from_url(url) {
        let ext_lower = ext.to_ascii_lowercase();
        if matches!(
            ext_lower.as_str(),
            "mp4" | "webm" | "mkv" | "mov" | "avi" | "flv" | "m4v" | "3gp" | "ogv" | "ts" | "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "wav" | "flac" | "gif" | "apng" | "webp"
        ) {
            return ext_lower;
        }
    }
    "mp4".to_string()
}

static VIDEO_SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<(?:video|audio)[^>]*\ssrc=["']([^"']+)["']"#).unwrap());
static SOURCE_SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<source[^>]*\ssrc=["']([^"']+)["']"#).unwrap());
static OG_VIDEO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]*property=["']og:video(?::url|:secure_url)?["'][^>]*content=["']([^"']+)["']"#).unwrap());
static TWITTER_STREAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]*name=["']twitter:player:stream["'][^>]*content=["']([^"']+)["']"#).unwrap());
static CONTENT_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""contentUrl"\s*:\s*"([^"]+)""#).unwrap());
static JSON_MEDIA_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""(?:video_url|videoUrl|playAddr|downloadAddr|playable_url|playable_url_quality_hd|sd_src|hd_src|hlsManifestUrl)"\s*:\s*"([^"]+)""#).unwrap()
});
static EMBEDDED_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https?:\\?/\\?/[^"'\s\\]+?\.(?:mp4|webm|m3u8|mpd)(?:\?[^"'\s\\]*)?"#).unwrap()
});
static HLS_FALLBACK_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)["']([^"'\s<>]*\.m3u8(?:[?#][^"'\s<>]*)?)["']"#).unwrap()
});
static TITLE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static OG_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<meta[^>]*property=["']og:image["'][^>]*content=["']([^"']+)["']"#).unwrap());

pub fn unescape_url(raw: &str) -> String {
    raw.replace(r"\/", "/")
       .replace(r"\u002F", "/")
       .replace(r"\u002f", "/")
       .replace(r"\u003A", ":")
       .replace(r"\u003a", ":")
       .replace(r"\u003F", "?")
       .replace(r"\u003f", "?")
       .replace(r"\u0026", "&")
       .replace(r"\u003D", "=")
       .replace(r"\u003d", "=")
       .replace(r"\u0025", "%")
       .replace("&amp;", "&")
}

pub async fn analyze_page(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    analyze_response(client, url, headers, false).await
}

pub async fn analyze_file(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    analyze_response(client, url, headers, true).await
}

async fn analyze_response(client: &Client, url: &str, headers: &Headers, file_hint: bool) -> Result<VideoMetadata> {
    let response = fetch_response(client, url, headers).await?;
    let page_url = response.url().to_string();
    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok()).unwrap_or("")
        .split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let filename = response.headers().get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok()).and_then(crate::probe::filename_from_disposition);
    let container = match content_type.as_str() {
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/quicktime" => Some("mov"),
        "video/x-matroska" => Some("mkv"),
        "video/x-msvideo" => Some("avi"),
        "video/x-flv" => Some("flv"),
        "video/3gpp" => Some("3gp"),
        "video/ogg" => Some("ogv"),
        "video/mp2t" => Some("ts"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "audio/ogg" => Some("ogg"),
        "audio/opus" => Some("opus"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/webm" => Some("webm"),
        other => animated_image_from_mime(other),
    };
    let file_response = container.is_some() ||
        (matches!(content_type.as_str(), "" | "application/octet-stream") &&
            (file_hint || filename.as_deref().is_some_and(is_direct_media)));
    if file_response {
        // Do not buffer an entire MP4 just to search it as HTML. The normal
        // track inspection below extraction verifies the selected source.
        let filesize = response.content_length();
        drop(response);
        let mut meta = single_format(&page_url);
        meta.webpage_url = url.to_string();
        if let Some(name) = filename { meta.title = name; }
        if let Some(container) = container { meta.formats[0].container = container.into(); }
        else { meta.formats[0].container = guess_container(&meta.title); }
        meta.formats[0].filesize = filesize;
        if content_type.starts_with("audio/") || is_audio_container(&meta.formats[0].container) {
            meta.formats[0].id = "audio".into();
            meta.formats[0].has_audio = Some(true);
        }
        return Ok(meta);
    }
    if matches!(content_type.as_str(), "application/vnd.apple.mpegurl" | "application/x-mpegurl") {
        drop(response);
        return super::hls::analyze(client, &page_url, headers).await;
    }
    if content_type == "application/dash+xml" {
        drop(response);
        return super::dash::analyze(client, &page_url, headers).await;
    }
    if content_type.starts_with("image/") {
        return Err(Error::InvalidInput("This URL returned an image, not a video.".into()));
    }
    if file_hint {
        return Err(Error::InvalidInput("The selected media URL did not return a media file. Resume playback and retry.".into()));
    }
    let html = response.text().await?;
    // Network capture can supply an extensionless HLS endpoint.
    if html.trim_start_matches('\u{feff}').trim_start().starts_with("#EXTM3U") {
        return super::hls::analyze(client, &page_url, headers).await;
    }
    let text = html.trim_start_matches('\u{feff}').trim_start();
    let xml = if text.starts_with("<?xml") { text.split_once("?>").map_or(text, |(_, tail)| tail.trim_start()) } else { text };
    if xml.starts_with("<MPD ") || xml.starts_with("<MPD>") || xml.starts_with("<MPD\n") {
        return super::dash::analyze(client, &page_url, headers).await;
    }

    let mut candidates: Vec<String> = Vec::new();

    // 1. Specific metadata & JSON attribute matches
    for re in [&*VIDEO_SRC, &*SOURCE_SRC, &*OG_VIDEO, &*TWITTER_STREAM, &*CONTENT_URL, &*JSON_MEDIA_URL] {
        for cap in re.captures_iter(&html) {
            let unescaped = unescape_url(&cap[1]);
            let resolved = resolve_against_page(&page_url, &unescaped);
            if !candidates.contains(&resolved) {
                candidates.push(resolved);
            }
        }
    }

    // 2. Generic media URL scan (captures YouTube hlsManifestUrl, TikTok playAddr, Facebook, Instagram, Twitter streams, etc.)
    for cap in EMBEDDED_URL_PATTERN.find_iter(&html) {
        let unescaped = unescape_url(cap.as_str());
        // Incidental playlists belong to the fallback, not the primary scan.
        if unescaped.to_ascii_lowercase().contains(".m3u8") { continue; }
        let resolved = resolve_against_page(&page_url, &unescaped);
        if !candidates.contains(&resolved) {
            candidates.push(resolved);
        }
    }

    // Drop poster/thumbnail images that came through `contentUrl`/`og:video`
    // and similar keys — accepting one turns the download into a still frame.
    candidates.retain(|c| (c.starts_with("http://") || c.starts_with("https://")) && !is_image_url(c));

    let mut stream_headers = headers.clone();
    if !stream_headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("referer")) {
        stream_headers.push(("Referer".into(), page_url.clone()));
    }
    let mut formats = Vec::new();
    for (i, candidate) in candidates.iter().enumerate() {
        let lower = candidate.to_ascii_lowercase();
        let is_hls = lower.contains(".m3u8");
        let is_dash = lower.contains(".mpd");

        let nested = if is_hls {
            super::hls::analyze(client, candidate, &stream_headers).await.ok()
        } else if is_dash {
            super::dash::analyze(client, candidate, &stream_headers).await.ok()
        } else {
            None
        };

        match nested {
            Some(meta) => formats.extend(meta.formats),
            None if is_hls || is_dash => {}, // A failed manifest is never a downloadable media file.
            None => {
                let (id, width, height) = super::site::parse_resolution_from_url(candidate);
                let container = guess_container(candidate);
                let audio = is_audio_container(&container);
                formats.push(VideoFormat {
                    validation: None,
                    download_options: None,
                    id: if audio { "audio".into() } else if id == "mp4" { format!("page-{i}") } else { id },
                    container,
                    video_codec: None,
                    audio_codec: None,
                    width,
                    height,
                    fps: None,
                    bitrate: None,
                    filesize: None,
                    video_url: Some(candidate.clone()),
                    audio_url: None,
                    has_audio: if audio { Some(true) } else { None },
                    headers: Vec::new(),
                    requires_merge: false,
                });
            }
        }
    }

    // Unknown players often store a relative or escaped playlist URL in
    // inline configuration. Only use that broad scan when normal extraction
    // yielded no formats, so incidental ads do not replace a working source.
    if formats.is_empty() {
        let decoded = unescape_url(&html);
        let mut tried: std::collections::HashSet<String> = candidates.iter().cloned().collect();
        let fallback = HLS_FALLBACK_URL.captures_iter(&decoded)
            .map(|cap| resolve_against_page(&page_url, &cap[1]))
            .filter(|candidate| (candidate.starts_with("http://") || candidate.starts_with("https://")) && tried.insert(candidate.clone()))
            .take(8).collect::<Vec<_>>();
        for candidate in fallback {
            if let Ok(meta) = super::hls::analyze(client, &candidate, &stream_headers).await {
                formats.extend(meta.formats);
            }
        }
        if formats.is_empty() {
            return Err(Error::InvalidInput(if tried.is_empty() {
                "No stream detected. Play the video in the browser and retry with the extension.".into()
            } else {
                "Stream detected but could not be resolved. Resume playback and retry with the extension.".into()
            }));
        }
    }

    // Keep the page's referrer with every rendition so pasted page URLs and
    // extension-prefilled URLs behave alike on CDNs that require it.
    for format in &mut formats {
        if !format.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("referer")) {
            if let Some(referer) = stream_headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("referer")) {
                format.headers.push(referer.clone());
            }
        }
    }

    // Deduplicate formats by video_url
    let mut seen_urls = std::collections::HashSet::new();
    formats.retain(|f| {
        if let Some(ref u) = f.video_url {
            seen_urls.insert(u.clone())
        } else {
            true
        }
    });

    // Sort formats so the highest quality with merges audio tag is listed first
    super::sort_formats(&mut formats);

    let has_quality = formats.iter().any(|f| f.height.is_some() || f.id.ends_with('p'));
    if has_quality {
        formats.retain(|f| !f.id.starts_with("page-") && f.id != "og" && f.id != "stream" && f.id != "mp4");
    }

    Ok(VideoMetadata {
        subtitles: super::subtitles::from_html(&html, &page_url),
        title: TITLE_TAG.captures(&html).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| title_from_url(url)),
        thumbnail: OG_IMAGE.captures(&html).map(|c| c[1].to_string()),
        duration: None,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

fn resolve_against_page(page_url: &str, candidate: &str) -> String {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_string();
    }
    reqwest::Url::parse(page_url)
        .and_then(|base| base.join(candidate))
        .map(|joined| joined.to_string())
        .unwrap_or_else(|_| candidate.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_direct_media_recognizes_common_containers() {
        assert!(is_direct_media("https://cdn.example/a.mp4"));
        assert!(is_direct_media("https://cdn.example/a.webm"));
        assert!(!is_direct_media("https://cdn.example/watch?v=1"));
    }

    #[test]
    fn title_from_url_uses_the_last_path_segment() {
        assert_eq!(title_from_url("https://cdn.example/dir/clip.mp4?x=1"), "clip.mp4");
    }

    #[test]
    fn single_format_carries_the_extension_as_the_container() {
        let meta = single_format("https://cdn.example/clip.webm");
        assert_eq!(meta.formats[0].container, "webm");
        assert!(!meta.formats[0].requires_merge);
    }

    #[test]
    fn single_format_strips_bytestart_byteend_but_keeps_signed_auth_params() {
        let meta = single_format(
            "https://instagram.fdvo2-2.fna.fbcdn.net/o1/v/t2/f2/m366/clip.mp4\
             ?_nc_cat=111&oh=00_AQ&oe=6A68&bytestart=7396193&byteend=10043710",
        );
        let url = meta.formats[0].video_url.as_deref().unwrap();
        assert!(!url.contains("bytestart"));
        assert!(!url.contains("byteend"));
        assert!(url.contains("_nc_cat=111"));
        assert!(url.contains("oh=00_AQ"));
        assert!(url.contains("oe=6A68"));
    }

    #[test]
    fn strip_byte_range_params_leaves_ordinary_urls_untouched() {
        assert_eq!(
            strip_byte_range_params("https://cdn.example/clip.mp4?token=abc"),
            "https://cdn.example/clip.mp4?token=abc"
        );
        let signed = "https://cdn.example/audio.mp4?sig=a%2Fb+z&bytestart=20&byteend=100";
        assert_eq!(strip_byte_range_params(signed), signed);
        assert_eq!(strip_byte_range_params("https://video.fbcdn.net/clip.mp4?sig=a%2Fb+z&bytestart=20&byteend=100"),
            "https://video.fbcdn.net/clip.mp4?sig=a%2Fb+z");
    }

    #[test]
    fn audio_files_route_as_audio_without_inventing_a_video_track() {
        for ext in ["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"] {
            let url = format!("https://cdn.example/sound.{ext}?sig=a%2Fb+z");
            assert!(is_direct_media(&url));
            let meta = single_format(&url);
            assert_eq!(meta.formats[0].id, "audio");
            assert_eq!(meta.formats[0].container, ext);
            assert_eq!(meta.formats[0].has_audio, Some(true));
            assert_eq!(meta.formats[0].video_url.as_deref(), Some(url.as_str()));
        }
    }

    #[test]
    fn image_urls_are_recognized_regardless_of_query_string() {
        assert!(is_image_url("https://cdn.example/thumb.jpg"));
        assert!(is_image_url("https://cdn.example/poster.JPEG?width=640"));
        assert!(is_image_url("https://cdn.example/still.webp#frag"));
    }

    #[test]
    fn animated_images_named_directly_are_downloadable_clips() {
        for ext in ["gif", "apng", "webp"] {
            let url = format!("https://cdn.example/loop.{ext}?sig=a%2Fb");
            assert!(is_direct_media(&url));
            assert_eq!(single_format(&url).formats[0].container, ext);
        }
        assert!(is_direct_media("https://cdn.example/download?format=gif&token=x"));
        // Still pictures stay out, and scraped posters are still filtered.
        assert!(!is_direct_media("https://cdn.example/photo.jpg"));
        assert!(is_image_url("https://cdn.example/loop.gif"));
        assert!(animated_image_from_mime("image/gif").is_some());
        assert!(animated_image_from_mime("image/jpeg").is_none());
    }

    #[test]
    fn test_unescape_url() {
        assert_eq!(
            unescape_url(r"https:\u002F\u002Fv16-webapp-prime.tiktok.com\u002Fvideo\u002Ftos\u002F?a=1\u0026b=2\u003D3"),
            "https://v16-webapp-prime.tiktok.com/video/tos/?a=1&b=2=3"
        );
    }

    #[test]
    fn image_urls_are_recognized_regardless_of_query_string_continued() {
        assert!(!is_image_url("https://cdn.example/clip.mp4"));
        assert!(!is_image_url("https://cdn.example/stream.m3u8?token=abc"));
        // .webm is a video container, not to be confused with .webp
        assert!(!is_image_url("https://cdn.example/clip.webm"));
    }
}
