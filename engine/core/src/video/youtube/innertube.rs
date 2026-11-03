//! InnerTube — YouTube's private player API — as spoken by the handful of
//! official clients whose responses are still usable for a plain download.
//!
//! Every client gets a different answer for the same video. The web clients
//! now only offer server-driven (SABR) streams with no fetchable URL at all;
//! the Android VR and iOS apps still get plain, unsigned URLs; mobile web and
//! the TV app get URLs with an obfuscated `n` token and, for the TV app, a
//! signature to undo with the player script. Which client works also depends
//! on the video (age gates, "made for kids", members-only) and on whether the
//! request carries a signed-in session. So the extractor asks several, in an
//! order tuned to what tends to work, and takes the first usable answer.
//!
//! Client identities (names, versions, user agents) follow yt-dlp's table,
//! which is the best-maintained public record of what YouTube accepts.

use reqwest::Client;
use serde_json::{Value, json};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    VisionOs,
    /// Oculus app: plain URLs, no PO token, no player script. Cannot play
    /// "made for kids" videos.
    AndroidVr,
    /// iOS app: plain URLs, no player script.
    Ios,
    /// Mobile web: full format ladder, `n`-challenged URLs.
    Mweb,
    /// TV app, current version: `n` and signature challenges.
    Tv,
    /// TV app, older version: the client YouTube still serves non-SABR signed
    /// formats to for signed-in users. Only useful with a session.
    TvDowngraded,
    /// Desktop web with a Safari user agent: what it still offers a downloader
    /// is an HLS manifest of pre-muxed renditions.
    WebSafari,
    /// Embedded player: works around age gates on embeddable videos.
    WebEmbedded,
}

/// Everything that goes into impersonating one client.
#[derive(Debug, Clone, Copy)]
pub struct ClientSpec {
    pub kind: ClientKind,
    pub name: &'static str,
    pub version: &'static str,
    /// The `X-YouTube-Client-Name` numeric id.
    pub id: u32,
    pub user_agent: &'static str,
    pub device_make: Option<&'static str>,
    pub device_model: Option<&'static str>,
    pub os_name: Option<&'static str>,
    pub os_version: Option<&'static str>,
    pub android_sdk_version: Option<u32>,
    pub embed_url: Option<&'static str>,
    /// Whether this client's URLs carry challenges only the player script can
    /// undo — in which case the request must also say which player it has.
    pub requires_js_player: bool,
    /// Whether a session cookie means anything to this client.
    pub supports_cookies: bool,
    /// Whether the client is useless without a signed-in session.
    pub requires_auth: bool,
}

impl ClientKind {
    /// Browser GVS tokens cannot attest Android/iOS or TV requests.
    pub fn accepts_browser_gvs_token(self) -> bool {
        matches!(self, Self::Mweb | Self::WebSafari | Self::WebEmbedded)
    }

    pub fn requires_gvs_token(self) -> bool {
        matches!(self, Self::Mweb | Self::WebSafari | Self::Ios | Self::AndroidVr)
    }

    pub fn spec(self) -> ClientSpec {
        const NONE: ClientSpec = ClientSpec {
            kind: ClientKind::AndroidVr,
            name: "",
            version: "",
            id: 0,
            user_agent: "",
            device_make: None,
            device_model: None,
            os_name: None,
            os_version: None,
            android_sdk_version: None,
            embed_url: None,
            requires_js_player: true,
            supports_cookies: false,
            requires_auth: false,
        };
        match self {
            ClientKind::VisionOs => ClientSpec {
                kind: self,
                name: "VISIONOS",
                version: "1.02",
                id: 101,
                user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
                device_make: Some("Apple"),
                device_model: Some("RealityDevice17,1"),
                os_name: Some("visionOS"),
                os_version: Some("26.5.23O471"),
                requires_js_player: false,
                ..NONE
            },
            ClientKind::AndroidVr => ClientSpec {
                kind: self,
                name: "ANDROID_VR",
                version: "1.65.10",
                id: 28,
                user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
                device_make: Some("Oculus"),
                device_model: Some("Quest 3"),
                os_name: Some("Android"),
                os_version: Some("12L"),
                android_sdk_version: Some(32),
                requires_js_player: false,
                ..NONE
            },
            ClientKind::Ios => ClientSpec {
                kind: self,
                name: "IOS",
                version: "21.26.4",
                id: 5,
                user_agent: "com.google.ios.youtube/21.26.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
                device_make: Some("Apple"),
                device_model: Some("iPhone16,2"),
                os_name: Some("iPhone"),
                os_version: Some("18.3.2.22D82"),
                requires_js_player: false,
                ..NONE
            },
            ClientKind::Mweb => ClientSpec {
                kind: self,
                name: "MWEB",
                version: "2.20260708.05.00",
                id: 2,
                user_agent: "Mozilla/5.0 (iPad; CPU OS 16_7_10 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1,gzip(gfe)",
                supports_cookies: true,
                ..NONE
            },
            ClientKind::Tv => ClientSpec {
                kind: self,
                name: "TVHTML5",
                version: "7.20260707.07.00",
                id: 7,
                user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/25.lts.30.1034943-gold (unlike Gecko), Unknown_TV_Unknown_0/Unknown (Unknown, Unknown)",
                supports_cookies: true,
                ..NONE
            },
            ClientKind::TvDowngraded => ClientSpec {
                kind: self,
                name: "TVHTML5",
                version: "5.20260707",
                id: 7,
                user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
                supports_cookies: true,
                requires_auth: true,
                ..NONE
            },
            ClientKind::WebSafari => ClientSpec {
                kind: self,
                name: "WEB",
                version: "2.20260708.00.00",
                id: 1,
                user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15,gzip(gfe)",
                supports_cookies: true,
                // HLS may carry no JS challenge; load the player lazily if
                // the returned manifest actually has an `n` to solve.
                requires_js_player: false,
                ..NONE
            },
            ClientKind::WebEmbedded => ClientSpec {
                kind: self,
                name: "WEB_EMBEDDED_PLAYER",
                version: "2.20260708.00.00",
                id: 56,
                user_agent: super::WEB_USER_AGENT,
                // Any non-YouTube page will do; YouTube only checks that the
                // player claims to be embedded somewhere.
                embed_url: Some("https://www.reddit.com/"),
                supports_cookies: true,
                ..NONE
            },
        }
    }

    /// Short name for logs and error messages.
    pub fn label(self) -> &'static str {
        match self {
            ClientKind::VisionOs => "visionos",
            ClientKind::AndroidVr => "android_vr",
            ClientKind::Ios => "ios",
            ClientKind::Mweb => "mweb",
            ClientKind::Tv => "tv",
            ClientKind::TvDowngraded => "tv_downgraded",
            ClientKind::WebSafari => "web_safari",
            ClientKind::WebEmbedded => "web_embedded",
        }
    }
}

/// What every player request is made with, beyond the client identity.
#[derive(Debug, Default, Clone)]
pub struct RequestContext {
    pub video_id: String,
    /// The visitor id YouTube handed the page. Sending a real one is what
    /// clears the "confirm you're not a bot" gate for the app clients; a
    /// request with none is a request from nobody.
    pub visitor_data: Option<String>,
    /// The signature timestamp of the player script the caller can run.
    pub sts: Option<u32>,
    /// Allowlisted session cookie, already reduced to the auth names.
    pub cookie: Option<String>,
    /// From the page's ytcfg when signed in.
    pub session_index: Option<u32>,
    pub delegated_session_id: Option<String>,
    pub user_session_id: Option<String>,
    pub logged_in: bool,
}

impl RequestContext {
    /// Whether the cookie amounts to a YouTube login. YouTube keeps stale
    /// `*APISID` cookies around after a logout but clears `LOGIN_INFO`, so
    /// both are required — the same test yt-dlp uses.
    pub fn signed_in(&self) -> bool {
        self.cookie.as_deref().is_some_and(super::cookie_signs_in)
    }
}

/// `POST /youtubei/v1/player` as `kind`. Returns the parsed response, whatever
/// its playability says — the caller decides what an answer without streams
/// means.
pub async fn player_response(client: &Client, kind: ClientKind, ctx: &RequestContext) -> Result<Value> {
    let spec = kind.spec();

    let mut client_obj = json!({
        "clientName": spec.name,
        "clientVersion": spec.version,
        "hl": "en",
        "gl": "US",
    });
    if let Some(v) = spec.device_make {
        client_obj["deviceMake"] = json!(v);
    }
    if let Some(v) = spec.device_model {
        client_obj["deviceModel"] = json!(v);
    }
    if let Some(v) = spec.os_name {
        client_obj["osName"] = json!(v);
    }
    if let Some(v) = spec.os_version {
        client_obj["osVersion"] = json!(v);
    }
    if let Some(v) = spec.android_sdk_version {
        client_obj["androidSdkVersion"] = json!(v);
    }
    if !spec.user_agent.is_empty() {
        client_obj["userAgent"] = json!(spec.user_agent);
    }
    if let Some(v) = &ctx.visitor_data {
        client_obj["visitorData"] = json!(v);
    }

    let mut context = json!({ "client": client_obj });
    if let Some(embed) = spec.embed_url {
        context["thirdParty"] = json!({ "embedUrl": embed });
    }

    let mut playback = json!({ "html5Preference": "HTML5_PREF_WANTS" });
    if spec.requires_js_player {
        if let Some(sts) = ctx.sts {
            playback["signatureTimestamp"] = json!(sts);
        }
    }

    let body = json!({
        "videoId": ctx.video_id,
        "context": context,
        "playbackContext": { "contentPlaybackContext": playback },
        // Both are the "I am old enough / I understand" acknowledgements the
        // player sends after the user clicks through; sending them up front
        // is what every downloader does.
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    // GVS tokens authorize stream requests, not the InnerTube player API.
    // Do not substitute one for a player-context token.

    let mut request = client
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header("Content-Type", "application/json")
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Origin", "https://www.youtube.com")
        .header("X-YouTube-Client-Name", spec.id.to_string())
        .header("X-YouTube-Client-Version", spec.version);
    if !spec.user_agent.is_empty() {
        request = request.header("User-Agent", spec.user_agent);
    }
    if let Some(visitor) = &ctx.visitor_data {
        request = request.header("X-Goog-Visitor-Id", visitor.as_str());
    }
    if let Some(embed) = spec.embed_url {
        request = request.header("Referer", embed);
    }

    if spec.supports_cookies {
        if let Some(cookie) = &ctx.cookie {
            request = request.header("Cookie", cookie.as_str());
            // A cookie alone is ignored by InnerTube: the request also has to
            // prove it can read SAPISID, via the hash in `Authorization`.
            if let Some(auth) =
                sid_authorization(cookie, "https://www.youtube.com", ctx.user_session_id.as_deref(), now())
            {
                request = request.header("Authorization", auth).header("X-Origin", "https://www.youtube.com");
            }
            if let Some(page_id) = &ctx.delegated_session_id {
                request = request.header("X-Goog-PageId", page_id.as_str());
            }
            if ctx.delegated_session_id.is_some() || ctx.session_index.is_some() {
                request = request.header("X-Goog-AuthUser", ctx.session_index.unwrap_or(0).to_string());
            }
            if ctx.logged_in {
                request = request.header("X-Youtube-Bootstrap-Logged-In", "true");
            }
        }
    }

    let response = request.body(body.to_string()).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus {
            status: status.as_u16(),
            url: format!("youtubei/v1/player ({})", kind.label()),
        });
    }
    let text = response.text().await?;
    serde_json::from_str(&text)
        .map_err(|e| Error::InvalidInput(format!("YouTube {} player response is not JSON: {e}", kind.label())))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The `Authorization` value YouTube's own clients send alongside cookies:
/// one `SAPISIDHASH`-style proof per session cookie present, each
/// `sha1("<unix seconds> <cookie> <origin>")`, prefixed with the user session
/// id when there is one. Without it a signed-in cookie is treated as
/// anonymous — which is how a logged-in user still got "age-restricted".
pub fn sid_authorization(cookie: &str, origin: &str, user_session_id: Option<&str>, unix_seconds: i64) -> Option<String> {
    use sha1::{Digest, Sha1};

    let sapisid = cookie_value(cookie, "SAPISID").or_else(|| cookie_value(cookie, "__Secure-3PAPISID"));
    let sapisid_1p = cookie_value(cookie, "__Secure-1PAPISID");
    let sapisid_3p = cookie_value(cookie, "__Secure-3PAPISID");

    let mut parts = Vec::new();
    for (scheme, sid) in [("SAPISIDHASH", sapisid), ("SAPISID1PHASH", sapisid_1p), ("SAPISID3PHASH", sapisid_3p)] {
        let Some(sid) = sid else { continue };
        let mut hashed = String::new();
        if let Some(user) = user_session_id {
            hashed.push_str(user);
            hashed.push(' ');
        }
        hashed.push_str(&format!("{unix_seconds} {sid} {origin}"));
        let digest = Sha1::digest(hashed.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let suffix = if user_session_id.is_some() { "_u" } else { "" };
        parts.push(format!("{scheme} {unix_seconds}_{hex}{suffix}"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The value of one cookie in a `Cookie` header.
pub fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie
        .split(';')
        .map(str::trim)
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

// ─── Playability ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playability {
    pub status: String,
    pub reason: Option<String>,
}

impl Playability {
    pub fn of(response: &Value) -> Self {
        let ps = response.get("playabilityStatus");
        let status = ps.and_then(|p| p.get("status")).and_then(Value::as_str).unwrap_or("").to_string();
        let reason = ps
            .and_then(|p| p.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                ps.and_then(|p| p.pointer("/errorScreen/playerErrorMessageRenderer/subreason/simpleText"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        Self { status, reason }
    }

    pub fn is_ok(&self) -> bool {
        self.status == "OK"
    }

    /// YouTube's bot check, which shows up as a login requirement.
    pub fn is_bot_check(&self) -> bool {
        self.status == "LOGIN_REQUIRED"
            && self.reason.as_deref().is_some_and(|r| r.to_ascii_lowercase().contains("bot"))
    }

    pub fn is_login_required(&self) -> bool {
        self.status == "LOGIN_REQUIRED" && !self.is_bot_check()
    }

    pub fn is_age_gated(&self, response: &Value) -> bool {
        if response.pointer("/playabilityStatus/desktopLegacyAgeGateReason").is_some() {
            return true;
        }
        let haystack = format!("{} {}", self.status, self.reason.as_deref().unwrap_or("")).to_ascii_lowercase();
        ["confirm your age", "age-restricted", "inappropriate", "age_verification_required", "age_check_required"]
            .iter()
            .any(|needle| haystack.contains(needle))
    }

    pub fn is_live_offline(&self) -> bool {
        self.status == "LIVE_STREAM_OFFLINE"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sapisid_hash_matches_the_reference_computation() {
        use sha1::{Digest, Sha1};
        let digest = Sha1::digest(b"1700000000 abc https://www.youtube.com");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let auth = sid_authorization("SAPISID=abc; other=1", "https://www.youtube.com", None, 1_700_000_000).unwrap();
        assert_eq!(auth, format!("SAPISIDHASH 1700000000_{hex}"));
    }

    #[test]
    fn every_session_cookie_present_gets_its_own_proof() {
        let auth = sid_authorization(
            "SAPISID=a; __Secure-1PAPISID=b; __Secure-3PAPISID=c",
            "https://www.youtube.com",
            Some("user1"),
            1,
        )
        .unwrap();
        assert!(auth.starts_with("SAPISIDHASH 1_"));
        assert!(auth.contains(" SAPISID1PHASH 1_"));
        assert!(auth.contains(" SAPISID3PHASH 1_"));
        assert!(auth.ends_with("_u"), "a user session id is announced in the suffix");
    }

    #[test]
    fn a_missing_sapisid_falls_back_to_the_third_party_one() {
        let auth = sid_authorization("__Secure-3PAPISID=c", "https://www.youtube.com", None, 1).unwrap();
        assert!(auth.starts_with("SAPISIDHASH 1_"));
        assert!(auth.contains("SAPISID3PHASH"));
        assert_eq!(sid_authorization("PREF=x", "https://www.youtube.com", None, 1), None);
    }

    #[test]
    fn playability_reads_status_and_reasons() {
        let bot = json!({ "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you’re not a bot" } });
        let p = Playability::of(&bot);
        assert!(p.is_bot_check());
        assert!(!p.is_login_required());

        let private = json!({ "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "This is a private video" } });
        assert!(Playability::of(&private).is_login_required());

        let age = json!({ "playabilityStatus": { "status": "UNPLAYABLE", "reason": "Sign in to confirm your age" } });
        assert!(Playability::of(&age).is_age_gated(&age));

        let ok = json!({ "playabilityStatus": { "status": "OK" } });
        assert!(Playability::of(&ok).is_ok());
    }

    #[test]
    fn client_specs_are_internally_consistent() {
        for kind in [
            ClientKind::VisionOs,
            ClientKind::AndroidVr,
            ClientKind::Ios,
            ClientKind::Mweb,
            ClientKind::Tv,
            ClientKind::TvDowngraded,
            ClientKind::WebSafari,
            ClientKind::WebEmbedded,
        ] {
            let spec = kind.spec();
            assert_eq!(spec.kind, kind);
            assert!(!spec.name.is_empty() && !spec.version.is_empty() && spec.id > 0, "{kind:?}");
        }
        assert!(!ClientKind::AndroidVr.spec().requires_js_player);
        assert!(ClientKind::Mweb.spec().requires_js_player);
        assert!(ClientKind::TvDowngraded.spec().requires_auth);
    }
}
