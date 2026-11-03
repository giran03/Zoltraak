//! Per-download request headers supplied by an external client.
//!
//! The browser extension is the reason this exists: a download link behind a
//! login is worthless to the engine unless the session cookie, the `Referer`
//! the page was serving from, and the browser's own `User-Agent` come along
//! with it. The server sees a request from a different client than the one the
//! user was browsing with, and answers with an HTML login page.
//!
//! Accepting arbitrary headers from a client is not free, so they are filtered
//! rather than trusted — see [`DENIED`].

use reqwest::RequestBuilder;
use reqwest::header::{HeaderName, HeaderValue};

/// Header name/value pairs, in the order the client sent them.
pub type Headers = Vec<(String, String)>;

/// Headers a client may never set, lowercase.
///
/// `range` is the load-bearing one: the engine issues its own ranged requests
/// and writes each response straight to its offset, so a client-supplied
/// `Range` would silently corrupt the output file. The rest are either owned
/// by the transport (`host`, `connection`, `content-length`) or would let a
/// caller reshape the response body in ways the chunk planner does not expect
/// (`accept-encoding`, `te`).
const DENIED: &[&str] = &[
    "accept-encoding",
    "connection",
    "content-length",
    "host",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "proxy-authorization",
    "range",
    "te",
    "transfer-encoding",
    "upgrade",
];

/// Drop anything unusable or unsafe, keeping the rest untouched.
///
/// Invalid names and values are dropped rather than rejected: a single bad
/// header from a browser extension should not fail the whole download the user
/// asked for, and the engine has no way to ask them about it.
pub fn sanitize(raw: impl IntoIterator<Item = (String, String)>) -> Headers {
    raw.into_iter()
        .filter(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            !DENIED.contains(&lower.as_str())
                && HeaderName::try_from(lower.as_str()).is_ok()
                && HeaderValue::try_from(value.as_str()).is_ok()
        })
        .collect()
}

/// Attach sanitized headers to an outgoing request.
///
/// Applied to every request the engine makes for a download — the probe, each
/// ranged chunk, and the single-stream fallback. A cookie that is only sent on
/// the probe would authenticate the size check and nothing else.
pub fn apply(mut builder: RequestBuilder, headers: &[(String, String)]) -> RequestBuilder {
    for (name, value) in headers {
        // Both were validated by `sanitize`; a pair that skipped it and is
        // malformed is dropped here rather than panicking mid-download.
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::try_from(value.as_str()),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder
}

/// Keep header waits bounded without imposing a deadline on the whole file.
pub(crate) async fn send(builder: RequestBuilder) -> crate::error::Result<reqwest::Response> {
    tokio::time::timeout(std::time::Duration::from_secs(30), builder.send())
        .await
        .map_err(|_| crate::error::Error::NetworkTimeout)?
        .map_err(Into::into)
}

/// Serialize for the queue's `headers` column. `None` when empty, so the
/// common case stores NULL instead of a `[]` literal on every row.
pub fn to_json(headers: &[(String, String)]) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    serde_json::to_string(headers).ok()
}

/// Parse the queue's `headers` column, treating corruption as "no headers".
///
/// A row that cannot be parsed is far better downloaded unauthenticated —
/// which fails loudly with a 401 — than not downloaded at all.
pub fn from_json(raw: Option<String>) -> Headers {
    match raw {
        None => Vec::new(),
        Some(json) => match serde_json::from_str::<Headers>(&json) {
            Ok(headers) => headers,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring unparseable stored headers");
                Vec::new()
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(raw: &[(&str, &str)]) -> Headers {
        raw.iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn ordinary_headers_survive() {
        let out = sanitize(pairs(&[
            ("Cookie", "session=abc"),
            ("Referer", "https://example.com/page"),
            ("User-Agent", "Mozilla/5.0"),
        ]));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1, "session=abc");
    }

    #[test]
    fn range_is_dropped_however_it_is_cased() {
        // The one that would corrupt the output file rather than merely
        // confuse the server.
        for name in ["Range", "range", "RANGE", "RaNgE"] {
            let out = sanitize(pairs(&[(name, "bytes=0-100")]));
            assert!(out.is_empty(), "{name} survived sanitizing");
        }
    }

    #[test]
    fn transport_owned_headers_are_dropped() {
        let out = sanitize(pairs(&[
            ("Host", "evil.example"),
            ("Content-Length", "0"),
            ("Connection", "close"),
            ("If-Range", "\"etag\""),
            ("Accept-Encoding", "identity"),
        ]));
        assert!(out.is_empty(), "kept: {out:?}");
    }

    #[test]
    fn malformed_pairs_are_dropped_not_fatal() {
        let out = sanitize(pairs(&[
            ("Bad Name", "value"),
            ("X-Newline", "a\nb"),
            ("X-Fine", "ok"),
        ]));
        // Casing is passed through untouched; only the deny check lowercases.
        assert_eq!(out, pairs(&[("X-Fine", "ok")]));
    }

    #[test]
    fn empty_headers_store_as_null() {
        assert_eq!(to_json(&[]), None);
        assert!(from_json(None).is_empty());
    }

    #[test]
    fn json_round_trips() {
        let headers = pairs(&[("cookie", "a=1"), ("referer", "https://x.example/")]);
        assert_eq!(from_json(to_json(&headers)), headers);
    }

    #[test]
    fn corrupt_json_degrades_to_no_headers() {
        assert!(from_json(Some("{not json".into())).is_empty());
    }
}
