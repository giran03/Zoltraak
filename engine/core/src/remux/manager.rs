//! Runs repackaging jobs: probe a local file, plan the crossing, run ffmpeg,
//! land the result.
//!
//! Desktop and headless instances persist recipes and history. Interrupted jobs
//! can be retried from the beginning; encoded byte streams are not resumable.
//! `new()` remains available for isolated callers and tests without a database.
//!
//! Concurrency is deliberately small. ffmpeg saturates whatever it is given,
//! and four of them fighting over one disk finish no sooner than two, while
//! making the machine unusable meanwhile.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Notify, RwLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

use super::{Container, MediaInfo, Plan, StreamAction};
use crate::error::{Error, Result};
use crate::model::DownloadId;
use crate::probe::dedupe_filename;

/// How many ffmpeg runs are allowed at once unless the user changes it.
const DEFAULT_PARALLELISM: usize = 2;
/// The most runs the user can allow at once.
pub const MAX_PARALLELISM: usize = 4;
/// How often a waiting job re-checks whether it is next, in case a wake-up was missed.
const TURN_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemuxJobState {
    /// Waiting for its turn and a free slot.
    Queued,
    /// Reading the file to decide what has to happen to it.
    Probing,
    Running,
    Finished,
    Failed,
    /// The file was already in the container that was asked for.
    Skipped,
    /// Stopped by the user. The half-written output is deleted.
    Cancelled,
}

impl RemuxJobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Probing => "probing",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Probing | Self::Running)
    }

    pub fn is_terminal(self) -> bool {
        !self.is_active()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemuxJob {
    pub id: DownloadId,
    pub input: PathBuf,
    /// The source file's own name, so a list has something to show before the
    /// probe has said anything.
    pub filename: String,
    pub container: Container,
    #[serde(default)]
    pub conversion: Option<crate::conversion::Settings>,
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    pub state: RemuxJobState,
    /// One line on what is happening — the plan, or why it stopped.
    pub stage: Option<String>,
    /// What the source turned out to be, once probed.
    pub source: Option<MediaInfo>,
    pub plan: Option<Plan>,
    /// Whether this run touches no pixels or samples. `None` until planned.
    pub lossless: Option<bool>,
    /// 0..100, from the source's duration. Stays at 0 for a file whose
    /// duration nothing could establish — an honest zero beats an invented bar.
    pub percent: f32,
    /// ffmpeg's own `speed=` reading, e.g. `48.2` for 48× realtime.
    pub speed: Option<f32>,
    pub input_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
    pub output: Option<PathBuf>,
    /// What the finished file turned out to be, so a row can compare it with the source.
    #[serde(default)]
    pub output_info: Option<MediaInfo>,
    pub error: Option<String>,
    /// Whether the source is deleted once the new file is written.
    pub replace_source: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl RemuxJob {
    fn new(input: PathBuf, container: Container, replace_source: bool) -> Self {
        let now = Utc::now();
        let filename = input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| input.to_string_lossy().into_owned());
        Self {
            id: DownloadId::new(),
            input,
            filename,
            container,
            conversion: None,
            output_directory: None,
            state: RemuxJobState::Queued,
            stage: None,
            source: None,
            plan: None,
            lossless: None,
            percent: 0.0,
            speed: None,
            input_bytes: None,
            output_bytes: None,
            output: None,
            output_info: None,
            error: None,
            replace_source,
            created_at: now,
            updated_at: now,
            finished_at: None,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.state, RemuxJobState::Failed | RemuxJobState::Cancelled)
    }
}

/// One running conversion's claim on a slot; dropping it frees the slot and wakes the queue.
pub struct Slot {
    running: Arc<AtomicUsize>,
    turn: Arc<Notify>,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
        self.turn.notify_waiters();
    }
}

/// Where a queued job can be moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMove {
    Top,
    Up,
    Down,
}

/// What the user can steer about the waiting line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub paused: bool,
    pub parallelism: usize,
    /// Waiting jobs, next to start first.
    pub queued: Vec<DownloadId>,
}

/// A batch of files to repackage with one set of choices.
#[derive(Debug, Clone)]
pub struct NewRemux {
    pub inputs: Vec<PathBuf>,
    pub container: Container,
    /// Where the results go. `None` writes each one beside its source, which
    /// is what "convert this file" almost always means.
    pub output_directory: Option<PathBuf>,
    /// Delete each source once its replacement is on disk. Only ever acted on
    /// after a successful run, and never when the output landed on the source.
    pub replace_source: bool,
}

/// Repackaging, as a service the app holds one of.
#[derive(Clone)]
pub struct RemuxManager {
    jobs: Arc<RwLock<HashMap<DownloadId, RemuxJob>>>,
    cancels: Arc<RwLock<HashMap<DownloadId, CancellationToken>>>,
    /// Queued jobs in the order they will start.
    order: Arc<std::sync::Mutex<Vec<DownloadId>>>,
    running: Arc<AtomicUsize>,
    limit: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    turn: Arc<Notify>,
    pool: Option<sqlx::SqlitePool>,
    history_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for RemuxManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RemuxManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            cancels: Arc::new(RwLock::new(HashMap::new())),
            order: Arc::new(std::sync::Mutex::new(Vec::new())),
            running: Arc::new(AtomicUsize::new(0)),
            limit: Arc::new(AtomicUsize::new(DEFAULT_PARALLELISM)),
            paused: Arc::new(AtomicBool::new(false)),
            turn: Arc::new(Notify::new()),
            pool: None,
            history_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn with_history(pool: sqlx::SqlitePool) -> Result<Self> {
        let mut manager = Self::new();
        let rows: Vec<String> = sqlx::query_scalar("SELECT payload FROM conversion_jobs")
            .fetch_all(&pool)
            .await?;
        manager.pool = Some(pool);
        for payload in rows {
            let mut job: RemuxJob = serde_json::from_str(&payload)
                .map_err(|e| Error::Corrupt(format!("conversion history: {e}")))?;
            if job.state.is_active() {
                job.state = RemuxJobState::Cancelled;
                job.stage =
                    Some("Interrupted when the app closed; retry starts from the beginning".into());
                job.finished_at = Some(Utc::now());
                // The working name contains this job's id, never an unrelated file.
                if let Some(settings) = &job.conversion {
                    let dir = job
                        .output_directory
                        .as_deref()
                        .or_else(|| job.input.parent());
                    if let Some(dir) = dir {
                        let suffix = format!(
                            "{}.zoltraak-convert.{}",
                            job.id,
                            settings.format.extension()
                        );
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.flatten() {
                                if entry.file_name().to_string_lossy().ends_with(&suffix)
                                    || entry.file_name().to_string_lossy().ends_with(&format!(
                                        "{}.zoltraak-convert.palette.png",
                                        job.id
                                    ))
                                {
                                    let _ = std::fs::remove_file(entry.path());
                                }
                            }
                        }
                    }
                }
            }
            let id = job.id;
            manager.jobs.write().await.insert(id, job);
            manager.persist(id).await?;
        }
        Ok(manager)
    }

    async fn persist(&self, id: DownloadId) -> Result<()> {
        let _history = self.history_lock.lock().await;
        if let (Some(pool), Some(job)) = (&self.pool, self.get(id).await) {
            let payload = serde_json::to_string(&job).map_err(|e| Error::Corrupt(e.to_string()))?;
            sqlx::query("INSERT INTO conversion_jobs(id,payload) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload").bind(id.to_string()).bind(payload).execute(pool).await?;
        }
        Ok(())
    }

    /// Queue every input as its own job and return their ids in the order
    /// they were given.
    ///
    /// A path that does not exist is rejected here rather than becoming a job
    /// that fails a second later: the caller is handing over paths it just
    /// read from a drop or a picker, and a typo deserves an answer at the
    /// point it was made.
    pub async fn enqueue(&self, request: NewRemux) -> Result<Vec<DownloadId>> {
        self.enqueue_with_settings(request, None).await
    }

    pub async fn enqueue_conversion(
        &self,
        request: NewRemux,
        settings: crate::conversion::Settings,
    ) -> Result<Vec<DownloadId>> {
        settings.validate()?;
        let capabilities = crate::conversion::capabilities().await?;
        if !capabilities.ffprobe || !capabilities.formats.contains(&settings.format) {
            return Err(Error::InvalidInput(
                "Install FFmpeg and FFprobe with support for the selected output format".into(),
            ));
        }
        // Validate the entire batch before scheduling any work.
        for input in &request.inputs {
            settings.plan(&super::probe(input).await?)?;
        }
        self.enqueue_with_settings(request, Some(settings)).await
    }

    async fn enqueue_with_settings(
        &self,
        mut request: NewRemux,
        settings: Option<crate::conversion::Settings>,
    ) -> Result<Vec<DownloadId>> {
        if request.inputs.is_empty() {
            return Err(Error::InvalidInput(
                "no files were given to repackage".into(),
            ));
        }
        if let Some(dir) = request.output_directory.take() {
            request.output_directory =
                Some(std::path::absolute(&dir).map_err(|source| Error::Io { path: dir, source })?);
        }
        if let Some(dir) = &request.output_directory {
            std::fs::create_dir_all(dir).map_err(|source| Error::Io {
                path: dir.clone(),
                source,
            })?;
        }

        for input in &request.inputs {
            if !input.is_file() {
                return Err(Error::InvalidInput(format!(
                    "{} is not a file that exists",
                    input.display()
                )));
            }
        }
        let mut ids = Vec::with_capacity(request.inputs.len());
        for input in request.inputs {
            let input = std::path::absolute(&input).map_err(|source| Error::Io {
                path: input,
                source,
            })?;
            let mut job = RemuxJob::new(input, request.container, request.replace_source);
            job.conversion = settings.clone();
            job.output_directory = request.output_directory.clone();
            let id = job.id;
            let cancel = CancellationToken::new();
            self.jobs.write().await.insert(id, job);
            self.persist(id).await?;
            self.cancels.write().await.insert(id, cancel.clone());
            self.order.lock().expect("queue order").push(id);
            self.turn.notify_waiters();
            ids.push(id);

            let this = self.clone();
            let directory = request.output_directory.clone();
            tokio::spawn(async move {
                this.run(id, directory, cancel).await;
            });
        }
        Ok(ids)
    }

    pub fn queue_state(&self) -> QueueState {
        QueueState {
            paused: self.paused.load(Ordering::SeqCst),
            parallelism: self.limit.load(Ordering::SeqCst),
            queued: self.order.lock().expect("queue order").clone(),
        }
    }

    /// Hold waiting jobs back. Jobs already running finish normally.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        self.turn.notify_waiters();
    }

    pub fn set_parallelism(&self, parallelism: usize) -> Result<()> {
        if !(1..=MAX_PARALLELISM).contains(&parallelism) {
            return Err(Error::InvalidInput(format!(
                "Run between 1 and {MAX_PARALLELISM} conversions at once"
            )));
        }
        self.limit.store(parallelism, Ordering::SeqCst);
        self.turn.notify_waiters();
        Ok(())
    }

    /// Reorder a job that has not started. Running and finished jobs cannot move.
    pub fn move_job(&self, id: DownloadId, movement: QueueMove) -> Result<()> {
        let mut order = self.order.lock().expect("queue order");
        let Some(index) = order.iter().position(|queued| *queued == id) else {
            return Err(Error::InvalidInput("Only a waiting conversion can be moved".into()));
        };
        let target = match movement {
            QueueMove::Top => 0,
            QueueMove::Up => index.saturating_sub(1),
            QueueMove::Down => (index + 1).min(order.len() - 1),
        };
        let moved = order.remove(index);
        order.insert(target, moved);
        drop(order);
        self.turn.notify_waiters();
        Ok(())
    }

    /// Run `work` only while nothing is converting or waiting, so a preview
    /// never takes a slot from a real conversion.
    pub async fn when_idle<T>(&self, work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        let notified = self.turn.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let _slot = {
            let order = self.order.lock().expect("queue order");
            if self.running.load(Ordering::SeqCst) != 0 || !order.is_empty() {
                return Err(Error::InvalidInput("Previews run when no conversion is working or waiting".into()));
            }
            self.claim_slot()
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                _ = &mut notified => {
                    notified.set(self.turn.notified());
                    notified.as_mut().enable();
                    if !self.order.lock().expect("queue order").is_empty() || self.running.load(Ordering::SeqCst) > 1 {
                        return Err(Error::InvalidInput("Preview cancelled to make room for queued conversions".into()));
                    }
                }
                result = &mut work => return result,
            }
        }
    }

    fn claim_slot(&self) -> Slot {
        self.running.fetch_add(1, Ordering::SeqCst);
        Slot {
            running: self.running.clone(),
            turn: self.turn.clone(),
        }
    }

    /// Wait until this job is next, nothing is paused, and a slot is free.
    /// `None` means it was cancelled while waiting.
    async fn wait_turn(&self, id: DownloadId, cancel: &CancellationToken) -> Option<Slot> {
        loop {
            let notified = self.turn.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut order = self.order.lock().expect("queue order");
                if !self.paused.load(Ordering::SeqCst)
                    && order.first() == Some(&id)
                    && self.running.load(Ordering::SeqCst) < self.limit.load(Ordering::SeqCst)
                {
                    order.remove(0);
                    let slot = self.claim_slot();
                    drop(order);
                    self.turn.notify_waiters();
                    return Some(slot);
                }
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled() => {
                    self.order.lock().expect("queue order").retain(|queued| *queued != id);
                    self.turn.notify_waiters();
                    return None;
                }
                _ = tokio::time::sleep(TURN_POLL) => {}
            }
        }
    }

    pub async fn list(&self) -> Vec<RemuxJob> {
        let mut jobs: Vec<RemuxJob> = self.jobs.read().await.values().cloned().collect();
        // Newest first, matching every other list in the app.
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs
    }

    pub async fn get(&self, id: DownloadId) -> Option<RemuxJob> {
        self.jobs.read().await.get(&id).cloned()
    }

    /// Stop a running job. A finished one is left alone.
    pub async fn cancel(&self, id: DownloadId) -> Result<()> {
        let token = self.cancels.read().await.get(&id).cloned();
        match token {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => Err(Error::InvalidInput(format!("no repackaging job {id}"))),
        }
    }

    /// Cancel if it is running, then drop it from the list.
    pub async fn remove(&self, id: DownloadId) -> Result<()> {
        let _history = self.history_lock.lock().await;
        if let Some(token) = self.cancels.write().await.remove(&id) {
            token.cancel();
        }
        if let Some(pool) = &self.pool {
            sqlx::query("DELETE FROM conversion_jobs WHERE id=?")
                .bind(id.to_string())
                .execute(pool)
                .await?;
        }
        match self.jobs.write().await.remove(&id) {
            Some(_) => Ok(()),
            None => Err(Error::InvalidInput(format!("no repackaging job {id}"))),
        }
    }

    /// Drop every job that is not still working. Returns how many went.
    pub async fn clear_done(&self) -> usize {
        let _history = self.history_lock.lock().await;
        let mut jobs = self.jobs.write().await;
        let before = jobs.len();
        let removed: Vec<DownloadId> = jobs
            .values()
            .filter(|job| !job.state.is_active())
            .map(|job| job.id)
            .collect();
        jobs.retain(|_, job| job.state.is_active());
        let keep: Vec<DownloadId> = jobs.keys().copied().collect();
        drop(jobs);
        self.cancels.write().await.retain(|id, _| keep.contains(id));
        if let Some(pool) = &self.pool {
            // Delete only the terminal ids from this snapshot, preserving newly queued work.
            for id in removed {
                let _ = sqlx::query("DELETE FROM conversion_jobs WHERE id=?")
                    .bind(id.to_string())
                    .execute(pool)
                    .await;
            }
        }
        before - keep.len()
    }

    /// Run a failed or cancelled job again, from the top.
    pub async fn retry(&self, id: DownloadId, output_directory: Option<PathBuf>) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let Some(job) = jobs.get_mut(&id) else {
            return Err(Error::InvalidInput(format!("no repackaging job {id}")));
        };
        if !job.is_retryable() {
            return Err(Error::InvalidInput(
                "only a stopped or failed job can be run again".into(),
            ));
        }
        job.state = RemuxJobState::Queued;
        job.stage = None;
        job.error = None;
        job.percent = 0.0;
        job.speed = None;
        job.output = None;
        job.output_info = None;
        job.output_bytes = None;
        job.finished_at = None;
        job.updated_at = Utc::now();
        let output_directory = if job.conversion.is_some() {
            job.output_directory.clone()
        } else {
            output_directory.or_else(|| job.output_directory.clone())
        };
        drop(jobs);

        self.persist(id).await?;

        let cancel = CancellationToken::new();
        self.cancels.write().await.insert(id, cancel.clone());
        self.order.lock().expect("queue order").push(id);
        self.turn.notify_waiters();
        let this = self.clone();
        tokio::spawn(async move {
            this.run(id, output_directory, cancel).await;
        });
        Ok(())
    }

    /// Apply an edit to one job's row, stamping `updated_at`.
    async fn update(&self, id: DownloadId, edit: impl FnOnce(&mut RemuxJob)) {
        if let Some(job) = self.jobs.write().await.get_mut(&id) {
            edit(job);
            job.updated_at = Utc::now();
        }
    }

    /// The whole life of one job. Every exit writes a terminal state, so a
    /// row can never be left saying `running` after the task has gone.
    async fn run(
        &self,
        id: DownloadId,
        output_directory: Option<PathBuf>,
        cancel: CancellationToken,
    ) {
        // Waiting for a turn *is* the queued state, so nothing is written here.
        let Some(permit) = self.wait_turn(id, &cancel).await else {
            self.finish(id, RemuxJobState::Cancelled, Some("stopped".into()), None)
                .await;
            return;
        };
        if cancel.is_cancelled() {
            self.finish(id, RemuxJobState::Cancelled, Some("stopped".into()), None)
                .await;
            return;
        }

        let Some(job) = self.get(id).await else {
            return;
        };

        self.update(id, |job| {
            job.state = RemuxJobState::Probing;
            job.stage = Some("reading the file".into());
        })
        .await;

        let info = match super::probe(&job.input).await {
            Ok(info) => info,
            Err(e) => {
                self.finish(id, RemuxJobState::Failed, None, Some(e.to_string()))
                    .await;
                return;
            }
        };

        if job.conversion.is_none() && info.already_is(job.container) {
            let container = job.container;
            self.update(id, |job| {
                job.source = Some(info.clone());
                job.input_bytes = info.bytes;
                job.output = Some(job.input.clone());
            })
            .await;
            self.finish(
                id,
                RemuxJobState::Skipped,
                Some(format!("already a .{container}")),
                None,
            )
            .await;
            return;
        }

        let conversion_plan = match &job.conversion {
            Some(settings) => match settings.plan(&info) {
                Ok(plan) => Some(plan),
                Err(e) => {
                    self.finish(id, RemuxJobState::Failed, None, Some(e.to_string()))
                        .await;
                    return;
                }
            },
            None => None,
        };
        let plan = conversion_plan
            .as_ref()
            .map(|p| p.streams)
            .unwrap_or_else(|| super::plan(&info, job.container));
        let stage = conversion_plan
            .as_ref()
            .map(|p| p.description.clone())
            .unwrap_or_else(|| plan.describe(job.container));
        self.update(id, |job| {
            job.state = RemuxJobState::Running;
            job.source = Some(info.clone());
            job.input_bytes = info.bytes;
            job.plan = Some(plan);
            job.lossless = Some(plan.is_lossless());
            job.stage = Some(stage);
        })
        .await;

        let destination_result = if let Some(settings) = &job.conversion {
            destination_for_format(
                &job.input,
                output_directory.as_deref(),
                settings.format.extension(),
                Some(settings),
            )
        } else {
            destination(&job.input, output_directory.as_deref(), job.container)
        };
        let dest = match destination_result {
            Ok(dest) => dest,
            Err(e) => {
                self.finish(id, RemuxJobState::Failed, None, Some(e.to_string()))
                    .await;
                return;
            }
        };

        // ffmpeg writes to a working name and the finished file is renamed
        // into place, so a half-written `.mp4` is never sitting in the folder
        // looking openable.
        let extension = job
            .conversion
            .as_ref()
            .map(|s| s.format.extension())
            .unwrap_or(job.container.extension());
        let working = dest.with_extension(format!("{id}.zoltraak-convert.{extension}"));
        let source_metadata = std::fs::metadata(&job.input)
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        let mut args =
            if let (Some(settings), Some(conversion_plan)) = (&job.conversion, &conversion_plan) {
                settings.args(&info, &working, conversion_plan)
            } else {
                super::ffmpeg_args(&job.input, &working, job.container, plan)
            };
        let measured = job.conversion.as_ref().is_some_and(|s| s.normalize && s.normalize_two_pass)
            && plan.audio == StreamAction::Encode;
        if measured {
            let settings = job.conversion.as_ref().expect("conversion recipe");
            let conversion_plan = conversion_plan.as_ref().expect("conversion plan");
            self.update(id, |job| job.stage = Some("Measuring audio loudness (pass 1 of 2)".into())).await;
            let analysis = settings.loudness_analysis_args(&info, conversion_plan);
            let result = self.transcode_capture(id, &Run { dest: &working, duration: conversion_plan.duration_seconds,
                args: &analysis, progress_start: 0.0, progress_span: 50.0 }, &cancel).await;
            match result {
                Ok(stderr) => {
                    let applied = settings.apply_loudness_measurement(&mut args, &stderr);
                    self.update(id, |job| job.stage = Some(if applied { "Normalizing audio (pass 2 of 2)" } else { "Encoding; no measurable loudness, using single pass" }.into())).await;
                }
                Err(error) => {
                    self.finish(id, if cancel.is_cancelled() { RemuxJobState::Cancelled } else { RemuxJobState::Failed }, None, Some(error.to_string())).await;
                    return;
                }
            }
        }
        let run = Run {
            dest: &working,
            duration: conversion_plan
                .as_ref()
                .and_then(|p| p.duration_seconds)
                .or(info.duration_seconds),
            args: &args,
            progress_start: if measured || job
                .conversion
                .as_ref()
                .is_some_and(|s| s.format == crate::conversion::Format::Gif)
            {
                50.0
            } else {
                0.0
            },
            progress_span: if measured || job
                .conversion
                .as_ref()
                .is_some_and(|s| s.format == crate::conversion::Format::Gif)
            {
                50.0
            } else {
                100.0
            },
        };
        let palette_result =
            if let (Some(settings), Some(plan)) = (&job.conversion, &conversion_plan) {
                if settings.format == crate::conversion::Format::Gif {
                    self.update(id, |job| job.stage = Some("Building GIF palette".into()))
                        .await;
                    let palette_args = settings.palette_args(&info, &working, plan);
                    let palette_path = working.with_extension("palette.png");
                    self.transcode(
                        id,
                        &Run {
                            dest: &palette_path,
                            duration: plan.duration_seconds,
                            args: &palette_args,
                            progress_start: 0.0,
                            progress_span: 50.0,
                        },
                        &cancel,
                    )
                    .await
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
        let outcome = match palette_result {
            Ok(()) => {
                if run.progress_start > 0.0 && !measured {
                    self.update(id, |job| job.stage = Some("Encoding GIF".into()))
                        .await;
                }
                self.transcode(id, &run, &cancel).await
            }
            Err(error) => Err(error),
        };
        let _ = std::fs::remove_file(working.with_extension("palette.png"));

        match outcome {
            Ok(()) => {}
            Err(e) => {
                let _ = std::fs::remove_file(&working);
                if cancel.is_cancelled() {
                    self.finish(id, RemuxJobState::Cancelled, Some("stopped".into()), None)
                        .await;
                } else {
                    self.finish(id, RemuxJobState::Failed, None, Some(e.to_string()))
                        .await;
                }
                return;
            }
        }

        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&working);
            self.finish(id, RemuxJobState::Cancelled, Some("stopped".into()), None)
                .await;
            return;
        }
        // A successful exit alone is insufficient before deleting an original.
        let verified = super::probe(&working).await;
        if !matches!(verified,Ok(ref info) if !info.unprobed && info.bytes.unwrap_or(0)>0 && (plan.video==StreamAction::Absent || info.video_codec.is_some()) && (plan.audio==StreamAction::Absent || info.audio_codec.is_some()))
        {
            let _ = std::fs::remove_file(&working);
            self.finish(
                id,
                RemuxJobState::Failed,
                None,
                Some("The output could not be verified; the original was kept".into()),
            )
            .await;
            return;
        }
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&working);
            self.finish(id, RemuxJobState::Cancelled, Some("stopped".into()), None)
                .await;
            return;
        }

        // The name was picked before the run; something may have taken it in
        // the meantime, so it is checked again at the moment of the rename.
        let mut dest = dest;
        loop {
            match finalize(&working, &dest) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    dest = unique(&dest)
                }
                Err(source) => {
                    let _ = std::fs::remove_file(&working);
                    self.finish(
                        id,
                        RemuxJobState::Failed,
                        None,
                        Some(Error::Io { path: dest, source }.to_string()),
                    )
                    .await;
                    return;
                }
            }
        }

        let bytes = std::fs::metadata(&dest).ok().map(|m| m.len());
        let mut stage = None;
        let decodable = matches!(verified,Ok(ref info) if !info.unsupported_animation);
        let unchanged = std::fs::metadata(&job.input)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
            == source_metadata;
        if job.replace_source && !decodable {
            stage =
                Some("done — original kept because animated WebP decoding is unavailable".into());
        }
        if job.replace_source && !unchanged {
            stage = Some("done — the source changed and was kept".into());
        }
        if job.replace_source && unchanged && decodable && dest != job.input {
            match std::fs::remove_file(&job.input) {
                Ok(()) => stage = Some("done — the original was deleted".into()),
                // The conversion worked; failing the job over the tidy-up
                // would be reporting a success as a failure.
                Err(e) => {
                    tracing::warn!(path = %job.input.display(), error = %e, "could not delete the source after repackaging");
                    stage = Some("done — the original could not be deleted".into());
                }
            }
        }

        let output_info = verified.as_ref().ok().cloned();
        self.update(id, |job| {
            job.output = Some(dest.clone());
            job.output_info = output_info;
            job.output_bytes = bytes;
            job.percent = 100.0;
        })
        .await;
        self.finish(id, RemuxJobState::Finished, stage, None).await;
        drop(permit);
    }

    /// Write the terminal row and forget the cancellation token.
    async fn finish(
        &self,
        id: DownloadId,
        state: RemuxJobState,
        stage: Option<String>,
        error: Option<String>,
    ) {
        self.cancels.write().await.remove(&id);
        self.update(id, |job| {
            job.state = state;
            job.finished_at = Some(Utc::now());
            job.speed = None;
            if let Some(stage) = stage {
                job.stage = Some(stage);
            } else if state == RemuxJobState::Finished {
                job.stage = Some("done".into());
            }
            if error.is_some() {
                job.error = error;
                job.stage = None;
            }
        })
        .await;
        if let Err(e) = self.persist(id).await {
            tracing::error!(%e,"could not save conversion history");
        }
    }

    /// One ffmpeg run, with its progress stream read into the job's row.
    async fn transcode(
        &self,
        id: DownloadId,
        run: &Run<'_>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.transcode_capture(id, run, cancel).await.map(|_| ())
    }

    async fn transcode_capture(
        &self, id: DownloadId, run: &Run<'_>, cancel: &CancellationToken,
    ) -> Result<String> {
        let Run {
            dest,
            duration,
            args,
            progress_start,
            progress_span,
            ..
        } = *run;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut child = crate::media_tools::ffmpeg_command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Without this a cancelled job leaves an ffmpeg running against a
            // file the user thinks nothing is touching.
            .kill_on_drop(true)
            .spawn()
            .map_err(super::launch_error)?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let progress = async {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                match key.trim() {
                    "out_time_us" | "out_time_ms" => {
                        // Both keys are microseconds in practice — ffmpeg's
                        // `out_time_ms` has always been misnamed.
                        let Ok(micros) = value.trim().parse::<i64>() else {
                            continue;
                        };
                        let Some(total) = duration else { continue };
                        let percent = (progress_start
                            + (micros as f64 / 1_000_000.0) / total * progress_span)
                            .clamp(progress_start, (progress_start + progress_span).min(99.9));
                        self.update(id, |job| job.percent = percent as f32).await;
                    }
                    "speed" => {
                        let parsed = value.trim().trim_end_matches('x').parse::<f32>().ok();
                        self.update(id, |job| {
                            job.speed = parsed.filter(|s| s.is_finite() && *s > 0.0)
                        })
                        .await;
                    }
                    _ => {}
                }
            }
        };

        // stderr is drained alongside rather than after: at `-loglevel error`
        // it is nearly always empty, but a chatty failure that filled the pipe
        // would deadlock a process nobody is reading from.
        let errors = async {
            let mut collected = String::new();
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if collected.len() > 32768 {
                    collected.clear();
                }
                collected.extend(line.chars().take(2048));
                collected.push('\n');
            }
            collected
        };

        let run = async {
            let (_, stderr_text) = tokio::join!(progress, errors);
            let status = child.wait().await;
            (stderr_text, status)
        };

        let (stderr_text, status) = tokio::select! {
            finished = run => finished,
            _ = cancel.cancelled() => {
                // `kill_on_drop` above takes the process down with the handle
                // this return drops, so there is nothing to kill by hand —
                // and nothing left holding the half-written file open.
                child.kill().await.ok();
                child.wait().await.ok();
                return Err(Error::InvalidInput("conversion stopped".into()));
            }
        };

        let status = status.map_err(|source| Error::Io {
            path: PathBuf::from("ffmpeg"),
            source,
        })?;
        if !status.success() {
            let detail = stderr_text
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("ffmpeg gave no reason");
            return Err(Error::InvalidInput(format!(
                "ffmpeg could not repackage this file: {detail}"
            )));
        }
        Ok(stderr_text)
    }
}

/// One ffmpeg invocation's inputs, gathered so the run reads as one thing
/// rather than seven positional arguments.
#[derive(Debug, Clone, Copy)]
struct Run<'a> {
    /// The working name; the finished file is renamed off it.
    dest: &'a Path,
    /// The source's length, for the percentage. `None` leaves it at zero.
    duration: Option<f64>,
    args: &'a [String],
    progress_start: f64,
    progress_span: f64,
}

/// Where a job's output lands: the source's own stem in the target container,
/// in `directory` when one was named and beside the source otherwise, never
/// over a file that already exists.
fn destination(input: &Path, directory: Option<&Path>, container: Container) -> Result<PathBuf> {
    destination_for_format(input, directory, container.extension(), None)
}
fn destination_for_format(
    input: &Path,
    directory: Option<&Path>,
    extension: &str,
    settings: Option<&crate::conversion::Settings>,
) -> Result<PathBuf> {
    let stem = input.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        Error::InvalidInput(format!("{} has no usable filename", input.display()))
    })?;
    let dir = match directory {
        Some(dir) => dir.to_path_buf(),
        None => input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    let stem = settings.map_or_else(|| stem.to_string(), |s| s.output_stem(stem));
    let name = dedupe_filename(&format!("{stem}.{extension}"), |candidate| {
        dir.join(candidate).exists()
    });
    Ok(dir.join(name))
}

/// A free name next to one that has just been taken.
fn unique(path: &Path) -> PathBuf {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.join(dedupe_filename(&name, |candidate| {
        dir.join(candidate).exists()
    }))
}

/// Publish a complete file without replacing an output another job just created.
#[cfg(windows)]
fn finalize(working: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = working.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
    // Zero flags: MoveFileExW fails if the destination exists.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn finalize(working: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::hard_link(working, dest)?;
    let _ = std::fs::remove_file(working);
    Ok(())
}

/// Whether a plan means real work. Used by callers that want to warn before
/// starting a run that will take minutes rather than seconds.
pub fn is_transcode(plan: Plan) -> bool {
    plan.video == StreamAction::Encode || plan.audio == StreamAction::Encode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_file_is_refused_before_it_becomes_a_job() {
        let manager = RemuxManager::new();
        let error = manager
            .enqueue(NewRemux {
                inputs: vec![PathBuf::from("no-such-file.mkv")],
                container: Container::Mp4,
                output_directory: None,
                replace_source: false,
            })
            .await
            .expect_err("a path that is not there is not queued");
        assert!(error.to_string().contains("no-such-file.mkv"), "{error}");
        assert!(manager.list().await.is_empty());
    }

    #[tokio::test]
    async fn an_empty_batch_is_refused() {
        let manager = RemuxManager::new();
        assert!(
            manager
                .enqueue(NewRemux {
                    inputs: Vec::new(),
                    container: Container::Mp4,
                    output_directory: None,
                    replace_source: false,
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_file_already_in_the_target_container_is_skipped_rather_than_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let input = dir.path().join("clip.mp4");
        std::fs::write(&input, b"not really an mp4").expect("write");

        let manager = RemuxManager::new();
        let ids = manager
            .enqueue(NewRemux {
                inputs: vec![input.clone()],
                container: Container::Mp4,
                output_directory: None,
                replace_source: false,
            })
            .await
            .expect("queued");

        // The skip is decided from the name, so it lands without ffmpeg ever
        // being launched — which is what makes this assertable on a machine
        // that has none.
        let job = wait_for_terminal(&manager, ids[0]).await;
        assert_eq!(job.state, RemuxJobState::Skipped, "{:?}", job.stage);
        assert_eq!(job.stage.as_deref(), Some("already a .mp4"));
        assert!(input.exists(), "a skipped job never touches the file");
    }

    #[tokio::test]
    async fn removing_a_job_takes_it_off_the_list() {
        let dir = tempfile::tempdir().expect("temp dir");
        let input = dir.path().join("clip.mkv");
        std::fs::write(&input, b"x").expect("write");

        let manager = RemuxManager::new();
        let ids = manager
            .enqueue(NewRemux {
                inputs: vec![input],
                container: Container::Mkv,
                output_directory: None,
                replace_source: false,
            })
            .await
            .expect("queued");
        wait_for_terminal(&manager, ids[0]).await;

        manager.remove(ids[0]).await.expect("removed");
        assert!(manager.list().await.is_empty());
        assert!(
            manager.remove(ids[0]).await.is_err(),
            "removing it twice is an error, not a panic"
        );
    }

    #[tokio::test]
    async fn clearing_done_keeps_nothing_terminal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut ids = Vec::new();
        let manager = RemuxManager::new();
        for n in 0..3 {
            let input = dir.path().join(format!("clip{n}.webm"));
            std::fs::write(&input, b"x").expect("write");
            ids.extend(
                manager
                    .enqueue(NewRemux {
                        inputs: vec![input],
                        container: Container::Webm,
                        output_directory: None,
                        replace_source: false,
                    })
                    .await
                    .expect("queued"),
            );
        }
        for id in &ids {
            wait_for_terminal(&manager, *id).await;
        }
        assert_eq!(manager.clear_done().await, 3);
        assert!(manager.list().await.is_empty());
    }

    #[test]
    fn output_lands_beside_the_source_by_default_and_never_over_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let input = dir.path().join("holiday.mkv");
        std::fs::write(&input, b"x").expect("write");

        let dest = destination(&input, None, Container::Mp4).expect("destination");
        assert_eq!(dest, dir.path().join("holiday.mp4"));

        std::fs::write(&dest, b"x").expect("write");
        let second = destination(&input, None, Container::Mp4).expect("destination");
        assert_eq!(
            second,
            dir.path().join("holiday (1).mp4"),
            "an existing file is never overwritten"
        );
    }

    #[test]
    fn a_named_directory_wins_over_the_sources_own() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = dir.path().join("converted");
        std::fs::create_dir_all(&out).expect("mkdir");
        let dest = destination(
            Path::new("C:/clips/holiday.mkv"),
            Some(&out),
            Container::Mov,
        )
        .expect("destination");
        assert_eq!(dest, out.join("holiday.mov"));
    }

    /// Poll until the job stops moving. Jobs here are either skipped outright
    /// or fail for want of ffmpeg, so this settles immediately in practice.
    async fn wait_for_terminal(manager: &RemuxManager, id: DownloadId) -> RemuxJob {
        for _ in 0..600 {
            if let Some(job) = manager.get(id).await {
                if job.state.is_terminal() {
                    return job;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the job never reached a terminal state");
    }
}
