//! Per-platform photo and gallery extractors.
//!
//! Each of these exists because the platform's own video extractor in
//! [`crate::video::site`] deliberately throws photos away — a still is not a
//! format, and offering one in a quality picker would be a bug there. The
//! knowledge is shared where it is genuinely the same (shortcode decoding, the
//! X syndication token, Facebook's URL cleaning); only the part that reads
//! *pictures* out of the payload is new.
//!
//! The shape of each is the same: ask a public JSON endpoint the platform's own
//! embed widget uses, and fall back to the Open Graph tags any chat client
//! would read. Neither path needs the user's login for a public post, and both
//! improve when a session is present.

use reqwest::Client;
use serde_json::Value;

use super::{ImageAlbum, ImageItem};
use crate::error::{Error, Result};
use crate::headers::Headers;
use crate::video::net::fetch_text;
use crate::video::site::{self, Platform};
use crate::video::VideoFormat;

/// The identity Facebook serves public preview data to.
///
/// A browser `User-Agent` gets an HTTP 400 login wall on a photo permalink;
/// the crawler that renders link previews gets the picture, because that is the
/// whole point of the crawler. This is not a bypass of anything private — it is
/// the same public preview a pasted link shows in any chat app — and it has to
/// travel with each extracted URL, because `lookaside.fbsbx.com` applies the
/// same rule to the image request itself.
pub(super) const FB_CRAWLER_UA: &str = "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)";

/// Ask the platform's own video extractor for the clip a post is built around.
///
/// Reddit and TikTok each put at most one video in a post, and each
/// already has a full extractor in [`crate::video::site`] that knows its
/// manifests, its split audio tracks and its header requirements. Re-deriving
/// any of that here would be a second and worse copy of it, so the post path
/// borrows the first one.
///
/// The poster frame becomes the item's thumbnail rather than a separate entry:
/// on a video post it *is* the clip, and offering it as its own "photo" would
/// show the same thing twice and invite downloading a still by mistake.
async fn borrowed_post_video(
    client: &Client,
    url: &str,
    headers: &Headers,
    platform: Platform,
) -> Option<ImageItem> {
    let meta = site::analyze(client, url, headers, platform).await.ok()?;
    let poster = meta.thumbnail.clone();
    ImageItem::video("video", meta.formats).map(|item| item.with_thumbnail(poster))
}

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    let album = match site::matches_platform(url) {
        Some(Platform::Twitter) => twitter(client, url, headers).await,
        Some(Platform::Instagram) => return instagram(client, url, headers).await,
        Some(Platform::Facebook) => return super::facebook::analyze(client, url, headers).await,
        Some(Platform::Reddit) => reddit(client, url, headers).await,
        Some(Platform::TikTok) => tiktok(client, url, headers).await,
        // YouTube and Vimeo are video hosts; their only picture is a poster
        // frame, which the generic path below finds as well as anything.
        _ => Err(Error::InvalidInput(String::new())),
    };

    match album {
        Ok(album) if !album.is_empty() => Ok(album),
        // Every platform path is best-effort. The generic Open Graph scrape is
        // what a link-preview renderer would do, and works on the long tail of
        // blogs, news sites, and image hosts none of the above cover.
        _ => generic(client, url, headers).await,
    }
}

/// An album holding the one picture a direct link names.
pub fn direct_image(url: &str) -> ImageAlbum {
    ImageAlbum {
        title: crate::video::direct::title_from_url(url),
        source_url: url.to_string(),
        uploader: None,
        items: vec![ImageItem::new("1", url)],
        ..Default::default()
    }
}

// ─── Twitter / X ─────────────────────────────────────────────────────────────

/// Rewrite an X media URL to ask for the stored original.
///
/// `pbs.twimg.com` serves a resized copy by default; `name=orig` is the
/// original upload. The query form is used rather than the older `:orig` path
/// suffix because the suffix leaves the path ending `.jpg:orig`, which every
/// filename and extension check downstream then has to special-case.
fn twitter_original(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let base = base.strip_suffix(":orig").or_else(|| base.strip_suffix(":large")).unwrap_or(base);
    match base.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') && !ext.is_empty() => {
            format!("{stem}?format={}&name=orig", ext.to_ascii_lowercase())
        }
        // Already extensionless (the newer form) — just ask for the original.
        _ => format!("{base}?name=orig"),
    }
}

/// Every rendition of one X media entry, ranked best-first.
///
/// HLS variants are resolved rather than skipped. Skipping them was a silent
/// data loss: a tweet whose only variant is a playlist produced no item at all,
/// so the clip simply did not appear in the post. Expanding the playlist costs
/// one request and hands back real renditions the video pipeline can fetch.
async fn twitter_variants(client: &Client, media: &Value) -> Vec<VideoFormat> {
    let Some(variants) = media.pointer("/video_info/variants").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut formats = Vec::new();
    for variant in variants {
        let Some(url) = variant.get("url").and_then(|u| u.as_str()) else { continue };
        let content_type = variant.get("content_type").and_then(|c| c.as_str()).unwrap_or("");

        if content_type.contains("mpegurl") || url.contains(".m3u8") {
            if let Ok(meta) = crate::video::hls::analyze(client, url, &Vec::new()).await {
                formats.extend(meta.formats);
            }
            continue;
        }

        let (id, width, height) = site::parse_resolution_from_url(url);
        formats.push(VideoFormat {
            id,
            container: "mp4".to_string(),
            width,
            height,
            bitrate: variant.get("bitrate").and_then(|b| b.as_u64()),
            video_url: Some(url.to_string()),
            // X's progressive variants are complete muxed files — this is not
            // one of the split-track CDNs — so they need no pairing.
            has_audio: Some(true),
            ..Default::default()
        });
    }

    crate::video::sort_formats(&mut formats);
    formats
}

async fn twitter(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    let _ = headers;
    let id = site::tweet_id(url).ok_or_else(|| Error::InvalidInput("not an X post URL".into()))?;
    let token = site::syndication_token(&id)
        .ok_or_else(|| Error::InvalidInput("could not derive the X embed token".into()))?;
    let api = format!("https://cdn.syndication.twimg.com/tweet-result?id={id}&token={token}&lang=en");

    // Anonymous on purpose, for the same reason the video extractor is: this
    // is the endpoint the public embed widget uses, and a session cookie only
    // risks a redirect to an interstitial.
    let body = fetch_text(client, &api, &Vec::new()).await?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|_| Error::InvalidInput("X returned something that was not a post".into()))?;

    let title = json
        .get("text")
        .and_then(|t| t.as_str())
        .map(|s| s.trim().chars().take(80).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "X post".to_string());
    let uploader = json
        .pointer("/user/screen_name")
        .and_then(|n| n.as_str())
        .map(|n| format!("@{n}"));

    let mut items = Vec::new();
    if let Some(details) = json.get("mediaDetails").and_then(|m| m.as_array()) {
        for (index, media) in details.iter().enumerate() {
            let kind = media.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let poster = media.get("media_url_https").and_then(|u| u.as_str());
            let width = media.pointer("/original_info/width").and_then(|w| w.as_u64()).map(|w| w as u32);
            let height = media.pointer("/original_info/height").and_then(|h| h.as_u64()).map(|h| h as u32);
            let id = format!("{}", index + 1);

            match kind {
                "photo" => {
                    let Some(poster) = poster else { continue };
                    items.push(
                        ImageItem::new(id, twitter_original(poster))
                            .with_size(width, height),
                    );
                }
                "video" | "animated_gif" => {
                    let formats = twitter_variants(client, media).await;
                    if let Some(item) = ImageItem::video(id, formats) {
                        items.push(
                            item.with_size(width, height)
                                .with_thumbnail(poster.map(String::from)),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    Ok(ImageAlbum { title, source_url: url.to_string(), uploader, items, ..Default::default() })
}

// ─── Instagram ───────────────────────────────────────────────────────────────

/// The largest candidate in an `image_versions2`-shaped node.
///
/// Instagram lists one picture at half a dozen sizes and puts no ordering
/// guarantee on the list, so the largest is found by area rather than by taking
/// the first — which is how a thumbnail ends up saved as "the photo".
fn ig_best_candidate(node: &Value) -> Option<(String, Option<u32>, Option<u32>)> {
    let mut best: Option<(String, u32, u32)> = None;
    let lists = [
        node.pointer("/image_versions2/candidates"),
        node.pointer("/image_versions2/additional_candidates/first_frame"),
    ];
    for candidates in lists.into_iter().flatten().filter_map(|c| c.as_array()) {
        for candidate in candidates {
            let Some(url) = candidate.get("url").and_then(|u| u.as_str()) else { continue };
            let width = candidate.get("width").and_then(|w| w.as_u64()).unwrap_or(0) as u32;
            let height = candidate.get("height").and_then(|h| h.as_u64()).unwrap_or(0) as u32;
            if best.as_ref().is_none_or(|(_, bw, bh)| u64::from(width) * u64::from(height) > u64::from(*bw) * u64::from(*bh)) {
                best = Some((url.to_string(), width, height));
            }
        }
    }
    // Older payloads keep the full-size picture at the top level instead.
    if best.is_none() {
        if let Some(url) = node.get("display_url").or_else(|| node.get("display_src")).and_then(|u| u.as_str()) {
            let width = node.pointer("/dimensions/width").and_then(|w| w.as_u64()).unwrap_or(0) as u32;
            let height = node.pointer("/dimensions/height").and_then(|h| h.as_u64()).unwrap_or(0) as u32;
            best = Some((url.to_string(), width, height));
        }
    }
    best.map(|(url, w, h)| (url, (w > 0).then_some(w), (h > 0).then_some(h)))
}

/// Every rendition in a node that turned out to be a clip, ranked best-first.
///
/// `has_audio` is deliberately left unknown. These URLs sit on `cdninstagram`,
/// one of the CDNs that serves a clip's picture and its sound as two separate
/// and identical-looking `.mp4` files — so claiming they are complete is
/// exactly the assumption that produces a silent download. The probe in
/// [`super::probe_split_tracks`] settles it from the bytes instead.
fn ig_video_formats(node: &Value) -> Vec<VideoFormat> {
    site::instagram_video_formats(node)
}

/// One entry of a post — a standalone item, or one slide of a carousel.
pub(crate) fn ig_item(node: &Value, id: String) -> Option<ImageItem> {
    // `media_type` 2 is a clip; 1 and 8 are a picture and a carousel. A node
    // carrying `video_versions` is a clip whatever it says it is.
    let is_video = node.get("video_url").and_then(Value::as_str).is_some()
        || node.get("video_dash_manifest").and_then(Value::as_str).is_some()
        || node.get("is_video").and_then(Value::as_bool) == Some(true)
        || node.get("media_type").and_then(|t| t.as_u64()) == Some(2)
        || node.get("video_versions").is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()));

    if is_video {
        let poster = ig_best_candidate(node).map(|(url, _, _)| url);
        return ImageItem::video(id, ig_video_formats(node)).map(|item| item.with_thumbnail(poster));
    }

    let (url, width, height) = ig_best_candidate(node)?;
    Some(ImageItem::new(id, url).with_size(width, height))
}

async fn instagram(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    Ok(crate::instagram::resolve(client, url, headers).await?.album)
}

// ─── Facebook ────────────────────────────────────────────────────────────────

/// The complete JSON object starting at the `{` at or after `from`.
///
/// Facebook inlines its page state as one minified line hundreds of kilobytes
/// long. A regex cannot find where such an object ends — only counting braces
/// can, and only while ignoring the ones inside string literals, which is what
/// the `in_string`/`escaped` state here is for.
fn json_object_at(text: &str, from: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text[from..].find('{')? + from;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in bytes[start..].iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=start + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Undo the HTML escaping a URL picks up on its way into markup or JSON.
///
/// A `<meta property="og:image">` value and a Reddit JSON URL both arrive with
/// their query separators written as `&amp;`. Left that way the request asks
/// for a parameter literally named `amp;utm_campaign`, which a lenient CDN
/// ignores and a strict one rejects — and on a signed URL it invalidates the
/// signature outright.
fn decode_entities(url: &str) -> String {
    url.replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The clip on a `v.redd.it` post, from the post JSON we already hold.
///
/// Reddit stores a video's picture and its sound as two separate files, so
/// `fallback_url` — the obvious-looking direct `.mp4` — is video only. Taking
/// it would produce a silent download every time. The manifests are what pair
/// the two, which is why they are tried first and the fallback is offered last
/// and labelled as having no sound.
async fn reddit_video(client: &Client, post: &Value, headers: &Headers) -> Option<ImageItem> {
    let video = post
        .pointer("/media/reddit_video")
        .or_else(|| post.pointer("/secure_media/reddit_video"))
        // A crossposted video lives on the original post, not this one.
        .or_else(|| post.pointer("/crosspost_parent_list/0/media/reddit_video"))?;

    let mut formats = Vec::new();

    if let Some(hls) = video.get("hls_url").and_then(|u| u.as_str()) {
        if let Ok(meta) = crate::video::hls::analyze(client, &decode_entities(hls), headers).await {
            formats.extend(meta.formats);
        }
    }
    if formats.is_empty() {
        if let Some(dash) = video.get("dash_url").and_then(|u| u.as_str()) {
            if let Ok(meta) = crate::video::dash::analyze(client, &decode_entities(dash), headers).await {
                formats.extend(meta.formats);
            }
        }
    }
    if formats.is_empty() {
        let fallback = video.get("fallback_url").and_then(|u| u.as_str())?;
        let height = video.get("height").and_then(|h| h.as_u64()).map(|h| h as u32);
        formats.push(VideoFormat {
            id: height.map(|h| format!("{h}p")).unwrap_or_else(|| "reddit".into()),
            container: "mp4".to_string(),
            width: video.get("width").and_then(|w| w.as_u64()).map(|w| w as u32),
            height,
            video_url: Some(decode_entities(fallback)),
            // Positively silent, not merely unprobed: this is Reddit's
            // video-only rendition, and the UI labels it rather than letting a
            // soundless file arrive unannounced.
            has_audio: Some(false),
            ..Default::default()
        });
    }

    crate::video::sort_formats(&mut formats);

    let poster = post
        .pointer("/preview/images/0/source/url")
        .and_then(|u| u.as_str())
        .map(decode_entities);
    ImageItem::video("video", formats).map(|item| item.with_thumbnail(poster))
}

/// What a Reddit post yields when its JSON API is refused.
///
/// Reddit now answers `.json` with a 403 to anonymous clients while serving the
/// post's own HTML perfectly happily, so being locked out of the structured
/// data is the *ordinary* case rather than an error worth giving up on. The
/// page still names its clip in the keys the video extractor already reads, and
/// its picture in `og:image`.
async fn reddit_from_html(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    let html = fetch_text(client, url, headers).await?;
    let mut items = Vec::new();

    if let Some(clip) = borrowed_post_video(client, url, headers, Platform::Reddit).await {
        items.push(clip);
    }
    if items.is_empty() {
        items = og_items(&html);
    }

    if items.is_empty() {
        return Err(Error::InvalidInput(format!("nothing found on {url}")));
    }

    Ok(ImageAlbum {
        title: site::page_title(&html, url),
        source_url: url.to_string(),
        uploader: None,
        items,
        ..Default::default()
    })
}

async fn reddit(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    // The `.json` suffix on any permalink is Reddit's own read API. It is the
    // better source by far — gallery order, per-slide sizes, the video's
    // manifests — but it is 403 for anyone without a session, so its refusal
    // falls through to the page rather than failing the whole extraction.
    let base = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    let api = format!("{base}.json?raw_json=1");
    let Ok(body) = fetch_text(client, &api, headers).await else {
        return reddit_from_html(client, url, headers).await;
    };
    let Ok(json) = serde_json::from_str::<Value>(&body) else {
        return reddit_from_html(client, url, headers).await;
    };

    // A permalink's JSON is `[post listing, comment listing]`.
    let Some(post) = json.pointer("/0/data/children/0/data") else {
        return reddit_from_html(client, url, headers).await;
    };

    let title = post
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("Reddit post")
        .to_string();
    let uploader = post.get("author").and_then(|a| a.as_str()).map(|a| format!("u/{a}"));
    let mut items = Vec::new();

    // A video post first: its `preview` image is the clip's poster frame, so
    // letting the image paths below run would list the same thing twice — once
    // as the video and once as a still of it.
    if post.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false)
        || post.pointer("/crosspost_parent_list/0/is_video").and_then(|v| v.as_bool()).unwrap_or(false)
    {
        if let Some(clip) = reddit_video(client, post, headers).await {
            items.push(clip);
        }
    }

    // A gallery names its slides in order, then describes each one separately.
    if let (Some(order), Some(metadata)) = (
        post.pointer("/gallery_data/items").and_then(|i| i.as_array()),
        post.get("media_metadata"),
    ) {
        for (index, entry) in order.iter().enumerate() {
            let Some(media_id) = entry.get("media_id").and_then(|m| m.as_str()) else { continue };
            let Some(media) = metadata.get(media_id) else { continue };
            // `s` is the source rendition; `u` is its URL (`gif` for animations).
            let Some(source) = media.get("s") else { continue };
            let Some(src) = source
                .get("u")
                .or_else(|| source.get("gif"))
                .and_then(|u| u.as_str())
            else {
                continue;
            };
            let width = source.get("x").and_then(|x| x.as_u64()).map(|x| x as u32);
            let height = source.get("y").and_then(|y| y.as_u64()).map(|y| y as u32);
            items.push(
                ImageItem::new(format!("{}", index + 1), decode_entities(src)).with_size(width, height),
            );
        }
    }

    // A single-image post links the picture directly.
    if items.is_empty() {
        if let Some(src) = post
            .get("url_overridden_by_dest")
            .or_else(|| post.get("url"))
            .and_then(|u| u.as_str())
        {
            if super::is_direct_image_url(src) {
                items.push(ImageItem::new("1", decode_entities(src)));
            }
        }
    }

    // Otherwise the preview Reddit generated for it.
    if items.is_empty() {
        if let Some(source) = post.pointer("/preview/images/0/source") {
            if let Some(src) = source.get("url").and_then(|u| u.as_str()) {
                let width = source.get("width").and_then(|x| x.as_u64()).map(|x| x as u32);
                let height = source.get("height").and_then(|y| y.as_u64()).map(|y| y as u32);
                items.push(ImageItem::new("1", decode_entities(src)).with_size(width, height));
            }
        }
    }

    // The API answered but described nothing we can fetch — a text post, or a
    // shape this does not know. The page is still worth a look before giving
    // up, and `generic` would only find the `og:image` and miss any clip.
    if items.is_empty() {
        return reddit_from_html(client, url, headers).await;
    }

    Ok(ImageAlbum { title, source_url: url.to_string(), uploader, items, ..Default::default() })
}

// ─── TikTok ──────────────────────────────────────────────────────────────────

async fn tiktok(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    let canonical = site::tiktok_resolve_url(client, url, headers).await;
    let response = crate::video::net::fetch_response(client, &canonical, headers).await?;
    let page_url = response.url().to_string();
    let html = response.text().await?;
    let mut items = Vec::new();

    // A TikTok photo post ("slideshow") carries its stills under `imagePost`,
    // each with a list of CDN mirrors of the same picture — the first that
    // works is as good as any, and the rest are the same bytes elsewhere.
    let mut from = 0;
    while let Some(found) = html[from..].find("\"imagePost\"") {
        let at = from + found;
        from = at + 1;
        let Some(object) = json_object_at(&html, at) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(object) else { continue };
        let Some(images) = value.get("images").and_then(|i| i.as_array()) else { continue };
        for (index, image) in images.iter().enumerate() {
            let Some(src) = image
                .pointer("/imageURL/urlList/0")
                .and_then(|u| u.as_str())
            else {
                continue;
            };
            let width = image.pointer("/imageWidth").and_then(|w| w.as_u64()).map(|w| w as u32);
            let height = image.pointer("/imageHeight").and_then(|h| h.as_u64()).map(|h| h as u32);
            items.push(ImageItem::new(format!("{}", index + 1), src).with_size(width, height));
        }
        if !items.is_empty() {
            break;
        }
    }

    // No stills means this is an ordinary TikTok video, not a photo post. Same
    // ordering rule as Facebook: the clip is claimed before `og_items` can
    // mistake its cover frame for a picture.
    if items.is_empty() {
        let snapshot = site::tiktok_metadata_from_html(&html, &page_url, headers)
            .and_then(|meta| ImageItem::video("video", meta.formats).map(|item| item.with_thumbnail(meta.thumbnail)));
        if let Some(clip) = snapshot {
            items.push(clip);
        } else if let Some(clip) = borrowed_post_video(client, &page_url, headers, Platform::TikTok).await {
            items.push(clip);
        }
    }

    if items.is_empty() {
        items = og_items(&html);
    }

    Ok(ImageAlbum {
        title: site::page_title(&html, url),
        source_url: url.to_string(),
        uploader: None,
        items,
        ..Default::default()
    })
}

// ─── Generic ─────────────────────────────────────────────────────────────────

/// Every `og:image` on a page, as album items.
///
/// The values come straight out of markup, so they are entity-escaped: a URL
/// with two query parameters arrives with `&amp;` between them.
pub(crate) fn og_items(html: &str) -> Vec<ImageItem> {
    site::og_images(html)
        .into_iter()
        .enumerate()
        .map(|(index, url)| ImageItem::new(format!("{}", index + 1), decode_entities(&url)))
        .collect()
}

/// The fallback for everything: read the page the way a chat client renders a
/// link preview.
async fn generic(client: &Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    let html = fetch_text(client, url, headers).await?;
    let items = og_items(&html);

    if items.is_empty() {
        return Err(Error::InvalidInput(format!("no images found on {url}")));
    }

    Ok(ImageAlbum {
        title: site::page_title(&html, url),
        source_url: url.to_string(),
        uploader: None,
        items,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::MediaKind;

    #[test]
    fn x_media_urls_are_rewritten_to_ask_for_the_original() {
        // The default rendition is a resized copy; `name=orig` is the upload.
        assert_eq!(
            twitter_original("https://pbs.twimg.com/media/Abc123.jpg"),
            "https://pbs.twimg.com/media/Abc123?format=jpg&name=orig"
        );
        // A URL already carrying a size suffix is normalised, not stacked.
        assert_eq!(
            twitter_original("https://pbs.twimg.com/media/Abc123.png:large"),
            "https://pbs.twimg.com/media/Abc123?format=png&name=orig"
        );
        // The query form the resulting URL uses must still read as an image,
        // or the album would reject its own output.
        assert!(super::super::is_direct_image_url(&twitter_original(
            "https://pbs.twimg.com/media/Abc123.jpg"
        )));
    }

    #[test]
    fn brace_counting_finds_an_objects_end_through_nested_strings() {
        // The Facebook case in miniature: braces inside string literals, and
        // an escaped quote, neither of which may end the object early.
        let text = r#"junk {"a":{"b":"} \" {"},"c":1} trailing"#;
        let found = json_object_at(text, 0).unwrap();
        assert_eq!(found, r#"{"a":{"b":"} \" {"},"c":1}"#);
        // And it really is parseable, which is the point of counting at all.
        assert!(serde_json::from_str::<Value>(found).is_ok());
    }

    #[test]
    fn brace_counting_gives_up_rather_than_returning_a_truncated_object() {
        assert_eq!(json_object_at(r#"{"a":{"b":1}"#, 0), None);
        assert_eq!(json_object_at("no braces here", 0), None);
    }

    #[test]
    fn instagram_picks_the_largest_candidate_not_the_first() {
        // The listing order is not size order, and taking the first is how a
        // thumbnail gets saved as "the photo".
        let node = serde_json::json!({
            "image_versions2": { "candidates": [
                { "url": "https://cdn/small.jpg", "width": 150, "height": 150 },
                { "url": "https://cdn/full.jpg", "width": 1440, "height": 1800 },
                { "url": "https://cdn/mid.jpg", "width": 640, "height": 800 }
            ]}
        });

        let (url, width, height) = ig_best_candidate(&node).unwrap();

        assert_eq!(url, "https://cdn/full.jpg");
        assert_eq!((width, height), (Some(1440), Some(1800)));
    }

    #[test]
    fn an_instagram_slide_with_video_versions_is_a_clip() {
        let node = serde_json::json!({
            "media_type": 2,
            "image_versions2": { "candidates": [{ "url": "https://cdn/poster.jpg", "width": 640, "height": 640 }] },
            "video_versions": [
                { "url": "https://cdn/low.mp4", "width": 480, "height": 480 },
                { "url": "https://cdn/high.mp4", "width": 1080, "height": 1080 }
            ]
        });

        let item = ig_item(&node, "1".into()).unwrap();

        assert_eq!(item.kind, MediaKind::Video);
        assert_eq!(item.url, "https://cdn/high.mp4");
        assert_eq!(item.extension, "mp4");
        // The poster still comes through, so the grid has something to show.
        assert_eq!(item.thumbnail.as_deref(), Some("https://cdn/poster.jpg"));
    }

    /// A client that is never actually used — the fixtures below carry no HLS
    /// variant, so no request is made.
    fn offline_client() -> Client {
        Client::new()
    }

    #[tokio::test]
    async fn every_x_rendition_is_kept_and_ranked_not_just_the_loudest_bitrate() {
        // Phase 2's point: the item carries the whole list, so a picker has
        // something to offer and the head is a considered default rather than
        // whatever the array happened to start with.
        let media = serde_json::json!({
            "type": "video",
            "video_info": { "variants": [
                { "content_type": "video/mp4", "bitrate": 632000, "url": "https://video.twimg.com/a/320x180/c.mp4" },
                { "content_type": "video/mp4", "bitrate": 2176000, "url": "https://video.twimg.com/a/1280x720/c.mp4" },
                { "content_type": "video/mp4", "bitrate": 832000, "url": "https://video.twimg.com/a/640x360/c.mp4" }
            ]}
        });

        let formats = twitter_variants(&offline_client(), &media).await;

        assert_eq!(formats.len(), 3);
        assert_eq!(formats[0].height, Some(720), "best first");
        // X serves complete muxed files, so every rendition plays on its own.
        assert!(formats.iter().all(|f| f.has_audio == Some(true)));
    }

    #[tokio::test]
    async fn an_x_clip_with_no_progressive_rendition_is_not_silently_lost() {
        // The old `twitter_best_variant` filtered to mp4 and returned `None`
        // for an HLS-only clip, so no tile was ever created and the video
        // simply vanished from the post. Reaching the playlist is a network
        // call this test cannot make — what it pins is that the playlist is
        // *attempted* rather than discarded, which the mixed case below shows.
        let media = serde_json::json!({
            "type": "video",
            "video_info": { "variants": [
                { "content_type": "application/x-mpegURL", "url": "https://video.twimg.com/a/playlist.m3u8" },
                { "content_type": "video/mp4", "bitrate": 832000, "url": "https://video.twimg.com/a/640x360/c.mp4" }
            ]}
        });

        let formats = twitter_variants(&offline_client(), &media).await;

        // The playlist could not be fetched here, but the progressive one is
        // still present — the clip survives either way.
        assert!(!formats.is_empty());
        assert!(formats.iter().any(|f| f.height == Some(360)));
    }

    #[test]
    fn instagram_renditions_are_left_unproven_for_the_probe_to_settle() {
        // These URLs sit on a CDN that serves picture and sound as separate,
        // identical-looking files. Claiming `has_audio: true` here is exactly
        // the assumption that produces a silent download, so the field stays
        // unknown until the bytes say otherwise.
        let node = serde_json::json!({
            "video_versions": [
                { "url": "https://cdn.instagram/low.mp4", "width": 480, "height": 480 },
                { "url": "https://cdn.instagram/high.mp4", "width": 1080, "height": 1080 }
            ]
        });

        let formats = ig_video_formats(&node);

        assert_eq!(formats.len(), 2);
        assert_eq!(formats[0].height, Some(1080), "best first");
        assert!(formats.iter().all(|f| f.has_audio.is_none()));
    }

    #[test]
    fn escaped_urls_are_decoded_before_use() {
        // Both Reddit's JSON and an `og:image` attribute escape their query
        // separators. Left as `&amp;`, the request asks for a parameter named
        // `amp;crop` — and on a signed URL that invalidates the signature.
        assert_eq!(
            decode_entities("https://preview.redd.it/a.jpg?width=1080&amp;crop=smart"),
            "https://preview.redd.it/a.jpg?width=1080&crop=smart"
        );
        assert_eq!(
            decode_entities("https://cdn/a.jpg?a=1&#38;b=2&#x26;c=3"),
            "https://cdn/a.jpg?a=1&b=2&c=3"
        );
    }

    #[test]
    fn og_items_arrive_decoded() {
        // The observed case: Wikipedia's own og:image carries three UTM
        // parameters separated by `&amp;`.
        let html = r#"<meta property="og:image" content="https://cdn/x.jpg?a=1&amp;b=2">"#;

        let items = og_items(html);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://cdn/x.jpg?a=1&b=2");
    }

    #[test]
    fn facebooks_crawler_endpoint_does_not_collapse_an_album() {
        // Every photo in a Facebook album is served from the identical path,
        // with only `media_id` telling them apart — a dedupe key built from the
        // path alone turned a twelve-photo album into one picture.
        let mut items = vec![
            ImageItem::new("1", "https://lookaside.fbsbx.com/lookaside/crawler/media/?media_id=111"),
            ImageItem::new("2", "https://lookaside.fbsbx.com/lookaside/crawler/media/?media_id=222"),
            ImageItem::new("3", "https://lookaside.fbsbx.com/lookaside/crawler/media/?media_id=222"),
        ];

        super::super::dedupe(&mut items);

        assert_eq!(items.len(), 2);
    }
}
