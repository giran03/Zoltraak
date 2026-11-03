//! Site-specific video extractors for major platforms.
//!
//! Each extractor works purely on the page HTML — no headless browser, no
//! JavaScript execution. They pull URLs from the JSON blobs these sites embed
//! in their initial HTML payload (e.g. `__NEXT_DATA__`, `__REHYDRATED_STATE__`,
//! inline `<script>` JSON) and from Open Graph / Twitter Card meta tags.
//!
//! The general-purpose `direct::analyze_page` already handles cases not covered
//! here; this module handles the sites that need specialised keys or header
//! treatment that the generic scraper would miss.

use std::sync::LazyLock;

use regex::Regex;
use reqwest::Client;

use super::net::fetch_text;
use super::{VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use crate::headers::Headers;
use super::direct::{title_from_url, unescape_url};

// ─── Hostname helpers ─────────────────────────────────────────────────────────

pub(crate) fn host_of(url: &str) -> &str {
    url.trim_start_matches("https://")
       .trim_start_matches("http://")
       .split(['/', '?', '#'])
       .next()
       .unwrap_or("")
}

pub fn matches_platform(url: &str) -> Option<Platform> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let matches = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if matches("facebook.com") || matches("fb.watch") {
        return Some(Platform::Facebook);
    }
    if matches("tiktok.com") {
        return Some(Platform::TikTok);
    }
    if matches("instagram.com") {
        return Some(Platform::Instagram);
    }
    if matches("youtube.com") || matches("youtu.be") {
        return Some(Platform::YouTube);
    }
    if matches("twitter.com") || matches("x.com") {
        return Some(Platform::Twitter);
    }
    if matches("reddit.com") || matches("v.redd.it") {
        return Some(Platform::Reddit);
    }
    if matches("vimeo.com") {
        return Some(Platform::Vimeo);
    }
    None
}

/// The canonical key [`VideoManager`](super::manager::VideoManager)'s
/// site-cookie cache stores a URL's cookie under, for the platforms whose
/// extractors above actually need a login session (their error messages all
/// say so). `None` for everything else — no point caching a cookie a public
/// direct-media link or a page-scrape target never asks for.
pub fn cookie_domain_for(url: &str) -> Option<&'static str> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let matches = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if matches("facebook.com") || matches("fb.watch") {
        return Some("facebook.com");
    }
    if matches("tiktok.com") {
        return Some("tiktok.com");
    }
    if matches("instagram.com") {
        return Some("instagram.com");
    }
    if matches("twitter.com") || matches("x.com") {
        return Some("x.com");
    }
    if matches("youtube.com") || matches("youtu.be") {
        return Some("youtube.com");
    }
    // Reddit answers its own `.json` API with 403 to anonymous clients now, so
    // a shared session is the difference between reading a post's structured
    // media and scraping its HTML for what is left.
    if matches("reddit.com") || matches("redd.it") {
        return Some("reddit.com");
    }
    None
}

#[derive(Debug, Clone, Copy)]
pub enum Platform {
    Facebook,
    TikTok,
    Instagram,
    YouTube,
    Twitter,
    Reddit,
    Vimeo,
}

pub async fn analyze(
    client: &Client,
    url: &str,
    headers: &Headers,
    platform: Platform,
) -> Result<VideoMetadata> {
    match platform {
        Platform::Facebook  => facebook(client, url, headers).await,
        Platform::TikTok    => tokio::time::timeout(std::time::Duration::from_secs(30), tiktok(client, url, headers))
            .await.map_err(|_| Error::NetworkTimeout)?,
        Platform::Instagram => instagram(client, url, headers).await,
        Platform::YouTube   => super::youtube::analyze(client, url, headers).await,
        Platform::Twitter   => twitter(client, url, headers).await,
        Platform::Reddit    => reddit(client, url, headers).await,
        Platform::Vimeo     => vimeo(client, url, headers).await,
    }
}

// ─── Common helpers ───────────────────────────────────────────────────────────

/// Pull all JSON string values whose key matches any of `keys` from `text`.
fn json_values<'a>(text: &'a str, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        // matches: "key":"value" or "key": "value"
        let pattern = format!(r#""{key}"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)""#);
        if let Ok(re) = regex::Regex::new(&pattern) {
            for cap in re.captures_iter(text) {
                let v = unescape_url(&cap[1]);
                if !v.is_empty() && !out.contains(&v) {
                    out.push(v);
                }
            }
        }
    }
    out
}

/// Pull all `og:video*` and `twitter:player:stream` meta content values.
static OG_VIDEO_META: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta[^>]*(?:property|name)=["'](?:og:video(?::(?:url|secure_url))?|twitter:player:stream)["'][^>]*content=["']([^"']+)["']"#).unwrap()
});
static OG_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta[^>]*property=["']og:title["'][^>]*content=["']([^"']+)["']"#).unwrap()
});
static OG_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta[^>]*property=["']og:image(?::url)?["'][^>]*content=["']([^"']+)["']"#).unwrap()
});
static HTML_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap()
});

fn og_videos(html: &str) -> Vec<String> {
    OG_VIDEO_META
        .captures_iter(html)
        .map(|c| unescape_url(&c[1]))
        .filter(|u| !u.is_empty())
        // An `og:video`/`twitter:player:stream` that points at a poster image
        // would otherwise be handed to `mp4_format` and downloaded as a still.
        .filter(|u| !super::direct::is_image_url(u))
        .collect()
}

pub(crate) fn page_title(html: &str, url: &str) -> String {
    let raw = OG_TITLE
        .captures(html)
        .map(|c| c[1].trim().to_string())
        .or_else(|| {
            HTML_TITLE
                .captures(html)
                .map(|c| c[1].trim().to_string())
        })
        .unwrap_or_default();

    super::title::ensure_valid_title(&raw, url)
}

pub(crate) fn page_thumbnail(html: &str) -> Option<String> {
    OG_IMAGE.captures(html).map(|c| c[1].to_string())
}

/// Every `og:image` on the page, in document order.
///
/// [`page_thumbnail`] wants one poster frame; the image extractors want the
/// whole set, because a gallery page that ships several `og:image` tags is
/// describing several pictures.
pub(crate) fn og_images(html: &str) -> Vec<String> {
    OG_IMAGE
        .captures_iter(html)
        .map(|c| unescape_url(&c[1]))
        .filter(|u| u.starts_with("http"))
        .collect()
}

/// Pull the *decoded* value of every JSON string field named by `keys`.
///
/// Unlike [`json_values`], this understands JSON escaping rather than
/// approximating it, which matters because the values it is used for are whole
/// XML documents: Meta inlines a video's entire DASH manifest as a JSON string,
/// full of escaped quotes and newlines. A regex that stops at the first `"` cuts
/// such a value off after a dozen characters.
fn json_string_entries(text: &str, keys: &[&str]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();

    for key in keys {
        let needle = format!("\"{key}\"");
        let mut from = 0;
        while let Some(found) = text[from..].find(&needle) {
            let key_offset = from + found;
            let after_key = key_offset + needle.len();
            from = after_key;

            // Only a `"key": "…"` counts; `"key": {` or `"key": null` do not.
            let rest = text[after_key..].trim_start();
            let Some(rest) = rest.strip_prefix(':') else { continue };
            let rest = rest.trim_start();
            if !rest.starts_with('"') {
                continue;
            }
            let value_start = text.len() - rest.len();

            let Some(value_end) = json_string_end(text, value_start) else { continue };
            if let Ok(decoded) = serde_json::from_str::<String>(&text[value_start..=value_end]) {
                if !decoded.is_empty() {
                    out.push((key_offset, decoded));
                }
            }
            from = value_end + 1;
        }
    }

    out
}

pub(crate) fn json_string_values(text: &str, keys: &[&str]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for (_, val) in json_string_entries(text, keys) {
        if !seen.contains(&val) {
            seen.push(val);
        }
    }
    seen
}

/// The index of the closing quote of the JSON string starting at `start`
/// (which must be its opening quote), honouring backslash escapes.
fn json_string_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn facebook_video_id(url: &str) -> Option<&str> {
    static RE_REEL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:/reel/|/videos/)(\d{3,25})(?:[/?#]|$)"#).unwrap()
    });
    if let Some(cap) = RE_REEL.captures(url) {
        return cap.get(1).map(|m| m.as_str());
    }
    static RE_PARAM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"[?&](?:v|story_fbid|video_id)=(\d{3,25})"#).unwrap()
    });
    if let Some(cap) = RE_PARAM.captures(url) {
        return cap.get(1).map(|m| m.as_str());
    }
    None
}

/// Selects the single best DASH manifest for the target video item.
///
/// On Meta platforms (Facebook / Instagram reels and feeds), the page payload
/// frequently inlines manifests for multiple clips (the current video PLUS
/// preloaded next/recommended reels). Taking all of them produced a chimera
/// format list where preloaded clips outranked the video on screen.
///
/// If `target_id` is given (e.g. reel/video ID), this chooses the manifest
/// whose byte offset in the HTML JSON is closest to `target_id`. Otherwise,
/// the first manifest in the page (the primary item) is chosen. Manifests from
/// different videos are NEVER combined.
fn best_manifest(text: &str, keys: &[&str], target_id: Option<&str>) -> Option<String> {
    let entries = json_string_entries(text, keys);
    let valid_entries: Vec<(usize, String)> = entries
        .into_iter()
        .filter(|(_, xml)| xml.contains("<MPD"))
        .collect();

    if valid_entries.is_empty() {
        return None;
    }

    if let Some(id) = target_id {
        if !id.is_empty() {
            let id_needle = format!("\"{id}\"");
            let id_offsets: Vec<usize> = text
                .match_indices(&id_needle)
                .map(|(idx, _)| idx)
                .chain(text.match_indices(id).map(|(idx, _)| idx))
                .collect();

            if !id_offsets.is_empty() {
                let mut best_xml = None;
                let mut min_dist = usize::MAX;

                for (offset, xml) in valid_entries.iter() {
                    for &id_off in &id_offsets {
                        let dist = if *offset > id_off {
                            *offset - id_off
                        } else {
                            id_off - *offset
                        };
                        if dist < min_dist {
                            min_dist = dist;
                            best_xml = Some(xml.clone());
                        }
                    }
                }

                if min_dist < 100_000 {
                    return best_xml;
                }
            }
        }
    }

    valid_entries.into_iter().next().map(|(_, xml)| xml)
}

/// DASH manifest for the target video item, parsed into formats.
fn inline_dash_formats(text: &str, page_url: &str, target_id: Option<&str>) -> Vec<VideoFormat> {
    const MANIFEST_KEYS: &[&str] = &[
        // Facebook
        "dash_manifest",
        "manifest_xml",
        // Instagram
        "video_dash_manifest",
    ];

    let base = super::net::base_url(page_url);
    let mut formats: Vec<VideoFormat> = Vec::new();

    if let Some(xml) = best_manifest(text, MANIFEST_KEYS, target_id) {
        if let Ok(parsed) = super::dash::formats_from_manifest(&xml, &base) {
            for format in parsed {
                formats.push(format);
            }
        }
    }

    formats
}

fn mp4_format(id: &str, url: String) -> VideoFormat {
    VideoFormat {
        validation: None,
        download_options: None,
        id: id.to_string(),
        container: "mp4".into(),
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        fps: None,
        bitrate: None,
        filesize: None,
        video_url: Some(url),
        audio_url: None,
        has_audio: None,
        headers: Vec::new(),
        requires_merge: false,
    }
}

// ─── Facebook ─────────────────────────────────────────────────────────────────
//
// Facebook serves the same clip two ways, and only one of them is safe to take
// at face value:
//
//   * DASH — the page inlines the whole manifest as a JSON string
//     (`dash_manifest`, `manifest_xml`). Picture and sound are separate files
//     on the same CDN, and nothing about either URL says which is which. This
//     is the accurate source: the manifest names the tracks, so the two halves
//     can be paired and muxed back together. Grabbing one of these URLs on its
//     own — which is what a network sniffer or a naive `fbcdn.net` regex does —
//     is how "download this video" produced an audio-only file.
//   * Progressive — one muxed mp4 per quality, under `playable_url`,
//     `playable_url_quality_hd`, `browser_native_*_url`, `hd_src`/`sd_src`, or
//     `progressive_url`. Complete on its own, but usually capped well below the
//     best DASH rendition.
//
// So: read the manifest first and offer its full ladder, then keep the
// progressive files as fallbacks (they need no ffmpeg). `base_url` is
// deliberately never scraped on its own — inside a manifest it is a track, and
// out of context it is unidentifiable.
//
// The share URL (`/share/v/…`) is a redirect to the canonical watch page.
// reqwest follows the redirect automatically; the cookies supplied by the
// browser extension (if any) ride along.

/// JSON keys whose value is a complete, muxed progressive mp4 — best quality
/// first. Anything not on this list is not known to carry both tracks and is
/// not treated as a whole video.
const FB_PROGRESSIVE_KEYS: &[&str] = &[
    "playable_url_quality_hd",
    "browser_native_hd_url",
    "hd_src",
    "progressive_url",
    "playable_url",
    "browser_native_sd_url",
    "sd_src",
];

pub(crate) fn facebook_formats(html: &str, url: &str) -> Vec<VideoFormat> {
    let target_id = facebook_video_id(url);
    let mut formats = inline_dash_formats(html, url, target_id);
    let mut seen: Vec<String> = formats.iter().filter_map(|f| f.video_url.clone()).collect();

    let push = |id: &str, candidate: String, formats: &mut Vec<VideoFormat>, seen: &mut Vec<String>| {
        if candidate.is_empty() || seen.contains(&candidate) || super::direct::is_image_url(&candidate) {
            return;
        }
        seen.push(candidate.clone());
        formats.push(mp4_format(id, candidate));
    };

    // Progressive files: only accept progressive files belonging to the target video!
    if let Some(id) = target_id {
        let id_needle = format!("\"{id}\"");
        let id_offsets: Vec<usize> = html
            .match_indices(&id_needle)
            .map(|(idx, _)| idx)
            .chain(html.match_indices(id).map(|(idx, _)| idx))
            .collect();

        for key in FB_PROGRESSIVE_KEYS {
            for (offset, value) in json_string_entries(html, &[key]) {
                if !value.contains("fbcdn.net") {
                    continue;
                }
                let is_near = id_offsets.iter().any(|&id_off| {
                    let dist = if offset > id_off { offset - id_off } else { id_off - offset };
                    dist < 50_000
                });
                if is_near {
                    push(key, value, &mut formats, &mut seen);
                }
            }
        }
    } else if formats.is_empty() {
        for key in FB_PROGRESSIVE_KEYS {
            for value in json_string_values(html, &[key]) {
                if !value.contains("fbcdn.net") {
                    continue;
                }
                push(key, value, &mut formats, &mut seen);
            }
        }
    } else {
        // DASH already provided formats, but no target ID was specified: only accept the first progressive file
        for key in FB_PROGRESSIVE_KEYS {
            if let Some((offset, value)) = json_string_entries(html, &[key]).into_iter().next() {
                if value.contains("fbcdn.net") && offset < 100_000 {
                    push(key, value, &mut formats, &mut seen);
                }
            }
        }
    }

    if formats.is_empty() {
        for (i, u) in og_videos(html).into_iter().enumerate() {
            push(&format!("og-{i}"), u, &mut formats, &mut seen);
        }

        static FB_VIDEO_TAG: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"(?is)<video[^>]*\bsrc=["']([^"']*fbcdn\.net[^"']+)["']"#).unwrap()
        });
        for cap in FB_VIDEO_TAG.captures_iter(html) {
            push("inline", unescape_url(&cap[1]), &mut formats, &mut seen);
        }
    }

    formats
}

pub(crate) fn clean_facebook_url(url: &str) -> String {
    if let Ok(mut parsed) = reqwest::Url::parse(url) {
        let is_fb = parsed.host_str().map(|h| h.contains("facebook.com") || h.contains("fb.watch")).unwrap_or(false);
        if is_fb {
            let pairs: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| {
                    let kl = k.to_ascii_lowercase();
                    !kl.starts_with("__") && kl != "s" && kl != "mibextid" && kl != "rdid" && kl != "share_url"
                })
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            parsed.query_pairs_mut().clear().extend_pairs(&pairs);
            if pairs.is_empty() {
                parsed.set_query(None);
            }
            return parsed.to_string();
        }
    }
    url.to_string()
}

async fn facebook(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let clean_url = clean_facebook_url(url);
    let target_url = if clean_url.is_empty() { url } else { &clean_url };
    if facebook_is_post(target_url) {
        // The post extractor scopes attachments to the requested Story and
        // already handles share redirects, crawler identities and sessions.
        let album = crate::image::facebook::analyze(client, target_url, headers).await?;
        return facebook_post_metadata(album, target_url);
    }
    // Keep the final redirect URL: share tokens are not numeric video IDs.
    let mut resolved_url = target_url.to_string();
    let mut html = String::new();
    if let Ok(response) = super::net::fetch_response(client, target_url, headers).await {
        resolved_url = response.url().to_string();
        html = response.text().await?;
    }
    if facebook_is_post(&resolved_url) {
        let album = crate::image::facebook::analyze(client, &resolved_url, headers).await?;
        return facebook_post_metadata(album, target_url);
    }
    let mut formats = facebook_formats(&html, &resolved_url);

    if formats.is_empty() {
        // Public crawler pages can expose the complete MPD when the browser
        // HTML is a login shell. Keep cookies off this anonymous attempt.
        let crawler = vec![("User-Agent".into(), "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)".into())];
        if let Ok(response) = super::net::fetch_response(client, target_url, &crawler).await {
            let final_url = response.url().to_string();
            if let Ok(page) = response.text().await {
                formats = facebook_formats(&page, &final_url);
                if !formats.is_empty() {
                    for format in &mut formats { format.headers = crawler.clone(); }
                    html = page;
                    resolved_url = final_url;
                }
            }
        }
    }

    if formats.is_empty() {
        // m.facebook.com renders a plain player for the same video and is far
        // less aggressive about gating it behind a session.
        let mobile_url = resolved_url
            .replace("www.facebook.com", "m.facebook.com")
            .replace("web.facebook.com", "m.facebook.com");
        if mobile_url != target_url {
            if let Ok(mobile_html) = fetch_text(client, &mobile_url, headers).await {
                formats = facebook_formats(&mobile_html, &mobile_url);
                if !formats.is_empty() {
                    html = mobile_html;
                }
            }
        }
    }

    // If still empty and headers had cookies, try anonymously as fallback
    let has_cookie = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie"));
    if formats.is_empty() && has_cookie {
        let anon_headers: Headers = headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("cookie"))
            .cloned()
            .collect();
        if let Ok(anon_html) = fetch_text(client, target_url, &anon_headers).await {
            formats = facebook_formats(&anon_html, target_url);
            if formats.is_empty() {
                let mobile_url = target_url
                    .replace("www.facebook.com", "m.facebook.com")
                    .replace("web.facebook.com", "m.facebook.com");
                if mobile_url != target_url {
                    if let Ok(mobile_html) = fetch_text(client, &mobile_url, &anon_headers).await {
                        formats = facebook_formats(&mobile_html, &mobile_url);
                        if !formats.is_empty() {
                            html = mobile_html;
                        }
                    }
                }
            } else {
                html = anon_html;
            }
        }
    }

    if formats.is_empty() {
        return Err(Error::InvalidInput(
            "no video found — Facebook may require a login session; try via the browser extension".into(),
        ));
    }

    let target_id = facebook_video_id(&resolved_url);
    let duration = facebook_duration(&html, target_id);
    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title: page_title(&html, target_url),
        thumbnail: page_thumbnail(&html).or_else(|| facebook_thumbnail(&html)),
        duration,
        uploader: None,
        webpage_url: target_url.to_string(),
        formats,
    })
}

fn facebook_is_post(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.path().contains("/posts/") || u.path().contains("/permalink/")
            || u.path().starts_with("/share/p/")
            || matches!(u.path(), "/permalink.php" | "/story.php")
    })
}

fn facebook_post_metadata(album: crate::image::ImageAlbum, url: &str) -> Result<VideoMetadata> {
    let videos: Vec<_> = album.items.into_iter()
        .filter(|item| item.kind == crate::image::MediaKind::Video && !item.formats.is_empty()).collect();
    if videos.is_empty() {
        return Err(Error::InvalidInput("No video attachments found in this Facebook post. Refresh the shared sign-in and retry.".into()));
    }
    let thumbnail = videos[0].thumbnail.clone();
    // An incomplete post may hide another clip. Its post ID alone cannot
    // prove that the one attachment found is the selected player.
    let multiple = videos.len() > 1 || album.completeness != Some(crate::image::Completeness::Complete);
    let mut formats = Vec::new();
    for (index, item) in videos.into_iter().enumerate() {
        for mut format in item.formats {
            if multiple { format.id = format!("clip-{}-{}", index + 1, format.id); }
            format.download_options.get_or_insert_with(Default::default).thumbnail = item.thumbnail.clone();
            formats.push(format);
        }
    }
    Ok(VideoMetadata { subtitles: Vec::new(), title: album.title, thumbnail,
        duration: None, uploader: album.uploader, webpage_url: url.into(), formats })
}

fn facebook_duration(html: &str, target_id: Option<&str>) -> Option<f64> {
    const MANIFEST_KEYS: &[&str] = &[
        "dash_manifest",
        "manifest_xml",
        "video_dash_manifest",
    ];
    if let Some(xml) = best_manifest(html, MANIFEST_KEYS, target_id) {
        let needle = "mediaPresentationDuration=\"";
        if let Some(pos) = xml.find(needle) {
            let rest = &xml[pos + needle.len()..];
            if let Some(end) = rest.find('"') {
                if let Some(d) = super::dash::parse_iso_duration(&rest[..end]) {
                    if d > 0.0 {
                        return Some(d);
                    }
                }
            }
        }
    }
    static DURATION_MS_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#""playable_duration_in_ms"\s*:\s*(\d+)"#).unwrap()
    });
    if let Some(cap) = DURATION_MS_RE.captures(html) {
        if let Ok(ms) = cap[1].parse::<f64>() {
            if ms > 0.0 {
                return Some(ms / 1000.0);
            }
        }
    }
    static DURATION_SEC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#""playable_duration"\s*:\s*([0-9.]+)"#).unwrap()
    });
    if let Some(cap) = DURATION_SEC_RE.captures(html) {
        if let Ok(sec) = cap[1].parse::<f64>() {
            if sec > 0.0 {
                return Some(sec);
            }
        }
    }
    None
}

/// The poster frame from the page JSON, for pages that ship no `og:image`.
/// It is the first `uri` after `preferred_thumbnail` — `uri` on its own is
/// far too common a key (every avatar on the page has one) to search for.
fn facebook_thumbnail(html: &str) -> Option<String> {
    let at = html.find("\"preferred_thumbnail\"")?;
    let window = &html[at..html.len().min(at + 4000)];
    json_string_values(window, &["uri"])
        .into_iter()
        .find(|u| u.starts_with("http") && super::direct::is_image_url(u))
}

// ─── TikTok ───────────────────────────────────────────────────────────────────
//
// TikTok inlines a JSON blob in a <script id="__UNIVERSAL_DATA_FOR_REHYDRATION__">
// tag (newer pages) or as `window.__INITIAL_STATE__` (older). The JSON is
// traversed to locate:
//   itemStruct.video.playAddr     – progressive playback source
//   itemStruct.video.downloadAddr – download-quality mp4 (may be missing)
//   itemStruct.video.play_addr    – alternate key
//
// TikTok's CDN domains: *.tiktokcdn.com, *.tiktok.com (video-* subdomains),
// *.tiktokv.com, *.bytefcdn.com, *.bytefed.com. The old flat `json_values` scan
// picked up image URLs as well, so we now restrict to these CDNs.
//
// Public posts may expose playback URLs without a login. Private or restricted
// posts can need the browser session synced by the extension. Video and Images
// share the same item-bound parser instead of scanning unrelated feed URLs.

static TIKTOK_VIDEO_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:/video/|/photo/|/v/|/embed/(?:v2/)?|/player/v1/|video_id=|itemId=|item_id=|share_item_id=|aweme_id=)(\d{15,25})"#).unwrap()
});

static TIKTOK_CANONICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<(?:meta|link)\b[^>]*(?:og:url|canonical)[^>]*>"#).unwrap()
});
static TIKTOK_CANONICAL_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:content|href)\s*=\s*["']([^"']+)["']"#).unwrap()
});

/// Share URLs can identify themselves through a canonical tag, never through
/// the first arbitrary `id` or recommendation link in a feed payload.
fn extract_tiktok_video_id(url: &str, html: &str) -> Option<String> {
    if let Some(c) = TIKTOK_VIDEO_ID.captures(url) { return Some(c[1].to_owned()); }
    for tag in TIKTOK_CANONICAL.find_iter(html) {
        if let Some(url) = TIKTOK_CANONICAL_URL.captures(tag.as_str()) {
            if matches!(matches_platform(&url[1]), Some(Platform::TikTok)) {
                if let Some(id) = TIKTOK_VIDEO_ID.captures(&url[1]) { return Some(id[1].to_owned()); }
            }
        }
    }
    None
}

/// TikTok CDN or video stream URL candidate.
fn is_tiktok_video_cdn(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    if super::direct::is_image_url(url) {
        return false;
    }
    u.contains("tiktokcdn")
        || u.contains("tiktokv")
        || u.contains("bytefcdn")
        || u.contains("bytefed")
        || u.contains("muscdn")
        || u.contains("musical.ly")
        || u.contains("akamaized")
        || u.contains("bytedance")
        || u.contains("ibytedtos")
        || u.contains("byteoversea")
        || u.contains("tik-tokcdn")
        || u.contains("ttwstatic")
        || u.contains("tikwm")
        || u.contains(".mp4")
        || u.contains(".m3u8")
        || (u.contains("tiktok.com") && (u.contains("/video/") || u.contains("v16") || u.contains("v19") || u.contains("v26") || u.contains("v36") || u.contains("v77") || u.contains("tos-")))
}

/// Attempt to extract `playAddr` / `downloadAddr` / `play_addr` from the
/// structured TikTok rehydration JSON, navigating multiple known paths.
#[cfg(test)]
fn tiktok_urls_from_json(json_text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_text) else { return Vec::new(); };
    super::tiktok::item(&value, None).map(|item| super::tiktok::video_urls(&item["video"])).unwrap_or_default()
}

fn extract_video_obj_urls(video: &serde_json::Value, out: &mut Vec<String>) {
    for url in super::tiktok::video_urls(video) {
        if !out.contains(&url) { out.push(url); }
    }
}

/// Build extra TikTok-specific request headers. TikTok's servers check that
/// the Referer looks like an in-app navigation before including playAddr in
/// the rehydration JSON; without it the page often renders as a login wall or
/// an empty video section.
fn tiktok_extra_headers() -> Headers {
    vec![("Referer".into(), "https://www.tiktok.com/".into())]
}

async fn tiktok_fetch_text(client: &Client, url: &str, headers: &Headers) -> Result<String> {
    tokio::time::timeout(std::time::Duration::from_secs(10), fetch_text(client, url, headers))
        .await.map_err(|_| Error::NetworkTimeout)?
}

pub(crate) fn tiktok_metadata_from_html(html: &str, url: &str, headers: &Headers) -> Option<VideoMetadata> {
    let expected = extract_tiktok_video_id(url, html);
    super::tiktok::from_html(html, expected.as_deref(), url, headers)
}

pub(crate) async fn tiktok_resolve_url(client: &Client, url: &str, headers: &Headers) -> String {
    // Share redirects cross TikTok subdomains. Resolve without credentials,
    // then request the canonical post with its own authenticated context.
    // Ordinary HTTP redirect handling correctly strips cookies across hosts.
    let mut canonical = url.to_owned();
    if extract_tiktok_video_id(url, "").is_none() {
        let public_headers: Headers = headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("user-agent")).cloned().collect();
        if let Ok(Ok(response)) = tokio::time::timeout(std::time::Duration::from_secs(10),
            crate::headers::send(crate::headers::apply(client.get(url), &public_headers))).await {
            let resolved = response.url().as_str();
            if matches!(matches_platform(resolved), Some(Platform::TikTok)) && extract_tiktok_video_id(resolved, "").is_some() {
                canonical = resolved.to_owned();
            }
        }
    }
    canonical
}

async fn tiktok(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let canonical = tiktok_resolve_url(client, url, headers).await;
    let url = canonical.as_str();
    // Begin with the same browser context Images uses; caller headers win.
    // Invented Chromium hints and a replaced Referer can change the response.
    let mut merged_headers = headers.clone();
    for (k, v) in tiktok_extra_headers() {
        if !merged_headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(&k)) { merged_headers.push((k, v)); }
    }
    let page = |context: Headers| async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let response = super::net::fetch_response(client, url, &context).await?;
            let page_url = response.url().to_string();
            Ok::<_, Error>((page_url, response.text().await?))
        }).await.map_err(|_| Error::NetworkTimeout)?
    };
    let (mut page_url, mut html) = page(headers.clone()).await.unwrap_or_else(|_| (url.to_owned(), String::new()));
    if let (Some(requested), Some(resolved)) = (extract_tiktok_video_id(url, ""), extract_tiktok_video_id(&page_url, "")) {
        if requested != resolved { return Err(Error::InvalidInput("TikTok redirected to a different post. Open the selected post and copy its link again.".into())); }
    }
    if let Some(meta) = tiktok_metadata_from_html(&html, &page_url, headers) { return Ok(meta); }
    if merged_headers != *headers {
        if let Ok((resolved, retry)) = page(merged_headers.clone()).await {
            if let (Some(requested), Some(actual)) = (extract_tiktok_video_id(url, ""), extract_tiktok_video_id(&resolved, "")) {
                if requested != actual { return Err(Error::InvalidInput("TikTok redirected to a different post. Open the selected post and copy its link again.".into())); }
            }
            if let Some(meta) = tiktok_metadata_from_html(&retry, &resolved, &merged_headers) { return Ok(meta); }
            if html.is_empty() { page_url = resolved; html = retry; }
        }
    }
    let mut formats = Vec::new();
    let mut seen = Vec::new();
    let mut oembed_title = None;
    let mut oembed_thumb = None;
    let resolved_video_id = extract_tiktok_video_id(&page_url, &html);

    // ── Strategy 5: TikTok Aweme Mobile API ──────────────────────────────────
    if formats.is_empty() {
        if let Some(ref vid) = resolved_video_id {
            for api_host in ["api16-normal-c-useast1a.tiktokv.com", "api22-normal-c-useast1a.tiktokv.com"] {
                if !formats.is_empty() { break; }
                let api_url = format!("https://{api_host}/aweme/v1/feed/?aweme_id={vid}");
                let mobile_headers = vec![
                    ("User-Agent".to_string(), "TikTok 26.2.0 rv:262018 (iPhone; iOS 14.4.2; en_US)".to_string()),
                    ("Accept".to_string(), "application/json".to_string()),
                ];
                if let Ok(json_resp) = tiktok_fetch_text(client, &api_url, &mobile_headers).await {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_resp) {
                        if let Some(aweme) = super::tiktok::item(&v, resolved_video_id.as_deref()) {
                            if oembed_title.is_none() {
                                if let Some(desc) = aweme.get("desc").and_then(|s| s.as_str()) {
                                    if !desc.trim().is_empty() {
                                        oembed_title = Some(desc.trim().to_string());
                                    }
                                }
                            }
                            if oembed_thumb.is_none() {
                                if let Some(cover_arr) = aweme.pointer("/video/cover/url_list").and_then(|v| v.as_array()) {
                                    if let Some(first_cover) = cover_arr.first().and_then(|s| s.as_str()) {
                                        oembed_thumb = Some(first_cover.to_string());
                                    }
                                }
                            }
                            if let Some(video) = aweme.get("video") {
                                let mut api_found = Vec::new();
                                extract_video_obj_urls(video, &mut api_found);
                                for (i, u) in api_found.into_iter().enumerate() {
                                    if is_tiktok_video_cdn(&u) && !seen.contains(&u) {
                                        seen.push(u.clone());
                                        formats.push(mp4_format(&format!("tiktok-mobile-api-{i}"), u));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Strategy 6: TikTok Web Item Detail API ──────────────────────────────
    if formats.is_empty() {
        if let Some(ref vid) = resolved_video_id {
            let api_url = format!("https://www.tiktok.com/api/item/detail/?itemId={vid}");
            if let Ok(json_resp) = tiktok_fetch_text(client, &api_url, &merged_headers).await {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_resp) {
                    if let Some(item_struct) = super::tiktok::item(&v, resolved_video_id.as_deref()) {
                        if oembed_title.is_none() {
                            if let Some(desc) = item_struct.get("desc").and_then(|s| s.as_str()) {
                                if !desc.trim().is_empty() {
                                    oembed_title = Some(desc.trim().to_string());
                                }
                            }
                        }
                        if let Some(video) = item_struct.get("video") {
                            let mut api_found = Vec::new();
                            extract_video_obj_urls(video, &mut api_found);
                            for (i, u) in api_found.into_iter().enumerate() {
                                if is_tiktok_video_cdn(&u) && !seen.contains(&u) {
                                    seen.push(u.clone());
                                    formats.push(mp4_format(&format!("tiktok-web-api-{i}"), u));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Strategy 7: TikTok oEmbed API + Embed player page ─────────────────────
    if formats.is_empty() {
        let oembed_url = reqwest::Url::parse_with_params("https://www.tiktok.com/oembed", &[("url", url)])
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("https://www.tiktok.com/oembed?url={url}"));
        if let Ok(oembed_json) = tiktok_fetch_text(client, &oembed_url, &merged_headers).await {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&oembed_json) {
                if oembed_title.is_none() {
                    if let Some(t) = v.get("title").and_then(|s| s.as_str()) {
                        if !t.trim().is_empty() {
                            oembed_title = Some(t.trim().to_string());
                        }
                    }
                }
                if oembed_thumb.is_none() {
                    if let Some(img) = v.get("thumbnail_url").and_then(|s| s.as_str()) {
                        oembed_thumb = Some(img.to_string());
                    }
                }
            }
        }

        if let Some(video_id) = &resolved_video_id {
            for embed_url in [
                format!("https://www.tiktok.com/embed/v2/{video_id}"),
                format!("https://www.tiktok.com/embed/{video_id}"),
            ] {
                if !formats.is_empty() { break; }
                if let Ok(embed_html) = tiktok_fetch_text(client, &embed_url, &merged_headers).await {
                    if let Some(meta) = tiktok_metadata_from_html(&embed_html, url, &merged_headers) {
                        return Ok(meta);
                    }
                }
            }
        }
    }

    // ── Strategy 8: TikWM Public Resolution API Fallback ──────────────────────
    if formats.is_empty() {
        for (i, u) in og_videos(&html).into_iter().filter(|u| is_tiktok_video_cdn(u)).enumerate() {
            formats.push(mp4_format(&format!("tiktok-og-{i}"), u));
        }
    }
    if formats.is_empty() {
        let tikwm_url = reqwest::Url::parse_with_params("https://www.tikwm.com/api/", &[("url", url)])
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("https://www.tikwm.com/api/?url={url}"));
        if let Ok(tikwm_resp) = tiktok_fetch_text(client, &tikwm_url, &Headers::new()).await {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&tikwm_resp) {
                if v.get("code").and_then(|c| c.as_i64()) == Some(0) {
                    if let Some(data) = v.get("data") {
                        if oembed_title.is_none() {
                            if let Some(t) = data.get("title").and_then(|s| s.as_str()) {
                                if !t.trim().is_empty() {
                                    oembed_title = Some(t.trim().to_string());
                                }
                            }
                        }
                        if oembed_thumb.is_none() {
                            if let Some(img) = data.get("cover").and_then(|s| s.as_str()) {
                                oembed_thumb = Some(img.to_string());
                            }
                        }
                        for key in &["play", "hdplay", "wmplay"] {
                            if let Some(u) = data.get(key).and_then(|s| s.as_str()) {
                                let u = unescape_url(u);
                                if !u.is_empty() && !seen.contains(&u) {
                                    seen.push(u.clone());
                                    formats.push(mp4_format(&format!("tikwm-{key}"), u));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if formats.is_empty() {
        return Err(Error::InvalidInput(
            "no video found — TikTok may require a login session; try via the browser extension while logged in to TikTok".into(),
        ));
    }

    let title = oembed_title
        .or_else(|| {
            let t = page_title(&html, url);
            (!t.trim().is_empty()).then_some(t)
        })
        .unwrap_or_else(|| super::title::ensure_valid_title("", url));

    let thumbnail = oembed_thumb.or_else(|| page_thumbnail(&html));

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title,
        thumbnail,
        duration: None,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}


// ─── Instagram ────────────────────────────────────────────────────────────────
//
// Instagram embeds its video in the `__additionalDataLoaded` window callback
// or a `<script type="application/json">` data blob. Key fields:
//   - `video_url`         – direct mp4
//   - `video_dash_manifest` – DASH manifest string (inline XML)
//
// Anonymous requests to the ordinary `/reel/`, `/p/`, `/tv/` URL are
// frequently served a stripped-down "log in to continue" page with none of
// that JSON embedded — Instagram gates the full page to logged-in sessions
// far more aggressively than it used to. The `/embed/captioned/` variant of
// the same post is Instagram's own public oEmbed-style page: it renders a
// plain `<video src=…>` tag for any post that isn't otherwise private, no
// session required. We scrape the ordinary page first (in case the caller's
// cookies got us the real thing) and fall back to the embed page only if
// that comes up empty.

pub(crate) fn instagram_shortcode(url: &str) -> Option<(&'static str, &str)> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    for kind in ["reel", "reels", "p", "tv"] {
        if let Some(idx) = path.find(&format!("/{kind}/")) {
            let rest = &path[idx + kind.len() + 2..];
            let code = rest.split('/').next().unwrap_or("").trim();
            if !code.is_empty() && !matches!(code, "audio" | "liked" | "saved") {
                return Some((kind, code));
            }
        }
    }
    None
}

/// Shared by the Media and Video paths: JSON object order and CDN names are
/// irrelevant when the API explicitly identifies a field as a video version.
pub(crate) fn instagram_video_formats(node: &serde_json::Value) -> Vec<VideoFormat> {
    let mut formats = Vec::new();
    if let Some(manifest) = node.get("video_dash_manifest").and_then(|value| value.as_str()) {
        if let Ok(paired) = super::dash::formats_from_manifest(&unescape_url(manifest), "https://www.instagram.com/") {
            formats.extend(paired);
        }
    }
    if let Some(versions) = node.get("video_versions").and_then(|value| value.as_array()) {
        for version in versions {
            let Some(url) = version.get("url").and_then(|value| value.as_str()) else { continue };
            let Ok(parsed) = reqwest::Url::parse(url) else { continue };
            if !matches!(parsed.scheme(), "http" | "https") || super::direct::is_image_url(url) { continue; }
            if formats.iter().any(|format| format.video_url.as_deref() == Some(url)) { continue; }
            let width = version.get("width").and_then(|value| value.as_u64()).and_then(|value| u32::try_from(value).ok());
            let height = version.get("height").and_then(|value| value.as_u64()).and_then(|value| u32::try_from(value).ok());
            formats.push(VideoFormat {
                id: height.map(|height| format!("{height}p")).unwrap_or_else(|| "ig".into()),
                container: "mp4".into(), width, height, video_url: Some(url.to_string()),
                ..Default::default()
            });
        }
    }
    if formats.is_empty() {
        if let Some(url) = node.get("video_url").and_then(|value| value.as_str()) {
            if reqwest::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
                && !super::direct::is_image_url(url) {
                formats.push(VideoFormat { id: "ig".into(), video_url: Some(url.to_string()), ..Default::default() });
            }
        }
    }
    super::sort_formats(&mut formats);
    formats
}

pub(crate) fn extract_instagram_formats(html: &str, url: &str) -> Vec<VideoFormat> {
    // 0. The inlined DASH manifest, when there is one. Instagram serves reels
    //    the same way Facebook does — picture and sound as two separate files
    //    on cdninstagram, indistinguishable by URL — and the manifest is the
    //    only thing that says which is which. Taking one of those URLs for the
    //    whole video is what produced a clip with no sound.
    let target_id = instagram_shortcode(url).map(|(_, code)| code);
    let mut formats = inline_dash_formats(html, url, target_id);
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(html) {
        let post = json.pointer("/items/0").unwrap_or(&json);
        formats.extend(instagram_video_formats(post));
        // A parsed API response must not be scanned as arbitrary page text:
        // carousel neighbors and poster candidates are separate media items.
        super::sort_formats(&mut formats);
        return formats;
    }
    let mut seen: Vec<String> = formats.iter().filter_map(|f| f.video_url.clone()).collect();

    // 1. Extract video_versions with resolution details
    static IG_VIDEO_VERSIONS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?is)"height"\s*:\s*(\d+)[^}]*?"url"\s*:\s*"(https?:[\\/]+[^"]*?(?:cdninstagram\.com|cdninstagram\.net|fbcdn\.net)[^"]*?)""#
        ).unwrap()
    });

    for cap in IG_VIDEO_VERSIONS.captures_iter(html) {
        let height: Option<u32> = cap[1].parse().ok();
        let u = unescape_url(&cap[2]);
        if u.starts_with("http") && !u.contains("<MPD") && !seen.contains(&u) {
            seen.push(u.clone());
            let label = height.map(|h| format!("{h}p")).unwrap_or_else(|| format!("ig-{}", formats.len()));
            let mut fmt = mp4_format(&label, u);
            fmt.height = height;
            formats.push(fmt);
        }
    }

    // 2. Direct video_url or contentUrl fields
    static IG_DIRECT_URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#""(?:video_url|contentUrl)"\s*:\s*"(https?:[\\/]+[^"]*?(?:cdninstagram\.com|cdninstagram\.net|fbcdn\.net)[^"]*?\.mp4[^"]*?)""#
        ).unwrap()
    });

    for cap in IG_DIRECT_URL.captures_iter(html) {
        let u = unescape_url(&cap[1]);
        if u.starts_with("http") && !u.contains("<MPD") && !seen.contains(&u) {
            seen.push(u.clone());
            let label = format!("ig-{}", formats.len());
            formats.push(mp4_format(&label, u));
        }
    }

    // 3. The embed page's own `<video src=…>` tag
    static IG_VIDEO_TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<video[^>]*\bsrc=["']([^"']*(?:cdninstagram\.com|cdninstagram\.net|fbcdn\.net)[^"']*)["']"#).unwrap()
    });
    for cap in IG_VIDEO_TAG.captures_iter(html) {
        let u = unescape_url(&cap[1]);
        if u.starts_with("http") && !seen.contains(&u) {
            seen.push(u.clone());
            formats.push(mp4_format(&format!("ig-embed-{}", formats.len()), u));
        }
    }

    // 4. og:video meta tags fallback
    for (i, u) in og_videos(html).into_iter().enumerate() {
        if u.starts_with("http") && !u.contains("<MPD") && !seen.contains(&u) {
            seen.push(u.clone());
            formats.push(mp4_format(&format!("og-{i}"), u));
        }
    }

    // Instagram's JSON carries `image_versions2` cover frames in the exact
    // same `{"height":…,"url":…}` shape as `video_versions`, so the regexes
    // above happily match a poster `.jpg` (its params even say
    // `video_default_cover_frame`). Downloaded, that's a still image with an
    // .mp4 name that won't play — drop anything whose URL is an image.
    formats.retain(|f| f.video_url.as_deref().map(|u| !super::direct::is_image_url(u)).unwrap_or(false));

    // Sort formats so the highest quality with merges audio tag is listed first
    super::sort_formats(&mut formats);
    formats
}

/// Decode an Instagram shortcode (from `/p/<code>/`, `/reel/<code>/`, …) into
/// the numeric media id its JSON API is keyed by. Shortcodes are a big-endian
/// base-64 encoding of the media pk using the URL-safe alphabet.
pub(crate) fn shortcode_to_media_id(shortcode: &str) -> Option<u128> {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut id: u128 = 0;
    for &c in shortcode.as_bytes() {
        let pos = ALPHABET.iter().position(|&a| a == c)? as u128;
        id = id.checked_mul(64)?.checked_add(pos)?;
    }
    (id > 0).then_some(id)
}

async fn instagram(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    crate::instagram::resolve(client, url, headers).await?.into_video()
}

// ─── Twitter / X ─────────────────────────────────────────────────────────────
//
// Twitter stores its video behind a card API. What is reachable in plain HTML:
//   - `twitter:player:stream` meta tag (occasionally present on public posts)
//   - `.m3u8` URLs embedded in the page JS bundle

static RES_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\d{3,4})x(\d{3,4})"#).unwrap());

pub fn parse_resolution_from_url(url: &str) -> (String, Option<u32>, Option<u32>) {
    if let Some(cap) = RES_REGEX.captures(url) {
        if let (Ok(d1), Ok(d2)) = (cap[1].parse::<u32>(), cap[2].parse::<u32>()) {
            let h = std::cmp::min(d1, d2);
            let w = std::cmp::max(d1, d2);
            return (format!("{h}p"), Some(w), Some(h));
        }
    }
    ("mp4".to_string(), None, None)
}



/// Append a unique query parameter so this exact request cannot be answered
/// from a cache keyed on the literal URL string.
///
/// X's edge intermittently serves the bare `/status/<id>` URL from a shared
/// cache holding a stripped response with none of the `og:video`/
/// `twitter:player:stream` metadata this extractor needs — the `Cache-Control:
/// no-cache` header `fetch_text` sends asks a well-behaved cache not to do
/// that, but does not guarantee it. A cache keyed purely on the URL string
/// cannot serve a hit for a URL it has never seen, so every analyze attempt
/// gets a fresh response without the user having to edit the address bar.
fn cache_busted_url(url: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}_odm={}", chrono::Utc::now().timestamp_millis())
}

async fn fetch_twitter_formats(
    client: &Client,
    target_url: &str,
    headers: &Headers,
) -> (Option<String>, Option<String>, Vec<VideoFormat>) {
    let Ok(html) = fetch_text(client, &cache_busted_url(target_url), headers).await else {
        return (None, None, Vec::new());
    };

    let title = page_title(&html, target_url);
    let thumbnail = page_thumbnail(&html);
    let mut formats = Vec::new();

    // Meta tag stream
    for u in og_videos(&html) {
        if u.contains(".m3u8") {
            if let Ok(meta) = super::hls::analyze(client, &u, headers).await {
                formats.extend(meta.formats);
            } else {
                formats.push(mp4_format("stream", u));
            }
        } else {
            let (id, width, height) = parse_resolution_from_url(&u);
            formats.push(VideoFormat {
                validation: None,
                download_options: None,
                id,
                container: "mp4".to_string(),
                video_codec: None,
                audio_codec: None,
                width,
                height,
                fps: None,
                bitrate: None,
                filesize: None,
                video_url: Some(u),
                audio_url: None,
                has_audio: None,
                headers: Vec::new(),
                requires_merge: false,
            });
        }
    }

    // Embedded HLS in inline scripts
    static TWITTER_M3U8: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"https://[^"'\s]+video\.twimg\.com[^"'\s]*\.m3u8[^"'\s]*"#).unwrap()
    });
    for cap in TWITTER_M3U8.find_iter(&html) {
        let u = unescape_url(cap.as_str());
        if let Ok(meta) = super::hls::analyze(client, &u, headers).await {
            formats.extend(meta.formats);
        }
    }

    // Embedded progressive MP4s in inline scripts / JSON
    static TWITTER_MP4: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"https://[^"'\s\\]+video\.twimg\.com[^"'\s\\]*\.mp4[^"'\s\\]*"#).unwrap()
    });
    for cap in TWITTER_MP4.find_iter(&html) {
        let u = unescape_url(cap.as_str());
        let (id, width, height) = parse_resolution_from_url(&u);
        formats.push(VideoFormat {
            validation: None,
            download_options: None,
            id,
            container: "mp4".to_string(),
            video_codec: None,
            audio_codec: None,
            width,
            height,
            fps: None,
            bitrate: None,
            filesize: None,
            video_url: Some(u),
            audio_url: None,
            has_audio: None,
            headers: Vec::new(),
            requires_merge: false,
        });
    }

    (Some(title), thumbnail, formats)
}

/// Headers with any `Cookie` removed.
///
/// X serves its `og:video`/`twitter:player:stream`-bearing HTML — the exact
/// thing this extractor scrapes — to *anonymous* requests: those tags exist so
/// third-party link-unfurlers (Slack, Discord, iMessage) can render a preview
/// without a login. A request carrying the viewer's own session cookie instead
/// gets the ordinary client-rendered app shell, which has no video URL in the
/// raw HTML at all. So forwarding the browser's cookie here — generally the
/// right move for a login-gated page — actively defeats this one.
fn without_cookie(headers: &Headers) -> Headers {
    headers.iter().filter(|(name, _)| !name.eq_ignore_ascii_case("cookie")).cloned().collect()
}

/// The next representable f64 above `v` — V8 uses this to size the rounding
/// delta in its radix conversion, so reproducing it exactly matters below.
fn next_double(v: f64) -> f64 {
    f64::from_bits(v.to_bits() + 1)
}

/// `f64` → base-36 string, matching V8's `Number.prototype.toString(36)`.
///
/// X's syndication endpoint validates a `token` that is exactly this base-36
/// rendering of a value derived from the tweet id, so an approximate float
/// conversion is not good enough — the digit-by-digit rounding has to match.
/// This mirrors V8's `DoubleToRadixCString`.
fn double_to_base36(value: f64) -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let radix = 36.0_f64;
    let mut integer = value.trunc();
    let mut fraction = value - integer;
    let mut delta = (0.5 * (next_double(value) - value)).max(next_double(0.0));

    let mut frac_digits: Vec<usize> = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= radix;
            delta *= radix;
            let digit = fraction as usize;
            fraction -= digit as f64;
            frac_digits.push(digit);
            if (fraction > 0.5 || (fraction == 0.5 && (digit & 1) == 1)) && fraction + delta > 1.0 {
                // Round up, carrying back through the digits and into the
                // integer part if the whole fraction rolls over.
                loop {
                    let last = frac_digits.len() - 1;
                    if frac_digits[last] + 1 < 36 {
                        frac_digits[last] += 1;
                        break;
                    }
                    frac_digits.pop();
                    if frac_digits.is_empty() {
                        integer += 1.0;
                        break;
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }

    let mut int_part = integer as u64;
    let int_str = if int_part == 0 {
        "0".to_string()
    } else {
        let mut buf = Vec::new();
        while int_part > 0 {
            buf.push(CHARS[(int_part % 36) as usize]);
            int_part /= 36;
        }
        buf.reverse();
        String::from_utf8(buf).unwrap()
    };

    if frac_digits.is_empty() {
        int_str
    } else {
        let frac: String = frac_digits.iter().map(|&d| CHARS[d] as char).collect();
        format!("{int_str}.{frac}")
    }
}

static STATUS_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/status/(\d+)").unwrap());

/// The 1-based media index in an X permalink (`/status/123/video/2`,
/// `/status/123/photo/1`), if it carries one.
///
/// A tweet can hold up to four media items and the permalink is the only thing
/// that says which one the user was looking at. Without it, a thread or a
/// multi-video tweet resolved to *every* clip at once and the "best" of that
/// pile — a different video from the one whose download button was clicked.
static MEDIA_INDEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/status/\d+/(?:video|photo)/(\d+)").unwrap());

pub(crate) fn media_index(url: &str) -> Option<usize> {
    let n: usize = MEDIA_INDEX.captures(url)?.get(1)?.as_str().parse().ok()?;
    n.checked_sub(1)
}

pub(crate) fn tweet_id(url: &str) -> Option<String> {
    STATUS_ID.captures(url).map(|c| c[1].to_string())
}

/// The `token` X's syndication endpoint derives from the tweet id:
/// `((id / 1e15) * PI).toString(36)` with every `0` and `.` stripped — the
/// same formula the official embed widget (`react-tweet`) uses.
pub(crate) fn syndication_token(id: &str) -> Option<String> {
    let id_num: f64 = id.parse().ok()?;
    let value = (id_num / 1e15) * std::f64::consts::PI;
    Some(double_to_base36(value).chars().filter(|&c| c != '0' && c != '.').collect())
}

/// Public embed JSON for a tweet — the reliable path to X video now that the
/// tweet-page HTML no longer carries `og:video`/`twitter:player:stream` for
/// anonymous *or* logged-in fetches. `cdn.syndication.twimg.com/tweet-result`
/// is the same endpoint the official embed widget hits, so it needs no login,
/// only the derived token.
async fn twitter_syndication(
    client: &Client,
    url: &str,
) -> (Option<String>, Option<String>, Vec<VideoFormat>) {
    let Some(id) = tweet_id(url) else {
        return (None, None, Vec::new());
    };
    let Some(token) = syndication_token(&id) else {
        return (None, None, Vec::new());
    };
    let api = format!("https://cdn.syndication.twimg.com/tweet-result?id={id}&token={token}&lang=en");

    // Anonymous on purpose: this is a public embed endpoint, and a session
    // cookie only risks a redirect to a login/interstitial page.
    let Ok(body) = fetch_text(client, &api, &Vec::new()).await else {
        return (None, None, Vec::new());
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return (None, None, Vec::new());
    };

    let title = json
        .get("text")
        .and_then(|t| t.as_str())
        .or_else(|| json.pointer("/user/name").and_then(|n| n.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut thumbnail = json
        .pointer("/mediaDetails/0/media_url_https")
        .and_then(|t| t.as_str())
        .map(String::from);

    let mut formats = Vec::new();
    if let Some(details) = json.get("mediaDetails").and_then(|m| m.as_array()) {
        // Exactly one media item, never the whole tweet. The permalink's own
        // `/video/N` index picks it when present; otherwise the first video in
        // the tweet is the one the overlay was sitting on.
        let is_video = |m: &&serde_json::Value| {
            let kind = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
            kind == "video" || kind == "animated_gif"
        };
        let chosen = media_index(url)
            .and_then(|i| details.get(i))
            .filter(|m| is_video(m))
            .or_else(|| details.iter().find(is_video));

        if let Some(poster) = chosen.and_then(|m| m.get("media_url_https")).and_then(|t| t.as_str()) {
            thumbnail = Some(poster.to_string());
        }

        if let Some(variants) = chosen
            .and_then(|media| media.pointer("/video_info/variants"))
            .and_then(|v| v.as_array())
        {
            for variant in variants {
                let Some(vurl) = variant.get("url").and_then(|u| u.as_str()) else {
                    continue;
                };
                let content_type = variant.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
                if content_type.contains("mpegurl") || vurl.contains(".m3u8") {
                    if let Ok(meta) = super::hls::analyze(client, vurl, &Vec::new()).await {
                        formats.extend(meta.formats);
                    }
                } else {
                    let (fid, width, height) = parse_resolution_from_url(vurl);
                    formats.push(VideoFormat {
                        validation: None,
                        download_options: None,
                        id: fid,
                        container: "mp4".to_string(),
                        video_codec: None,
                        audio_codec: None,
                        width,
                        height,
                        fps: None,
                        bitrate: variant.get("bitrate").and_then(|b| b.as_u64()),
                        filesize: None,
                        video_url: Some(vurl.to_string()),
                        audio_url: None,
                        has_audio: None,
                        headers: Vec::new(),
                        requires_merge: false,
                    });
                }
            }
        }
    }

    (title, thumbnail, formats)
}

async fn twitter(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    // The syndication embed API is the reliable source now; the HTML scrape is
    // kept only as a legacy fallback for the rare tweet it doesn't cover.
    let (mut title, mut thumbnail, mut formats) = twitter_syndication(client, url).await;

    if formats.is_empty() {
        // Anonymous first: the crawler-preview HTML (when it exists at all) is
        // served to logged-out requests. Retry with the cookie only if that is
        // empty — a protected/followers-only tweet genuinely needs the session.
        let anonymous = without_cookie(headers);
        let (t, th, f) = fetch_twitter_formats(client, url, &anonymous).await;
        title = title.or(t);
        thumbnail = thumbnail.or(th);
        formats = f;
        if formats.is_empty() && anonymous.len() != headers.len() {
            let (t2, th2, f2) = fetch_twitter_formats(client, url, headers).await;
            title = title.or(t2);
            thumbnail = thumbnail.or(th2);
            formats = f2;
        }
    }
    let title = title.unwrap_or_else(|| title_from_url(url));

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

    if formats.is_empty() {
        return Err(Error::InvalidInput(
            "no video found — Twitter/X requires login for most videos; try via the browser extension".into(),
        ));
    }

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title,
        thumbnail,
        duration: None,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

// ─── Reddit ───────────────────────────────────────────────────────────────────
//
// Reddit's video player (RPAN/v.redd.it) uses HLS. The manifest URL is usually
// found in `og:video:secure_url` or as a `fallbackUrl` / `hlsUrl` JSON key.

async fn reddit(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    // v.redd.it direct links already carry the HLS manifest in the redirect
    let canonical = if host_of(url).contains("v.redd.it") {
        format!("{url}HLSPlaylist.m3u8")
    } else {
        url.to_string()
    };

    // Try HLS manifest path directly first for v.redd.it
    if canonical.ends_with(".m3u8") {
        if let Ok(meta) = super::hls::analyze(client, &canonical, headers).await {
            return Ok(VideoMetadata {
                subtitles: Vec::new(),
                title: title_from_url(url),
                thumbnail: None,
                duration: None,
                uploader: None,
                webpage_url: url.to_string(),
                formats: meta.formats,
            });
        }
    }

    let html = fetch_text(client, url, headers).await?;

    let mut formats = Vec::new();

    // JSON keys embedded in Reddit's page data
    let keys = ["hlsUrl", "fallbackUrl", "hlsPlaylist", "dashUrl"];
    for u in json_values(&html, &keys) {
        if u.contains(".m3u8") {
            if let Ok(meta) = super::hls::analyze(client, &u, headers).await {
                formats.extend(meta.formats);
            }
        } else if u.contains(".mpd") {
            if let Ok(meta) = super::dash::analyze(client, &u, headers).await {
                formats.extend(meta.formats);
            }
        } else {
            formats.push(mp4_format("reddit", u));
        }
    }

    // OG video
    for u in og_videos(&html) {
        formats.push(mp4_format("og", u));
    }

    if formats.is_empty() {
        return Err(Error::InvalidInput(format!("no video found on {url}")));
    }

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title: page_title(&html, url),
        thumbnail: page_thumbnail(&html),
        duration: None,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

// ─── Vimeo ────────────────────────────────────────────────────────────────────
//
// Vimeo embeds a `config` JSON object in the page with progressive and HLS
// stream entries under `request.files`.

async fn vimeo(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let html = fetch_text(client, url, headers).await?;

    let mut formats = Vec::new();

    // HLS master playlist from vimeo player config
    let hls_keys = ["hls", "master", "avc_url", "url"];
    for u in json_values(&html, &hls_keys) {
        if u.contains("vimeo") {
            if u.contains(".m3u8") {
                if let Ok(meta) = super::hls::analyze(client, &u, headers).await {
                    formats.extend(meta.formats);
                }
            } else if u.contains(".mp4") {
                formats.push(mp4_format("vimeo", u));
            }
        }
    }

    for u in og_videos(&html) {
        formats.push(mp4_format("og", u));
    }

    if formats.is_empty() {
        return Err(Error::InvalidInput(format!("no video found on {url}")));
    }

    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title: page_title(&html, url),
        thumbnail: page_thumbnail(&html),
        duration: None,
        uploader: None,
        webpage_url: url.to_string(),
        formats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instagram_api_video_versions_do_not_depend_on_field_order_or_cdn_name() {
        let body = r#"{"items":[{"video_versions":[
            {"url":"https://video.example/clip.mp4?token=signed","width":720,"height":1280},
            {"width":480,"url":"https://video.example/low.mp4","height":854}
        ],"image_versions2":{"candidates":[{"height":1920,"url":"https://video.example/poster.jpg"}]}}]}"#;
        let formats = extract_instagram_formats(body, "https://www.instagram.com/reel/ABC123/");
        assert_eq!(formats.len(), 2);
        assert_eq!(formats[0].height, Some(1280));
        assert_eq!(formats[0].width, Some(720));
        assert_eq!(formats[0].video_url.as_deref(), Some("https://video.example/clip.mp4?token=signed"));
        assert!(formats.iter().all(|format| format.has_audio.is_none()));
    }

    #[test]
    fn instagram_json_video_url_is_supported_but_posters_and_audio_pages_are_not() {
        let formats = extract_instagram_formats(r#"{"video_url":"https://cdn.example/clip.mp4?token=signed"}"#,
            "https://www.instagram.com/reel/ABC123/");
        assert_eq!(formats.len(), 1);
        let posters = extract_instagram_formats(r#"{"image_versions2":{"candidates":[{"height":1080,"url":"https://cdn.example/poster.jpg"}]}}"#,
            "https://www.instagram.com/reel/ABC123/");
        assert!(posters.is_empty());
        assert_eq!(instagram_shortcode("https://www.instagram.com/reels/audio/123/"), None);
    }

    #[test]
    fn shared_instagram_parser_pairs_only_the_selected_nodes_dash_audio() {
        let manifest = json_string_values(META_PAGE, &["dash_manifest"]).pop().unwrap();
        let node = serde_json::json!({"video_dash_manifest":manifest});
        let formats = instagram_video_formats(&node);
        assert!(!formats.is_empty());
        assert!(formats[0].requires_merge);
        assert!(formats[0].audio_url.as_ref().unwrap().contains("audio.mp4"));
    }

    #[test]
    fn instagram_shortcode_reads_reel_p_reels_and_tv_paths() {
        assert_eq!(
            instagram_shortcode("https://www.instagram.com/reel/DbCY8-aPl1a/?utm_source=ig_web_copy_link"),
            Some(("reel", "DbCY8-aPl1a"))
        );
        assert_eq!(instagram_shortcode("https://www.instagram.com/p/ABC123/"), Some(("p", "ABC123")));
        assert_eq!(instagram_shortcode("https://www.instagram.com/reels/XYZ789/"), Some(("reels", "XYZ789")));
        assert_eq!(instagram_shortcode("https://www.instagram.com/tv/QWE456/"), Some(("tv", "QWE456")));
        assert_eq!(instagram_shortcode("https://www.instagram.com/some.user/"), None);
    }

    #[test]
    fn shortcode_to_media_id_decodes_base64_alphabet() {
        // Base-64 positional math: 'A'=0, 'B'=1, so "B" -> 1 and "BA" -> 64.
        assert_eq!(shortcode_to_media_id("B"), Some(1));
        assert_eq!(shortcode_to_media_id("BA"), Some(64));
        assert_eq!(shortcode_to_media_id("Ba"), Some(64 + 26));
        // An all-'A' code decodes to 0, which is not a real media id.
        assert_eq!(shortcode_to_media_id("A"), None);
    }

    #[test]
    fn tweet_id_reads_the_status_id() {
        assert_eq!(tweet_id("https://x.com/Baniy01/status/2080181230083940715").as_deref(), Some("2080181230083940715"));
        assert_eq!(tweet_id("https://twitter.com/u/status/123?s=20").as_deref(), Some("123"));
        assert_eq!(tweet_id("https://x.com/u"), None);
    }

    #[test]
    fn syndication_token_is_deterministic_and_stripped() {
        // The exact value must match V8's Number.toString(36); here we lock in
        // that it's stable, non-empty, and contains no `0` or `.` (the two
        // characters the derivation strips).
        let a = syndication_token("2080181230083940715").unwrap();
        let b = syndication_token("2080181230083940715").unwrap();
        assert_eq!(a, b);
        assert!(!a.is_empty());
        assert!(!a.contains('0') && !a.contains('.'));
    }

    #[test]
    fn double_to_base36_matches_v8_for_known_values() {
        // Whole numbers and simple fractions, cross-checked against
        // Number.prototype.toString(36) in a JS engine.
        assert_eq!(double_to_base36(0.0), "0");
        assert_eq!(double_to_base36(35.0), "z");
        assert_eq!(double_to_base36(36.0), "10");
        assert_eq!(double_to_base36(0.5), "0.i"); // 0.5 * 36 = 18 = 'i'
    }

    #[test]
    fn without_cookie_drops_only_the_cookie_header_case_insensitively() {
        let headers = vec![
            ("Cookie".to_string(), "auth_token=abc".to_string()),
            ("Referer".to_string(), "https://x.com/".to_string()),
            ("cookie".to_string(), "also_dropped=1".to_string()),
        ];
        let out = without_cookie(&headers);
        assert_eq!(out, vec![("Referer".to_string(), "https://x.com/".to_string())]);
    }

    #[test]
    fn cache_busted_url_appends_a_unique_param_with_the_right_separator() {
        let a = cache_busted_url("https://x.com/user/status/123");
        assert!(a.starts_with("https://x.com/user/status/123?_odm="));

        let b = cache_busted_url("https://x.com/user/status/123?foo=bar");
        assert!(b.starts_with("https://x.com/user/status/123?foo=bar&_odm="));
    }

    #[test]
    fn tiktok_video_id_extraction() {
        assert_eq!(
            extract_tiktok_video_id("https://www.tiktok.com/@user/video/7123456789012345678", ""),
            Some("7123456789012345678".to_string())
        );
        assert_eq!(
            extract_tiktok_video_id("https://www.tiktok.com/?itemId=7123456789012345678", ""),
            Some("7123456789012345678".to_string())
        );
        let html = r#"<meta property="og:url" content="https://www.tiktok.com/@test/video/7999888777666555444">"#;
        assert_eq!(
            extract_tiktok_video_id("https://vt.tiktok.com/ZSN12345/", html),
            Some("7999888777666555444".to_string())
        );
    }


    #[test]
    fn tiktok_cdn_url_detection() {
        assert!(is_tiktok_video_cdn("https://v16-webapp-prime.tiktok.com/video/tos/12345.mp4"));
        assert!(is_tiktok_video_cdn("https://v16m.tiktokcdn.com/stream/abc.mp4"));
        assert!(is_tiktok_video_cdn("https://www.tikwm.com/video/media/123.mp4"));
        assert!(is_tiktok_video_cdn("https://v19.tiktokv.com/aweme/v1/play/?video_id=123"));
        assert!(!is_tiktok_video_cdn("https://p16-sign.tiktokcdn.com/tos-useast2a-avt-0068/avatar.jpg"));
    }






    #[test]
    fn tiktok_urls_from_json_rehydration() {
        let json = r#"{
            "__DEFAULT_SCOPE__": {
                "webapp.video-detail": {
                    "itemInfo": {
                        "itemStruct": {
                            "video": {
                                "playAddr": "https://v16-webapp.tiktokcdn.com/play.mp4",
                                "downloadAddr": "https://v16-webapp.tiktokcdn.com/download.mp4",
                                "bitrateInfo": [
                                    {
                                        "PlayAddr": {
                                            "UrlList": ["https://v19-web.tiktokcdn.com/hd.mp4"]
                                        }
                                    }
                                ]
                            }
                        }
                    }
                }
            }
        }"#;
        let urls = tiktok_urls_from_json(json);
        assert!(urls.contains(&"https://v16-webapp.tiktokcdn.com/play.mp4".to_string()));
        assert!(urls.contains(&"https://v16-webapp.tiktokcdn.com/download.mp4".to_string()));
        assert!(urls.contains(&"https://v19-web.tiktokcdn.com/hd.mp4".to_string()));
    }

    #[tokio::test]
    #[ignore]
    async fn test_youtube_shorts() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap();
        let headers = crate::headers::Headers::default();
        let res = analyze(&client, "https://www.youtube.com/shorts/h49uAuUWHRY", &headers, Platform::YouTube).await;
        assert!(res.is_ok());
        let meta = res.unwrap();
        assert!(!meta.formats.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_facebook_reel() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap();
        let headers = crate::headers::Headers::default();
        let res = analyze(&client, "https://www.facebook.com/reel/1030025359507910", &headers, Platform::Facebook).await;
        assert!(res.is_ok());
        let meta = res.unwrap();
        assert!(!meta.formats.is_empty());
        assert_eq!(meta.formats[0].container, "mp4");
    }
    // ─── Meta (Facebook / Instagram) split tracks ───────────────────────────

    /// A DASH manifest the way Meta ships it: inside a JSON string, so every
    /// quote is escaped and the newlines are `\n` rather than real ones.
    const META_PAGE: &str = concat!(
        r#"<html><head><meta property="og:title" content="A clip"></head><body>"#,
        r#"<script>window.__d=({"video":{"id":"123","#,
        r#""dash_manifest":"#,
        r#""<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT12S\">"#,
        r#"<Period><AdaptationSet contentType=\"video\">"#,
        r#"<Representation id=\"v1\" mimeType=\"video/mp4\" bandwidth=\"3000000\" width=\"1920\" height=\"1080\" codecs=\"avc1.640028\">"#,
        r#"<BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/1080.mp4?oh=a</BaseURL>"#,
        r#"<SegmentBase indexRange=\"881-1200\"><Initialization range=\"0-880\"/></SegmentBase>"#,
        r#"</Representation></AdaptationSet>"#,
        r#"<AdaptationSet contentType=\"audio\">"#,
        r#"<Representation id=\"a0\" mimeType=\"audio/mp4\" bandwidth=\"128000\" codecs=\"mp4a.40.5\">"#,
        r#"<BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/audio.mp4?oh=b</BaseURL>"#,
        r#"<SegmentBase indexRange=\"700-900\"><Initialization range=\"0-699\"/></SegmentBase>"#,
        r#"</Representation></AdaptationSet></Period></MPD>","#,
        r#""playable_url":"https:\/\/video.xx.fbcdn.net\/v\/t2\/f2\/m69\/sd.mp4?oh=c","#,
        r#""playable_url_quality_hd":"https:\/\/video.xx.fbcdn.net\/v\/t2\/f2\/m69\/hd.mp4?oh=d""#,
        r#"}});</script></body></html>"#,
    );

    #[test]
    fn json_string_values_decodes_a_whole_escaped_manifest() {
        let found = json_string_values(META_PAGE, &["dash_manifest"]);
        assert_eq!(found.len(), 1);
        let xml = &found[0];
        // The value is the entire document, not the handful of characters a
        // regex stopping at the first quote would have returned.
        assert!(xml.starts_with("<?xml"));
        assert!(xml.ends_with("</MPD>"));
        assert!(xml.contains(r#"mimeType="audio/mp4""#), "escaped quotes were decoded");
    }

    #[test]
    fn json_string_values_ignores_keys_whose_value_is_not_a_string() {
        let text = r#"{"dash_manifest":null,"manifest_xml":{"nested":1},"video_dash_manifest":"<MPD/>"}"#;
        assert_eq!(
            json_string_values(text, &["dash_manifest", "manifest_xml", "video_dash_manifest"]),
            vec!["<MPD/>".to_string()],
        );
    }

    #[test]
    fn facebook_pairs_the_manifests_video_and_audio_tracks() {
        let formats = facebook_formats(META_PAGE, "https://www.facebook.com/watch/?v=123");

        let best = &formats[0];
        assert_eq!(best.height, Some(1080));
        assert!(best.requires_merge, "the 1080p track is picture only until muxed");
        assert_eq!(
            best.audio_url.as_deref(),
            Some("https://video.xx.fbcdn.net/v/t2/f2/m69/audio.mp4?oh=b"),
            "and it knows where its sound is",
        );
        assert_eq!(
            best.video_url.as_deref(),
            Some("https://video.xx.fbcdn.net/v/t2/f2/m69/1080.mp4?oh=a"),
        );
        assert!(best.plays_with_audio());
    }

    #[test]
    fn facebook_post_video_metadata_preserves_pairs_and_separates_clips() {
        use crate::image::{ImageAlbum, ImageItem};
        let post = "https://www.facebook.com/user/posts/pfbidSelected";
        let paired = facebook_formats(META_PAGE, "https://www.facebook.com/watch/?v=123");
        let clip = ImageItem::video("facebook:123", paired.clone()).unwrap()
            .with_thumbnail(Some("https://cdn.example/poster.jpg".into()));
        let album = ImageAlbum { title: "Selected post".into(), completeness: Some(crate::image::Completeness::Complete), items: vec![
            ImageItem::new("facebook:photo", "https://cdn.example/photo.jpg"), clip.clone()], ..Default::default() };
        let one = facebook_post_metadata(album.clone(), post).unwrap();
        assert_eq!(one.webpage_url, post);
        assert_eq!(one.formats.len(), paired.len());
        assert!(!one.formats[0].id.starts_with("clip-"));
        assert!(one.formats[0].requires_merge);
        assert_eq!(one.formats[0].audio_url, paired[0].audio_url);
        assert_eq!(one.formats[0].download_options.as_ref().unwrap().thumbnail, clip.thumbnail);
        let mut partial = album.clone();
        partial.completeness = Some(crate::image::Completeness::Partial);
        assert!(facebook_post_metadata(partial, post).unwrap().formats[0].id.starts_with("clip-1-"));
        let mut two = album;
        two.items.push(clip);
        let multiple = facebook_post_metadata(two, post).unwrap();
        assert_eq!(multiple.formats.len(), paired.len() * 2);
        assert!(multiple.formats[..paired.len()].iter().all(|f| f.id.starts_with("clip-1-")));
        assert!(multiple.formats[paired.len()..].iter().all(|f| f.id.starts_with("clip-2-")));
        assert!(facebook_post_metadata(ImageAlbum::default(), post).is_err());
    }

    #[test]
    fn facebook_share_tokens_are_not_numeric_video_ids() {
        assert!(facebook_is_post("https://www.facebook.com/share/p/1DmzsRpa8N/"));
        assert!(facebook_is_post("https://www.facebook.com/user/posts/pfbidSelected"));
        assert!(facebook_is_post("https://www.facebook.com/permalink.php?story_fbid=123&id=456"));
        assert!(!facebook_is_post("https://www.facebook.com/watch/?v=123"));
        assert_eq!(facebook_video_id("https://www.facebook.com/share/v/123AbCd/"), None);
        assert_eq!(facebook_video_id("https://www.facebook.com/videos/123456/"), Some("123456"));
    }

    #[test]
    fn facebook_keeps_the_progressive_files_as_fallbacks() {
        let formats = facebook_formats(META_PAGE, "https://www.facebook.com/watch/?v=123");
        let urls: Vec<&str> = formats.iter().filter_map(|f| f.video_url.as_deref()).collect();

        assert!(urls.contains(&"https://video.xx.fbcdn.net/v/t2/f2/m69/hd.mp4?oh=d"));
        assert!(urls.contains(&"https://video.xx.fbcdn.net/v/t2/f2/m69/sd.mp4?oh=c"));
        // HD before SD: the key order in FB_PROGRESSIVE_KEYS is the ranking,
        // since a progressive URL carries no resolution of its own.
        let hd = urls.iter().position(|u| u.contains("hd.mp4")).unwrap();
        let sd = urls.iter().position(|u| u.contains("sd.mp4")).unwrap();
        assert!(hd < sd);
    }

    #[test]
    fn facebook_never_offers_a_bare_audio_track_as_the_video() {
        let formats = facebook_formats(META_PAGE, "https://www.facebook.com/watch/?v=123");
        for f in &formats {
            assert_ne!(
                f.video_url.as_deref(),
                Some("https://video.xx.fbcdn.net/v/t2/f2/m69/audio.mp4?oh=b"),
                "the audio representation is a track, never a format",
            );
        }
    }

    #[test]
    fn instagram_reads_its_inline_dash_manifest() {
        let page = META_PAGE.replace("dash_manifest", "video_dash_manifest");
        let formats = extract_instagram_formats(&page, "https://www.instagram.com/reel/Cabc123/");

        assert!(!formats.is_empty());
        let best = &formats[0];
        assert_eq!(best.height, Some(1080));
        assert!(best.requires_merge, "an Instagram reel's picture and sound are separate files");
        assert!(best.audio_url.as_deref().unwrap().contains("audio.mp4"));
    }

    #[test]
    fn facebook_duration_parses_manifest_and_json_durations() {
        // 1. From MPD manifest mediaPresentationDuration
        let d = facebook_duration(META_PAGE, Some("123"));
        assert_eq!(d, Some(12.0));

        // 2. From playable_duration_in_ms
        let json_ms = r#"{"video":{"playable_duration_in_ms":24500}}"#;
        assert_eq!(facebook_duration(json_ms, None), Some(24.5));

        // 3. From playable_duration (seconds)
        let json_sec = r#"{"video":{"playable_duration":42.3}}"#;
        assert_eq!(facebook_duration(json_sec, None), Some(42.3));
    }

    #[test]
    fn facebook_does_not_mix_preloaded_video_formats() {
        let multi_page = concat!(
            r#"<html><body>"#,
            r#"<script>window.__d={"video":{"id":"1001","#,
            r#""dash_manifest":"<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT10S\">"#,
            r#"<Period><AdaptationSet contentType=\"video\">"#,
            r#"<Representation id=\"v1\" mimeType=\"video/mp4\" bandwidth=\"1500000\" width=\"1280\" height=\"720\" codecs=\"av01.0.08M.08\">"#,
            r#"<BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/target_video.mp4?oh=a</BaseURL>"#,
            r#"<SegmentBase indexRange=\"800-1200\"><Initialization range=\"0-799\"/></SegmentBase>"#,
            r#"</Representation></AdaptationSet></Period></MPD>"#,
            r#""}};</script>"#,
            r#"<script>window.__d={"video":{"id":"2002","#,
            r#""dash_manifest":"<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT25S\">"#,
            r#"<Period><AdaptationSet contentType=\"video\">"#,
            r#"<Representation id=\"v2\" mimeType=\"video/mp4\" bandwidth=\"3000000\" width=\"1920\" height=\"1080\" codecs=\"av01.0.08M.08.0.111\">"#,
            r#"<BaseURL>https://video.xx.fbcdn.net/v/t2/f2/m69/preloaded_video.mp4?oh=b</BaseURL>"#,
            r#"<SegmentBase indexRange=\"800-1200\"><Initialization range=\"0-799\"/></SegmentBase>"#,
            r#"</Representation></AdaptationSet></Period></MPD>"#,
            r#""}};</script>"#,
            r#"</body></html>"#,
        );

        // When requesting reel 1001, only target_video (av01.0.08M.08) should be returned
        let formats_1001 = facebook_formats(multi_page, "https://www.facebook.com/reel/1001");
        assert_eq!(formats_1001.len(), 1);
        assert_eq!(formats_1001[0].video_codec.as_deref(), Some("av01.0.08M.08"));
        assert!(formats_1001[0].video_url.as_deref().unwrap().contains("target_video.mp4"));

        // Duration for reel 1001 should be 10.0s, NOT the preloaded 25.0s
        assert_eq!(facebook_duration(multi_page, Some("1001")), Some(10.0));

        // When requesting reel 2002, only preloaded_video (av01.0.08M.08.0.111) should be returned
        let formats_2002 = facebook_formats(multi_page, "https://www.facebook.com/reel/2002");
        assert_eq!(formats_2002.len(), 1);
        assert_eq!(formats_2002[0].video_codec.as_deref(), Some("av01.0.08M.08.0.111"));
        assert!(formats_2002[0].video_url.as_deref().unwrap().contains("preloaded_video.mp4"));
        assert_eq!(facebook_duration(multi_page, Some("2002")), Some(25.0));
    }

    // ─── X / Twitter media selection ────────────────────────────────────────

    #[test]
    fn media_index_reads_the_item_the_permalink_points_at() {
        assert_eq!(media_index("https://x.com/someone/status/1234567890/video/1"), Some(0));
        assert_eq!(media_index("https://x.com/someone/status/1234567890/video/3"), Some(2));
        assert_eq!(media_index("https://x.com/someone/status/1234567890/photo/2"), Some(1));
        // A bare permalink says nothing about which media — the first video wins.
        assert_eq!(media_index("https://x.com/someone/status/1234567890"), None);
        assert_eq!(media_index("https://x.com/home"), None);
    }

    #[test]
    fn a_tweets_status_id_survives_a_media_suffix() {
        // `tweet_id` still has to find the id once the index is kept on the URL.
        assert_eq!(
            tweet_id("https://x.com/someone/status/1234567890/video/2").as_deref(),
            Some("1234567890"),
        );
    }
}
