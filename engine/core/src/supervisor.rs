//! Long-lived queue supervisor for interactive clients.
//!
//! [`Runner`](crate::runner::Runner) drains the queue and returns, which is
//! what a CLI invocation wants. A desktop app instead needs an engine that
//! stays up for the life of the process, accepts work at any time, and can
//! pause one download without touching the others — so this owns a
//! cancellation token per in-flight item rather than one for the whole run.
//!
//! It also keeps the live rate/ETA for active downloads in memory. Those are
//! derived, change many times a second, and are meaningless once a download
//! stops, so persisting them would be pure write amplification.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use chrono::Timelike;
use serde::Serialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::downloader::Downloader;
use crate::error::{Error, Result};
use crate::model::{DownloadId, DownloadState, Progress};
use crate::queue::{NewDownload, Queue, QueueItem};
use crate::runner::RunnerConfig;
use crate::settings::{CompletionAction, EngineSettings};

/// A queue row plus whatever live state the engine holds for it.
///
/// Field names are camelCase because the desktop UI consumes this directly;
/// keeping the mapping here avoids a hand-written translation layer that would
/// drift every time a field is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSnapshot {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub directory: String,
    pub state: DownloadState,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub bytes_per_second: u64,
    pub eta_seconds: Option<u64>,
    pub connections: u8,
    pub priority: i64,
    pub error: Option<String>,
    pub created_at: String,
}

impl DownloadSnapshot {
    fn new(item: &QueueItem, live: Option<Progress>) -> Self {
        // A live sample is fresher than the row, which is only flushed on a
        // timer, so an active download would otherwise appear to stutter.
        let downloaded = live
            .map(|p| p.downloaded_bytes)
            .unwrap_or(item.downloaded_bytes)
            .max(item.downloaded_bytes);
        let total = live.and_then(|p| p.total_bytes).or(item.total_bytes);

        // Use the live active connection count when bytes are moving; fall back
        // to the planned count from the queue row for queued / finished items.
        let connections = live
            .filter(|p| p.active_connections > 0)
            .map(|p| p.active_connections)
            .unwrap_or_else(|| {
                item.connections
                    .unwrap_or_else(|| crate::default_connections(total))
            });

        Self {
            id: item.id.to_string(),
            url: item.url.clone(),
            filename: item.filename.clone(),
            directory: item.directory.to_string_lossy().into_owned(),
            state: item.state,
            total_bytes: total,
            downloaded_bytes: downloaded,
            // Rate and ETA only mean something while bytes are moving.
            bytes_per_second: live.map(|p| p.bytes_per_second).unwrap_or(0),
            eta_seconds: live.and_then(|p| p.eta_secs()),
            connections,
            priority: item.priority,
            error: item.error.clone(),
            created_at: item.created_at.to_rfc3339(),
        }
    }
}

struct Active {
    cancel: CancellationToken,
    progress: Option<Progress>,
    /// Set when the user pressed pause, as opposed to the engine shutting
    /// down. Both cancel the same way; only one should persist as paused.
    user_paused: bool,
}

struct Inner {
    queue: Queue,
    downloader: Arc<Downloader>,
    config: RunnerConfig,
    active: Mutex<HashMap<DownloadId, Active>>,
    /// Poked when the queue changes, so a new item starts immediately instead
    /// of waiting out the idle poll.
    wake: Notify,
    shutdown: CancellationToken,
    /// Cached from the database so the hot loop never blocks on a query just
    /// to check the schedule. [`Supervisor::update_settings`] keeps this and
    /// the row in sync.
    settings: RwLock<EngineSettings>,
    /// Installed once at startup by whoever wants to know about finished
    /// files. A `OnceLock` rather than a lock the hot loop would have to take:
    /// this is set before any download runs and never changes afterwards.
    finished: std::sync::OnceLock<crate::runner::FinishedHook>,
    /// Bumped whenever a row may have changed state, order or membership, so
    /// a UI can push only what moved instead of re-reading the whole queue on
    /// a timer. Live progress is not a revision: it is read from `active`.
    revision: tokio::sync::watch::Sender<u64>,
}

/// Returns whether a worker was actually cancelled.
fn cancel_worker(inner: &Inner, id: DownloadId, by_user: bool) -> bool {
    let mut active = inner.active.lock().expect("supervisor mutex poisoned");
    match active.get_mut(&id) {
        Some(entry) => {
            entry.user_paused = by_user;
            entry.cancel.cancel();
            true
        }
        None => false,
    }
}

/// Minutes since local midnight, for comparing against a [`crate::settings::TimeWindow`].
fn current_minutes() -> u16 {
    let now = chrono::Local::now();
    (now.hour() * 60 + now.minute()) as u16
}

/// How long the loop may sleep without missing anything. Every in-process
/// queue change pokes `wake`, so the timer only has to catch a schedule
/// window opening or closing — which happens on a minute boundary — and,
/// at the slow `fallback` rate, a queue changed by another process.
fn idle_wait(schedule: &crate::settings::TimeWindow, seconds_into_minute: u32, fallback: std::time::Duration) -> std::time::Duration {
    if !schedule.enabled || schedule.start_minutes == schedule.stop_minutes {
        return fallback;
    }
    let to_boundary = std::time::Duration::from_secs(u64::from(60 - seconds_into_minute.min(59)));
    to_boundary.min(fallback)
}

fn idle_wait_now(inner: &Inner) -> std::time::Duration {
    let schedule = inner.settings.read().expect("supervisor mutex poisoned").schedule;
    idle_wait(&schedule, chrono::Local::now().second(), inner.config.idle_poll)
}

impl Inner {
    fn bump(&self) {
        self.revision.send_modify(|r| *r = r.wrapping_add(1));
    }

    /// A queue change made through the supervisor: start new work at once
    /// and tell subscribers the rows moved.
    fn changed(&self) {
        self.wake.notify_one();
        self.bump();
    }

    fn observe(self: &Arc<Self>, id: DownloadId) -> crate::runner::ProgressHook {
        let inner = Arc::clone(self);
        Arc::new(move |progress: Progress| {
            if let Ok(mut active) = inner.active.lock() {
                if let Some(entry) = active.get_mut(&id) {
                    entry.progress = Some(progress);
                }
            }
        })
    }
}

/// Cloneable handle to a running engine.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
    tracker: TaskTracker,
}

impl Supervisor {
    /// Start the supervision loop. It runs until [`Supervisor::shutdown`].
    pub fn start(queue: Queue, downloader: Downloader, config: RunnerConfig) -> Self {
        let inner = Arc::new(Inner {
            queue,
            downloader: Arc::new(downloader),
            config,
            active: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            shutdown: CancellationToken::new(),
            // The real value is loaded from the database once `supervise`
            // starts running; this default holds for the brief window before
            // that completes.
            settings: RwLock::new(EngineSettings::default()),
            finished: std::sync::OnceLock::new(),
            revision: tokio::sync::watch::channel(0).0,
        });

        let tracker = TaskTracker::new();
        tracker.spawn(supervise(Arc::clone(&inner)));
        tracker.close();

        Self { inner, tracker }
    }

    /// Install the callback that runs when a download finishes, with its
    /// resolved path. Called once during startup; later calls are ignored, so
    /// two subsystems cannot silently fight over the slot.
    pub fn on_finished(&self, hook: crate::runner::FinishedHook) {
        if self.inner.finished.set(hook).is_err() {
            tracing::warn!("a finished-download hook was already installed; ignoring the second one");
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.inner.queue
    }

    /// The bandwidth cap this supervisor enforces, for handing to a subsystem
    /// that moves bytes outside the queue — see
    /// [`crate::VideoManager::with_rate_limiter`]. The cap is only meaningfully
    /// app-wide if every download path shares this one handle.
    pub fn rate_limiter(&self) -> crate::speed::SharedRateLimit {
        self.inner.downloader.rate_limiter()
    }

    /// Add a download and start it as soon as a slot frees up.
    pub async fn enqueue(&self, new: NewDownload) -> Result<DownloadId> {
        let id = self.inner.queue.enqueue(new).await?;
        self.inner.changed();
        Ok(id)
    }

    /// Every queue row, newest engine state merged in.
    pub async fn snapshot(&self) -> Result<Vec<DownloadSnapshot>> {
        let items = self.inner.queue.list().await?;
        let active = self.inner.active.lock().expect("supervisor mutex poisoned");

        Ok(items
            .iter()
            .map(|item| {
                DownloadSnapshot::new(item, active.get(&item.id).and_then(|a| a.progress))
            })
            .collect())
    }

    /// Changes whenever a row may have changed state, order or membership.
    /// Progress of running downloads does not bump it; read that with
    /// [`Supervisor::active_snapshots`].
    pub fn revisions(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.revision.subscribe()
    }

    /// Fresh rows for just the downloads in flight, with live progress — the
    /// cheap read for a progress tick, in place of the full [`Supervisor::snapshot`].
    pub async fn active_snapshots(&self) -> Result<Vec<DownloadSnapshot>> {
        let ids: Vec<DownloadId> = self.inner.active.lock().expect("supervisor mutex poisoned").keys().copied().collect();
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            match self.inner.queue.get(id).await {
                Ok(item) => {
                    let live = self.inner.active.lock().expect("supervisor mutex poisoned").get(&id).and_then(|a| a.progress);
                    rows.push(DownloadSnapshot::new(&item, live));
                }
                Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(rows)
    }

    /// Stop a download, keeping its partial file and resume state.
    ///
    /// An item that has not started yet is paused directly: there is no worker
    /// to cancel, and leaving it `waiting` would let the loop claim it anyway.
    pub async fn pause(&self, id: DownloadId) -> Result<()> {
        let cancelled = self.cancel_active(id, true);
        if !cancelled {
            let item = self.inner.queue.get(id).await?;
            if item.state == DownloadState::Waiting {
                self.inner.queue.set_state(id, DownloadState::Paused).await?;
                self.inner.bump();
            }
        }
        Ok(())
    }

    /// Put a paused or failed item back in line.
    pub async fn resume(&self, id: DownloadId) -> Result<()> {
        let item = self.inner.queue.get(id).await?;
        if item.state == DownloadState::Finished {
            return Ok(());
        }
        self.inner.queue.set_state(id, DownloadState::Waiting).await?;
        self.inner.changed();
        Ok(())
    }

    pub async fn restart_missing(&self, id: DownloadId) -> Result<()> {
        self.inner.queue.restart_missing(id).await?;
        self.inner.changed();
        Ok(())
    }

    /// Drop an item from the queue, stopping it first if it is running.
    ///
    /// The row is deleted immediately rather than after the worker settles:
    /// the worker's closing `UPDATE` simply matches nothing, and waiting would
    /// leave a row the user just deleted visible for another second.
    pub async fn remove(&self, id: DownloadId) -> Result<()> {
        self.inner.queue.get(id).await?;
        self.cancel_active(id, true);
        self.inner.queue.remove(id).await?;
        self.inner.changed();
        Ok(())
    }

    /// Delete the stored output only after its writer has stopped. Keep the
    /// history entry when stopping or deleting the file fails.
    pub async fn remove_with_file(&self, id: DownloadId, delete_file: bool) -> Result<()> {
        if !delete_file { return self.remove(id).await; }
        self.pause(id).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            self.cancel_active(id, true);
            let active = self.inner.active.lock().expect("supervisor mutex poisoned").contains_key(&id);
            let item = self.inner.queue.get(id).await?;
            if !active && item.state != DownloadState::Downloading {
                // A waiting item has never owned a file; do not delete an
                // unrelated file that happens to occupy its planned name.
                if item.state == DownloadState::Finished || item.downloaded_bytes > 0 {
                    if std::path::Path::new(&item.filename).file_name().and_then(|s| s.to_str()) != Some(item.filename.as_str()) {
                        return Err(Error::InvalidInput("Invalid stored download filename".into()));
                    }
                    crate::file_removal::remove_output(&item.path()).await?;
                    crate::resume::ResumeState::discard(&item.path());
                }
                self.inner.queue.remove(id).await?;
                self.inner.changed();
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::InvalidInput("Download is still stopping; try deleting it again shortly".into()));
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    pub async fn set_priority(&self, id: DownloadId, priority: i64) -> Result<()> {
        self.inner.queue.set_priority(id, priority).await?;
        self.inner.changed();
        Ok(())
    }

    /// The settings currently in effect — the cached copy the claim loop
    /// reads, not necessarily the very latest database row if another
    /// process wrote one directly.
    pub fn settings(&self) -> EngineSettings {
        self.inner.settings.read().expect("supervisor mutex poisoned").clone()
    }

    /// Persist new settings and apply them immediately: the bandwidth cap
    /// takes effect on downloads already in flight, and a schedule change is
    /// picked up on the loop's next wake rather than waiting out the idle
    /// poll.
    pub async fn update_settings(&self, settings: EngineSettings) -> Result<()> {
        self.inner.queue.save_settings(&settings).await?;
        self.inner
            .downloader
            .set_bandwidth_limit(settings.bandwidth_limit_bytes_per_sec)
            .await;
        *self.inner.settings.write().expect("supervisor mutex poisoned") = settings;
        // `notify_one` keeps a permit if the loop is mid-iteration, so a
        // schedule change is never left waiting for the slow idle timer.
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Stop accepting work, pause everything in flight, and wait for it.
    ///
    /// Downloads stopped this way are left `waiting`, not `paused`, so the next
    /// launch resumes them — closing the app is not a decision to abandon a
    /// download. See [`Supervisor::pause`] for the deliberate contrast.
    pub async fn shutdown(&self) {
        // Each worker's token is a child of this one, so a single cancel
        // reaches every download — including one spawned in the instant after
        // this call, which a sweep over the active map would have missed.
        self.inner.shutdown.cancel();
        self.inner.wake.notify_waiters();
        self.tracker.wait().await;
    }

    /// Returns whether a worker was actually cancelled.
    fn cancel_active(&self, id: DownloadId, by_user: bool) -> bool {
        cancel_worker(&self.inner, id, by_user)
    }
}

async fn supervise(inner: Arc<Inner>) {
    match inner.queue.recover_interrupted().await {
        Ok(n) if n > 0 => {
            tracing::info!(recovered = n, "requeued downloads interrupted by a previous run")
        }
        Err(e) => tracing::error!(error = %e, "crash recovery failed"),
        _ => {}
    }

    // Loaded before the first claim so the bandwidth cap and schedule are in
    // effect from the start, not just after the first `update_settings` call.
    match inner.queue.load_settings().await {
        Ok(settings) => {
            inner
                .downloader
                .set_bandwidth_limit(settings.bandwidth_limit_bytes_per_sec)
                .await;
            *inner.settings.write().expect("supervisor mutex poisoned") = settings;
        }
        Err(e) => tracing::error!(error = %e, "could not load settings; using defaults"),
    }

    let mut set = tokio::task::JoinSet::new();
    // Whether anything has been claimed since the queue was last fully
    // drained — the on-completion action fires once per drain, not on every
    // idle tick of an already-empty queue.
    let mut had_work = false;

    loop {
        let settings = inner.settings.read().expect("supervisor mutex poisoned").clone();
        let schedule_open = settings.schedule.allows(current_minutes());

        if !schedule_open {
            // Stop everything in flight. Each one settles as `waiting`, the
            // same as a shutdown, so the next open window resumes it rather
            // than requiring the user to notice and press resume.
            let ids: Vec<DownloadId> = inner
                .active
                .lock()
                .expect("supervisor mutex poisoned")
                .keys()
                .copied()
                .collect();
            for id in ids {
                cancel_worker(&inner, id, false);
            }
        }

        while !inner.shutdown.is_cancelled() && schedule_open && set.len() < inner.config.max_concurrent {
            match inner.queue.claim_next().await {
                Ok(Some(item)) => {
                    had_work = true;
                    let id = item.id;
                    // A child of the shutdown token: pausing this one download
                    // cancels only this token, while shutting the engine down
                    // cancels the parent and takes every worker with it.
                    let cancel = inner.shutdown.child_token();
                    inner.active.lock().expect("supervisor mutex poisoned").insert(
                        id,
                        Active {
                            cancel: cancel.clone(),
                            progress: None,
                            user_paused: false,
                        },
                    );

                    inner.bump();
                    tracing::info!(%id, url = %item.url, "starting");
                    let hook = inner.observe(id);
                    let finished = inner.finished.get().cloned();
                    let downloader = Arc::clone(&inner.downloader);
                    let queue = inner.queue.clone();
                    let config = inner.config;
                    set.spawn(async move {
                        let result = crate::runner::run_one(
                            item,
                            downloader,
                            queue,
                            cancel,
                            config,
                            Some(hook),
                            finished,
                        )
                        .await;
                        (id, result)
                    });
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "claiming from the queue failed");
                    break;
                }
            }
        }

        if set.is_empty() {
            if inner.shutdown.is_cancelled() {
                break;
            }

            if had_work {
                match inner.queue.list_by_state(DownloadState::Waiting).await {
                    Ok(waiting) if waiting.is_empty() => {
                        had_work = false;
                        run_completion_action(&settings);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "could not check for queue completion"),
                }
            }

            // Idle. Wake on new work, on shutdown, or after a poll interval —
            // the last one covers a queue changed by another process and a
            // schedule window opening with nothing else to prompt a recheck.
            tokio::select! {
                _ = inner.wake.notified() => {}
                _ = inner.shutdown.cancelled() => {}
                _ = tokio::time::sleep(idle_wait_now(&inner)) => {}
            }
            continue;
        }

        tokio::select! {
            joined = set.join_next() => match joined {
                Some(Ok((id, _result))) => settle(&inner, id).await,
                Some(Err(e)) => tracing::error!(error = %e, "download task panicked"),
                None => {}
            },
            // A slot may be free while others still run; do not make new work
            // wait for an unrelated download to finish.
            _ = inner.wake.notified(), if !inner.shutdown.is_cancelled() => {}
            // Otherwise a schedule closing mid-download would not be noticed
            // until some other worker happens to finish.
            _ = tokio::time::sleep(idle_wait_now(&inner)) => {}
        }
    }

    tracing::info!("supervisor stopped");
}

/// Fire the configured on-completion action. Errors are logged, not
/// propagated — a failed "open folder" must not be treated like a download
/// failure, and there is no caller left to report it to by this point.
fn run_completion_action(settings: &EngineSettings) {
    match &settings.on_complete {
        CompletionAction::None => {}
        CompletionAction::OpenFolder => match &settings.default_directory {
            Some(dir) => {
                if let Err(e) = crate::shell::reveal_dir(dir) {
                    tracing::warn!(error = %e, dir = %dir.display(), "could not open the default folder");
                }
            }
            None => tracing::warn!("on-completion action is \"open folder\" but no default directory is set"),
        },
        CompletionAction::RunCommand { command } => {
            if let Err(e) = crate::shell::run_command(command) {
                tracing::warn!(error = %e, %command, "on-completion command failed to start");
            }
        }
        CompletionAction::ExitApp => {
            tracing::info!("queue drained; exiting per the on-completion setting");
            std::process::exit(0);
        }
    }
}

/// Reconcile the persisted state of a download whose worker just exited.
async fn settle(inner: &Arc<Inner>, id: DownloadId) {
    // Whatever happens below, the worker wrote a final state for this row.
    inner.bump();
    let user_paused = inner
        .active
        .lock()
        .expect("supervisor mutex poisoned")
        .remove(&id)
        .map(|a| a.user_paused)
        .unwrap_or(false);

    if user_paused {
        return;
    }

    // Not a user pause, so the worker stopped because the engine is going
    // down or a schedule window just closed. `run_one` already wrote
    // `paused`; flip it back so the item resumes on its own, matching what
    // crash recovery does for an interrupted download.
    match inner.queue.get(id).await {
        Ok(item) if item.state == DownloadState::Paused => {
            if let Err(e) = inner.queue.set_state(id, DownloadState::Waiting).await {
                tracing::error!(%id, error = %e, "could not requeue after shutdown");
            }
        }
        // A removed row is the expected case when the user deleted the item.
        Err(Error::NotFound(_)) | Ok(_) => {}
        Err(e) => tracing::error!(%id, error = %e, "could not read state after stop"),
    }
    // The requeue above changed the row again.
    inner.bump();
}

/// Absolute path so a download resolves the same way regardless of the
/// working directory the app happened to launch from.
pub fn absolute_directory(dir: impl Into<PathBuf>) -> std::io::Result<PathBuf> {
    std::path::absolute(dir.into())
}

#[cfg(test)]
mod tests {
    use super::idle_wait;
    use crate::settings::TimeWindow;
    use std::time::Duration;

    const FALLBACK: Duration = Duration::from_secs(10);

    #[test]
    fn idle_wait_is_the_slow_fallback_without_a_schedule() {
        assert_eq!(idle_wait(&TimeWindow::default(), 12, FALLBACK), FALLBACK);
        let zero_width = TimeWindow { enabled: true, start_minutes: 60, stop_minutes: 60 };
        assert_eq!(idle_wait(&zero_width, 12, FALLBACK), FALLBACK);
    }

    #[test]
    fn idle_wait_never_sleeps_past_the_next_minute_boundary_with_a_schedule() {
        let window = TimeWindow { enabled: true, start_minutes: 60, stop_minutes: 120 };
        assert_eq!(idle_wait(&window, 55, FALLBACK), Duration::from_secs(5));
        assert_eq!(idle_wait(&window, 0, FALLBACK), FALLBACK);
        assert_eq!(idle_wait(&window, 59, FALLBACK), Duration::from_secs(1));
    }
}
