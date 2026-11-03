//! Tiny shared HTTP helpers used by the analyzers. Downloading the actual
//! stream bytes goes through [`super::stream`] instead — this module is only
//! for the small text/manifest fetches analysis needs.

use reqwest::Client;

use crate::error::{Error, Result};
use crate::headers::{self, Headers};

pub async fn fetch_text(client: &Client, url: &str, headers: &Headers) -> Result<String> {
    Ok(fetch_response(client, url, headers).await?.text().await?)
}

/// Inspect response headers and the final URL before consuming a page body.
pub async fn fetch_response(client: &Client, url: &str, headers: &Headers) -> Result<reqwest::Response> {
    let mut req = client.get(url);
    let has_ua = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent"));
    if !has_ua {
        req = req.header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        );
    }
    let has_accept = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept"));
    if !has_accept {
        req = req.header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
        );
    }
    let has_lang = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept-language"));
    if !has_lang {
        req = req.header("Accept-Language", "en-US,en;q=0.9");
    }
    let has_referer = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("referer"));
    let fetch_site = if has_referer { "same-origin" } else { "none" };
    req = req
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", fetch_site)
        .header("Sec-Fetch-User", "?1")
        .header("Upgrade-Insecure-Requests", "1")
        // A page fetch analyzing a URL must see what's there *right now* —
        // some sites (X/Twitter in particular) serve a stripped, metadata-free
        // response from an edge/shared cache for the bare canonical URL while
        // a URL with any query string bypasses it and reaches a fresh render.
        // These headers ask any well-behaved cache in the path not to do that;
        // callers that also need to defeat a cache keyed purely on the literal
        // URL string add a unique query parameter themselves (see
        // `site::twitter`'s `cache_busted_url`).
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache");

    let response = headers::apply(req, headers).send().await?;
    let status = response.status();
    if !status.is_success() {
        let has_cookie = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie"));
        let code = status.as_u16();
        if has_cookie && (code == 413 || code == 431 || code == 400 || code == 403) {
            let anon_headers: Headers = headers
                .iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("cookie"))
                .cloned()
                .collect();
            let mut req_anon = client.get(url);
            if !has_ua {
                req_anon = req_anon.header(
                    "User-Agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
                );
            }
            if !has_accept {
                req_anon = req_anon.header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
                );
            }
            if !has_lang {
                req_anon = req_anon.header("Accept-Language", "en-US,en;q=0.9");
            }
            req_anon = req_anon
                .header("Sec-Fetch-Dest", "document")
                .header("Sec-Fetch-Mode", "navigate")
                .header("Sec-Fetch-Site", fetch_site)
                .header("Sec-Fetch-User", "?1")
                .header("Upgrade-Insecure-Requests", "1")
                .header("Cache-Control", "no-cache")
                .header("Pragma", "no-cache");
            if let Ok(resp2) = headers::apply(req_anon, &anon_headers).send().await {
                if resp2.status().is_success() {
                    return Ok(resp2);
                }
            }
        }
        return Err(Error::UnexpectedStatus {
            status: code,
            url: url.to_string(),
        });
    }
    Ok(response)
}

/// The URL up to and including the last `/`, used to resolve relative
/// references in playlists and manifests that do not carry their own base.
pub fn base_url(url: &str) -> String {
    match url.rfind('/') {
        Some(i) => url[..=i].to_string(),
        None => url.to_string(),
    }
}

/// Resolve a manifest-relative URI against a base, handling the absolute,
/// protocol-relative, and origin-relative cases a client might encounter.
pub fn resolve(base: &str, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    if let Some(rest) = uri.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if uri.starts_with('/') {
        if let Ok(b) = reqwest::Url::parse(base) {
            return format!("{}://{}{}", b.scheme(), b.host_str().unwrap_or(""), uri);
        }
        return uri.to_string();
    }
    format!("{base}{uri}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_stops_at_the_last_slash() {
        assert_eq!(base_url("https://cdn.example/a/b/master.m3u8"), "https://cdn.example/a/b/");
        assert_eq!(base_url("no-slash-here"), "no-slash-here");
    }

    #[test]
    fn resolve_leaves_absolute_urls_untouched() {
        assert_eq!(resolve("https://x/", "https://y/z.ts"), "https://y/z.ts");
    }

    #[test]
    fn resolve_joins_relative_paths_to_base() {
        assert_eq!(resolve("https://cdn.example/a/", "seg1.ts"), "https://cdn.example/a/seg1.ts");
    }

    #[test]
    fn resolve_handles_origin_relative_paths() {
        assert_eq!(resolve("https://cdn.example/a/", "/seg1.ts"), "https://cdn.example/seg1.ts");
    }

    #[test]
    fn resolve_handles_protocol_relative_paths() {
        assert_eq!(resolve("https://cdn.example/a/", "//other.example/seg1.ts"), "https://other.example/seg1.ts");
    }
}
