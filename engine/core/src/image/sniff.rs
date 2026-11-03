//! The integrity guard: deciding whether the bytes behind a candidate URL are
//! actually a picture, and what they really are.
//!
//! Every platform this module talks to answers a request it does not like with
//! `HTTP 200` and a *page* — a login wall, a consent interstitial, a rate-limit
//! notice. Saved under the extension the URL implied, that page becomes a
//! `photo.jpg` no viewer can open, and the failure surfaces hours later when
//! the user opens the folder. The header alone cannot be trusted either:
//! Instagram routinely serves WebP from a path ending `.jpg`.
//!
//! So the first bytes of the response decide. A file's magic number is the one
//! piece of evidence neither the URL nor the `Content-Type` can contradict.

/// A picture format recognised by its leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
    Gif,
    WebP,
    Avif,
    Heic,
    Bmp,
    /// Scalable vector graphics — text, so recognised by its root element
    /// rather than a magic number.
    Svg,
}

impl ImageKind {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Avif => "avif",
            Self::Heic => "heic",
            Self::Bmp => "bmp",
            Self::Svg => "svg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
            Self::Avif => "image/avif",
            Self::Heic => "image/heic",
            Self::Bmp => "image/bmp",
            Self::Svg => "image/svg+xml",
        }
    }
}

/// The number of leading bytes [`identify`] needs. Every signature it knows
/// fits inside this, so a ranged request for `bytes=0-31` is enough to classify
/// a response without fetching the file.
pub const SNIFF_LEN: usize = 32;

/// Identify `head` — the first bytes of a response body — as a picture format,
/// or `None` if it is not one.
///
/// `None` is the interesting answer: it is how an HTML login page announces
/// itself, whatever the URL said and whatever `Content-Type` claimed.
pub fn identify(head: &[u8]) -> Option<ImageKind> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageKind::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageKind::Png);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(ImageKind::Gif);
    }
    // RIFF containers name their payload at byte 8; only `WEBP` is a picture.
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(ImageKind::WebP);
    }
    // ISO base-media files (AVIF, HEIC) put a length then `ftyp` then a brand.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some(ImageKind::Avif);
        }
        if brand == b"heic" || brand == b"heix" || brand == b"heim" || brand == b"hevc" || brand == b"mif1" {
            return Some(ImageKind::Heic);
        }
    }
    if head.starts_with(b"BM") {
        return Some(ImageKind::Bmp);
    }
    if is_svg(head) {
        return Some(ImageKind::Svg);
    }
    None
}

/// SVG has no magic number, so it is recognised the way a browser does: the
/// first markup element is `<svg`, allowing for a leading XML declaration,
/// doctype, comment, or byte-order mark.
///
/// Deliberately strict about `<html`: an HTML error page is the exact thing
/// this module exists to reject, and both start with `<`.
fn is_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("<svg") {
        return true;
    }
    // `<?xml …?>` or `<!DOCTYPE svg …>` before the root element. Anything that
    // names `svg` this early is one; an HTML page names `html`.
    let head_lower = trimmed.to_ascii_lowercase();
    (head_lower.starts_with("<?xml") || head_lower.starts_with("<!doctype"))
        && head_lower.contains("svg")
        && !head_lower.contains("html")
}

/// Whether a `Content-Type` names something that cannot be a picture.
///
/// Used as a cheap pre-filter before the body is read at all: a response that
/// says `text/html` needs no sniffing to be rejected. It is not the decider —
/// a CDN mislabelling a WebP as `image/jpeg` still passes here and is corrected
/// by [`identify`] — only a fast path for the obvious refusals.
pub fn is_definitely_not_image(content_type: &str) -> bool {
    let value = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    value.starts_with("text/")
        || value == "application/json"
        || value == "application/xml"
        || value == "application/xhtml+xml"
        || value.starts_with("application/x-www-form-urlencoded")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_common_formats_by_their_magic_numbers() {
        assert_eq!(identify(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageKind::Jpeg));
        assert_eq!(identify(b"\x89PNG\r\n\x1a\n\0\0\0\r"), Some(ImageKind::Png));
        assert_eq!(identify(b"GIF89a\x01\x00"), Some(ImageKind::Gif));
        assert_eq!(identify(b"BM\x36\x00\x00\x00"), Some(ImageKind::Bmp));
    }

    #[test]
    fn a_riff_container_is_only_an_image_when_it_holds_webp() {
        assert_eq!(identify(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some(ImageKind::WebP));
        // The same container shape holding audio is not a picture.
        assert_eq!(identify(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
    }

    #[test]
    fn iso_base_media_brands_separate_avif_from_video() {
        assert_eq!(identify(b"\0\0\0\x20ftypavif\0\0\0\0"), Some(ImageKind::Avif));
        assert_eq!(identify(b"\0\0\0\x20ftypheic\0\0\0\0"), Some(ImageKind::Heic));
        // An mp4 shares the `ftyp` box but is not an image.
        assert_eq!(identify(b"\0\0\0\x20ftypisom\0\0\0\0"), None);
    }

    #[test]
    fn a_login_page_is_not_an_image_however_it_arrives() {
        // The failure this module exists to prevent: HTML saved as `photo.jpg`.
        assert_eq!(identify(b"<!DOCTYPE html><html lang=\"en\"><head>"), None);
        assert_eq!(identify(b"<html><body>Log in to continue</body>"), None);
        assert_eq!(identify(b"{\"error\":\"rate limited\"}"), None);
        assert_eq!(identify(b""), None);
    }

    #[test]
    fn svg_is_recognised_without_being_confused_for_html() {
        assert_eq!(identify(br#"<svg xmlns="http://www.w3.org/2000/svg">"#), Some(ImageKind::Svg));
        assert_eq!(identify(br#"<?xml version="1.0"?><svg width="10">"#), Some(ImageKind::Svg));
        assert_eq!(identify(br#"<?xml version="1.0"?><rss><channel>"#), None);
    }

    #[test]
    fn content_type_prefilter_rejects_pages_but_tolerates_mislabelled_images() {
        assert!(is_definitely_not_image("text/html; charset=utf-8"));
        assert!(is_definitely_not_image("application/json"));
        // Instagram serving WebP as `image/jpeg` must survive to be corrected
        // by the magic bytes rather than being thrown away here.
        assert!(!is_definitely_not_image("image/jpeg"));
        assert!(!is_definitely_not_image("application/octet-stream"));
    }
}
