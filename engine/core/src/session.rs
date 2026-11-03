//! The session vault: browser sessions the extension shares with the engine,
//! held encrypted at rest.
//!
//! A download behind a login needs the session that unlocked it, and a video
//! extractor needs the same to see age-gated or members-only content. The
//! browser extension therefore hands over the relevant cookies (and, for
//! YouTube, the proof-of-origin token the player minted). Those are the user's
//! credentials, so the engine treats them the way a browser treats its own
//! cookie jar:
//!
//! - **Minimised.** Only the cookie names an extractor actually needs are
//!   kept; everything else is dropped on arrival (see [`minimize_for`]).
//! - **Encrypted at rest.** The vault file is AES-256-GCM. Its key is wrapped
//!   with DPAPI on Windows (only the same Windows user can unwrap it), and
//!   kept in an owner-only file elsewhere. Copying the vault file alone
//!   yields nothing.
//! - **Never persisted anywhere else.** Job rows in SQLite carry no cookies;
//!   a resumed job re-attaches its session from here.
//! - **Never logged.** The types' `Debug` output redacts every value.
//! - **Forgettable.** One call removes a site's session, or all of them, and
//!   the file with it.
//!
//! This is defence in depth for a local-only file, not a cross-machine
//! secret store: a process running as the same user could still read the
//! wrapped key — as it could read the browser's own cookie database.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use zeroize::Zeroizing;

use crate::headers::Headers;
use crate::vault::{decrypt, encrypt, key_store};

/// One site's session as the extension shared it.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SiteSession {
    /// A single `Cookie` header value, already minimised for the site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    /// YouTube GVS token, paired with its captured video and visitor context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub po_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visitor_data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub po_token_video_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for SiteSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteSession")
            .field("cookie", &self.cookie.as_ref().map(|_| "<redacted>"))
            .field("po_token", &self.po_token.as_ref().map(|_| "<redacted>"))
            .field("visitor_data", &self.visitor_data.as_ref().map(|_| "<redacted>"))
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl SiteSession {
    /// Build from the header set an API client sends: the `Cookie` header and
    /// the extension's YouTube pseudo-headers.
    pub fn from_headers(headers: &Headers) -> Self {
        let lookup = |name: &str| -> Option<String> {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            cookie: lookup("cookie"),
            po_token: lookup(crate::video::youtube::POTOKEN_HEADER),
            visitor_data: lookup(crate::video::youtube::VISITOR_HEADER),
            po_token_video_id: lookup(crate::video::youtube::POTOKEN_VIDEO_HEADER),
            updated_at: Utc::now(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cookie.is_none() && self.po_token.is_none() && self.visitor_data.is_none()
    }

    /// The headers to attach to a request for this site.
    pub fn to_headers(&self) -> Headers {
        let mut out = Headers::new();
        if let Some(c) = &self.cookie {
            out.push(("Cookie".into(), c.clone()));
        }
        if let Some(p) = &self.po_token {
            out.push((crate::video::youtube::POTOKEN_HEADER.into(), p.clone()));
        }
        if let Some(v) = &self.visitor_data {
            out.push((crate::video::youtube::VISITOR_HEADER.into(), v.clone()));
        }
        if let Some(v) = &self.po_token_video_id {
            out.push((crate::video::youtube::POTOKEN_VIDEO_HEADER.into(), v.clone()));
        }
        out
    }

    /// Names of the cookies held, for display. Values never leave the vault.
    pub fn cookie_names(&self) -> Vec<String> {
        self.cookie
            .as_deref()
            .map(|c| {
                c.split(';')
                    .filter_map(|pair| pair.trim().split('=').next())
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// What a UI may show about a stored session: which site, when, and what
/// kind of material — never the material itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub domain: String,
    pub updated_at: DateTime<Utc>,
    pub cookie_names: Vec<String>,
    pub has_po_token: bool,
    /// Whether the cookie amounts to a login, where the engine can tell.
    pub signed_in: bool,
}

/// Reduce a site's cookie to what its extractor needs. Sites without a known
/// allowlist keep their cookie as sent — the extractor for them uses the
/// whole session, and the extension already scoped it to that site.
pub fn minimize_for(domain: &str, cookie: &str) -> Option<String> {
    if domain.eq_ignore_ascii_case("youtube.com") {
        return crate::video::youtube::minimize_cookie(cookie);
    }
    let trimmed = cookie.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn signed_in_for(domain: &str, cookie: Option<&str>) -> bool {
    match cookie {
        Some(c) if domain.eq_ignore_ascii_case("youtube.com") => crate::video::youtube::cookie_signs_in(c),
        Some(_) => true,
        None => false,
    }
}

/// Keys within the vault that belong to one download job rather than a site:
/// a cookie the extension sent for a single link on a site without a known
/// session domain. Removed with the job.
pub fn job_key(id: &crate::model::DownloadId) -> String {
    format!("job:{id}")
}

const FILE_NAME: &str = "sessions.vault";
const KEY_FILE_NAME: &str = "sessions.key";
const AAD: &[u8] = b"zoltraak-sessions-v1";
const LEGACY_AAD: &[u8] = b"opendm-sessions-v1";

pub struct SessionVault {
    /// `None` for an in-memory vault (tests, or no config directory).
    path: Option<PathBuf>,
    key: Option<Zeroizing<[u8; 32]>>,
    entries: RwLock<HashMap<String, SiteSession>>,
    /// Announces the domain whose session just changed, so work that is
    /// blocked for want of one can pick itself up the moment it arrives
    /// rather than waiting to be retried by hand. Carries the domain only —
    /// never the material.
    changes: broadcast::Sender<String>,
}

impl fmt::Debug for SessionVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionVault").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Default for SessionVault {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl SessionVault {
    /// A vault that forgets everything when the process exits.
    pub fn in_memory() -> Self {
        Self { path: None, key: None, entries: RwLock::new(HashMap::new()), changes: broadcast::channel(16).0 }
    }

    /// Watch for sessions arriving, leaving or being refreshed. Each message
    /// is the domain that changed.
    ///
    /// A receiver that falls behind is told it lagged rather than given a
    /// partial history; every consumer here re-reads the vault anyway, so a
    /// missed message costs nothing.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    /// Whether a session is stored for `domain` and amounts to a login.
    pub async fn is_signed_in(&self, domain: &str) -> bool {
        let domain = domain.trim().to_ascii_lowercase();
        match self.entries.read().await.get(&domain) {
            Some(session) => signed_in_for(&domain, session.cookie.as_deref()),
            None => false,
        }
    }

    /// Re-read the vault from disk, replacing what is held in memory.
    ///
    /// The file is shared: the desktop app, a headless `zoltraak serve` and the
    /// CLI all open the same one, and only the process the extension talks to
    /// learns of a new session first-hand. This is how the others find out —
    /// so a download that stalled for want of a sign-in can check whether one
    /// has since arrived instead of insisting on what it read at startup.
    ///
    /// Returns whether anything actually changed. An in-memory vault has no
    /// file to consult and always reports `false`.
    pub async fn reload(&self) -> io::Result<bool> {
        let (Some(path), Some(key)) = (&self.path, &self.key) else { return Ok(false) };
        let fresh: HashMap<String, SiteSession> = match std::fs::read(path) {
            Ok(bytes) => match decrypt(key, AAD, &bytes).or_else(|_| decrypt(key, LEGACY_AAD, &bytes)) {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "session vault unreadable on reload; keeping what is in memory");
                    return Ok(false);
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };

        let mut entries = self.entries.write().await;
        // Compare on the update timestamps and the key set: the sessions
        // themselves are secrets, and nothing here needs to look at them.
        let changed = fresh.len() != entries.len()
            || fresh.iter().any(|(domain, session)| entries.get(domain).is_none_or(|held| held.updated_at != session.updated_at));
        if !changed {
            return Ok(false);
        }
        let domains: Vec<String> = fresh.keys().cloned().chain(entries.keys().cloned()).collect();
        *entries = fresh;
        drop(entries);
        for domain in domains {
            let _ = self.changes.send(domain);
        }
        Ok(true)
    }

    /// Open (or create) the vault in `dir`. A file that cannot be decrypted —
    /// a lost key, another user's profile, tampering — is treated as empty
    /// and overwritten on the next write: the sessions are re-shared by the
    /// extension on the next visit, so nothing of value is lost by starting
    /// over, and refusing to start would strand every other feature.
    pub fn open(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let key = key_store::load_or_create(&dir.join(KEY_FILE_NAME))?;
        let path = dir.join(FILE_NAME);
        let entries = match std::fs::read(&path) {
            Ok(bytes) => match decrypt(&key, AAD, &bytes).or_else(|_| decrypt(&key, LEGACY_AAD, &bytes)) {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "session vault unreadable; starting empty");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };
        tracing::info!(path = %path.display(), sessions = entries.len(), "session vault opened");
        Ok(Self { path: Some(path), key: Some(key), entries: RwLock::new(entries), changes: broadcast::channel(16).0 })
    }

    pub fn is_persistent(&self) -> bool {
        self.path.is_some()
    }

    pub async fn get(&self, domain: &str) -> Option<SiteSession> {
        self.entries.read().await.get(&domain.to_ascii_lowercase()).cloned()
    }

    /// Store a site's session, minimising its cookie first. An empty session
    /// (nothing usable after minimising) removes any existing entry instead:
    /// the browser has been signed out, and a stale session must not outlive
    /// that.
    pub async fn set(&self, domain: &str, mut session: SiteSession) -> io::Result<()> {
        let domain = domain.trim().to_ascii_lowercase();
        session.cookie = session.cookie.as_deref().and_then(|c| minimize_for(&domain, c));
        session.updated_at = Utc::now();
        let mut entries = self.entries.write().await;
        if session.is_empty() {
            entries.remove(&domain);
        } else {
            entries.insert(domain.clone(), session);
        }
        let result = self.persist(&entries);
        drop(entries);
        // Announced after the write, so anything woken by this reads a vault
        // that already holds the new session.
        let _ = self.changes.send(domain);
        result
    }

    pub async fn remove(&self, domain: &str) -> io::Result<bool> {
        let domain = domain.trim().to_ascii_lowercase();
        let mut entries = self.entries.write().await;
        let removed = entries.remove(&domain).is_some();
        if removed {
            self.persist(&entries)?;
        }
        drop(entries);
        if removed {
            let _ = self.changes.send(domain);
        }
        Ok(removed)
    }

    pub async fn clear(&self) -> io::Result<()> {
        let mut entries = self.entries.write().await;
        entries.clear();
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Remove browser-wide site sessions while preserving credentials tied to
    /// an already-created job. Switching the active browser must not make an
    /// in-flight resumable download forget the session it started with.
    pub async fn clear_shared(&self) -> io::Result<()> {
        let mut entries = self.entries.write().await;
        let domains = entries
            .keys()
            .filter(|key| !key.starts_with("job:"))
            .cloned()
            .collect::<Vec<_>>();
        if domains.is_empty() {
            return Ok(());
        }
        for domain in &domains {
            entries.remove(domain);
        }
        self.persist(&entries)?;
        drop(entries);
        for domain in domains {
            let _ = self.changes.send(domain);
        }
        Ok(())
    }

    /// Site sessions only — job-scoped entries are internal.
    pub async fn summaries(&self) -> Vec<SessionSummary> {
        let entries = self.entries.read().await;
        let mut out: Vec<SessionSummary> = entries
            .iter()
            .filter(|(domain, _)| !domain.starts_with("job:"))
            .map(|(domain, s)| SessionSummary {
                domain: domain.clone(),
                updated_at: s.updated_at,
                cookie_names: s.cookie_names(),
                has_po_token: s.po_token.is_some(),
                signed_in: signed_in_for(domain, s.cookie.as_deref()),
            })
            .collect();
        out.sort_by(|a, b| a.domain.cmp(&b.domain));
        out
    }

    fn persist(&self, entries: &HashMap<String, SiteSession>) -> io::Result<()> {
        let (Some(path), Some(key)) = (&self.path, &self.key) else { return Ok(()) };
        if entries.is_empty() {
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
        }
        let bytes = encrypt(key, AAD, entries)?;
        // Write-then-rename so a crash mid-write leaves the previous vault,
        // not half of a new one.
        let tmp = path.with_extension("vault.tmp");
        key_store::write_private(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cookie that reads as a YouTube login: `LOGIN_INFO` plus an
    /// `*APISID`, which is the pair `cookie_signs_in` looks for.
    const SIGNED_IN: &str = "LOGIN_INFO=abc; SAPISID=def; YSC=ghi";

    #[tokio::test]
    async fn clearing_shared_sessions_preserves_job_credentials() {
        let vault = SessionVault::in_memory();
        vault.set("youtube.com", SiteSession::from_headers(&vec![("Cookie".into(), SIGNED_IN.into())])).await.unwrap();
        vault.set("job:kept", SiteSession::from_headers(&vec![("Cookie".into(), "private=1".into())])).await.unwrap();

        vault.clear_shared().await.unwrap();

        assert!(vault.get("youtube.com").await.is_none());
        assert!(vault.get("job:kept").await.is_some());
    }

    #[tokio::test]
    async fn a_reload_picks_up_another_processs_write_and_announces_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Two handles on one vault file: the desktop app and a headless
        // `zoltraak serve` are exactly this.
        let app = SessionVault::open(dir.path()).expect("open");
        let other = SessionVault::open(dir.path()).expect("open again");
        let mut changes = app.subscribe();

        assert!(!app.is_signed_in("youtube.com").await);

        other
            .set("youtube.com", SiteSession { cookie: Some(SIGNED_IN.into()), ..Default::default() })
            .await
            .expect("store");
        // The other process's write is invisible until this one re-reads.
        assert!(!app.is_signed_in("youtube.com").await, "an in-memory copy does not see another process's write on its own");

        assert!(app.reload().await.expect("reload"), "the file changed, so the reload reports it");
        assert!(app.is_signed_in("youtube.com").await, "and the session is now available");
        assert_eq!(changes.recv().await.as_deref(), Ok("youtube.com"), "a reload announces what changed");

        // A reload with nothing new is not reported as a change, so nothing
        // downstream is woken for no reason.
        assert!(!app.reload().await.expect("reload"));
    }

    #[tokio::test]
    async fn storing_and_forgetting_a_session_both_announce_the_domain() {
        let vault = SessionVault::in_memory();
        let mut changes = vault.subscribe();

        vault.set("youtube.com", SiteSession { cookie: Some(SIGNED_IN.into()), ..Default::default() }).await.expect("set");
        assert_eq!(changes.recv().await.as_deref(), Ok("youtube.com"));
        assert!(vault.is_signed_in("youtube.com").await);

        vault.remove("youtube.com").await.expect("remove");
        assert_eq!(changes.recv().await.as_deref(), Ok("youtube.com"));
        assert!(!vault.is_signed_in("youtube.com").await);
    }

    #[tokio::test]
    async fn a_signed_out_cookie_is_not_a_sign_in() {
        let vault = SessionVault::in_memory();
        // YouTube keeps a stale APISID after a logout; without LOGIN_INFO it
        // is not a session, and treating it as one would send every matched
        // track back into the same refusal.
        vault.set("youtube.com", SiteSession { cookie: Some("SAPISID=def; YSC=ghi".into()), ..Default::default() }).await.expect("set");
        assert!(!vault.is_signed_in("youtube.com").await);
        assert!(!vault.is_signed_in("nothing-stored.example").await);
    }

    fn yt_session(cookie: &str) -> SiteSession {
        SiteSession { cookie: Some(cookie.into()), po_token: Some("tok".into()), visitor_data: Some("vis".into()), po_token_video_id: Some("jNQXAC9IVRw".into()), updated_at: Utc::now() }
    }

    #[tokio::test]
    async fn a_vault_survives_a_reopen_and_never_stores_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SessionVault::open(dir.path()).unwrap();
        vault.set("youtube.com", yt_session("SAPISID=secret123; LOGIN_INFO=x; tracking=no")).await.unwrap();

        let on_disk = std::fs::read(dir.path().join("sessions.vault")).unwrap();
        let as_text = String::from_utf8_lossy(&on_disk);
        assert!(!as_text.contains("secret123"), "cookie value leaked to disk");
        assert!(!as_text.contains("tok"), "token leaked to disk");

        let reopened = SessionVault::open(dir.path()).unwrap();
        let session = reopened.get("youtube.com").await.unwrap();
        assert_eq!(session.cookie.as_deref(), Some("SAPISID=secret123; LOGIN_INFO=x"), "minimised on the way in");
        assert_eq!(session.po_token.as_deref(), Some("tok"));
        assert_eq!(session.po_token_video_id.as_deref(), Some("jNQXAC9IVRw"));
        assert_eq!(SiteSession::from_headers(&session.to_headers()).po_token_video_id, session.po_token_video_id);
    }

    #[tokio::test]
    async fn a_tampered_or_foreign_vault_is_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SessionVault::open(dir.path()).unwrap();
        vault.set("x.com", SiteSession { cookie: Some("auth_token=abc".into()), ..Default::default() }).await.unwrap();
        drop(vault);

        // Flip a byte of ciphertext.
        let path = dir.path().join("sessions.vault");
        let mut envelope: crate::vault::Envelope = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut chars: Vec<char> = envelope.ciphertext.chars().collect();
        chars[5] = if chars[5] == 'A' { 'B' } else { 'A' };
        envelope.ciphertext = chars.into_iter().collect();
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

        let reopened = SessionVault::open(dir.path()).unwrap();
        assert!(reopened.get("x.com").await.is_none());

        // A different key cannot read it either.
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::copy(dir.path().join("sessions.vault"), other.join("sessions.vault")).ok();
        let foreign = SessionVault::open(&other).unwrap();
        assert!(foreign.get("x.com").await.is_none());
    }

    #[tokio::test]
    async fn summaries_expose_names_not_values_and_forgetting_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SessionVault::open(dir.path()).unwrap();
        vault.set("youtube.com", yt_session("SAPISID=v1; LOGIN_INFO=v2")).await.unwrap();
        vault.set("instagram.com", SiteSession { cookie: Some("sessionid=zzz".into()), ..Default::default() }).await.unwrap();
        vault.set("job:abc", SiteSession { cookie: Some("k=v".into()), ..Default::default() }).await.unwrap();

        let summaries = vault.summaries().await;
        let rendered = serde_json::to_string(&summaries).unwrap();
        assert!(!rendered.contains("v1") && !rendered.contains("zzz"), "{rendered}");
        assert_eq!(summaries.iter().map(|s| s.domain.as_str()).collect::<Vec<_>>(), vec!["instagram.com", "youtube.com"]);
        let yt = summaries.iter().find(|s| s.domain == "youtube.com").unwrap();
        assert!(yt.signed_in && yt.has_po_token);
        assert_eq!(yt.cookie_names, vec!["SAPISID", "LOGIN_INFO"]);

        assert!(vault.remove("instagram.com").await.unwrap());
        assert!(!vault.remove("instagram.com").await.unwrap());
        vault.clear().await.unwrap();
        assert!(!dir.path().join("sessions.vault").exists());
        assert!(vault.summaries().await.is_empty());
    }

    #[tokio::test]
    async fn a_signed_out_session_replaces_the_stored_one_with_nothing() {
        let vault = SessionVault::in_memory();
        vault.set("youtube.com", yt_session("SAPISID=a; LOGIN_INFO=b")).await.unwrap();
        vault.set("youtube.com", SiteSession { cookie: Some("tracking=only".into()), ..Default::default() }).await.unwrap();
        assert!(vault.get("youtube.com").await.is_none(), "nothing usable remained, so the old session must go");
    }

    #[test]
    fn debug_output_never_shows_a_value() {
        let s = yt_session("SAPISID=supersecret");
        let rendered = format!("{s:?}");
        assert!(!rendered.contains("supersecret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn headers_round_trip_through_a_session() {
        let headers: Headers = vec![
            ("Cookie".into(), "a=1".into()),
            ("X-Youtube-Potoken".into(), "t".into()),
            ("X-Youtube-Visitor-Data".into(), "v".into()),
            ("Referer".into(), "https://x/".into()),
        ];
        let s = SiteSession::from_headers(&headers);
        assert_eq!(s.to_headers(), headers[..3].to_vec());
    }
}
