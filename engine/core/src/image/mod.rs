//! Image and gallery capture: turning a social-media post URL into the set of
//! pictures it holds, at the resolution the platform actually stored.
//!
//! This is the sibling of [`crate::video`], and deliberately not part of it. A
//! video post has one subject offered at several qualities, so the user picks a
//! *format*; a photo post has several subjects each at one quality, so the user
//! picks *items*. Folding galleries into the format list would have made "which
//! of these four photos" and "which of these four bitrates" the same question,
//! and the answer to one is never the answer to the other.
//!
//! The extractors are page-scrapers and public-JSON clients, like the video
//! ones — no headless browser. What they produce is a plain list of fetchable
//! URLs, which the ordinary download queue then handles: an image is a small
//! static file, and the segmented downloader already knows how to get one.
//! Nothing here needs a job manager of its own.

pub mod sites;
pub mod sniff;
pub mod verify;
pub(crate) mod facebook;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::video::VideoFormat;
use crate::headers::Headers;

/// What a single entry in a post turned out to be.
///
/// A carousel is not always pictures. Instagram and X both let one post mix
/// stills with clips, and a gallery view that silently dropped the clips would
/// be lying about what the post contains — so a video entry is carried too,
/// whenever the platform exposes a directly fetchable file for it. Anything
/// needing the manifest/merge machinery is left to the Video view, which is
/// built for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
}

/// One downloadable picture (or clip) from a post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageItem {
    /// Stable within an album, and used as the selection key by the UI.
    pub id: String,
    pub kind: MediaKind,
    /// The full-resolution source to download.
    pub url: String,
    /// A smaller rendition for the grid, when the platform offers one. Falls
    /// back to `url` in the UI — correct, just heavier.
    pub thumbnail: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub filesize: Option<u64>,
    /// Best current guess at the container, corrected by [`verify`] when the
    /// bytes disagree with the URL. Never carries a leading dot.
    pub extension: String,
    /// Request headers this specific URL must be fetched with.
    ///
    /// Not decoration: `lookaside.fbsbx.com` answers an ordinary browser
    /// `User-Agent` with a login page and the crawler one with the picture, so
    /// the header that made extraction work has to travel with the item to the
    /// download that follows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Headers,
    /// For a video item: every rendition the platform offers, best first.
    ///
    /// Always non-empty on a [`MediaKind::Video`] item and always empty on an
    /// image, because that is what decides how the download is run. A clip is
    /// handed to the video pipeline with `formats[0]`, which is the only route
    /// that can fetch a manifest, pair a split audio track, and mux the result;
    /// `url` alone cannot express any of that. An extractor that cannot produce
    /// a format for a clip does not emit the item at all, rather than emitting
    /// one that would download as a broken file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<VideoFormat>,
    /// Set once the first bytes have been read and agreed they are a picture.
    /// `None` means "not checked" — shown as unverified rather than as bad.
    #[serde(default)]
    pub verified: Option<bool>,
}

impl ImageItem {
    /// A picture with nothing known about it but its address.
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        let url = url.into();
        let extension = extension_from_url(&url).unwrap_or_else(|| "jpg".to_string());
        Self {
            id: id.into(),
            kind: MediaKind::Image,
            url,
            thumbnail: None,
            width: None,
            height: None,
            filesize: None,
            extension,
            headers: Vec::new(),
            formats: Vec::new(),
            verified: None,
        }
    }

    /// A clip, carrying the renditions the platform offers for it.
    ///
    /// `formats` is ranked before it gets here (see [`crate::video::sort_formats`]),
    /// so the head is what a plain "download this" should take: sound before
    /// resolution. Returns `None` for an empty list — a video with no fetchable
    /// rendition is not something to show a download button for.
    pub fn video(id: impl Into<String>, formats: Vec<VideoFormat>) -> Option<Self> {
        let best = formats.first()?;
        let url = best.video_url.clone()?;
        let (width, height) = (best.width, best.height);
        let container = best.container.clone();
        let headers = best.headers.clone();

        Some(Self {
            id: id.into(),
            kind: MediaKind::Video,
            url,
            thumbnail: None,
            width,
            height,
            filesize: best.filesize,
            extension: container,
            headers,
            formats,
            verified: None,
        })
    }

    pub fn with_size(mut self, width: Option<u32>, height: Option<u32>) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn with_thumbnail(mut self, thumbnail: Option<String>) -> Self {
        self.thumbnail = thumbnail;
        self
    }

    /// Re-derive the item's own fields from whatever now sits at the head of
    /// `formats`. Called after a re-rank, so the tile and the filename describe
    /// the rendition that will actually be fetched rather than the one that led
    /// before the probe.
    fn refresh_from_formats(&mut self) {
        let Some(best) = self.formats.first() else { return };
        if let Some(url) = best.video_url.clone() {
            self.url = url;
        }
        self.width = best.width;
        self.height = best.height;
        self.filesize = best.filesize;
        self.extension = best.container.clone();
    }

    /// Pixel count, used to pick the largest of several renditions of the same
    /// picture. Unknown dimensions sort last rather than first.
    pub fn area(&self) -> u64 {
        match (self.width, self.height) {
            (Some(w), Some(h)) => u64::from(w) * u64::from(h),
            _ => 0,
        }
    }
}

/// Everything a post turned out to contain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAlbum {
    pub title: String,
    pub source_url: String,
    pub uploader: Option<String>,
    pub items: Vec<ImageItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<Completeness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Completeness { Complete, Partial, Unknown }

impl ImageAlbum {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) { self.warnings.push(message); }
    }

    fn retain_verified(&mut self) {
        let before = self.items.len();
        self.items.retain(|item| item.verified != Some(false));
        if self.items.len() != before {
            self.completeness = Some(Completeness::Partial);
            self.warn("Some attachment links did not return media and were excluded. Extract again to refresh expired links; if access was refused, refresh the site's shared sign-in.");
        } else if self.completeness == Some(Completeness::Complete)
            && self.items.iter().any(|i| i.kind == MediaKind::Image && i.verified.is_none()) {
            self.completeness = Some(Completeness::Unknown);
            self.warn("The attachment list was retrieved, but some images could not be verified. You can still try downloading them.");
        }
    }
}

/// Image containers this module recognises in a URL path. Kept separate from
/// [`crate::video::direct`]'s list, which exists to *exclude* images from video
/// results; this one is the positive test and includes the vector case.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif", "heic", "bmp", "svg", "jfif"];

/// Whether a URL names a picture outright — by its path extension, or by the
/// `format=`/`ext=` query parameter the CDNs that serve extensionless paths use
/// (`pbs.twimg.com/media/<id>?format=jpg&name=orig` is the common one).
pub fn is_direct_image_url(url: &str) -> bool {
    let (path, query) = split_url(url);
    let lower = path.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(&format!(".{ext}"))) {
        return true;
    }
    query_extension(query).is_some()
}

/// The picture extension a URL implies, from its path or its query.
pub fn extension_from_url(url: &str) -> Option<String> {
    let (path, query) = split_url(url);
    if let Some(ext) = query_extension(query) {
        return Some(ext);
    }
    let lower = path.to_ascii_lowercase();
    let candidate = lower.rsplit('.').next()?;
    IMAGE_EXTENSIONS
        .iter()
        .find(|ext| **ext == candidate)
        .map(|ext| normalise_ext(ext))
}

/// `format=jpg` / `ext=png`, as used by CDNs whose paths carry no extension.
fn query_extension(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key.eq_ignore_ascii_case("format") || key.eq_ignore_ascii_case("ext") {
            let value = value.to_ascii_lowercase();
            if IMAGE_EXTENSIONS.contains(&value.as_str()) {
                return Some(normalise_ext(&value));
            }
        }
    }
    None
}

/// Spellings that name the same container as something else. Keeping both in
/// filenames would scatter one format across two extensions for no gain.
fn normalise_ext(ext: &str) -> String {
    match ext {
        "jpeg" | "jfif" => "jpg".to_string(),
        other => other.to_string(),
    }
}

fn split_url(url: &str) -> (&str, &str) {
    let without_fragment = url.split('#').next().unwrap_or(url);
    match without_fragment.split_once('?') {
        Some((path, query)) => (path, query),
        None => (without_fragment, ""),
    }
}

/// Analyze a post URL — or a direct image link — into the pictures it holds.
///
/// Platform extractors run first; a URL none of them claims falls through to
/// the generic scraper, which reads the same Open Graph tags a chat client
/// would use to render a preview.
pub async fn analyze(client: &reqwest::Client, url: &str, headers: &Headers) -> Result<ImageAlbum> {
    analyze_with_browser(client, url, headers, None).await
}

pub(crate) async fn analyze_with_browser(client: &reqwest::Client, url: &str, headers: &Headers, browser: Option<ImageAlbum>) -> Result<ImageAlbum> {
    let mut album = if browser.as_ref().is_some_and(|a| a.completeness == Some(Completeness::Complete)) {
        browser.unwrap()
    } else if let Some(browser) = browser {
        match sites::analyze(client, url, headers).await {
            Ok(mut album) => { facebook::merge_album(&mut album, browser); album },
            Err(_) => browser,
        }
    } else if is_direct_image_url(url) {
        sites::direct_image(url)
    } else {
        sites::analyze(client, url, headers).await?
    };

    dedupe(&mut album.items);

    if album.items.is_empty() {
        return Err(Error::InvalidInput(format!("no images found on {url}")));
    }

    // Read the first bytes of each candidate before any of them reach the
    // queue. This is what keeps a login wall from being saved as `photo.jpg`,
    // and what corrects the extension when the CDN's path lies about it.
    verify::verify_all(client, &mut album.items, headers).await;
    album.retain_verified();

    probe_split_tracks(client, &mut album.items, headers).await;

    if album.items.is_empty() {
        return Err(Error::InvalidInput(format!(
            "found image links on {url}, but none of them returned an image — the post is likely private or requires a login session"
        )));
    }

    album.title = crate::video::title::ensure_valid_title(&album.title, url);
    Ok(album)
}

/// Settle whether each clip's default rendition actually carries sound.
///
/// Instagram and Facebook hand out a video's picture and its audio from the
/// same CDN paths, with the same `.mp4` extension and the same signed query —
/// nothing about the URL distinguishes them. A post video taken from one of
/// those hosts and *assumed* complete is precisely how a download arrives
/// silent, so the same probe the Video view uses on a pasted CDN link is run
/// here.
///
/// Once the answer is known the item is re-ranked: a rendition proven to have
/// no sound should not stay the default when an unprobed sibling might. That
/// ordering is [`crate::video::sort_formats`]'s job, and this only has to feed
/// it the finding.
async fn probe_split_tracks(client: &reqwest::Client, items: &mut [ImageItem], headers: &Headers) {
    for item in items.iter_mut().filter(|item| item.kind == MediaKind::Video) {
        let Some(best) = item.formats.first() else { continue };
        // Nothing to learn: either it is already established, or this host does
        // not serve tracks separately and a round trip would prove nothing.
        if best.has_audio.is_some() || best.requires_merge {
            continue;
        }
        let Some(target) = best.video_url.clone() else { continue };
        if !crate::video::serves_split_tracks(&target) {
            continue;
        }
        let Some(tracks) = crate::video::mp4probe::probe(client, &target, headers).await else {
            continue;
        };

        if let Some(best) = item.formats.first_mut() {
            best.has_audio = Some(tracks.audio);
        }
        crate::video::sort_formats(&mut item.formats);
        item.refresh_from_formats();
    }
}

/// Collapse entries that point at the same bytes, keeping the largest known
/// rendition of each.
///
/// Every one of these pages lists the same picture several times over — a
/// thumbnail, a feed-sized copy, the original — and a grid that showed all
/// three would ask the user to choose between three identical pictures.
pub(crate) fn dedupe(items: &mut Vec<ImageItem>) {
    let mut kept: Vec<ImageItem> = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let key = dedupe_key(&item.url);
        match kept.iter_mut().find(|existing| {
            if item.id.starts_with("facebook:") && existing.id.starts_with("facebook:") {
                existing.id == item.id
            } else { dedupe_key(&existing.url) == key }
        }) {
            Some(existing) => {
                if item.area() > existing.area() {
                    let id = existing.id.clone();
                    *existing = ImageItem { id, ..item };
                }
            }
            None => kept.push(item),
        }
    }
    *items = kept;
}

/// Query parameters that name *which* picture, rather than which rendering of
/// it.
///
/// Facebook's crawler endpoint is the reason this list exists: every photo in
/// an album is served from the identical path
/// (`/lookaside/crawler/media/?media_id=…`), so a key built from the path alone
/// collapses a whole album into one item. Everything else in a signed URL —
/// expiry, size, the `_nc_*` cache hints — varies between two fetches of the
/// *same* picture and must stay out of the key.
const IDENTITY_PARAMS: &[&str] = &["media_id", "fbid", "asset_id", "photo_id", "id"];

/// What makes two URLs the same picture.
///
/// The path, plus any parameter that identifies the object rather than the
/// rendition. Signed CDN links for one image differ in their expiry and size
/// parameters while naming the same object, so the whole URL is never the
/// identity.
fn dedupe_key(url: &str) -> String {
    let (path, query) = split_url(url);
    let mut key = path.trim_end_matches('/').to_ascii_lowercase();

    let mut identity: Vec<String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(name, _)| {
            IDENTITY_PARAMS.iter().any(|known| known.eq_ignore_ascii_case(name))
        })
        .map(|(name, value)| format!("{}={value}", name.to_ascii_lowercase()))
        .collect();
    identity.sort();

    if !identity.is_empty() {
        key.push('?');
        key.push_str(&identity.join("&"));
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_image_urls_are_recognised_by_path_or_query() {
        assert!(is_direct_image_url("https://cdn.example/photo.jpg"));
        assert!(is_direct_image_url("https://cdn.example/photo.PNG?v=2"));
        // X's CDN serves extensionless paths and names the format in the query.
        assert!(is_direct_image_url("https://pbs.twimg.com/media/Abc123?format=jpg&name=orig"));
        assert!(!is_direct_image_url("https://example.com/status/123"));
        assert!(!is_direct_image_url("https://cdn.example/clip.mp4"));
    }

    #[test]
    fn the_extension_comes_from_the_query_when_the_path_has_none() {
        assert_eq!(
            extension_from_url("https://pbs.twimg.com/media/Abc?format=webp&name=orig").as_deref(),
            Some("webp")
        );
        assert_eq!(extension_from_url("https://cdn.example/a/b.png").as_deref(), Some("png"));
        assert_eq!(extension_from_url("https://cdn.example/a/b").as_deref(), None);
    }

    #[test]
    fn jpeg_spellings_collapse_to_one_extension() {
        assert_eq!(extension_from_url("https://cdn.example/a.jpeg").as_deref(), Some("jpg"));
        assert_eq!(extension_from_url("https://cdn.example/a.jfif").as_deref(), Some("jpg"));
    }

    #[test]
    fn deduping_keeps_the_largest_rendition_of_one_picture() {
        // The same object offered small and large, as every one of these pages
        // does. Showing both would ask the user to pick between two identical
        // pictures.
        let mut items = vec![
            ImageItem::new("a", "https://cdn.example/p/1.jpg?name=small").with_size(Some(150), Some(150)),
            ImageItem::new("b", "https://cdn.example/p/1.jpg?name=orig").with_size(Some(2048), Some(1536)),
            ImageItem::new("c", "https://cdn.example/p/2.jpg").with_size(Some(800), Some(600)),
        ];

        dedupe(&mut items);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].width, Some(2048));
        // The surviving entry keeps the first id it was seen under, so a
        // selection made before a re-analysis still refers to the same slot.
        assert_eq!(items[0].id, "a");
        assert_eq!(items[1].id, "c");
    }

    #[test]
    fn an_unsized_duplicate_never_displaces_a_measured_one() {
        let mut items = vec![
            ImageItem::new("a", "https://cdn.example/p/1.jpg?name=orig").with_size(Some(2048), Some(1536)),
            ImageItem::new("b", "https://cdn.example/p/1.jpg?name=small"),
        ];

        dedupe(&mut items);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].width, Some(2048));
    }
}
