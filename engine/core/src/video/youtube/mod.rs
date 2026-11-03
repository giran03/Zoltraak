//! YouTube extractor.
//!
//! What it takes to turn a watch URL into fetchable streams today:
//!
//! 1. The watch page, for the things only it knows — the current player
//!    script, its signature timestamp, the visitor id YouTube assigned, and
//!    whether the session is signed in.
//! 2. InnerTube's `/player` endpoint, asked as several official clients in
//!    turn ([`innertube`]) until one answers with streams that can actually be
//!    fetched. The web player itself no longer does.
//! 3. The player script, run in an embedded JS engine ([`player`]) to undo
//!    the `n` throttling token and, where present, the URL signature.
//! 4. A proof-of-origin token and the session cookie, when the browser
//!    extension has shared them, attached where each client expects them.
//!
//! The result is one [`VideoFormat`] per quality rung with sound attached
//! ([`formats`]), URLs that the CDN will serve at full speed, and — when the
//! answer is "no" — an error that says which gate is in the way and what
//! opens it.
//!
//! Privacy: the session cookie is reduced to the handful of names YouTube's
//! auth needs before it is used anywhere ([`minimize_cookie`]); nothing else
//! the browser holds is ever sent.

pub mod formats;
pub mod innertube;
pub mod player;

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, LazyLock};

use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use super::net::fetch_text;
use super::{VideoFormat, VideoMetadata};
use crate::error::{Error, Result};
use crate::headers::Headers;
use formats::{ReadyStream, parse_streams, query_param, with_query_param};
use innertube::{ClientKind, Playability, RequestContext};
use player::{ChallengeKind, LoadedPlayer};

/// A current desktop Chrome, for the watch page and player script fetches.
pub const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// Pseudo-headers the browser extension uses to hand over the harvested
/// proof-of-origin token and the visitor id it is bound to. Read here, never
/// forwarded to YouTube as headers.
pub const POTOKEN_HEADER: &str = "X-Youtube-Potoken";
pub const VISITOR_HEADER: &str = "X-Youtube-Visitor-Data";
pub const POTOKEN_VIDEO_HEADER: &str = "X-Youtube-Potoken-Video-Id";

// ─── URLs ────────────────────────────────────────────────────────────────────

static VIDEO_ID_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:/shorts/|/watch\?v=|/v/|/e/|/embed/|/live/|youtu\.be/)([A-Za-z0-9_-]{11})").unwrap());

pub fn is_youtube_url(url: &str) -> bool {
    let host = host_of(url);
    host == "youtu.be"
        || host.ends_with(".youtu.be")
        || host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com")
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The 11-character video id in any YouTube URL shape a user might paste.
pub fn video_id(url: &str) -> Option<String> {
    if let Some(c) = VIDEO_ID_PATH.captures(url) {
        return Some(c[1].to_string());
    }
    let id = query_param(url, "v")?;
    (id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(id)
}

/// Whether a URL is a googlevideo stream — the CDN URL a format points at.
pub fn is_stream_url(url: &str) -> bool {
    host_of(url).ends_with(".googlevideo.com") && url.contains("/videoplayback")
}

/// Size of one ranged request to googlevideo when the URL carries a
/// proof-of-origin token. The CDN serves an unbounded request at a trickle
/// and a bounded one at full speed; 10 MB is the size yt-dlp settled on.
pub const CHUNK_SIZE: u64 = 10 << 20;

/// Size of one ranged request without a token.
///
/// One mebibyte, and the exact value matters. Measured against googlevideo: a
/// 2 MiB range is refused outright (403) while 1 MiB is served in full. A
/// chunk over that ceiling does not degrade, it fails — and since the *first*
/// chunk fails, nothing reaches disk at all and the retry loop sees no
/// progress to continue from. That presented as "the CDN refused immediately"
/// when it was the request being too large to answer.
///
/// It does not lift the allowance itself, which for a visitor with no session
/// is about a megabyte per video and is *not* reset by re-resolving the link
/// (measured: sixty fresh URLs, all refused at the same offset). This is the
/// difference between getting that megabyte and getting nothing.
pub const UNTOKENED_CHUNK_SIZE: u64 = 1 << 20;

/// What it takes to fetch a googlevideo stream in parallel ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    /// Total size, from the URL's own `clen` parameter.
    pub total: u64,
    /// Bytes per ranged request.
    pub chunk_size: u64,
    /// Identifies the rendition across URL refreshes: the CDN edge, the
    /// signature, the expiry and even the `id` parameter all change when a
    /// stream is re-resolved, but itag, last-modified time and size name the
    /// same bytes.
    pub identity: String,
}

/// The chunk plan for a googlevideo stream, when the URL declares its size.
pub fn chunk_plan(url: &str) -> Option<ChunkPlan> {
    if !is_stream_url(url) {
        return None;
    }
    let total: u64 = query_param(url, "clen")?.parse().ok()?;
    if total == 0 {
        return None;
    }
    let itag = query_param(url, "itag").unwrap_or_default();
    let lmt = query_param(url, "lmt").unwrap_or_default();
    let chunk_size = if query_param(url, "pot").is_some() { CHUNK_SIZE } else { UNTOKENED_CHUNK_SIZE };
    Some(ChunkPlan { total, chunk_size, identity: format!("yt:{itag}:{lmt}:{total}") })
}

/// When a googlevideo URL stops working, as a unix timestamp.
pub fn stream_expiry(url: &str) -> Option<i64> {
    if !is_stream_url(url) {
        return None;
    }
    query_param(url, "expire")?.parse().ok()
}

/// Whether any of a format's stream URLs has expired (or is about to).
pub fn format_expired(format: &VideoFormat, now_unix: i64) -> bool {
    // Refresh a little early: a job that starts ten seconds before expiry
    // would still be fetching after it.
    const MARGIN: i64 = 5 * 60;
    [format.video_url.as_deref(), format.audio_url.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(stream_expiry)
        .any(|expiry| expiry - MARGIN <= now_unix)
}

// ─── Session ─────────────────────────────────────────────────────────────────

/// The cookie names YouTube's authentication actually reads. Everything else
/// the browser holds for youtube.com is tracking, preferences or noise — and
/// sending a full cookie jar also draws HTTP 413 from InnerTube.
pub const SESSION_COOKIES: &[&str] = &[
    "SID",
    "HSID",
    "SSID",
    "APISID",
    "SAPISID",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
    "__Secure-1PSIDCC",
    "__Secure-3PSIDCC",
    "SIDCC",
    "LOGIN_INFO",
    "PREF",
    "VISITOR_INFO1_LIVE",
    "VISITOR_PRIVACY_METADATA",
    "YSC",
    "__Secure-YEC",
    "__Secure-ROLLOUT_TOKEN",
    "CONSENT",
    "SOCS",
];

/// Keep only the allowlisted cookies. `None` when nothing useful remains.
pub fn minimize_cookie(cookie: &str) -> Option<String> {
    let kept: Vec<&str> = cookie
        .split(';')
        .map(str::trim)
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or("").trim();
            !name.is_empty() && SESSION_COOKIES.iter().any(|allowed| allowed.eq_ignore_ascii_case(name))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// Whether a cookie amounts to a login: `LOGIN_INFO` plus a `*APISID`.
pub fn cookie_signs_in(cookie: &str) -> bool {
    let has = |name: &str| innertube::cookie_value(cookie, name).is_some();
    has("LOGIN_INFO") && (has("SAPISID") || has("__Secure-1PAPISID") || has("__Secure-3PAPISID"))
}

/// What the browser extension (or the session vault) contributed to a request.
#[derive(Debug, Default, Clone)]
pub struct Session {
    /// Already minimized.
    pub cookie: Option<String>,
    pub po_token: Option<String>,
    pub visitor_data: Option<String>,
    pub po_token_video_id: Option<String>,
}

impl Session {
    pub fn from_headers(headers: &Headers) -> Self {
        let lookup = |name: &str| -> Option<String> {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            cookie: lookup("cookie").and_then(|c| minimize_cookie(&c)),
            po_token: lookup(POTOKEN_HEADER),
            visitor_data: lookup(VISITOR_HEADER),
            po_token_video_id: lookup(POTOKEN_VIDEO_HEADER),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.cookie.as_deref().is_some_and(cookie_signs_in)
    }

    fn has_gvs_token_for(&self, id: &str) -> bool {
        self.po_token.is_some() && self.po_token_video_id.as_deref() == Some(id)
    }
}

// ─── Watch page ──────────────────────────────────────────────────────────────

/// What the watch page tells us that InnerTube does not.
#[derive(Debug, Default, Clone)]
struct WatchPage {
    visitor_data: Option<String>,
    sts: Option<u32>,
    player_url: Option<String>,
    logged_in: bool,
    session_index: Option<u32>,
    delegated_session_id: Option<String>,
    user_session_id: Option<String>,
    /// The web client's own player response, embedded in the page. Useful
    /// for metadata; its streams are server-driven and not fetchable.
    initial_response: Option<Value>,
}

async fn fetch_watch_page(client: &Client, id: &str, session: &Session) -> Option<WatchPage> {
    let url = format!("https://www.youtube.com/watch?v={id}&bpctr=9999999999&has_verified=1");
    let mut headers: Headers = vec![("User-Agent".into(), WEB_USER_AGENT.into())];
    let mut cookie = session.cookie.clone().unwrap_or_default();
    // The EU consent interstitial: without a consent cookie the page is a
    // redirect to consent.youtube.com with no player in it. `CAI` is what
    // "accept" sets, and the only value the mix/playlist endpoints accept.
    if innertube::cookie_value(&cookie, "SOCS").is_none() {
        if !cookie.is_empty() {
            cookie.push_str("; ");
        }
        cookie.push_str("SOCS=CAI");
    }
    headers.push(("Cookie".into(), cookie));

    let html = match fetch_text(client, &url, &headers).await {
        Ok(html) => html,
        Err(e) => {
            tracing::debug!(error = %e, "YouTube watch page fetch failed; continuing without it");
            return None;
        }
    };

    let mut page = WatchPage::default();
    for cfg in json_objects_after(&html, "ytcfg.set(") {
        if page.visitor_data.is_none() {
            page.visitor_data = cfg
                .get("VISITOR_DATA")
                .or_else(|| cfg.pointer("/INNERTUBE_CONTEXT/client/visitorData"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if page.sts.is_none() {
            page.sts = cfg.get("STS").and_then(Value::as_u64).map(|n| n as u32);
        }
        if page.player_url.is_none() {
            page.player_url = cfg
                .get("PLAYER_JS_URL")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    cfg.get("WEB_PLAYER_CONTEXT_CONFIGS")
                        .and_then(Value::as_object)
                        .and_then(|configs| configs.values().find_map(|c| c.get("jsUrl").and_then(Value::as_str)))
                        .map(str::to_string)
                });
        }
        page.logged_in |= cfg.get("LOGGED_IN").and_then(Value::as_bool).unwrap_or(false);
        if page.session_index.is_none() {
            page.session_index = cfg.get("SESSION_INDEX").and_then(|v| match v {
                Value::Number(n) => n.as_u64().map(|n| n as u32),
                Value::String(s) => s.parse().ok(),
                _ => None,
            });
        }
        if page.delegated_session_id.is_none() {
            page.delegated_session_id = cfg.get("DELEGATED_SESSION_ID").and_then(Value::as_str).map(str::to_string);
        }
        if page.user_session_id.is_none() {
            page.user_session_id = cfg.get("USER_SESSION_ID").and_then(Value::as_str).map(str::to_string);
        }
        if let Some(sync) = cfg.get("DATASYNC_ID").and_then(Value::as_str) {
            // "delegated||user" for a secondary channel, "user||" for the
            // primary one.
            let (first, second) = sync.split_once("||").unwrap_or((sync, ""));
            if second.is_empty() {
                page.user_session_id.get_or_insert_with(|| first.to_string());
            } else {
                page.delegated_session_id.get_or_insert_with(|| first.to_string());
                page.user_session_id.get_or_insert_with(|| second.to_string());
            }
        }
    }
    if page.player_url.is_none() {
        page.player_url = PLAYER_JS_IN_PAGE.captures(&html).map(|c| c[1].to_string());
    }
    page.initial_response = json_objects_after(&html, "ytInitialPlayerResponse")
        .into_iter()
        .find(|v| v.get("videoDetails").is_some() || v.get("playabilityStatus").is_some());
    Some(page)
}

static PLAYER_JS_IN_PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""jsUrl"\s*:\s*"(/s/player/[^"]+)""#).unwrap());

/// Every JSON object that follows an occurrence of `marker` (after any
/// punctuation), for the `name = {...}` / `name({...})` shapes pages use.
fn json_objects_after(text: &str, marker: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(marker) {
        let at = from + rel + marker.len();
        from = at;
        let rest = &text[at..];
        let Some(open) = rest.find('{') else { continue };
        // Only a brace right after the marker (and some `=`/whitespace) counts.
        if rest[..open].trim().trim_start_matches('=').trim().len() > 0 {
            continue;
        }
        if let Some(end) = matching_json_brace(&rest[open..]) {
            if let Ok(value) = serde_json::from_str::<Value>(&rest[open..open + end]) {
                out.push(value);
            }
        }
    }
    out
}

/// Length of the JSON object starting at `s[0] == '{'`, honouring strings.
fn matching_json_brace(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

// ─── Analysis ────────────────────────────────────────────────────────────────

/// Which clients to try, in which order.
///
/// Prefer visionOS without attestation, or mobile web with a video-bound GVS
/// token. Account-capable clients remain available for gated videos. Android
/// VR 1.65.10 is excluded: upstream reports all its formats refused since
/// 2026-08-17 despite successful player responses. Client policy follows
/// https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_base.py.
fn client_order(signed_in: bool, has_pot: bool) -> Vec<ClientKind> {
    match (signed_in, has_pot) {
        (true, true) => {
            vec![ClientKind::Mweb, ClientKind::VisionOs, ClientKind::WebSafari, ClientKind::TvDowngraded, ClientKind::WebEmbedded]
        }
        (true, false) => {
            vec![ClientKind::VisionOs, ClientKind::WebSafari, ClientKind::TvDowngraded, ClientKind::Mweb, ClientKind::WebEmbedded]
        }
        (false, true) => {
            vec![ClientKind::Mweb, ClientKind::WebSafari, ClientKind::Tv, ClientKind::VisionOs, ClientKind::WebEmbedded]
        }
        (false, false) => {
            vec![ClientKind::VisionOs, ClientKind::WebSafari, ClientKind::Tv, ClientKind::Mweb, ClientKind::WebEmbedded]
        }
    }
}

pub async fn analyze(client: &Client, url: &str, headers: &Headers) -> Result<VideoMetadata> {
    let id = video_id(url).ok_or_else(|| {
        Error::InvalidInput("this YouTube link does not point at a single video (no video id found)".into())
    })?;
    let session = Session::from_headers(headers);
    let signed_in = session.signed_in();

    let page = fetch_watch_page(client, &id, &session).await.unwrap_or_default();
    let visitor_data = session.visitor_data.clone().or_else(|| page.visitor_data.clone());

    let mut details: Option<Value> = page
        .initial_response
        .as_ref()
        .and_then(|pr| pr.get("videoDetails"))
        .filter(|d| d.get("videoId").and_then(Value::as_str) == Some(id.as_str()))
        .cloned();

    let base_ctx = RequestContext {
        video_id: id.clone(),
        visitor_data: visitor_data.clone(),
        sts: page.sts,
        cookie: session.cookie.clone(),
        session_index: page.session_index,
        delegated_session_id: page.delegated_session_id.clone(),
        user_session_id: page.user_session_id.clone(),
        logged_in: page.logged_in || signed_in,
    };

    // Old extensions sent unscoped or player-context tokens. Only use GVS
    // tokens whose captured video binding matches the requested video.
    let has_pot = session.has_gvs_token_for(&id);

    let mut order = client_order(signed_in, has_pot);
    // Diagnostic override: `ZOLTRAAK_YT_CLIENTS=mweb,tv` forces the client
    // order, for chasing down which client a site is serving what to.
    if let Ok(forced) = std::env::var("ZOLTRAAK_YT_CLIENTS").or_else(|_| std::env::var("OPENDM_YT_CLIENTS")) {
        let forced: Vec<ClientKind> = forced
            .split(',')
            .filter_map(|name| {
                [
                    ClientKind::VisionOs,
                    ClientKind::AndroidVr,
                    ClientKind::Ios,
                    ClientKind::Mweb,
                    ClientKind::Tv,
                    ClientKind::TvDowngraded,
                    ClientKind::WebSafari,
                    ClientKind::WebEmbedded,
                ]
                .into_iter()
                .find(|k| k.label() == name.trim())
            })
            .collect();
        if !forced.is_empty() {
            tracing::warn!(?forced, "YouTube client order overridden by ZOLTRAAK_YT_CLIENTS");
            order = forced;
        }
    }

    let mut queue: VecDeque<ClientKind> = order.into();
    let mut tried: HashSet<ClientKind> = HashSet::new();
    let mut player: Option<Arc<LoadedPlayer>> = None;
    let mut player_failed = false;
    let mut winner: Option<(ClientKind, Value)> = None;
    let mut hls_fallback: Option<(ClientKind, String)> = None;
    let mut worst: Option<(ClientKind, Playability)> = None;
    // Clients dropped before they were ever asked, because the player script
    // they decode their URLs with could not be loaded or prepared. Every
    // challenged client needs it, so an unusable player narrows the ladder.
    // Keep that local failure distinct from YouTube's playability responses.
    let mut skipped_for_player: Vec<ClientKind> = Vec::new();

    while let Some(kind) = queue.pop_front() {
        if !tried.insert(kind) {
            continue;
        }
        let spec = kind.spec();
        if spec.requires_auth && !signed_in {
            continue;
        }

        let mut ctx = base_ctx.clone();
        if spec.requires_js_player {
            if player.is_none() && !player_failed {
                match load_player(client, page.player_url.as_deref()).await {
                    Ok(p) => player = Some(p),
                    Err(e) => {
                        tracing::warn!(error = %e, "YouTube player script unavailable; skipping clients that need it");
                        player_failed = true;
                    }
                }
            }
            match &player {
                Some(p) if p.can_solve() => ctx.sts = ctx.sts.or(p.sts),
                // Was loaded, but prepared badly — the case that used to fall
                // through without a word.
                Some(_) => {
                    tracing::warn!(client = kind.label(), "skipping client: the player script loaded but cannot solve its challenges");
                    skipped_for_player.push(kind);
                    continue;
                }
                None => {
                    tracing::warn!(client = kind.label(), "skipping client: it needs the player script, which is unavailable");
                    skipped_for_player.push(kind);
                    continue;
                }
            }
        }

        let response = match innertube::player_response(client, kind, &ctx).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(client = kind.label(), error = %e, "YouTube player request failed");
                continue;
            }
        };
        let responded_id = response.pointer("/videoDetails/videoId").and_then(Value::as_str);
        if responded_id.is_some_and(|r| r != id) {
            tracing::warn!(client = kind.label(), got = ?responded_id, "YouTube answered for a different video");
            continue;
        }
        if details.is_none() {
            details = response.get("videoDetails").cloned();
        }

        let playability = Playability::of(&response);
        let streams = response.get("streamingData").map(parse_streams).unwrap_or_default();
        let usable_video = streams.iter().filter(|s| s.is_video() && s.is_downloadable()).count();
        tracing::debug!(
            client = kind.label(), status = %playability.status, reason = playability.reason.as_deref().unwrap_or(""),
            streams = streams.len(), usable_video, "YouTube player response"
        );
        let pot_applies = has_pot && kind.accepts_browser_gvs_token();
        if usable_video > 0 && (!kind.requires_gvs_token() || pot_applies) {
            winner = Some((kind, response));
            break;
        }
        if let Some(hls) = response.pointer("/streamingData/hlsManifestUrl").and_then(Value::as_str) {
            // Safari's HLS renditions do not currently require a GVS token.
            // Prefer them to URLs known to require missing attestation.
            if kind == ClientKind::WebSafari {
                hls_fallback = Some((kind, hls.to_string()));
                break;
            }
            if !kind.requires_gvs_token() || pot_applies {
                hls_fallback.get_or_insert((kind, hls.to_string()));
            }
        }
        if playability.is_age_gated(&response) && !signed_in {
            queue.push_back(ClientKind::WebEmbedded);
        }
        if !playability.is_ok() {
            // Keep the most informative failure: a bot check or login gate
            // explains more than a generic "unavailable".
            let more_specific = worst.as_ref().is_none_or(|(_, w)| {
                !(w.is_bot_check() || w.is_login_required() || w.is_age_gated(&response))
            });
            if more_specific {
                worst = Some((kind, playability));
            }
        }
    }

    let is_live = details
        .as_ref()
        .and_then(|d| d.get("isLive"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let duration = details
        .as_ref()
        .and_then(|d| d.get("lengthSeconds"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| *d > 0.0);

    let mut formats: Vec<VideoFormat> = Vec::new();
    let mut hls_source: Option<String> = None;

    if let Some((kind, response)) = &winner {
        let sd = &response["streamingData"];
        let streams = parse_streams(sd);
        let pot_applies = has_pot && kind.accepts_browser_gvs_token();

        // Everything the player has to decode, solved in one batch.
        let mut requests: Vec<(ChallengeKind, String)> = Vec::new();
        for s in streams.iter().filter(|s| s.is_downloadable()) {
            if let Some(c) = &s.cipher {
                requests.push((ChallengeKind::Sig, c.s.clone()));
            }
            if let Some(n) = s.n_challenge() {
                requests.push((ChallengeKind::N, n));
            }
        }
        let answers = if requests.is_empty() {
            Vec::new()
        } else {
            let p = match &player {
                Some(p) => p.clone(),
                None => load_player(client, page.player_url.as_deref()).await.map_err(|e| {
                    Error::InvalidInput(format!(
                        "YouTube's {} client answered with streams that need the player script to decode, \
                         but the script could not be loaded: {e}",
                        kind.label()
                    ))
                })?,
            };
            p.solve(requests.clone()).await?
        };
        let mut answer = requests.iter().zip(answers).collect::<Vec<_>>().into_iter();

        let mut ready: Vec<ReadyStream> = Vec::new();
        for s in streams.into_iter().filter(|s| s.is_downloadable()) {
            let mut url = s.base_url().unwrap_or_default().to_string();
            if let Some(c) = &s.cipher {
                let (_, solved) = answer.next().expect("one answer per request");
                url = with_query_param(&url, &c.sp, &solved);
            }
            if s.n_challenge().is_some() {
                let (_, solved) = answer.next().expect("one answer per request");
                url = with_query_param(&url, "n", &solved);
            }
            if pot_applies && query_param(&url, "pot").is_none() {
                url = with_query_param(&url, "pot", session.po_token.as_deref().unwrap_or(""));
            }
            ready.push(ReadyStream { stream: s, url });
        }
        formats = formats::build_formats(&ready, duration);
        // The CDN checks the requesting client against the one the URL was
        // signed for: a URL minted for the iOS app must be fetched as it.
        let identity = client_headers(*kind);
        for f in &mut formats {
            f.headers = identity.clone();
        }
        // `skipped_for_player` belongs at info level even on the success path:
        // a win by an app client may hide failures preparing challenged
        // clients, and this line preserves that diagnostic context.
        tracing::info!(
            client = kind.label(),
            formats = formats.len(),
            pot = pot_applies,
            skipped_needing_player = skipped_for_player.len(),
            "YouTube formats resolved"
        );

        if is_live || formats.is_empty() {
            hls_source = sd.get("hlsManifestUrl").and_then(Value::as_str).map(str::to_string);
        }
    } else if let Some((_, hls)) = &hls_fallback {
        hls_source = Some(hls.clone());
    }

    if let Some(manifest) = hls_source {
        let hls_client = winner.as_ref().map(|(k, _)| *k).or_else(|| hls_fallback.as_ref().map(|(k, _)| *k));
        let pot_applies = has_pot && hls_client.is_some_and(ClientKind::accepts_browser_gvs_token);
        match hls_formats(client, &manifest, &session, pot_applies, page.player_url.as_deref(), player.as_ref()).await {
            Ok(mut extra) => {
                if let Some(k) = hls_client {
                    let identity = client_headers(k);
                    for f in &mut extra {
                        f.headers = identity.clone();
                    }
                }
                formats.append(&mut extra);
            }
            Err(e) => tracing::warn!(error = %e, "YouTube HLS manifest could not be used"),
        }
    }

    if formats.is_empty() {
        return Err(Error::InvalidInput(explain_failure(worst.as_ref(), signed_in, is_live, &skipped_for_player)));
    }

    let (title, thumbnail, uploader) = metadata(client, &id, details.as_ref()).await;
    Ok(VideoMetadata {
        subtitles: Vec::new(),
        title,
        thumbnail,
        duration,
        uploader,
        webpage_url: format!("https://www.youtube.com/watch?v={id}"),
        formats,
    })
}

/// The headers a stream URL from `kind` must be fetched with.
fn client_headers(kind: ClientKind) -> Headers {
    let ua = kind.spec().user_agent;
    if ua.is_empty() { Vec::new() } else { vec![("User-Agent".into(), ua.into())] }
}

async fn load_player(client: &Client, page_player_url: Option<&str>) -> Result<Arc<LoadedPlayer>> {
    let url = match page_player_url {
        Some(u) => u.to_string(),
        None => player::current_player_url(client).await?,
    };
    player::load(client, &url).await
}

/// Resolve an HLS master manifest into formats: the manifest path carries
/// its own `n` challenge (`/n/<value>/`) and takes the PO token as a path
/// segment (`/pot/<token>`) rather than a query parameter.
async fn hls_formats(
    client: &Client,
    manifest: &str,
    session: &Session,
    visitor_sent: bool,
    page_player_url: Option<&str>,
    player: Option<&Arc<LoadedPlayer>>,
) -> Result<Vec<VideoFormat>> {
    let mut url = manifest.to_string();
    let (path, rest) = match url.find('?') {
        Some(i) => (url[..i].to_string(), url[i..].to_string()),
        None => (url.clone(), String::new()),
    };
    let mut path = path;
    if let Some(c) = MANIFEST_N.captures(&path) {
        let challenge = c[1].to_string();
        let p = match player {
            Some(p) => p.clone(),
            None => load_player(client, page_player_url).await?,
        };
        let solved = p.solve(vec![(ChallengeKind::N, challenge.clone())]).await?.remove(0);
        path = path.replace(&format!("/n/{challenge}/"), &format!("/n/{solved}/"));
    }
    if let (Some(pot), true) = (&session.po_token, visitor_sent) {
        // `/file/index.m3u8` or `/playlist/index.m3u8` stays the last segment.
        let (base, suffix) = match path.rfind("/file/index.m3u8").or_else(|| path.rfind("/playlist/index.m3u8")) {
            Some(i) => (path[..i].to_string(), path[i..].to_string()),
            None => (path.clone(), String::new()),
        };
        path = format!("{}/pot/{pot}{suffix}", base.trim_end_matches('/'));
    }
    url = format!("{path}{rest}");

    let meta = super::hls::analyze(client, &url, &Headers::new()).await?;
    Ok(meta
        .formats
        .into_iter()
        .map(|mut f| {
            f.id = format!("hls-{}", f.id);
            // A YouTube HLS rendition is a complete muxed stream.
            f.has_audio = Some(true);
            f
        })
        .collect())
}

static MANIFEST_N: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/n/([^/]+)/").unwrap());

async fn metadata(client: &Client, id: &str, details: Option<&Value>) -> (String, Option<String>, Option<String>) {
    let mut title = details
        .and_then(|d| d.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let mut uploader = details
        .and_then(|d| d.get("author"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let mut thumbnail = details
        .and_then(|d| d.pointer("/thumbnail/thumbnails"))
        .and_then(Value::as_array)
        .and_then(|thumbs| thumbs.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);

    if title.is_none() || thumbnail.is_none() {
        // oEmbed is public and needs nothing; a fine last resort for a name.
        let oembed = format!("https://www.youtube.com/oembed?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3D{id}&format=json");
        if let Ok(text) = fetch_text(client, &oembed, &Headers::new()).await {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if title.is_none() {
                    title = v.get("title").and_then(Value::as_str).map(str::to_string);
                }
                if thumbnail.is_none() {
                    thumbnail = v.get("thumbnail_url").and_then(Value::as_str).map(str::to_string);
                }
                if uploader.is_none() {
                    uploader = v.get("author_name").and_then(Value::as_str).map(str::to_string);
                }
            }
        }
    }

    (
        title.unwrap_or_else(|| format!("youtube_{id}")),
        thumbnail.or_else(|| Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))),
        uploader,
    )
}

/// Why nothing could be downloaded, in terms of what the user can do about it.
///
/// `skipped_for_player` are the clients that were never asked at all because
/// the player script could not decode their URLs. That is a local fault, not
/// YouTube refusing anything, and it has to be said first: every other branch
/// here reads as "YouTube said no", which would send the user off to fix a
/// session that was never the problem.
fn explain_failure(
    worst: Option<&(ClientKind, Playability)>,
    signed_in: bool,
    is_live: bool,
    skipped_for_player: &[ClientKind],
) -> String {
    const EXTENSION_HINT: &str =
        "open the video in your browser with the Zoltraak extension installed so it can share your YouTube session, then try again";
    if worst.is_none() && !skipped_for_player.is_empty() {
        let names = skipped_for_player.iter().map(|k| k.label()).collect::<Vec<_>>().join(", ");
        return format!(
            "YouTube's player script could not be loaded or prepared, so the clients that need it to decode stream \
             URLs ({names}) were all skipped and nothing was left to ask. This is a fault on this machine, not a \
             refusal from YouTube — the log records why the script failed"
        );
    }
    match worst {
        Some((_, p)) if p.is_bot_check() => {
            if signed_in {
                "YouTube asked this session to confirm it is not a bot. Play the video in your browser once (the extension \
                 refreshes the proof-of-origin token from the player), then try again"
                    .into()
            } else {
                format!("YouTube wants a sign-in to confirm you're not a bot before it will serve this video — {EXTENSION_HINT}")
            }
        }
        Some((_, p)) if p.is_login_required() => {
            if signed_in {
                "your YouTube session was sent but this video still needs a different one — it may be private, \
                 members-only, or removed"
                    .into()
            } else {
                format!(
                    "this video needs a signed-in YouTube session (it may be private or members-only) — sign in to YouTube and {EXTENSION_HINT}"
                )
            }
        }
        Some((_, p)) if p.is_live_offline() => "this live stream has not started yet".into(),
        Some((client, p)) if p.reason.is_some() => {
            format!("YouTube ({}) reports: {}", client.label(), p.reason.as_deref().unwrap_or(""))
        }
        Some((client, p)) => format!("YouTube ({}) reports status {}", client.label(), p.status),
        None if is_live => "this live stream offered no downloadable renditions".into(),
        None => "YouTube offered only server-driven playback for this video, with no stream URL a downloader can fetch".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_are_read_from_every_link_shape() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=10",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(video_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
        }
        assert_eq!(video_id("https://www.youtube.com/@channel/videos"), None);
        assert_eq!(video_id("https://www.youtube.com/playlist?list=PL123"), None);
    }

    #[test]
    fn youtube_hosts_are_recognised_without_substring_tricks() {
        assert!(is_youtube_url("https://www.youtube.com/watch?v=x"));
        assert!(is_youtube_url("https://youtu.be/x"));
        assert!(is_youtube_url("https://music.youtube.com/watch?v=x"));
        assert!(!is_youtube_url("https://youtube.com.evil.example/watch?v=x"));
        assert!(!is_youtube_url("https://notyoutube.com/watch?v=x"));
    }

    #[test]
    fn cookies_are_reduced_to_the_auth_names() {
        let raw = "SAPISID=abc; PREF=f6=1; tracking=zzz; __Secure-3PSID=sid; LOGIN_INFO=li; junk=1; YSC=y";
        let kept = minimize_cookie(raw).unwrap();
        assert_eq!(kept, "SAPISID=abc; PREF=f6=1; __Secure-3PSID=sid; LOGIN_INFO=li; YSC=y");
        assert_eq!(minimize_cookie("tracking=zzz; junk=1"), None);
        assert!(cookie_signs_in(&kept));
        assert!(!cookie_signs_in("SAPISID=abc"), "a stale SAPISID without LOGIN_INFO is a logout");
        assert!(!cookie_signs_in("LOGIN_INFO=li"));
    }

    #[test]
    fn a_session_is_read_from_the_extension_pseudo_headers() {
        let headers: Headers = vec![
            ("cookie".into(), "SAPISID=a; LOGIN_INFO=b; noise=c".into()),
            ("X-Youtube-Potoken".into(), "tok".into()),
            ("x-youtube-visitor-data".into(), "vis".into()),
        ];
        let s = Session::from_headers(&headers);
        assert_eq!(s.cookie.as_deref(), Some("SAPISID=a; LOGIN_INFO=b"));
        assert_eq!(s.po_token.as_deref(), Some("tok"));
        assert_eq!(s.visitor_data.as_deref(), Some("vis"));
        assert!(s.signed_in());
    }

    #[test]
    fn chunk_plans_come_from_the_urls_own_size_and_identity() {
        let url = "https://rr3---sn-abc.googlevideo.com/videoplayback?expire=1700000000&id=o-AbC&itag=137&clen=1234567&lmt=1690000000000000&n=x";
        let plan = chunk_plan(url).unwrap();
        assert_eq!(plan.total, 1_234_567);
        assert_eq!(plan.identity, "yt:137:1690000000000000:1234567");
        // A re-resolved link names the same bytes even though its `id` differs.
        let refreshed = url.replace("id=o-AbC", "id=o-XyZ").replace("expire=1700000000", "expire=1700009999");
        assert_eq!(chunk_plan(&refreshed).unwrap().identity, plan.identity);
        assert_eq!(plan.chunk_size, UNTOKENED_CHUNK_SIZE);
        assert_eq!(UNTOKENED_CHUNK_SIZE, 1 << 20, "measured: googlevideo answers a 1 MiB range and refuses a 2 MiB one");
        assert_eq!(chunk_plan(&format!("{url}&pot=tok")).unwrap().chunk_size, CHUNK_SIZE);
        assert_eq!(stream_expiry(url), Some(1_700_000_000));

        assert_eq!(chunk_plan("https://rr3---sn-abc.googlevideo.com/videoplayback?itag=137"), None, "no clen");
        assert_eq!(chunk_plan("https://cdn.example/clip.mp4?clen=5"), None, "not googlevideo");
        assert!(!is_stream_url("https://evil.example/googlevideo.com/videoplayback"));
    }

    #[test]
    fn a_proof_of_origin_token_puts_the_web_clients_first() {
        // The token is bound to the browser's `visitor_data`, so it only
        // validates on the URLs of a client that shares that session. The app
        // clients follow as the fallback for a token YouTube has since expired.
        let web = |k: &ClientKind| k.spec().supports_cookies;

        let tokened = client_order(false, true);
        assert_eq!(tokened[0], ClientKind::Mweb);
        assert!(tokened[..3].iter().all(web), "the web-family clients lead: {tokened:?}");
        assert_eq!(tokened[3], ClientKind::VisionOs);

        // Without one there is nothing to bind, and the app clients are still
        // the cheapest route: plain URLs, no player script to load and run.
        let bare = client_order(false, false);
        assert_eq!(bare[0], ClientKind::VisionOs);
        assert!(!bare.contains(&ClientKind::AndroidVr));

        // A signed-in session outranks either: it is the only thing that gets
        // the downgraded TV client, and that client is useless without it.
        for has_pot in [false, true] {
            let signed = client_order(true, has_pot);
            assert_eq!(signed[0], if has_pot { ClientKind::Mweb } else { ClientKind::VisionOs });
            assert!(signed.contains(&ClientKind::TvDowngraded));
        }

        // Every ladder offers the same clients, only in a different order.
        let sorted = |mut v: Vec<ClientKind>| {
            v.sort_by_key(|k| k.label());
            v
        };
        assert_eq!(sorted(client_order(false, true)), sorted(client_order(false, false)));
    }

    #[test]
    fn gvs_tokens_require_the_same_video_and_a_compatible_client() {
        let mut session = Session {
            po_token: Some("gvs".into()),
            po_token_video_id: Some("jNQXAC9IVRw".into()),
            ..Default::default()
        };
        assert!(session.has_gvs_token_for("jNQXAC9IVRw"));
        assert!(!session.has_gvs_token_for("9FtpzAqIPIU"));
        session.po_token_video_id = None;
        assert!(!session.has_gvs_token_for("jNQXAC9IVRw"));
        for kind in [ClientKind::AndroidVr, ClientKind::Ios, ClientKind::Tv, ClientKind::TvDowngraded] {
            assert!(!kind.accepts_browser_gvs_token());
        }
        assert!(ClientKind::Mweb.accepts_browser_gvs_token());
        assert!(ClientKind::Ios.requires_gvs_token());
        assert!(ClientKind::AndroidVr.requires_gvs_token());
        assert!(!ClientKind::VisionOs.requires_gvs_token());
    }

    #[test]
    fn expiry_is_checked_with_a_margin_on_both_streams() {
        let fmt = VideoFormat {
            video_url: Some("https://a.googlevideo.com/videoplayback?expire=2000".into()),
            audio_url: Some("https://a.googlevideo.com/videoplayback?expire=1000".into()),
            ..Default::default()
        };
        assert!(!format_expired(&fmt, 500));
        assert!(format_expired(&fmt, 1000 - 60), "the audio stream is within the margin");
        assert!(format_expired(&fmt, 3000));
        let plain = VideoFormat { video_url: Some("https://cdn.example/x.mp4".into()), ..Default::default() };
        assert!(!format_expired(&plain, i64::MAX));
    }

    #[test]
    fn json_objects_are_found_after_their_markers() {
        let html = r#"<script>ytcfg.set({"STS":20697,"PLAYER_JS_URL":"/s/player/abc/base.js"}); ytcfg.set({"LOGGED_IN":true});</script>
            var ytInitialPlayerResponse = {"videoDetails":{"videoId":"x","title":"T \"q\" }"}};"#;
        let cfgs = json_objects_after(html, "ytcfg.set(");
        assert_eq!(cfgs.len(), 2);
        assert_eq!(cfgs[0]["STS"], 20697);
        assert_eq!(cfgs[1]["LOGGED_IN"], true);
        let pr = json_objects_after(html, "ytInitialPlayerResponse");
        assert_eq!(pr[0]["videoDetails"]["title"], "T \"q\" }");
    }

    #[test]
    fn failures_are_explained_in_terms_of_what_to_do() {
        let bot = Playability { status: "LOGIN_REQUIRED".into(), reason: Some("Sign in to confirm you’re not a bot".into()) };
        let msg = explain_failure(Some(&(ClientKind::AndroidVr, bot)), false, false, &[]);
        assert!(msg.contains("extension"), "{msg}");

        let private = Playability { status: "LOGIN_REQUIRED".into(), reason: Some("private".into()) };
        let msg = explain_failure(Some(&(ClientKind::Ios, private)), true, false, &[]);
        assert!(msg.contains("private"), "{msg}");

        assert!(explain_failure(None, false, false, &[]).contains("server-driven"));
    }

    #[test]
    fn a_broken_player_is_blamed_on_this_machine_not_on_youtube() {
        // Nothing was asked, so there is no refusal to report: an unusable
        // player script is the whole story, and the clients it cost us are
        // named so the log and the message agree.
        let skipped = [ClientKind::TvDowngraded, ClientKind::Mweb];
        let msg = explain_failure(None, true, false, &skipped);
        assert!(msg.contains("player script"), "{msg}");
        assert!(msg.contains("tv_downgraded") && msg.contains("mweb"), "{msg}");
        assert!(!msg.contains("server-driven"), "the generic no-formats text would mislead here: {msg}");

        // A client that *was* asked and refused outranks it: YouTube actually
        // said something, and that is more actionable than a local fault.
        let bot = Playability { status: "LOGIN_REQUIRED".into(), reason: Some("Sign in to confirm you’re not a bot".into()) };
        let msg = explain_failure(Some(&(ClientKind::Ios, bot)), false, false, &skipped);
        assert!(msg.contains("bot"), "{msg}");
    }

    // Diagnoses how the CDN treats the engine's own request shape: HTTP/2
    // versus HTTP/1.1, several ranges at once, and — with
    // `ZOLTRAAK_TEST_POT` / `ZOLTRAAK_TEST_VISITOR` set to a browser-harvested
    // pair — whether a proof-of-origin token lifts the anonymous preview cap
    // (a range deep into the file). Run on request.
    #[tokio::test]
    #[ignore]
    async fn live_cdn_range_request_shapes() {
        let client = reqwest::Client::new();
        let mut headers = Headers::new();
        if let (Ok(pot), Ok(visitor)) = (
            std::env::var("ZOLTRAAK_TEST_POT").or_else(|_| std::env::var("OPENDM_TEST_POT")),
            std::env::var("ZOLTRAAK_TEST_VISITOR").or_else(|_| std::env::var("OPENDM_TEST_VISITOR")),
        ) {
            headers.push((POTOKEN_HEADER.into(), pot));
            headers.push((VISITOR_HEADER.into(), visitor));
        }
        let meta = analyze(&client, "https://www.youtube.com/watch?v=9FtpzAqIPIU", &headers).await.unwrap();
        let Some(f) = meta.formats.iter().find(|f| f.height == Some(720)) else {
            println!("no 720p rendition from this client; formats: {:?}", meta.formats.iter().map(|f| &f.id).collect::<Vec<_>>());
            return;
        };
        let url = f.video_url.clone().unwrap();
        println!("format {} via {:?}; pot on url: {}", f.id, f.headers, query_param(&url, "pot").is_some());
        {
            let deep = client.get(&url).header("User-Agent", f.headers.iter().find(|(k, _)| k == "User-Agent").map(|(_, v)| v.clone()).unwrap_or_default()).header("Range", "bytes=31457280-33554431").send().await.unwrap();
            println!("deep range (30MB in): {}", deep.status());
            let deeper = client.get(&url).header("Range", "bytes=104857600-106954751").send().await.unwrap();
            println!("deeper range (100MB in): {}", deeper.status());
        }
        let ua = f.headers.iter().find(|(k, _)| k == "User-Agent").map(|(_, v)| v.clone()).unwrap();
        let fetch = |client: reqwest::Client, url: String, ua: String, range: &'static str| async move {
            let r = client.get(&url).header("User-Agent", ua).header("Range", range).send().await.unwrap();
            (r.status().as_u16(), format!("{:?}", r.version()))
        };
        println!("h2 default   : {:?}", fetch(client.clone(), url.clone(), ua.clone(), "bytes=0-2097151").await);
        let h1 = reqwest::Client::builder().http1_only().build().unwrap();
        println!("http1 only   : {:?}", fetch(h1.clone(), url.clone(), ua.clone(), "bytes=2097152-4194303").await);
        let ranges = ["bytes=4194304-6291455", "bytes=6291456-8388607", "bytes=8388608-10485759", "bytes=10485760-12582911"];
        let joined = futures_util::future::join_all(ranges.iter().map(|r| fetch(client.clone(), url.clone(), ua.clone(), r))).await;
        println!("h2 4 parallel: {joined:?}");
        let ranges = ["bytes=12582912-14680063", "bytes=14680064-16777215", "bytes=16777216-18874367", "bytes=18874368-20971519"];
        let joined = futures_util::future::join_all(ranges.iter().map(|r| fetch(h1.clone(), url.clone(), ua.clone(), r))).await;
        println!("h1 4 parallel: {joined:?}");
    }

    // Hits YouTube for real; run on request with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_analyze_resolves_fetchable_formats() {
        let _ = tracing_subscriber::fmt().with_env_filter("zoltraak_core::video::youtube=debug").with_test_writer().try_init();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(30))
            .build().unwrap();
        let url = std::env::var("ZOLTRAAK_TEST_YOUTUBE_URL")
            .or_else(|_| std::env::var("OPENDM_TEST_YOUTUBE_URL"))
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
        let meta = analyze(&client, &url, &Headers::new()).await.unwrap();
        println!("{} formats for {:?}", meta.formats.len(), meta.title);
        assert!(!meta.formats.is_empty());
        let best = meta.formats.iter().max_by_key(|f| f.filesize.unwrap_or(0)).unwrap();
        println!("downloading format {} ({:?}p)", best.id, best.height);
        let url = best.video_url.as_ref().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("video.part");
        let result = super::super::stream::fetch(&client, url, &best.headers, &dest).await.unwrap();
        match result {
            super::super::stream::StreamOutcome::Completed { bytes } => assert!(bytes > 0),
            super::super::stream::StreamOutcome::Refused { status, progress, .. } =>
                panic!("CDN refused with {status} after {} bytes", progress.bytes),
            _ => panic!("download unexpectedly paused"),
        }
        let mut total = std::fs::metadata(&dest).unwrap().len();
        if let Some(audio_url) = &best.audio_url {
            let audio = dir.path().join("audio.part");
            let result = super::super::stream::fetch(&client, audio_url, &best.headers, &audio).await.unwrap();
            assert!(matches!(result, super::super::stream::StreamOutcome::Completed { bytes } if bytes > 0));
            total += std::fs::metadata(&audio).unwrap().len();
        }
        if let Some(expected) = best.filesize {
            assert_eq!(total, expected, "complete audio/video byte count");
        }
        println!("complete video + audio: {total} bytes");
    }
}
