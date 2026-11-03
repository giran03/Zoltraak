//! Server capability probing.
//!
//! Decides whether a URL can be downloaded with multiple connections, and
//! captures the validator used later to detect a changed remote file.

use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderMap,
    LAST_MODIFIED, RANGE,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub content_length: Option<u64>,
    pub accepts_ranges: bool,
    /// `ETag` when present, otherwise `Last-Modified`.
    pub validator: Option<String>,
    pub filename: Option<String>,
    pub content_type: Option<String>,
}

impl Capabilities {
    /// Multi-connection downloads need both an explicit length and range
    /// support; without either, the file must be fetched in one stream.
    pub fn supports_segmentation(&self) -> bool {
        self.accepts_ranges && self.content_length.is_some_and(|n| n > 0)
    }
}

/// Ask the server what it supports.
///
/// Tries `HEAD` first, then falls back to a single-byte ranged `GET`. Plenty of
/// CDNs and app servers either reject `HEAD` outright or answer it without the
/// `Accept-Ranges`/`Content-Length` headers they return on a real `GET`, so
/// trusting `HEAD` alone silently downgrades those hosts to one connection.
pub async fn probe(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Capabilities> {
    if let Ok(response) = crate::headers::send(
        crate::headers::apply(client.head(url), headers)
            .header(reqwest::header::ACCEPT_ENCODING, "identity"),
    )
    .await
    {
        if response.status().is_success() {
            let caps = from_headers(response.headers());
            if caps.supports_segmentation() {
                return Ok(caps);
            }
        }
    }

    // `bytes=0-0` asks for exactly one byte: a server that honours it answers
    // 206 with a `Content-Range` giving the full length.
    let mut response = crate::headers::send(
        crate::headers::apply(client.get(url), headers)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(RANGE, "bytes=0-0"),
    )
    .await?;

    // Empty files and origins refusing ranges can legitimately answer 416.
    // Verify the ordinary GET before deciding the URL itself is unavailable.
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        response = crate::headers::send(
            crate::headers::apply(client.get(url), headers)
                .header(reqwest::header::ACCEPT_ENCODING, "identity"),
        )
        .await?;
    }

    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus {
            status: status.as_u16(),
            url: url.to_string(),
        });
    }

    let headers = response.headers();
    let mut caps = from_headers(headers);

    if status == StatusCode::PARTIAL_CONTENT {
        // With a 206, Content-Length is 1 (the single byte), so the real size
        // has to come from Content-Range.
        let range = headers
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range);
        caps.accepts_ranges = range.is_some_and(|(start, end, _)| start == 0 && end == 0);
        caps.content_length = range.and_then(|(_, _, total)| total);
    } else {
        // A 200 here means the server ignored Range entirely.
        caps.accepts_ranges = false;
    }

    Ok(caps)
}

/// Map a `Content-Type` header value to a standard file extension.
pub fn extension_from_mime(mime: &str) -> Option<&'static str> {
    let clean = mime.split(';').next()?.trim().to_ascii_lowercase();
    match clean.as_str() {
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "video/x-matroska" | "video/mkv" => Some("mkv"),
        "video/quicktime" => Some("mov"),
        "video/x-msvideo" => Some("avi"),
        "video/3gpp" => Some("3gp"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/ogg" | "video/ogg" | "application/ogg" => Some("ogg"),
        "audio/wav" | "audio/x-wav" => Some("wav"),
        "audio/aac" | "audio/x-aac" => Some("aac"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/avif" => Some("avif"),
        "image/svg+xml" => Some("svg"),
        "application/pdf" => Some("pdf"),
        "application/zip" => Some("zip"),
        "application/x-tar" => Some("tar"),
        "application/gzip" | "application/x-gzip" => Some("gz"),
        "application/x-7z-compressed" => Some("7z"),
        "application/x-rar-compressed" => Some("rar"),
        "text/html" => Some("html"),
        "text/css" => Some("css"),
        "application/json" => Some("json"),
        _ => None,
    }
}

/// Try to extract a file extension from URL query parameters or path segments.
pub fn extension_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;

    for (k, v) in parsed.query_pairs() {
        let key = k.to_ascii_lowercase();
        if key == "ext" || key == "format" || key == "type" {
            let val = v.trim().trim_start_matches('.').to_ascii_lowercase();
            if !val.is_empty() && val.len() <= 5 && val.chars().all(|c| c.is_alphanumeric()) {
                return Some(val);
            }
        }
        if key == "filename" || key == "name" || key == "file" {
            if let Some(ext) = std::path::Path::new(v.as_ref())
                .extension()
                .and_then(|e| e.to_str())
            {
                return Some(ext.to_ascii_lowercase());
            }
        }
    }

    let path = parsed.path();
    let segment = path.rsplit('/').next()?;
    let ext = std::path::Path::new(segment).extension()?.to_str()?;
    if !ext.is_empty() && ext.len() <= 5 {
        Some(ext.to_ascii_lowercase())
    } else {
        None
    }
}

/// The name a download is saved under, before deduplication.
///
/// The queue always stores *a* name, defaulting to the URL's last segment.
/// That placeholder yields to the server's name; anything else was chosen by
/// the user and is final, with only a missing extension filled in.
pub fn resolve_filename(requested: Option<&str>, url: &str, caps: &Capabilities) -> String {
    let name_from_url = filename_from_url(url);
    let name = match requested.map(sanitize_filename) {
        Some(req_name) => match caps.filename {
            Some(ref cap_name) if req_name == sanitize_filename(&name_from_url) => cap_name.clone(),
            _ => req_name,
        },
        None => caps.filename.clone().unwrap_or(name_from_url),
    };

    let ext_from_mime = caps.content_type.as_deref().and_then(extension_from_mime);
    let ext_from_url = extension_from_url(url);
    ensure_extension(&name, ext_from_mime.or(ext_from_url.as_deref()))
}

/// Whether a download would save a web page rather than the file behind it.
pub fn is_html(filename: &str, content_type: Option<&str>) -> bool {
    let html_type = content_type
        .and_then(|value| value.split(';').next())
        .map(|mime| mime.trim().to_ascii_lowercase())
        .is_some_and(|mime| mime == "text/html" || mime == "application/xhtml+xml");
    let html_name = std::path::Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ["html", "htm", "xhtml"].contains(&ext.to_ascii_lowercase().as_str()));
    html_type || html_name
}

/// Ensure `filename` has a valid file extension. If it lacks one, attach `fallback_ext`.
pub fn ensure_extension(filename: &str, fallback_ext: Option<&str>) -> String {
    let path = std::path::Path::new(filename);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if !ext_lower.is_empty()
            && ext_lower.len() <= 6
            && ext_lower.chars().all(|c| c.is_alphanumeric())
        {
            return filename.to_string();
        }
    }
    match fallback_ext {
        Some(e) if !e.is_empty() && e.len() <= 6 && e.chars().all(|c| c.is_alphanumeric()) => {
            let clean_ext = e.trim_start_matches('.');
            format!("{filename}.{clean_ext}")
        }
        _ => filename.to_string(),
    }
}

/// If `filename` is already taken (per the caller's `taken` check), append
/// " (1)", " (2)", … before the extension until a free name is found — the
/// same numbering Windows/most browsers use so a second file with the same
/// name never silently overwrites the first.
pub fn dedupe_filename(filename: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    if !taken(filename) {
        return filename.to_string();
    }
    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_string);

    let mut n: u32 = 1;
    loop {
        let candidate = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn from_headers(headers: &HeaderMap) -> Capabilities {
    let header = |name: reqwest::header::HeaderName| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };

    Capabilities {
        content_length: headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok()),
        accepts_ranges: headers
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("bytes")),
        // Prefer the strong validator; fall back to the timestamp.
        validator: header(ETAG).or_else(|| header(LAST_MODIFIED)),
        filename: header(CONTENT_DISPOSITION)
            .as_deref()
            .and_then(filename_from_disposition),
        content_type: header(reqwest::header::CONTENT_TYPE),
    }
}

/// Total size from `Content-Range: bytes 0-0/12345`.
pub fn total_from_content_range(value: &str) -> Option<u64> {
    parse_content_range(value).and_then(|(_, _, total)| total)
}

/// Inclusive bounds, with an optional total for `bytes start-end/*`.
pub(crate) fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let (unit, value) = value.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (bounds, total) = value.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end: u64 = end.parse().ok()?;
    let total: Option<u64> = if total == "*" {
        None
    } else {
        Some(total.parse().ok()?)
    };
    if start > end || total.is_some_and(|n| end >= n) {
        return None;
    }
    Some((start, end, total))
}

#[cfg(test)]
#[test]
fn content_range_rejects_wrong_units_bounds_and_overflow() {
    for value in [
        "garbage/100",
        "items 0-1/100",
        "bytes 10-1/100",
        "bytes 0-100/100",
        "bytes 0-18446744073709551616/18446744073709551616",
        "bytes */0",
    ] {
        assert_eq!(parse_content_range(value), None, "{value}");
    }
    assert_eq!(
        parse_content_range("bytes 0-99/100"),
        Some((0, 99, Some(100)))
    );
    assert_eq!(parse_content_range("bytes 0-99/*"), Some((0, 99, None)));
}

/// Derive a filename from the URL path when the server offers no
/// `Content-Disposition`.
pub fn filename_from_url(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);

    // Skip past the scheme and authority. Without this, a URL with no path at
    // all (`https://host.test`) would yield the *hostname* as the filename.
    let path = match without_query.split_once("://") {
        Some((_scheme, rest)) => match rest.split_once('/') {
            Some((_authority, path)) => path,
            None => return ensure_extension("download", extension_from_url(url).as_deref()),
        },
        None => without_query,
    };

    let candidate = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    // A path segment is percent-encoded; a filename is not. Without decoding,
    // `My%20Song.mp3` is saved under that literal name and read back as a
    // title with `%20` in it — which then becomes the tag, and the search
    // term any lookup is built from.
    //
    // Decoding before sanitising is what keeps it safe: an escaped separator
    // (`%2F`) becomes a real one and is then dropped with the rest of the
    // path, rather than surviving as a name that could steer a write.
    let decoded = percent_decode(candidate);
    let base_name = if decoded.trim().is_empty() {
        "download".to_string()
    } else {
        sanitize_filename(&decoded)
    };

    let ext = extension_from_url(url);
    ensure_extension(&base_name, ext.as_deref())
}

/// Parse a filename from a `Content-Disposition` header value.
///
/// Prefers RFC 5987 `filename*` over plain `filename`. Any path separators are
/// stripped so a malicious header cannot escape the download directory.
pub fn filename_from_disposition(header: &str) -> Option<String> {
    let extended = header
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("filename*="))
        .and_then(|v| v.rsplit_once('\'').map(|(_, name)| name))
        .map(percent_decode);

    let plain = header
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("filename="))
        .map(|v| v.trim_matches('"').to_string());

    extended.or(plain).map(|n| sanitize_filename(&n))
}

fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '&' {
            let mut entity = String::new();
            let mut found_semi = false;
            let mut count = 0;
            while let Some(&next_c) = chars.peek() {
                if next_c == ';' {
                    chars.next();
                    found_semi = true;
                    break;
                }
                if next_c.is_alphanumeric() || next_c == '#' {
                    entity.push(next_c);
                    chars.next();
                    count += 1;
                    if count > 10 {
                        break;
                    }
                } else {
                    break;
                }
            }

            if found_semi && !entity.is_empty() {
                if let Some(ch) = parse_html_entity(&entity) {
                    out.push(ch);
                    continue;
                }
            }
            out.push('&');
            out.push_str(&entity);
            if found_semi {
                out.push(';');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_html_entity(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
    } else if let Some(dec) = entity.strip_prefix('#') {
        dec.parse::<u32>().ok().and_then(char::from_u32)
    } else {
        match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => None,
        }
    }
}

fn is_invalid_filename_char(c: char) -> bool {
    match c {
        // Windows reserved filename characters
        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => true,
        // Control characters
        c if c.is_control() => true,
        // Zero-width and formatting/bidi Unicode characters that break Windows path APIs
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}'
        | '\u{FEFF}' => true,
        _ => false,
    }
}

fn is_windows_reserved_stem(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(
        base.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Reduce an arbitrary string to a safe, single-segment filename.
pub fn sanitize_filename(name: &str) -> String {
    let decoded = decode_html_entities(name);
    let stem = decoded
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&decoded)
        .trim();

    let cleaned: String = stem
        .chars()
        .map(|c| if is_invalid_filename_char(c) { '_' } else { c })
        .collect();

    let trimmed = cleaned.trim().trim_matches('.').to_string();

    if trimmed.is_empty() {
        "download".to_string()
    } else if is_windows_reserved_stem(&trimmed) {
        format!("{trimmed}_")
    } else {
        trimmed
    }
}

/// A caller-supplied name reduced to one safe segment, or `None` when there is
/// nothing usable in it and the URL should be asked instead.
///
/// The browser hands over the *absolute path* it was about to write to, so the
/// name the user recognises is the last segment of it. Flattening the
/// separators instead — which is what the API and the app both used to do to
/// keep a caller inside the destination directory — turned
/// `C:\Users\me\Downloads\thing.jar` into `C__Users_me_Downloads_thing.jar`.
/// Taking the final component keeps the name *and* the containment: there is
/// no separator left to steer the write anywhere.
pub fn requested_filename(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    // `.` and `..` are directory references, not names; falling through to the
    // URL gives a better answer than a file literally called `download`.
    if base.is_empty() || base == "." || base == ".." {
        return None;
    }
    Some(sanitize_filename(base))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_link_resolves_to_an_html_file() {
        let page = Capabilities {
            content_type: Some("text/html; charset=utf-8".into()),
            ..Default::default()
        };
        let name = resolve_filename(None, "https://example.com/article", &page);
        assert_eq!(name, "article.html");
        assert!(is_html(&name, page.content_type.as_deref()));

        let image = Capabilities {
            content_type: Some("image/png".into()),
            ..Default::default()
        };
        let name = resolve_filename(None, "https://example.com/cat", &image);
        assert_eq!(name, "cat.png");
        assert!(!is_html(&name, image.content_type.as_deref()));
        // Without a probe answer, the URL's own extension still tells.
        assert!(is_html(&resolve_filename(None, "https://example.com/index.htm", &Capabilities::default()), None));
    }

    #[test]
    fn a_url_filename_is_percent_decoded_before_it_is_used() {
        // Left encoded, this name reaches the disk, the tags and any search
        // built from them with `%20` still in it.
        assert_eq!(
            filename_from_url("http://h/Some%20Artist%20-%20Some%20Song.mp3"),
            "Some Artist - Some Song.mp3"
        );
        assert_eq!(filename_from_url("http://h/caf%C3%A9.flac"), "café.flac");
        // A percent that is not an escape is left exactly as it is.
        assert_eq!(filename_from_url("http://h/50%.mp3"), "50%.mp3");
        assert_eq!(filename_from_url("http://h/100%25.mp3"), "100%.mp3");
        // An escaped separator becomes a real one and is dropped with the
        // rest of the path; it must never survive into the name.
        let escaped = filename_from_url("http://h/a%2F..%2Fescape.mp3");
        assert!(
            !escaped.contains("/") && !escaped.contains("\\"),
            "{escaped}"
        );
        assert_eq!(escaped, "escape.mp3");
        // A segment that decodes to nothing still yields a usable name.
        assert_eq!(filename_from_url("http://h/%20"), "download");
    }

    #[test]
    fn segmentation_requires_ranges_and_a_length() {
        let base = Capabilities {
            content_length: Some(1000),
            accepts_ranges: true,
            ..Default::default()
        };
        assert!(base.supports_segmentation());

        assert!(
            !Capabilities {
                accepts_ranges: false,
                ..base.clone()
            }
            .supports_segmentation()
        );

        assert!(
            !Capabilities {
                content_length: None,
                ..base.clone()
            }
            .supports_segmentation()
        );

        assert!(
            !Capabilities {
                content_length: Some(0),
                ..base
            }
            .supports_segmentation(),
            "a zero-length body cannot be split"
        );
    }

    #[test]
    fn plain_filename_is_extracted() {
        assert_eq!(
            filename_from_disposition(r#"attachment; filename="report.pdf""#).as_deref(),
            Some("report.pdf")
        );
    }

    #[test]
    fn extended_filename_wins_and_is_decoded() {
        let header = "attachment; filename=\"fallback.bin\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf";
        assert_eq!(
            filename_from_disposition(header).as_deref(),
            Some("résumé.pdf")
        );
    }

    #[test]
    fn traversal_in_a_header_cannot_escape_the_directory() {
        // A server must never be able to steer the write outside the download dir.
        assert_eq!(
            filename_from_disposition(r#"attachment; filename="../../etc/passwd""#).as_deref(),
            Some("passwd")
        );
        assert_eq!(
            filename_from_disposition(r#"attachment; filename="..\..\windows\system.ini""#)
                .as_deref(),
            Some("system.ini")
        );
    }

    #[test]
    fn reserved_characters_are_replaced() {
        assert_eq!(sanitize_filename(r#"a<b>c:d"e|f?g*h"#), "a_b_c_d_e_f_g_h");
    }

    #[test]
    fn empty_or_dot_only_names_fall_back() {
        assert_eq!(sanitize_filename(""), "download");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename("/"), "download");
    }

    #[test]
    fn a_browsers_absolute_path_keeps_only_its_last_segment() {
        // The regression: a captured download arrived as the full path Chrome
        // had chosen, and flattening its separators saved the file under
        // `C__Users_guill_Downloads_create-glowstone-dust-recipe-1.jar`.
        assert_eq!(
            requested_filename(r"C:\Users\guill\Downloads\create-glowstone-dust-recipe-1.jar")
                .as_deref(),
            Some("create-glowstone-dust-recipe-1.jar")
        );
        assert_eq!(
            requested_filename("/home/guill/Downloads/report.pdf").as_deref(),
            Some("report.pdf")
        );
        // A plain name is already the answer.
        assert_eq!(
            requested_filename("photo_1.png").as_deref(),
            Some("photo_1.png")
        );
    }

    #[test]
    fn a_requested_name_still_cannot_escape_the_directory() {
        assert_eq!(
            requested_filename("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            requested_filename(r"..\..\windows\system.ini").as_deref(),
            Some("system.ini")
        );
        // Nothing usable: the caller gets no say and the URL decides.
        assert_eq!(requested_filename(""), None);
        assert_eq!(requested_filename("   "), None);
        assert_eq!(requested_filename(".."), None);
        assert_eq!(requested_filename("some/dir/"), None);
    }

    #[test]
    fn missing_header_yields_none() {
        assert_eq!(filename_from_disposition("attachment"), None);
    }

    #[test]
    fn content_range_yields_the_total() {
        assert_eq!(total_from_content_range("bytes 0-0/12345"), Some(12345));
        assert_eq!(total_from_content_range("bytes 0-499/1000"), Some(1000));
    }

    #[test]
    fn dedupe_filename_returns_the_name_unchanged_when_free() {
        assert_eq!(dedupe_filename("movie.mp4", |_| false), "movie.mp4");
    }

    #[test]
    fn dedupe_filename_numbers_before_the_extension() {
        let taken = |name: &str| matches!(name, "movie.mp4" | "movie (1).mp4");
        assert_eq!(dedupe_filename("movie.mp4", taken), "movie (2).mp4");
    }

    #[test]
    fn dedupe_filename_handles_extensionless_names() {
        let taken = |name: &str| name == "README";
        assert_eq!(dedupe_filename("README", taken), "README (1)");
    }

    #[test]
    fn unknown_content_range_total_is_none() {
        // `*` is legal and means the server will not commit to a length.
        assert_eq!(total_from_content_range("bytes 0-0/*"), None);
        assert_eq!(total_from_content_range("garbage"), None);
    }

    #[test]
    fn url_filename_ignores_query_and_fragment() {
        assert_eq!(
            filename_from_url("https://host.test/a/b/file.tar.gz?token=xyz#frag"),
            "file.tar.gz"
        );
    }

    #[test]
    fn url_without_a_path_segment_falls_back() {
        // The hostname must never become the filename.
        assert_eq!(filename_from_url("https://host.test"), "download");
        assert_eq!(filename_from_url("https://host.test/"), "download");
        assert_eq!(filename_from_url("https://host.test?a=b"), "download");
        assert_eq!(filename_from_url("https://host.test:8080"), "download");
    }

    #[test]
    fn url_filename_survives_a_port_and_nested_path() {
        assert_eq!(
            filename_from_url("https://host.test:8443/a/b/c/setup.exe"),
            "setup.exe"
        );
    }
}
