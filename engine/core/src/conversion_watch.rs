//! Folder rules that convert new files automatically.
//!
//! A rule only ever acts on files that appear after it starts watching: what is
//! already in the folder is left alone. A file is converted once it stops
//! growing, and results go to a separate folder so they are never picked up again.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::conversion::Settings;
use crate::error::{Error, Result};
use crate::remux::manager::{NewRemux, RemuxManager};

pub const MAX_RULES: usize = 16;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_OUTPUT_FOLDER: &str = "Converted";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchRule {
    pub id: String,
    pub folder: PathBuf,
    /// Lower-case extensions without the dot, such as `webm`.
    pub extensions: Vec<String>,
    pub settings: Settings,
    /// Where results go. Empty means a `Converted` folder inside the watched one.
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    pub enabled: bool,
}

impl WatchRule {
    fn output(&self) -> PathBuf {
        self.output_directory
            .clone()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| self.folder.join(DEFAULT_OUTPUT_FOLDER))
    }

    fn validate(&self) -> Result<()> {
        let fail = |message: &str| Err(Error::InvalidInput(message.into()));
        if self.id.trim().is_empty() || self.id.len() > 64 {
            return fail("Each rule needs a short id");
        }
        if !self.folder.is_dir() {
            return fail("The watched folder does not exist");
        }
        if self.extensions.is_empty() || self.extensions.len() > 32 {
            return fail("Choose at least one file type to watch for");
        }
        if self.extensions.iter().any(|ext| {
            !crate::remux::MEDIA_EXTENSIONS.contains(&ext.as_str())
        }) {
            return fail("Watch rules can only match video, audio, image and animation files");
        }
        if same_folder(&self.output(), &self.folder) {
            return fail("Results must go to a different folder than the one being watched");
        }
        self.settings.validate()
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        std::path::absolute(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    key(a) == key(b)
}

/// How a rule has fared since the app started.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub rule_id: String,
    pub queued: u32,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct WatchState {
    /// Rules (by id and folder) whose existing files have been set aside.
    baselined: HashSet<(String, PathBuf)>,
    handled: HashSet<PathBuf>,
    pending: HashMap<PathBuf, (u64, Option<SystemTime>)>,
    statuses: HashMap<String, WatchStatus>,
}

#[derive(Clone)]
pub struct ConversionWatcher {
    path: Option<PathBuf>,
    rules: Arc<Mutex<Vec<WatchRule>>>,
    state: Arc<Mutex<WatchState>>,
}

impl ConversionWatcher {
    /// Load saved rules from `path`. A missing or unreadable file means no rules.
    pub fn load(path: PathBuf) -> Self {
        let rules = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<WatchRule>>(&text).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            rules: Arc::new(Mutex::new(rules)),
            state: Arc::default(),
        }
    }

    /// Rules kept only in memory, for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            rules: Arc::default(),
            state: Arc::default(),
        }
    }

    pub fn rules(&self) -> Vec<WatchRule> {
        self.rules.lock().expect("watch rules").clone()
    }

    pub fn statuses(&self) -> Vec<WatchStatus> {
        self.state
            .lock()
            .expect("watch state")
            .statuses
            .values()
            .cloned()
            .collect()
    }

    /// Replace every rule. All are checked first; nothing changes if one is invalid.
    pub fn set_rules(&self, rules: Vec<WatchRule>) -> Result<()> {
        if rules.len() > MAX_RULES {
            return Err(Error::InvalidInput(format!(
                "Use at most {MAX_RULES} watch rules"
            )));
        }
        let mut ids = HashSet::new();
        for rule in &rules {
            rule.validate()?;
            if !ids.insert(rule.id.clone()) {
                return Err(Error::InvalidInput("Watch rule ids must be unique".into()));
            }
        }
        if let Some(path) = &self.path {
            let text = serde_json::to_string_pretty(&rules)
                .map_err(|e| Error::Corrupt(e.to_string()))?;
            let staging = path.with_extension("json.tmp");
            std::fs::write(&staging, text).map_err(|source| Error::Io {
                path: staging.clone(),
                source,
            })?;
            std::fs::rename(&staging, path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        }
        *self.rules.lock().expect("watch rules") = rules;
        Ok(())
    }

    /// Check every enabled rule once, converting files that have settled.
    pub async fn poll(&self, manager: &RemuxManager) {
        for rule in self.rules().into_iter().filter(|rule| rule.enabled) {
            let files = list_candidates(&rule);
            let ready = self.settle(&rule, files);
            for path in ready {
                self.convert(&rule, manager, path).await;
            }
        }
    }

    /// Decide which candidates are new and finished growing.
    fn settle(&self, rule: &WatchRule, files: Vec<(PathBuf, u64, Option<SystemTime>)>) -> Vec<PathBuf> {
        let mut state = self.state.lock().expect("watch state");
        let key = (rule.id.clone(), rule.folder.clone());
        if state.baselined.insert(key) {
            // First look at this rule: everything here already existed.
            state.handled.extend(files.into_iter().map(|(path, ..)| path));
            return Vec::new();
        }
        let mut ready = Vec::new();
        for (path, size, modified) in files {
            if state.handled.contains(&path) || size == 0 {
                continue;
            }
            match state.pending.get(&path) {
                Some(previous) if *previous == (size, modified) => ready.push(path),
                _ => {
                    state.pending.insert(path, (size, modified));
                }
            }
        }
        for path in &ready {
            state.pending.remove(path);
            state.handled.insert(path.clone());
        }
        ready
    }

    async fn convert(&self, rule: &WatchRule, manager: &RemuxManager, path: PathBuf) {
        let request = NewRemux {
            inputs: vec![path],
            container: rule
                .settings
                .format
                .container()
                .unwrap_or(crate::remux::Container::Mp4),
            output_directory: Some(rule.output()),
            replace_source: false,
        };
        let outcome = manager
            .enqueue_conversion(request, rule.settings.clone())
            .await;
        let mut state = self.state.lock().expect("watch state");
        let status = state
            .statuses
            .entry(rule.id.clone())
            .or_insert_with(|| WatchStatus {
                rule_id: rule.id.clone(),
                ..Default::default()
            });
        match outcome {
            Ok(ids) => {
                status.queued += ids.len() as u32;
                status.last_error = None;
            }
            Err(error) => status.last_error = Some(error.to_string()),
        }
    }

    /// Poll forever on a timer. Dropping the runtime stops it.
    pub fn spawn(self, manager: RemuxManager) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                self.poll(&manager).await;
            }
        });
    }
}

fn list_candidates(rule: &WatchRule) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    let Ok(entries) = std::fs::read_dir(&rule.folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_lowercase();
            if !rule.extensions.contains(&extension) {
                return None;
            }
            let metadata = entry.metadata().ok().filter(|m| m.is_file())?;
            Some((path, metadata.len(), metadata.modified().ok()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(folder: &Path) -> WatchRule {
        WatchRule {
            id: "r1".into(),
            folder: folder.to_path_buf(),
            extensions: vec!["webm".into()],
            settings: Settings::default(),
            output_directory: None,
            enabled: true,
        }
    }

    #[test]
    fn existing_files_are_set_aside_and_new_ones_wait_until_they_stop_growing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.webm"), b"old").unwrap();
        let watcher = ConversionWatcher::in_memory();
        let rule = rule(dir.path());
        assert!(watcher.settle(&rule, list_candidates(&rule)).is_empty());

        std::fs::write(dir.path().join("new.webm"), b"abc").unwrap();
        std::fs::write(dir.path().join("note.txt"), b"x").unwrap();
        assert!(watcher.settle(&rule, list_candidates(&rule)).is_empty(), "first sighting waits");
        std::fs::write(dir.path().join("new.webm"), b"abcdef").unwrap();
        assert!(watcher.settle(&rule, list_candidates(&rule)).is_empty(), "still growing");
        let ready = watcher.settle(&rule, list_candidates(&rule));
        assert_eq!(ready, vec![dir.path().join("new.webm")]);
        assert!(watcher.settle(&rule, list_candidates(&rule)).is_empty(), "converted once");
    }

    #[test]
    fn rules_are_validated_before_anything_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = ConversionWatcher::in_memory();
        assert!(watcher.set_rules(vec![rule(dir.path())]).is_ok());
        assert_eq!(watcher.rules().len(), 1);

        let mut same_output = rule(dir.path());
        same_output.output_directory = Some(dir.path().to_path_buf());
        let mut bad_type = rule(dir.path());
        bad_type.extensions = vec!["exe".into()];
        let mut missing = rule(dir.path());
        missing.folder = dir.path().join("nope");
        for bad in [same_output, bad_type, missing] {
            assert!(watcher.set_rules(vec![bad]).is_err());
        }
        assert_eq!(watcher.rules().len(), 1, "failed saves change nothing");

        let duplicate = vec![rule(dir.path()), rule(dir.path())];
        assert!(watcher.set_rules(duplicate).is_err());
    }

    #[test]
    fn rules_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rules.json");
        let watched = tempfile::tempdir().unwrap();
        let watcher = ConversionWatcher::load(file.clone());
        watcher.set_rules(vec![rule(watched.path())]).unwrap();
        assert_eq!(ConversionWatcher::load(file).rules().len(), 1);
    }
}
