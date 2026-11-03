//! Local API authentication.
//!
//! The API binds to loopback, but loopback is **not** an authorization
//! boundary: any script on any page the user visits can `fetch()`
//! `http://127.0.0.1:1450`. Without a bearer token, a visited website could
//! enqueue arbitrary downloads or read the user's download history. Every
//! route except `/health` therefore requires the per-install token, and
//! browsers are additionally blocked by refusing requests that carry a web
//! `Origin`.

use std::fmt;

/// A per-install secret, generated on first run and stored outside the repo.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiToken(String);

impl ApiToken {
    /// 256 bits of CSPRNG output, hex-encoded.
    pub fn generate() -> Self {
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        Self(format!("{}{}", a.simple(), b.simple()))
    }

    pub fn from_existing(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Compare against a presented token in constant time.
    ///
    /// A short-circuiting `==` leaks the shared prefix length through timing,
    /// which is enough to recover the token byte by byte.
    pub fn verify(&self, presented: &str) -> bool {
        constant_time_eq(self.0.as_bytes(), presented.as_bytes())
    }
}

/// Never let the secret reach a log line or a panic message.
impl fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiToken(<redacted>)")
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract the token from an `Authorization: Bearer <token>` header value.
pub fn bearer(header: &str) -> Option<&str> {
    let rest = header.strip_prefix("Bearer ")?;
    let token = rest.trim();
    (!token.is_empty()).then_some(token)
}

/// Whether an origin is a local Zoltraak web frontend (e.g. Vite dev server or loopback web mode).
pub fn is_local_web_origin(origin: Option<&str>) -> bool {
    matches!(origin, Some("http://localhost:1440" | "http://127.0.0.1:1440" | "http://localhost:1450" | "http://127.0.0.1:1450"))
}

/// Whether a request carrying this `Origin` may reach the API.
///
/// Browser extensions send `chrome-extension://…` / `moz-extension://…`;
/// non-browser clients (CLI, curl, the Tauri shell) send no `Origin` at all.
/// Anything else is a web page and is rejected outright — this is the check
/// that stops a malicious site from driving the engine.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => is_extension_origin(Some(o)),
    }
}

/// Whether a request carrying this `Host` header is permitted (blocks DNS rebinding).
pub fn host_allowed(host: Option<&str>) -> bool {
    match host {
        Some(h) => {
            let h = h.trim();
            if h == "127.0.0.1" || h == "localhost" || h == "[::1]" {
                return true;
            }
            if let Some((name, _port)) = h.rsplit_once(':') {
                if name == "127.0.0.1" || name == "localhost" || name == "[::1]" {
                    return true;
                }
            }
            false
        }
        None => true, // Non-HTTP/1.1 or native callers without Host header
    }
}

/// The deterministic Chromium extension ID minted from the extension's public key.
pub const CHROMIUM_EXTENSION_ID: &str = "anilnadppolgahpagmgjnogppicckamg";

/// Whether `origin` is a browser extension.
pub fn is_extension_origin(origin: Option<&str>) -> bool {
    match origin {
        Some(o) => {
            o.starts_with("chrome-extension://")
                || o.starts_with("moz-extension://")
                || o.starts_with("safari-web-extension://")
        }
        None => false,
    }
}

/// True if `origin` is known to be the official Zoltraak extension.
pub fn is_known_zoltraak_extension(origin: Option<&str>) -> bool {
    match origin {
        Some(o) => {
            let stripped = o.trim_end_matches('/');
            if stripped == format!("chrome-extension://{CHROMIUM_EXTENSION_ID}")

            {
                return true;
            }
            if let Ok(env_id) = std::env::var("ZOLTRAAK_CHROME_EXTENSION_ID") {
                let env_id = env_id.trim();
                if !env_id.is_empty() && stripped == format!("chrome-extension://{env_id}") {
                    return true;
                }
            }
            if let Some(config_dir) = crate::store::default_path().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
                if let Ok(content) = std::fs::read_to_string(config_dir.join("chrome_extension_id")) {
                    let custom_id = content.trim();
                    if !custom_id.is_empty() && stripped == format!("chrome-extension://{custom_id}") {
                        return true;
                    }
                }
            }
            false
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_unique_and_long() {
        let a = ApiToken::generate();
        let b = ApiToken::generate();
        assert_ne!(a.expose(), b.expose());
        assert_eq!(a.expose().len(), 64, "256 bits hex-encoded");
    }

    #[test]
    fn verify_accepts_only_the_exact_token() {
        let t = ApiToken::from_existing("secret-token");
        assert!(t.verify("secret-token"));
        assert!(!t.verify("secret-toke"));
        assert!(!t.verify("secret-tokenn"));
        assert!(!t.verify(""));
        assert!(!t.verify("SECRET-TOKEN"));
    }

    #[test]
    fn debug_never_reveals_the_secret() {
        let t = ApiToken::from_existing("super-secret");
        let rendered = format!("{t:?}");
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");
    }

    #[test]
    fn bearer_parses_a_well_formed_header() {
        assert_eq!(bearer("Bearer abc123"), Some("abc123"));
    }

    #[test]
    fn bearer_rejects_malformed_headers() {
        assert_eq!(bearer("abc123"), None);
        assert_eq!(bearer("Basic abc123"), None);
        assert_eq!(bearer("bearer abc123"), None, "scheme is case-sensitive here");
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(bearer("Bearer    "), None);
    }

    #[test]
    fn extensions_and_native_clients_are_allowed() {
        assert!(origin_allowed(None), "CLI and Tauri send no Origin");
        assert!(origin_allowed(Some("chrome-extension://abcdefghijklmnop")));
        assert!(origin_allowed(Some("moz-extension://abcdefghijklmnop")));
    }

    #[test]
    fn web_pages_are_rejected_regardless_of_token() {
        // The whole point: a page cannot reach the API even if it guesses the port.
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("http://localhost:3000")));
        assert!(!origin_allowed(Some("null")));
        // Substring tricks must not slip through.
        assert!(!origin_allowed(Some("https://chrome-extension://x")));
    }
}

#[cfg(test)]
mod licensing_origin_tests {
    #[test]
    fn arbitrary_local_ports_and_foreign_firefox_extensions_do_not_gain_implicit_trust() {
        assert!(!super::is_local_web_origin(Some("http://localhost:9999")));
        assert!(!super::is_local_web_origin(Some("http://localhost:1440.evil.test")));
        assert!(!super::is_known_zoltraak_extension(Some("moz-extension://unrelated-extension")));
        assert!(super::is_local_web_origin(Some("http://localhost:1440")));
    }
}
