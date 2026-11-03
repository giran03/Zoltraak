use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const ONLINE_TTL: Duration = Duration::from_secs(65);

#[derive(Debug, Clone)]
pub struct ExtensionHeartbeat {
    pub client_id: String,
    pub browser_id: String,
    pub extension_version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionClientSummary {
    pub client_id: String,
    pub browser_id: String,
    pub browser_name: String,
    pub extension_version: String,
    pub last_seen_at_ms: Option<u64>,
    pub online: bool,
    pub preferred: bool,
    pub active: bool,
    pub temporary: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionSourceState {
    pub revision: u64,
    pub preferred_client_id: Option<String>,
    pub active_client_id: Option<String>,
    pub clients: Vec<ExtensionClientSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreferredSource {
    client_id: String,
    browser_id: String,
    extension_version: String,
}

#[derive(Debug, Clone)]
struct ClientRecord {
    client_id: String,
    browser_id: String,
    extension_version: String,
    last_seen: Option<Instant>,
    last_seen_at_ms: Option<u64>,
}

impl From<&PreferredSource> for ClientRecord {
    fn from(value: &PreferredSource) -> Self {
        Self {
            client_id: value.client_id.clone(),
            browser_id: value.browser_id.clone(),
            extension_version: value.extension_version.clone(),
            last_seen: None,
            last_seen_at_ms: None,
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    clients: HashMap<String, ClientRecord>,
    preferred: Option<PreferredSource>,
    effective: Option<String>,
    revision: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Mutation {
    pub source_changed: bool,
}

#[derive(Clone)]
pub struct ExtensionSourceManager {
    inner: Arc<Mutex<Inner>>,
    path: Option<Arc<PathBuf>>,
    changes: Arc<tokio::sync::watch::Sender<u64>>,
}

impl Default for ExtensionSourceManager {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl ExtensionSourceManager {
    pub fn in_memory() -> Self {
        Self::new(None)
    }

    pub fn open(path: PathBuf) -> Self {
        Self::new(Some(path))
    }

    fn new(path: Option<PathBuf>) -> Self {
        let preferred = path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<PreferredSource>(&bytes).ok())
            .filter(valid_preference);
        let mut inner = Inner {
            preferred: preferred.clone(),
            ..Inner::default()
        };
        if let Some(preferred) = &preferred {
            inner
                .clients
                .insert(preferred.client_id.clone(), ClientRecord::from(preferred));
        }
        let (changes, _) = tokio::sync::watch::channel(0);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            path: path.map(Arc::new),
            changes: Arc::new(changes),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn touch(&self, heartbeat: ExtensionHeartbeat) -> io::Result<Mutation> {
        validate_client_id(&heartbeat.client_id)?;
        let browser_id = normalize_browser_id(&heartbeat.browser_id);
        let extension_version = heartbeat
            .extension_version
            .trim()
            .chars()
            .take(32)
            .collect::<String>();
        let now = Instant::now();
        let mut inner = self.inner.lock().expect("extension source lock poisoned");
        inner.clients.insert(
            heartbeat.client_id.clone(),
            ClientRecord {
                client_id: heartbeat.client_id.clone(),
                browser_id: browser_id.clone(),
                extension_version: extension_version.clone(),
                last_seen: Some(now),
                last_seen_at_ms: Some(unix_ms()),
            },
        );

        if inner.preferred.is_none() {
            let preferred = PreferredSource {
                client_id: heartbeat.client_id,
                browser_id,
                extension_version,
            };
            self.persist(&preferred)?;
            inner.preferred = Some(preferred);
        }
        Ok(recompute(&mut inner, now))
    }

    pub fn refresh(&self) -> Mutation {
        let mut inner = self.inner.lock().expect("extension source lock poisoned");
        recompute(&mut inner, Instant::now())
    }

    pub fn select(&self, client_id: &str) -> io::Result<Mutation> {
        validate_client_id(client_id)?;
        let mut inner = self.inner.lock().expect("extension source lock poisoned");
        let client =
            inner.clients.get(client_id).cloned().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "extension client not found")
            })?;
        let preferred = PreferredSource {
            client_id: client.client_id,
            browser_id: client.browser_id,
            extension_version: client.extension_version,
        };
        self.persist(&preferred)?;
        inner.preferred = Some(preferred);
        Ok(recompute(&mut inner, Instant::now()))
    }

    pub fn commit_source_change(&self) {
        let revision = {
            let mut inner = self.inner.lock().expect("extension source lock poisoned");
            inner.revision = inner.revision.saturating_add(1);
            inner.revision
        };
        let _ = self.changes.send(revision);
    }

    pub fn is_active(&self, client_id: &str) -> bool {
        self.inner
            .lock()
            .expect("extension source lock poisoned")
            .effective
            .as_deref()
            == Some(client_id)
    }

    pub fn state(&self) -> ExtensionSourceState {
        let inner = self.inner.lock().expect("extension source lock poisoned");
        let now = Instant::now();
        let preferred_id = inner.preferred.as_ref().map(|p| p.client_id.as_str());
        let mut clients = inner
            .clients
            .values()
            .map(|client| {
                let online = is_online(client, now);
                let preferred = preferred_id == Some(client.client_id.as_str());
                let active = inner.effective.as_deref() == Some(client.client_id.as_str());
                ExtensionClientSummary {
                    client_id: client.client_id.clone(),
                    browser_id: client.browser_id.clone(),
                    browser_name: browser_name(&client.browser_id).to_string(),
                    extension_version: client.extension_version.clone(),
                    last_seen_at_ms: client.last_seen_at_ms,
                    online,
                    preferred,
                    active,
                    temporary: active && !preferred,
                }
            })
            .collect::<Vec<_>>();
        clients.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| b.online.cmp(&a.online))
                .then_with(|| a.browser_name.cmp(&b.browser_name))
        });
        ExtensionSourceState {
            revision: inner.revision,
            preferred_client_id: inner.preferred.as_ref().map(|p| p.client_id.clone()),
            active_client_id: inner.effective.clone(),
            clients,
        }
    }

    fn persist(&self, preferred: &PreferredSource) -> io::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(preferred)?)?;
        std::fs::rename(tmp, path)
    }
}

fn recompute(inner: &mut Inner, now: Instant) -> Mutation {
    let preferred = inner.preferred.as_ref().map(|p| p.client_id.clone());
    inner.clients.retain(|client_id, client| {
        preferred.as_deref() == Some(client_id.as_str()) || is_online(client, now)
    });
    let online = inner
        .clients
        .values()
        .filter(|client| is_online(client, now))
        .map(|client| client.client_id.as_str())
        .collect::<Vec<_>>();
    let next = preferred
        .as_deref()
        .filter(|id| online.contains(id))
        .map(str::to_string)
        .or_else(|| (online.len() == 1).then(|| online[0].to_string()));
    let source_changed = inner.effective != next;
    inner.effective = next;
    Mutation { source_changed }
}

fn is_online(client: &ClientRecord, now: Instant) -> bool {
    client
        .last_seen
        .is_some_and(|last_seen| now.saturating_duration_since(last_seen) <= ONLINE_TTL)
}

fn validate_client_id(client_id: &str) -> io::Result<()> {
    uuid::Uuid::parse_str(client_id)
        .map(|_| ())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid extension client id"))
}

fn valid_preference(preferred: &PreferredSource) -> bool {
    validate_client_id(&preferred.client_id).is_ok()
}

fn normalize_browser_id(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "chrome" | "edge" | "brave" | "vivaldi" | "opera" | "firefox" | "zen" => {
            value.trim().to_ascii_lowercase()
        }
        value if value.contains("firefox") || value.contains("gecko") => "firefox".into(),
        value if value.contains("chrom") => "chromium".into(),
        _ => "browser".into(),
    }
}

fn browser_name(browser_id: &str) -> &'static str {
    match browser_id {
        "chrome" => "Google Chrome",
        "edge" => "Microsoft Edge",
        "brave" => "Brave",
        "vivaldi" => "Vivaldi",
        "opera" => "Opera",
        "firefox" => "Firefox",
        "zen" => "Zen",
        "chromium" => "Chromium browser",
        _ => "Browser",
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heartbeat(id: &str, browser: &str) -> ExtensionHeartbeat {
        ExtensionHeartbeat {
            client_id: id.into(),
            browser_id: browser.into(),
            extension_version: "1.2.3".into(),
        }
    }

    #[test]
    fn first_client_is_preferred_and_duplicate_profiles_are_distinct() {
        let manager = ExtensionSourceManager::in_memory();
        let a = "00000000-0000-4000-8000-000000000001";
        let b = "00000000-0000-4000-8000-000000000002";
        assert!(
            manager
                .touch(heartbeat(a, "chrome"))
                .unwrap()
                .source_changed
        );
        manager.commit_source_change();
        manager.touch(heartbeat(b, "chrome")).unwrap();
        let state = manager.state();
        assert_eq!(state.preferred_client_id.as_deref(), Some(a));
        assert_eq!(state.active_client_id.as_deref(), Some(a));
        assert_eq!(state.clients.len(), 2);
    }

    #[test]
    fn manual_selection_persists_and_is_restored_offline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extension-source.json");
        let a = "00000000-0000-4000-8000-000000000001";
        let b = "00000000-0000-4000-8000-000000000002";
        let manager = ExtensionSourceManager::open(path.clone());
        manager.touch(heartbeat(a, "chrome")).unwrap();
        manager.touch(heartbeat(b, "firefox")).unwrap();
        assert!(manager.select(b).unwrap().source_changed);

        let reopened = ExtensionSourceManager::open(path);
        let state = reopened.state();
        assert_eq!(state.preferred_client_id.as_deref(), Some(b));
        assert_eq!(state.active_client_id, None);
        assert_eq!(state.clients[0].browser_name, "Firefox");
    }

    #[test]
    fn preferred_source_wins_again_after_temporary_failover() {
        let manager = ExtensionSourceManager::in_memory();
        let a = "00000000-0000-4000-8000-000000000001";
        let b = "00000000-0000-4000-8000-000000000002";
        manager.touch(heartbeat(a, "chrome")).unwrap();
        manager.touch(heartbeat(b, "firefox")).unwrap();
        {
            let mut inner = manager.inner.lock().unwrap();
            inner.clients.get_mut(a).unwrap().last_seen =
                Some(Instant::now() - ONLINE_TTL - Duration::from_secs(1));
        }
        assert!(manager.refresh().source_changed);
        assert_eq!(manager.state().active_client_id.as_deref(), Some(b));
        assert!(
            manager
                .touch(heartbeat(a, "chrome"))
                .unwrap()
                .source_changed
        );
        assert_eq!(manager.state().active_client_id.as_deref(), Some(a));
    }

    #[test]
    fn ambiguous_failover_uses_no_cookie_source() {
        let manager = ExtensionSourceManager::in_memory();
        let ids = [
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000003",
        ];
        for (id, browser) in ids.iter().zip(["chrome", "firefox", "edge"]) {
            manager.touch(heartbeat(id, browser)).unwrap();
        }
        {
            let mut inner = manager.inner.lock().unwrap();
            inner.clients.get_mut(ids[0]).unwrap().last_seen =
                Some(Instant::now() - ONLINE_TTL - Duration::from_secs(1));
        }
        manager.refresh();
        assert_eq!(manager.state().active_client_id, None);
    }
}
