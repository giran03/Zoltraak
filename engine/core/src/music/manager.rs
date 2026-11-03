//! Runs music jobs: resolve where a track's audio is, fetch it, convert and
//! tag it, land the file.
//!
//! Its own manager for the same reason video has one: a music job is a
//! different shape of work. It starts from a catalogue entry rather than a
//! URL, spends a request or two *finding* a stream before it can fetch
//! anything, and ends in an `ffmpeg` run that writes tags and art. None of
//! that belongs in the byte-range queue, and the video table's columns
//! describe a pipeline this one only partly shares.
//!
//! Unlike video jobs, music jobs are *queued*: a playlist paste creates a
//! hundred rows, and running a hundred YouTube lookups and ffmpeg encodes at
//! once would be a good way to get rate-limited and thrash the disk. A worker
//! keeps [`MusicSettings::max_concurrent`] of them in flight, oldest first.
//!
//! Interrupted work (a crash, a quit) comes back `queued`, not `failed`: a
//! track's stream is re-resolved on every run anyway, so nothing was lost
//! that a fresh start does not recover.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

use super::convert::{self, Tags};
use super::direct::{self, AudioStream, pick_audio_stream};
use super::{AudioFormat, AudioSource, FolderMode, MusicSettings, MusicSource, MusicTrack, OutputOptions, SourceKind, artwork, bandcamp, lyrics, name, soundcloud, ytmusic};
use crate::error::{Error, Result};
use crate::headers::Headers;
use crate::model::DownloadId;
use crate::session::SessionVault;
use crate::speed::{SharedRateLimit, SpeedMeter};
use crate::video::stream::{StreamOptions, StreamOutcome, StreamProgress, StreamTick};
use crate::video::youtube;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicJobState {
    /// Waiting for a worker slot.
    Queued,
    /// Finding the stream: a YouTube search, a session lookup, a stream mint.
    Resolving,
    Downloading,
    Converting,
    Finished,
    Failed,
    /// Deliberately not fetched: the file was already there.
    Skipped,
    /// Held by the user. Resumes from the start of the track.
    Paused,
}

impl MusicJobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Converting => "converting",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Paused => "paused",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "resolving" => Self::Resolving,
            "downloading" => Self::Downloading,
            "converting" => Self::Converting,
            "finished" => Self::Finished,
            "skipped" => Self::Skipped,
            "paused" => Self::Paused,
            _ => Self::Failed,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Resolving | Self::Downloading | Self::Converting)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::Skipped)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicJob {
    pub id: DownloadId,
    pub collection_id: Option<String>,
    pub collection_title: Option<String>,
    pub collection_url: Option<String>,
    pub position: u32,
    pub track: MusicTrack,
    pub format: AudioFormat,
    pub state: MusicJobState,
    /// One line on what the job is doing right now.
    pub stage: Option<String>,
    /// The stream the audio was taken from, once known.
    pub matched_url: Option<String>,
    pub matched_title: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    /// Live only; never persisted.
    pub bytes_per_second: u64,
    pub eta_seconds: Option<u64>,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
    pub directory: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl MusicJob {
    pub fn is_resumable(&self) -> bool {
        matches!(self.state, MusicJobState::Paused | MusicJobState::Failed | MusicJobState::Skipped)
    }
}

/// What a job needs to run, off its row.
#[derive(Debug, Clone)]
struct Plan {
    track: MusicTrack,
    options: OutputOptions,
    directory: PathBuf,
    stem: String,
    collection_id: Option<String>,
    collection_title: Option<String>,
    /// The album's or playlist's own cover, which is not any one track's: on a
    /// compilation the first track's art is the wrong picture for the folder.
    collection_cover: Option<String>,
    position: u32,
}

/// A set of tracks to download with one set of options.
#[derive(Debug, Clone)]
pub struct NewMusicDownload {
    /// The source, holding only the tracks the user kept.
    pub source: MusicSource,
    pub options: OutputOptions,
    /// Where files go when `options.directory` is unset.
    pub base_directory: PathBuf,
}

/// What enqueuing produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enqueued {
    pub collection_id: Option<String>,
    pub job_ids: Vec<DownloadId>,
    /// Already on disk, so not queued.
    pub skipped: usize,
    pub directory: PathBuf,
}

/// The settings row's key in the shared `settings` table.
const SETTINGS_KEY: &str = "music";

/// The domain whose session a matched track needs.
const YOUTUBE_DOMAIN: &str = "youtube.com";

/// Written to a failed job's `stage` when the only thing wrong was the
/// absence of a signed-in YouTube session.
///
/// It is a marker as much as a caption: [`MusicManager::retry_waiting_for_session`]
/// looks for it, so the moment the extension shares a YouTube sign-in every
/// track that stalled for one goes back in the queue by itself. Kept in the
/// row rather than in memory so that survives a restart.
pub const WAITING_FOR_YOUTUBE_SESSION: &str = "waiting for a YouTube sign-in";

fn job_from_row(row: &SqliteRow) -> Result<MusicJob> {
    let track: MusicTrack = serde_json::from_str(&row.try_get::<String, _>("track")?).map_err(|e| Error::Corrupt(format!("stored music track: {e}")))?;
    let options: OutputOptions = serde_json::from_str(&row.try_get::<String, _>("options")?).unwrap_or_default();
    let parse_time = |raw: String| {
        DateTime::parse_from_rfc3339(&raw).map(|t| t.with_timezone(&Utc)).map_err(|e| Error::Corrupt(e.to_string()))
    };
    Ok(MusicJob {
        id: DownloadId(uuid::Uuid::parse_str(row.try_get::<String, _>("id")?.as_str()).map_err(|e| Error::Corrupt(e.to_string()))?),
        collection_id: row.try_get("collection_id")?,
        collection_title: row.try_get("collection_title")?,
        collection_url: row.try_get("collection_url")?,
        position: row.try_get::<i64, _>("position")? as u32,
        track,
        format: options.format,
        state: MusicJobState::parse(&row.try_get::<String, _>("state")?),
        stage: row.try_get("stage")?,
        matched_url: row.try_get("matched_url")?,
        matched_title: row.try_get("matched_title")?,
        downloaded_bytes: row.try_get::<i64, _>("downloaded_bytes")? as u64,
        total_bytes: row.try_get::<Option<i64>, _>("total_bytes")?.map(|n| n as u64),
        bytes_per_second: 0,
        eta_seconds: None,
        error: row.try_get("error")?,
        path: row.try_get::<Option<String>, _>("path")?.map(PathBuf::from),
        directory: PathBuf::from(row.try_get::<String, _>("directory")?),
        created_at: parse_time(row.try_get("created_at")?)?,
        updated_at: parse_time(row.try_get("updated_at")?)?,
        finished_at: row.try_get::<Option<String>, _>("finished_at")?.and_then(|t| parse_time(t).ok()),
    })
}

#[derive(Clone)]
pub struct MusicManager {
    client: Client,
    pool: SqlitePool,
    jobs: Arc<RwLock<HashMap<DownloadId, MusicJob>>>,
    running: Arc<RwLock<HashMap<DownloadId, CancellationToken>>>,
    /// The browser sessions the extension shared — a signed-in YouTube
    /// session is what lifts the anonymous preview cap on its streams.
    vault: Arc<SessionVault>,
    stream_options: StreamOptions,
    on_finished: Option<crate::runner::FinishedHook>,
    settings: Arc<RwLock<MusicSettings>>,
    /// Pokes the worker when there is something new to claim.
    wake: Arc<Notify>,
    worker_started: Arc<AtomicBool>,
}

impl MusicManager {
    pub fn new(client: Client, pool: SqlitePool) -> Self {
        Self {
            client,
            pool,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashMap::new())),
            vault: Arc::new(SessionVault::in_memory()),
            stream_options: StreamOptions::default(),
            on_finished: None,
            settings: Arc::new(RwLock::new(MusicSettings::default())),
            wake: Arc::new(Notify::new()),
            worker_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_session_vault(mut self, vault: Arc<SessionVault>) -> Self {
        self.vault = vault;
        self
    }

    pub fn with_completion_hook(mut self, hook: crate::runner::FinishedHook) -> Self {
        self.on_finished = Some(hook);
        self
    }

    /// Count music downloads against the engine's shared bandwidth cap.
    pub fn with_rate_limiter(mut self, limiter: SharedRateLimit) -> Self {
        self.stream_options.limiter = Some(limiter);
        self
    }

    // ─── Settings ────────────────────────────────────────────────────────

    pub async fn settings(&self) -> MusicSettings {
        self.settings.read().await.clone()
    }

    pub async fn update_settings(&self, mut settings: MusicSettings) -> Result<MusicSettings> {
        settings.max_concurrent = settings.max_concurrent.clamp(1, 8);
        if settings.output.filename_template.trim().is_empty() {
            settings.output.filename_template = name::DEFAULT_TEMPLATE.into();
        }
        let json = serde_json::to_string(&settings).map_err(|e| Error::InvalidInput(format!("could not serialize music settings: {e}")))?;
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(SETTINGS_KEY)
            .bind(json)
            .execute(&self.pool)
            .await?;
        *self.settings.write().await = settings.clone();
        // More slots may have opened.
        self.wake.notify_one();
        Ok(settings)
    }

    async fn load_settings(&self) -> Result<()> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?").bind(SETTINGS_KEY).fetch_optional(&self.pool).await?;
        if let Some(row) = row {
            let raw: String = row.try_get(0)?;
            match serde_json::from_str::<MusicSettings>(&raw) {
                Ok(settings) => *self.settings.write().await = settings,
                Err(e) => tracing::warn!(error = %e, "ignoring unparseable music settings; using defaults"),
            }
        }
        Ok(())
    }

    // ─── Analysis ────────────────────────────────────────────────────────

    /// The session the vault holds for a URL's site, merged under the
    /// caller's own headers.
    async fn with_session(&self, url: &str, headers: &Headers) -> Headers {
        let Some(domain) = crate::video::site::cookie_domain_for(url) else { return headers.clone() };
        let Some(stored) = self.vault.get(domain).await else { return headers.clone() };
        let mut merged = headers.clone();
        for (name, value) in stored.to_headers() {
            if !merged.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(&name)) {
                merged.push((name, value));
            }
        }
        merged
    }

    /// Resolve a link, or search by name when it is not one.
    pub async fn analyze(&self, input: &str, headers: &Headers) -> Result<MusicSource> {
        let input = input.trim();
        if super::looks_like_url(input) {
            let headers = self.with_session(input, headers).await;
            super::analyze(&self.client, input, &headers).await
        } else {
            let prefer = self.settings.read().await.prefer_youtube_music;
            super::search(&self.client, input, prefer).await
        }
    }

    // ─── Lifecycle ───────────────────────────────────────────────────────

    /// Load history, requeue whatever a previous run left mid-flight, and
    /// start the worker. Call once at startup.
    pub async fn recover(&self) -> Result<()> {
        self.load_settings().await?;
        sqlx::query("UPDATE music_jobs SET state = 'queued', stage = NULL, error = NULL, downloaded_bytes = 0 WHERE state IN ('resolving', 'downloading', 'converting')")
            .execute(&self.pool)
            .await?;
        let rows = sqlx::query("SELECT * FROM music_jobs ORDER BY created_at ASC, position ASC").fetch_all(&self.pool).await?;
        let mut jobs = self.jobs.write().await;
        for row in &rows {
            match job_from_row(row) {
                Ok(job) => {
                    jobs.insert(job.id, job);
                }
                Err(e) => tracing::warn!(error = %e, "skipping an unreadable music job row"),
            }
        }
        drop(jobs);
        self.start_worker();
        self.watch_sessions();
        Ok(())
    }

    /// Put every track that stalled for want of a YouTube sign-in back in the
    /// queue. Returns how many were requeued.
    ///
    /// Driven by [`Self::watch_sessions`] rather than by the user: the whole
    /// point is that signing in is the only action needed, and the fifty
    /// tracks that failed while signed out do not each need a click.
    pub async fn retry_waiting_for_session(&self) -> Result<usize> {
        let rows = sqlx::query("SELECT id FROM music_jobs WHERE state = 'failed' AND stage = ?")
            .bind(WAITING_FOR_YOUTUBE_SESSION)
            .fetch_all(&self.pool)
            .await?;
        let ids: Vec<DownloadId> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("id").ok())
            .filter_map(|raw| uuid::Uuid::parse_str(&raw).ok().map(DownloadId))
            .collect();
        let mut requeued = 0;
        for id in ids {
            // A job the user has since removed is simply gone; `resume`
            // reports that rather than treating it as an error worth raising.
            if self.resume(id, false).await.is_ok() {
                requeued += 1;
            }
        }
        if requeued > 0 {
            tracing::info!(requeued, "a YouTube sign-in arrived; requeued the tracks that were waiting for it");
        }
        Ok(requeued)
    }

    /// Watch the vault, and requeue the waiting tracks when YouTube's session
    /// shows up. Runs for the life of the manager.
    fn watch_sessions(&self) {
        let manager = self.clone();
        let mut changes = self.vault.subscribe();
        tokio::spawn(async move {
            loop {
                let domain = match changes.recv().await {
                    Ok(domain) => domain,
                    // Lagged: sessions changed faster than this loop read
                    // them, which only means it should look now.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => YOUTUBE_DOMAIN.to_string(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                };
                if !domain.eq_ignore_ascii_case(YOUTUBE_DOMAIN) {
                    continue;
                }
                if !manager.vault.is_signed_in(YOUTUBE_DOMAIN).await {
                    continue;
                }
                if let Err(e) = manager.retry_waiting_for_session().await {
                    tracing::warn!(error = %e, "could not requeue the tracks waiting for a YouTube sign-in");
                }
            }
        });
    }

    fn start_worker(&self) {
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = self.clone();
        tokio::spawn(async move { manager.worker().await });
    }

    async fn worker(&self) {
        loop {
            self.fill_slots().await;
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
        }
    }

    /// Start queued jobs until the concurrency limit is reached.
    async fn fill_slots(&self) {
        let limit = self.settings.read().await.max_concurrent.max(1);
        loop {
            let running = self.running.read().await.len();
            if running >= limit {
                return;
            }
            // The two maps are read one after another, never held together:
            // `remove` locks them in the opposite order, and tokio's RwLock
            // is write-preferring, so overlapping the two here is all it
            // takes to deadlock against a `running.write()` waiting behind
            // this read.
            let in_flight: std::collections::HashSet<DownloadId> = self.running.read().await.keys().copied().collect();
            let next = {
                let jobs = self.jobs.read().await;
                let mut queued: Vec<&MusicJob> = jobs
                    .values()
                    .filter(|j| j.state == MusicJobState::Queued && !in_flight.contains(&j.id))
                    .collect();
                queued.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.position.cmp(&b.position)));
                queued.first().map(|j| j.id)
            };
            let Some(id) = next else { return };
            let plan = match self.load_plan(id).await {
                Ok(plan) => plan,
                Err(e) => {
                    self.update(id, |job| {
                        job.state = MusicJobState::Failed;
                        job.error = Some(e.to_string());
                    })
                    .await;
                    continue;
                }
            };
            // A pause/removal may have happened while loading the plan.
            // Claim and register the writer together before releasing the row.
            let token = CancellationToken::new();
            let claimed = {
                let mut jobs = self.jobs.write().await;
                let Some(job) = jobs.get_mut(&id) else { continue };
                if job.state != MusicJobState::Queued { continue; }
                job.state = MusicJobState::Resolving;
                job.stage = Some("finding the audio".into());
                job.error = None;
                job.updated_at = Utc::now();
                self.running.write().await.insert(id, token.clone());
                job.clone()
            };
            let _ = self.persist(&claimed).await;
            let manager = self.clone();
            tokio::spawn(async move {
                manager.run(id, plan, token).await;
                manager.running.write().await.remove(&id);
                manager.wake.notify_one();
            });
        }
    }

    // ─── Enqueue ─────────────────────────────────────────────────────────

    /// Where a source's files go, given the options.
    fn destination(new: &NewMusicDownload) -> PathBuf {
        let base = new.options.directory.clone().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| new.base_directory.clone());
        let is_collection = new.source.kind != SourceKind::Track;
        match new.options.folder_mode {
            FolderMode::Collection if is_collection => base.join(name::folder_name(&new.source.title, "playlist")),
            FolderMode::Collection | FolderMode::None => base,
            FolderMode::Custom => {
                let custom = new.options.folder_name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                match custom {
                    Some(n) => base.join(name::folder_name(n, "music")),
                    None if is_collection => base.join(name::folder_name(&new.source.title, "playlist")),
                    None => base,
                }
            }
        }
    }

    /// The extensions a stem may already exist under, for the skip check.
    fn extensions_for(format: AudioFormat) -> Vec<&'static str> {
        match format.extension() {
            Some(ext) => vec![ext],
            None => vec!["m4a", "opus", "mp3", "ogg", "webm", "flac", "wav"],
        }
    }

    pub async fn enqueue(&self, new: NewMusicDownload) -> Result<Enqueued> {
        if new.source.tracks.is_empty() {
            return Err(Error::InvalidInput("no tracks were selected".into()));
        }
        let directory = crate::supervisor::absolute_directory(Self::destination(&new))
            .map_err(|e| Error::InvalidInput(format!("invalid destination folder: {e}")))?;
        let is_collection = new.source.kind != SourceKind::Track;
        let collection_id = is_collection.then(|| uuid::Uuid::new_v4().to_string());
        let template = if new.options.filename_template.trim().is_empty() { name::DEFAULT_TEMPLATE } else { new.options.filename_template.as_str() };
        let numbered = is_collection && new.options.number_tracks && !template.contains("{track");
        let total = new.source.tracks.len() as u32;

        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut job_ids = Vec::with_capacity(new.source.tracks.len());
        let mut skipped = 0;
        let now = Utc::now();

        for (index, track) in new.source.tracks.iter().enumerate() {
            let position = index as u32 + 1;
            let mut track = track.clone();
            if is_collection && new.source.kind == SourceKind::Album {
                track.track_number = track.track_number.or(Some(position));
                track.track_total = track.track_total.or(Some(total));
            }
            let base_stem = name::render(template, &track, Some(position));
            let mut stem = if numbered { format!("{position:02} - {base_stem}") } else { base_stem };
            // Two tracks of one paste rendering to the same name would race
            // for the same file; number the second here rather than later.
            let mut n = 2;
            while !used.insert(stem.to_ascii_lowercase()) {
                stem = format!("{} ({n})", stem.trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == ' ' || c == '('));
                n += 1;
            }

            let existing = if new.options.skip_existing {
                Self::extensions_for(new.options.format).into_iter().map(|ext| directory.join(format!("{stem}.{ext}"))).find(|p| p.exists())
            } else {
                None
            };

            let id = DownloadId::new();
            let job = MusicJob {
                id,
                collection_id: collection_id.clone(),
                collection_title: is_collection.then(|| new.source.title.clone()),
                collection_url: is_collection.then(|| new.source.source_url.clone()),
                position,
                track: track.clone(),
                format: new.options.format,
                state: if existing.is_some() { MusicJobState::Skipped } else { MusicJobState::Queued },
                stage: existing.is_some().then(|| "already on disk".to_string()),
                matched_url: None,
                matched_title: None,
                downloaded_bytes: 0,
                total_bytes: existing.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()),
                bytes_per_second: 0,
                eta_seconds: None,
                error: None,
                path: existing.clone(),
                directory: directory.clone(),
                created_at: now,
                updated_at: now,
                finished_at: existing.is_some().then_some(now),
            };
            let plan = Plan {
                track,
                options: new.options.clone(),
                directory: directory.clone(),
                stem,
                collection_id: collection_id.clone(),
                collection_title: job.collection_title.clone(),
                collection_cover: is_collection.then(|| new.source.cover_url.clone()).flatten(),
                position,
            };
            self.persist_new(&job, &plan).await?;
            self.jobs.write().await.insert(id, job);
            if existing.is_some() {
                skipped += 1;
            }
            job_ids.push(id);
        }

        self.start_worker();
        self.wake.notify_one();
        Ok(Enqueued { collection_id, job_ids, skipped, directory })
    }

    // ─── Control ─────────────────────────────────────────────────────────

    pub async fn list(&self) -> Vec<MusicJob> {
        let mut jobs: Vec<_> = self.jobs.read().await.values().cloned().collect();
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.position.cmp(&b.position)));
        jobs
    }

    pub async fn get(&self, id: DownloadId) -> Option<MusicJob> {
        self.jobs.read().await.get(&id).cloned()
    }

    pub async fn locate_file(&self, id: DownloadId, path: &Path) -> Result<()> {
        crate::file_status::require_file(path).await?;
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&id).ok_or(Error::NotFound(id))?;
        if !matches!(job.state, MusicJobState::Finished | MusicJobState::Skipped) { return Err(Error::InvalidInput("Only completed tracks can be located.".into())); }
        let mut next = job.clone();
        next.path = Some(path.into());
        self.persist(&next).await?;
        *job = next;
        Ok(())
    }

    pub async fn restart_missing(&self, id: DownloadId) -> Result<()> {
        let recorded = self.get(id).await.ok_or(Error::NotFound(id))?;
        let job = &recorded;
        if !matches!(job.state, MusicJobState::Finished | MusicJobState::Skipped) { return Err(Error::InvalidInput("Only completed tracks can be downloaded again.".into())); }
        let path = job.path.as_ref().ok_or_else(|| Error::InvalidInput("This entry has no recorded output.".into()))?;
        crate::file_status::require_missing(path).await?;
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&id).ok_or(Error::NotFound(id))?;
        if !matches!(job.state, MusicJobState::Finished | MusicJobState::Skipped) || job.path != recorded.path {
            return Err(Error::InvalidInput("The track entry changed; check it again before restarting.".into()));
        }
        let mut next = job.clone();
        next.state = MusicJobState::Queued; next.path = None; next.downloaded_bytes = 0; next.total_bytes = None; next.finished_at = None; next.error = None; next.stage = None;
        self.persist(&next).await?;
        *job = next;
        drop(jobs);
        self.start_worker(); self.wake.notify_one();
        Ok(())
    }

    /// Hold a job. A running one stops at its next boundary; a queued one
    /// simply is not started. Returns whether anything changed.
    pub async fn pause(&self, id: DownloadId) -> bool {
        let token = self.running.read().await.get(&id).cloned();
        if let Some(token) = token {
            token.cancel();
            return true;
        }
        let mut jobs = self.jobs.write().await;
        let Some(job) = jobs.get_mut(&id) else { return false };
        if job.state == MusicJobState::Queued {
            job.state = MusicJobState::Paused;
            job.stage = None;
            let job = job.clone();
            drop(jobs);
            let _ = self.persist(&job).await;
            return true;
        }
        false
    }

    /// Put a paused, failed or skipped job back in the queue. `force` on a
    /// skipped job downloads it even though the file exists.
    pub async fn resume(&self, id: DownloadId, force: bool) -> Result<()> {
        if self.running.read().await.contains_key(&id) {
            return Ok(());
        }
        let job = self.get(id).await.ok_or(Error::NotFound(id))?;
        if !job.is_resumable() {
            return Ok(());
        }
        if force && job.state == MusicJobState::Skipped {
            sqlx::query("UPDATE music_jobs SET options = json_set(options, '$.skipExisting', json('false')) WHERE id = ?")
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        // Only a paused, failed or skipped job gets here (`is_resumable`), and
        // each of those is about to be fetched again — so the previous run's
        // path and finish time are cleared rather than left to describe a file
        // this job no longer stands behind.
        self.update(id, |job| {
            job.state = MusicJobState::Queued;
            job.stage = None;
            job.error = None;
            job.downloaded_bytes = 0;
            job.bytes_per_second = 0;
            job.eta_seconds = None;
            job.path = None;
            job.finished_at = None;
        })
        .await;
        self.start_worker();
        self.wake.notify_one();
        Ok(())
    }

    /// Wait for a cancelled job's task to let go of its files.
    async fn await_stopped(&self, id: DownloadId) {
        for _ in 0..150 {
            if !self.running.read().await.contains_key(&id) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tracing::warn!(%id, "music job did not stop within three seconds; proceeding anyway");
    }

    /// Forget a job, stopping it first. A finished file is left alone.
    pub async fn remove(&self, id: DownloadId) -> Result<bool> {
        self.remove_with_file(id, false).await
    }

    pub async fn remove_with_file(&self, id: DownloadId, delete_file: bool) -> Result<bool> {
        self.pause(id).await;
        // Cover a claim that finished between pause's token lookup and its
        // queued-state check. The claim cannot start after a queued pause.
        if let Some(token) = self.running.read().await.get(&id).cloned() {
            token.cancel();
        }
        self.await_stopped(id).await;
        if delete_file {
            if self.running.read().await.contains_key(&id) {
                return Err(Error::InvalidInput("Music is still stopping; try deleting it again shortly".into()));
            }
            if let Some(path) = self.get(id).await.and_then(|job| job.path) {
                crate::file_removal::remove_output(&path).await?;
            }
        }
        if let Ok(plan) = self.load_plan(id).await {
            Self::remove_temp_files(&plan);
        }
        let existed = self.jobs.write().await.remove(&id).is_some();
        sqlx::query("DELETE FROM music_jobs WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        Ok(existed)
    }

    /// Every job of a collection, whatever its state.
    pub async fn remove_collection(&self, collection_id: &str) -> Result<usize> {
        let ids: Vec<DownloadId> = self.jobs.read().await.values().filter(|j| j.collection_id.as_deref() == Some(collection_id)).map(|j| j.id).collect();
        let mut removed = 0;
        for id in ids {
            if self.remove(id).await? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Requeue a collection's failed tracks.
    pub async fn retry_collection(&self, collection_id: &str) -> Result<usize> {
        let ids: Vec<DownloadId> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|j| j.collection_id.as_deref() == Some(collection_id) && matches!(j.state, MusicJobState::Failed | MusicJobState::Paused))
            .map(|j| j.id)
            .collect();
        for id in &ids {
            self.resume(*id, false).await?;
        }
        Ok(ids.len())
    }

    /// Drop finished and skipped rows from the list. Files stay.
    pub async fn clear_done(&self) -> Result<usize> {
        let ids: Vec<DownloadId> = self.jobs.read().await.values().filter(|j| j.state.is_terminal() && j.state != MusicJobState::Failed).map(|j| j.id).collect();
        for id in &ids {
            self.remove(*id).await?;
        }
        Ok(ids.len())
    }

    // ─── Running a job ───────────────────────────────────────────────────

    fn temp_paths(plan: &Plan) -> (PathBuf, PathBuf) {
        (plan.directory.join(format!("{}.zoltraak-source.tmp", plan.stem)), plan.directory.join(format!("{}.zoltraak-cover.tmp", plan.stem)))
    }

    /// Scratch for the collection's cover, kept apart from the track's so
    /// fetching one never overwrites the other.
    fn collection_cover_tmp(plan: &Plan) -> PathBuf {
        plan.directory.join(format!("{}.zoltraak-collection-cover.tmp", plan.stem))
    }

    fn remove_temp_files(plan: &Plan) {
        let (source, cover) = Self::temp_paths(plan);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(cover);
        let _ = std::fs::remove_file(Self::collection_cover_tmp(plan));
        let _ = std::fs::remove_file(plan.directory.join(format!("{}.opendm-source.tmp", plan.stem)));
        let _ = std::fs::remove_file(plan.directory.join(format!("{}.opendm-cover.tmp", plan.stem)));
        let _ = std::fs::remove_file(plan.directory.join(format!("{}.opendm-collection-cover.tmp", plan.stem)));
        for ext in ["m4a", "mp3", "opus", "ogg", "flac", "wav"] {
            let _ = std::fs::remove_file(plan.directory.join(format!("{}.zoltraak-convert.{ext}", plan.stem)));
            let _ = std::fs::remove_file(plan.directory.join(format!("{}.opendm-convert.{ext}", plan.stem)));
        }
    }

    async fn run(&self, id: DownloadId, plan: Plan, cancel: CancellationToken) {
        // Set by the fetch when it gave up purely for want of a YouTube
        // sign-in. A flag rather than a look at the error text: the reason
        // decides whether this job retries itself later, and that is too much
        // to hang on a message someone may reword.
        let needs_session = AtomicBool::new(false);
        let result = self.run_inner(id, &plan, &cancel, &needs_session).await;
        match result {
            Ok(()) => {}
            Err(_) if cancel.is_cancelled() => {
                Self::remove_temp_files(&plan);
                self.clear_rate(id).await;
                self.update(id, |job| {
                    job.state = MusicJobState::Paused;
                    job.stage = None;
                    job.error = None;
                    job.downloaded_bytes = 0;
                })
                .await;
            }
            Err(e) => {
                let waiting = needs_session.load(Ordering::SeqCst);
                if waiting {
                    tracing::info!(%id, "music job is waiting for a YouTube sign-in; it will retry itself when one arrives");
                } else {
                    tracing::warn!(error = %e, %id, "music job failed");
                }
                Self::remove_temp_files(&plan);
                self.clear_rate(id).await;
                self.update(id, |job| {
                    job.state = MusicJobState::Failed;
                    job.stage = waiting.then(|| WAITING_FOR_YOUTUBE_SESSION.to_string());
                    job.error = Some(e.to_string());
                })
                .await;
            }
        }
    }

    async fn stage(&self, id: DownloadId, state: MusicJobState, stage: &str) {
        self.update(id, |job| {
            job.state = state;
            job.stage = Some(stage.to_string());
        })
        .await;
    }

    async fn run_inner(&self, id: DownloadId, plan: &Plan, cancel: &CancellationToken, needs_session: &AtomicBool) -> Result<()> {
        std::fs::create_dir_all(&plan.directory).map_err(|source| Error::Io { path: plan.directory.clone(), source })?;

        // A retry of a skipped job arrives here with the file present; the
        // stored options say whether that still means "skip".
        if plan.options.skip_existing {
            if let Some(existing) = Self::extensions_for(plan.options.format).into_iter().map(|ext| plan.directory.join(format!("{}.{ext}", plan.stem))).find(|p| p.exists()) {
                let size = std::fs::metadata(&existing).map(|m| m.len()).ok();
                self.update(id, |job| {
                    job.state = MusicJobState::Skipped;
                    job.stage = Some("already on disk".into());
                    job.path = Some(existing.clone());
                    job.total_bytes = size;
                    job.finished_at = Some(Utc::now());
                })
                .await;
                return Ok(());
            }
        }

        // 1. Where the audio is.
        let mut track = plan.track.clone();
        let (stream, video_id) = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::InvalidInput("stopped".into())),
            resolved = self.resolve_stream(id, &mut track, cancel) => resolved?,
        };
        if cancel.is_cancelled() {
            return Err(Error::InvalidInput("stopped".into()));
        }

        // 2. The bytes.
        let (source_tmp, cover_tmp) = Self::temp_paths(plan);
        let _ = std::fs::remove_file(&source_tmp);
        self.stage(id, MusicJobState::Downloading, "downloading").await;
        let stream = self.fetch_source(id, &track, stream, video_id.as_deref(), &source_tmp, cancel, needs_session).await?;
        let downloaded = std::fs::metadata(&source_tmp).map(|m| m.len()).unwrap_or(0);
        if downloaded == 0 {
            return Err(Error::InvalidInput("the stream delivered no data — it may have expired or need a signed-in session".into()));
        }
        self.update(id, |job| {
            job.downloaded_bytes = downloaded;
            job.total_bytes = Some(downloaded);
        })
        .await;

        // 3. The extras that go into the file.
        let want_cover = plan.options.embed_cover || plan.options.save_cover_file;
        if want_cover && plan.options.fetch_artwork && artwork::wants_better_artwork(&track) {
            // A matched track arrives with YouTube's 16:9 still, which would
            // otherwise become this song's album art everywhere it is played.
            self.stage(id, MusicJobState::Converting, "looking up cover art").await;
            if let Some((url, origin)) = artwork::look_up(&self.client, &track).await {
                tracing::info!(%id, found_on = origin.label(), "cover art looked up");
                track.cover_url = Some(url);
            }
        }
        let cover = if want_cover {
            match track.cover_url.as_deref() {
                Some(url) => self.fetch_cover(url, &cover_tmp).await,
                None => None,
            }
        } else {
            None
        };
        let lyrics = if plan.options.fetch_lyrics {
            self.stage(id, MusicJobState::Converting, "looking up lyrics").await;
            lyrics::fetch(&self.client, &track).await.unwrap_or(None)
        } else {
            None
        };

        // 4. Convert and tag, or — with no ffmpeg and nothing to convert —
        //    just give the stream its proper name.
        self.stage(id, MusicJobState::Converting, if plan.options.format == AudioFormat::Best { "packaging" } else { "converting" }).await;
        let tags = plan.options.embed_metadata.then(|| tags_for(&track, lyrics.as_ref().and_then(|l| l.plain.clone().or_else(|| l.synced.clone()))));
        let final_path = match self.convert(plan, &source_tmp, &stream, tags.as_ref(), cover.as_deref(), cancel).await {
            Ok(path) => path,
            Err(e) => {
                let _ = std::fs::remove_file(&source_tmp);
                let _ = std::fs::remove_file(&cover_tmp);
                return Err(e);
            }
        };
        let _ = std::fs::remove_file(&source_tmp);

        // 5. Sidecars.
        if plan.options.save_cover_file {
            self.save_cover_files(plan, cover.as_deref(), &final_path).await;
        }
        let _ = std::fs::remove_file(&cover_tmp);
        let _ = std::fs::remove_file(Self::collection_cover_tmp(plan));
        if let Some(synced) = lyrics.as_ref().and_then(|l| l.synced.as_deref()) {
            let lrc = final_path.with_extension("lrc");
            let _ = std::fs::write(lrc, synced);
        }

        let size = std::fs::metadata(&final_path).map(|m| m.len()).ok();
        self.clear_rate(id).await;
        self.update(id, |job| {
            job.state = MusicJobState::Finished;
            job.stage = None;
            job.error = None;
            job.path = Some(final_path.clone());
            job.total_bytes = size.or(job.total_bytes);
            job.downloaded_bytes = size.unwrap_or(job.downloaded_bytes);
            job.finished_at = Some(Utc::now());
            // What the match settled on is worth keeping on the row.
            job.track = track.clone();
        })
        .await;

        if plan.options.write_playlist_file {
            self.write_playlist_file(plan).await;
        }
        if let Some(finished) = &self.on_finished {
            finished(id, final_path);
        }
        Ok(())
    }

    /// Find the stream for a track. Fills in metadata the catalogue lacked
    /// and the match supplied (an album name, most often).
    async fn resolve_stream(&self, id: DownloadId, track: &mut MusicTrack, cancel: &CancellationToken) -> Result<(AudioStream, Option<String>)> {
        let source = track.source.clone();
        match source {
            AudioSource::Match => {
                self.stage(id, MusicJobState::Resolving, "matching on YouTube Music").await;
                let prefer = self.settings.read().await.prefer_youtube_music;
                let found = ytmusic::find_match(&self.client, track, prefer).await?;
                if track.album.is_none() {
                    track.album = found.album.clone();
                }
                let url = found.url();
                self.update(id, |job| {
                    job.matched_url = Some(url.clone());
                    job.matched_title = Some(format!("{} — {}", found.artists.join(", "), found.title));
                })
                .await;
                if cancel.is_cancelled() {
                    return Err(Error::InvalidInput("stopped".into()));
                }
                let stream = self.youtube_stream(id, &found.video_id, track).await?;
                Ok((stream, Some(found.video_id)))
            }
            AudioSource::YouTube { video_id } => {
                self.stage(id, MusicJobState::Resolving, "resolving the YouTube stream").await;
                let url = youtube_url(&video_id);
                self.update(id, |job| {
                    job.matched_url = Some(url.clone());
                })
                .await;
                let stream = self.youtube_stream(id, &video_id, track).await?;
                Ok((stream, Some(video_id)))
            }
            AudioSource::Direct { url, headers, container } => {
                self.update(id, |job| {
                    job.matched_url = Some(url.clone());
                })
                .await;
                let cached = AudioStream { url, headers, container, has_video: false, filesize: None };
                // A generic page's signed media URL can expire while queued.
                // Keep the cached link as a last resort if its page is offline.
                if track.source_url != cached.url && !direct::is_audio_url(&track.source_url) {
                    let page_headers = self.with_session(&track.source_url, &cached.headers).await;
                    let resolved = tokio::select! {
                        _ = cancel.cancelled() => return Err(Error::InvalidInput("stopped".into())),
                        result = direct::streams_from_page(&self.client, &track.source_url, &page_headers) => result,
                    };
                    if let Ok(streams) = resolved {
                        if let Some(fresh) = streams.into_iter().next() { return Ok((fresh, None)); }
                    }
                }
                Ok((cached, None))
            }
            AudioSource::SoundCloud { track_id } => {
                self.stage(id, MusicJobState::Resolving, "minting the SoundCloud stream").await;
                let (url, container) = soundcloud::stream_url(&self.client, track_id).await?;
                self.update(id, |job| {
                    job.matched_url = Some(track.source_url.clone());
                })
                .await;
                Ok((AudioStream { url, headers: Vec::new(), container: Some(container.into()), has_video: false, filesize: None }, None))
            }
            AudioSource::Bandcamp { page_url, track_id } => {
                self.stage(id, MusicJobState::Resolving, "reading the Bandcamp page").await;
                let url = bandcamp::stream_url(&self.client, &page_url, track_id).await?;
                self.update(id, |job| {
                    job.matched_url = Some(page_url.clone());
                })
                .await;
                Ok((AudioStream { url, headers: Vec::new(), container: Some("mp3".into()), has_video: false, filesize: None }, None))
            }
        }
    }

    /// The audio-only stream of a YouTube video, resolved with whatever
    /// session the vault holds for YouTube — and carrying it, so the bytes are
    /// fetched as the same visitor that was allowed to see the formats.
    async fn youtube_stream(&self, _id: DownloadId, video_id: &str, track: &mut MusicTrack) -> Result<AudioStream> {
        let url = youtube_url(video_id);
        let headers = self.with_session(&url, &Headers::new()).await;
        let meta = youtube::analyze(&self.client, &url, &headers).await?;
        if track.duration.is_none() {
            track.duration = meta.duration;
        }
        let mut stream =
            pick_audio_stream(&meta.formats).ok_or_else(|| Error::InvalidInput("YouTube offered no audio stream for this video".into()))?;
        stream.headers = self.with_stream_session(&url, stream.headers).await;
        Ok(stream)
    }

    /// A stream's own headers with the site's session filled in behind them.
    ///
    /// The format's headers are the client identity its URLs were signed for
    /// and must win; the session is what lifts the CDN's limit on an
    /// anonymous visitor. Sending only the first — which is what this did
    /// until the byte fetch was traced — resolves the formats with a sign-in
    /// and then fetches them without one, which is the worst of both: the
    /// user has shared a session and still gets a stranger's allowance.
    async fn with_stream_session(&self, page_url: &str, stream_headers: Headers) -> Headers {
        let session = self.with_session(page_url, &Headers::new()).await;
        merge_session_under(stream_headers, session)
    }

    /// Renew other resolvable sources on an expired or withdrawn CDN URL.
    /// Switching renditions starts a clean temporary file: even two MP3s may
    /// use different bitrates, so their offsets must never be combined.
    async fn fetch_source(
        &self, id: DownloadId, track: &MusicTrack, mut stream: AudioStream,
        video_id: Option<&str>, dest: &Path, cancel: &CancellationToken, needs_session: &AtomicBool,
    ) -> Result<AudioStream> {
        let mut failed = Vec::new();
        for attempt in 0..=2 {
            if cancel.is_cancelled() { return Err(Error::InvalidInput("stopped".into())); }
            match self.fetch(id, &stream, video_id, dest, cancel, needs_session).await {
                Ok(()) => return Ok(stream),
                Err(error) => {
                    if attempt == 2 || !matches!(error, Error::UnexpectedStatus { status: 401 | 403 | 404 | 410, .. }) {
                        return Err(error);
                    }
                    failed.push(stream.url.clone());
                    self.stage(id, MusicJobState::Resolving, "refreshing an unavailable music stream").await;
                    let refresh = async {
                        match &track.source {
                            AudioSource::SoundCloud { track_id } => {
                                let (url, container) = soundcloud::stream_url_excluding(&self.client, *track_id, &failed).await?;
                                Ok(Some(AudioStream { url, headers: Vec::new(), container: Some(container.into()), has_video: false, filesize: None }))
                            }
                            AudioSource::Bandcamp { page_url, track_id } => {
                                let url = bandcamp::stream_url(&self.client, page_url, *track_id).await?;
                                Ok(Some(AudioStream { url, headers: Vec::new(), container: Some("mp3".into()), has_video: false, filesize: None }))
                            }
                            AudioSource::Direct { url, headers, .. } if track.source_url != *url && !direct::is_audio_url(&track.source_url) => {
                                let page_headers = self.with_session(&track.source_url, headers).await;
                                let fresh = direct::streams_from_page(&self.client, &track.source_url, &page_headers).await?;
                                Ok(fresh.into_iter().find(|candidate| !failed.contains(&candidate.url)))
                            }
                            // Catalogue matches and YouTube already have their
                            // session-aware renewal path. Raw signed links
                            // have no source page from which to renew them.
                            _ => Ok(None),
                        }
                    };
                    let fresh: Result<Option<AudioStream>> = tokio::select! {
                        _ = cancel.cancelled() => return Err(Error::InvalidInput("stopped".into())),
                        result = refresh => result,
                    };
                    let Some(fresh) = fresh? else { return Err(error) };
                    stream = fresh;
                    let _ = std::fs::remove_file(dest);
                    self.update(id, |job| { job.downloaded_bytes = 0; job.total_bytes = None; job.bytes_per_second = 0; job.eta_seconds = None; }).await;
                    self.stage(id, MusicJobState::Downloading, "downloading refreshed music stream").await;
                }
            }
        }
        unreachable!("bounded stream retry returns on its final attempt")
    }

    /// Fetch a stream to `dest`, re-resolving a YouTube link the CDN refuses
    /// and continuing from the last whole chunk — the same loop the video
    /// manager runs, for the same reason (see its `run_inner`).
    async fn fetch(
        &self,
        id: DownloadId,
        stream: &AudioStream,
        video_id: Option<&str>,
        dest: &Path,
        cancel: &CancellationToken,
        needs_session: &AtomicBool,
    ) -> Result<()> {
        const MAX_REFRESHES: usize = 400;
        let mut url = stream.url.clone();
        let mut headers = stream.headers.clone();
        let mut resume = StreamProgress::default();
        let mut refreshes = 0;
        // Whether this attempt began with a sign-in behind it. A refusal that
        // arrives *without* one is a different event from one that arrives
        // with: the first is a gate, the second is a link that expired.
        let mut had_session = self.vault.is_signed_in(YOUTUBE_DOMAIN).await;

        loop {
            let options = StreamOptions {
                live_seconds: None,
                segments: self.stream_options.segments.clone(),
                limiter: self.stream_options.limiter.clone(),
                resume: resume.clone(),
                cancel: cancel.clone(),
            };
            let cb = self.progress_callback(id);
            let outcome = crate::video::stream::fetch_with_progress(&self.client, &url, &headers, dest, Some(&cb), &options).await;
            // A refusal part-way through and a refusal up front are the same
            // event for a signed link: the link has stopped working.
            let (progress, status, refused) = match outcome {
                Ok(StreamOutcome::Completed { .. }) => return Ok(()),
                Ok(StreamOutcome::Paused(_)) => return Err(Error::InvalidInput("stopped".into())),
                Ok(StreamOutcome::Refused { progress, status, url }) => (progress, status, url),
                Err(Error::UnexpectedStatus { status, url }) => (resume.clone(), status, url),
                Err(e) => return Err(e),
            };
            let expired_youtube_link = youtube::is_stream_url(&refused) && (status == 403 || status == 410);
            let Some(video_id) = video_id.filter(|_| expired_youtube_link) else {
                return Err(Error::UnexpectedStatus { status, url: refused });
            };
            let advanced = progress.bytes > resume.bytes;
            if refreshes >= MAX_REFRESHES || (refreshes > 0 && !advanced) {
                // Before calling it: the sign-in that would lift this refusal
                // may have arrived since the attempt began — the user reading
                // the first failure and signing in is the ordinary case, and
                // the vault is shared, so another process may hold it. Re-read
                // it, and carry on rather than fail if it is there now.
                if !had_session {
                    if let Err(e) = self.vault.reload().await {
                        tracing::debug!(error = %e, "could not re-read the session vault");
                    }
                    if self.vault.is_signed_in(YOUTUBE_DOMAIN).await {
                        tracing::info!(%id, "a YouTube sign-in arrived; continuing with it");
                        had_session = true;
                        refreshes = 0;
                        resume = progress;
                        let page = youtube_url(video_id);
                        let session = self.with_session(&page, &Headers::new()).await;
                        let meta = youtube::analyze(&self.client, &page, &session).await?;
                        let fresh = pick_audio_stream(&meta.formats)
                            .ok_or_else(|| Error::InvalidInput("YouTube no longer offers an audio stream for this video".into()))?;
                        url = fresh.url;
                        headers = self.with_stream_session(&page, fresh.headers).await;
                        continue;
                    }
                    // Still none. This track is not broken — it is waiting,
                    // and it will take itself off the failed list as soon as
                    // one is shared.
                    needs_session.store(true, Ordering::SeqCst);
                }
                return Err(Error::InvalidInput(format!(
                    "YouTube's host refused the stream ({status}) after {} KB. Without a signed-in session it allows an anonymous \
                     downloader about a megabyte of a track and then refuses, whatever link is asked for — re-resolving does not get \
                     past it. Sign in to YouTube in a browser with the Zoltraak extension installed and play any video once so the \
                     extension can share the session; this track then retries itself",
                    progress.bytes / 1024
                )));
            }
            refreshes += 1;
            resume = progress;
            tracing::info!(%id, refreshes, on_disk = resume.bytes, "stream link refused; re-resolving and continuing");
            let page = youtube_url(video_id);
            let session = self.with_session(&page, &Headers::new()).await;
            let meta = youtube::analyze(&self.client, &page, &session).await?;
            let fresh = pick_audio_stream(&meta.formats).ok_or_else(|| Error::InvalidInput("YouTube no longer offers an audio stream for this video".into()))?;
            url = fresh.url;
            headers = self.with_stream_session(&page, fresh.headers).await;
        }
    }

    /// A progress callback publishing rate, bytes and ETA for a job.
    fn progress_callback(&self, id: DownloadId) -> impl Fn(StreamTick) + Send + Sync + 'static {
        struct Meter {
            speed: SpeedMeter,
            last_push: std::time::Instant,
            last_bytes: u64,
        }
        let meter = Arc::new(std::sync::Mutex::new(Meter { speed: SpeedMeter::default(), last_push: std::time::Instant::now(), last_bytes: 0 }));
        let manager = self.clone();
        move |tick: StreamTick| {
            let now = std::time::Instant::now();
            let (rate, downloaded, total) = {
                let mut meter = meter.lock().unwrap();
                let delta = tick.bytes.saturating_sub(meter.last_bytes);
                meter.last_bytes = tick.bytes;
                meter.speed.record(delta, now);
                if now.duration_since(meter.last_push) < std::time::Duration::from_millis(100) {
                    return;
                }
                meter.last_push = now;
                (meter.speed.bytes_per_second(), tick.bytes, tick.total)
            };
            let manager = manager.clone();
            tokio::spawn(async move {
                let mut jobs = manager.jobs.write().await;
                let Some(job) = jobs.get_mut(&id) else { return };
                job.downloaded_bytes = downloaded;
                if total.is_some() {
                    job.total_bytes = total;
                }
                job.bytes_per_second = rate;
                job.eta_seconds = match (job.total_bytes, rate) {
                    (Some(total), rate) if rate > 0 && total > downloaded => Some((total - downloaded) / rate),
                    _ => None,
                };
            });
        }
    }

    /// Write the cover beside the audio.
    ///
    /// A collection gets one picture for the folder — the album's or
    /// playlist's own where the service published one, since the first
    /// track's art is the wrong cover for a compilation — written as
    /// `cover.jpg`, the name most players and library scanners look for, and
    /// `folder.jpg`, the one Windows reads for a folder thumbnail. Both are
    /// written once and then left alone, so a fifty-track album does not
    /// download its cover fifty times.
    ///
    /// A single track has no folder to speak for, so its picture goes beside
    /// the file under the file's own name.
    ///
    /// The extension follows the bytes rather than the convention: naming a
    /// PNG `cover.jpg` produces a file that some scanners refuse and every
    /// image viewer complains about. `folder.jpg` is written only when the
    /// picture really is a JPEG.
    async fn save_cover_files(&self, plan: &Plan, track_cover: Option<&Path>, audio_path: &Path) {
        let Some(collection) = plan.collection_id.as_deref() else {
            // A single: name the picture after the file it belongs to.
            let Some(cover) = track_cover else { return };
            let extension = sniff_image_extension(cover);
            let dest = audio_path.with_extension(extension);
            if dest != audio_path && !dest.exists() {
                let _ = std::fs::copy(cover, dest);
            }
            return;
        };
        let _ = collection;

        // Already there from an earlier track of the same collection.
        if plan.directory.join("cover.jpg").exists() || plan.directory.join("cover.png").exists() {
            return;
        }

        // The collection's own cover first; this track's only as a stand-in.
        let scratch = Self::collection_cover_tmp(plan);
        let source = match plan.collection_cover.as_deref() {
            Some(url) => self.fetch_cover(url, &scratch).await.or_else(|| track_cover.map(Path::to_path_buf)),
            None => track_cover.map(Path::to_path_buf),
        };
        let Some(source) = source else { return };

        let extension = sniff_image_extension(&source);
        let cover = plan.directory.join(format!("cover.{extension}"));
        if !cover.exists() {
            let _ = std::fs::copy(&source, &cover);
        }
        if extension == "jpg" {
            let folder = plan.directory.join("folder.jpg");
            if !folder.exists() {
                let _ = std::fs::copy(&source, folder);
            }
        }
    }

    /// A cover image to `dest`, or `None` when it cannot be had. Never an
    /// error: a track without its art is still the track.
    async fn fetch_cover(&self, url: &str, dest: &Path) -> Option<PathBuf> {
        const MAX_COVER_BYTES: usize = 20 * 1024 * 1024;
        let response = self.client.get(url).header("User-Agent", super::BROWSER_UA).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
            return None;
        }
        let looks_like_image = bytes.starts_with(&[0xFF, 0xD8]) || bytes.starts_with(b"\x89PNG") || bytes.starts_with(b"RIFF") || bytes.starts_with(b"GIF8");
        if !looks_like_image {
            return None;
        }
        std::fs::write(dest, &bytes).ok()?;
        Some(dest.to_path_buf())
    }

    async fn convert(&self, plan: &Plan, source: &Path, stream: &AudioStream, tags: Option<&Tags>, cover: Option<&Path>, cancel: &CancellationToken) -> Result<PathBuf> {
        let ffmpeg = convert::ffmpeg_version().await.is_some();
        if !ffmpeg {
            if plan.options.format == AudioFormat::Best && !stream.has_video {
                // The honest fallback: the stream, named, untagged.
                let ext = stream.container.as_deref().map(|c| match c {
                    "mp4" => "m4a",
                    other => other,
                });
                let ext = ext.unwrap_or("m4a");
                let name = crate::probe::dedupe_filename(&format!("{}.{ext}", plan.stem), |c| plan.directory.join(c).exists());
                let dest = plan.directory.join(name);
                std::fs::rename(source, &dest).map_err(|e| Error::Io { path: dest.clone(), source: e })?;
                tracing::warn!("ffmpeg is not installed; the track was saved as it arrived, without tags");
                return Ok(dest);
            }
            return Err(Error::InvalidInput(format!(
                "ffmpeg is needed to convert this track to {} and was not found on PATH. Install it from https://ffmpeg.org/download.html, or choose the \"best\" format to keep the stream as it arrives",
                plan.options.format.as_str()
            )));
        }
        let request = convert::Request {
            input: source,
            input_container: stream.container.as_deref(),
            has_video: stream.has_video,
            output_dir: &plan.directory,
            stem: &plan.stem,
            format: plan.options.format,
            bitrate: plan.options.bitrate(),
            normalize: plan.options.normalize_loudness,
            tags: if plan.options.embed_metadata { tags } else { None },
            cover: if plan.options.embed_cover { cover } else { None },
        };
        convert::convert(&request, cancel).await
    }

    /// Rewrite a collection's `.m3u8` with its finished files, in order.
    async fn write_playlist_file(&self, plan: &Plan) {
        let Some(collection_id) = &plan.collection_id else { return };
        let title = plan.collection_title.clone().unwrap_or_else(|| "playlist".into());
        let mut entries: Vec<(u32, PathBuf, String)> = self
            .jobs
            .read()
            .await
            .values()
            .filter(|j| j.collection_id.as_deref() == Some(collection_id) && j.state == MusicJobState::Finished)
            .filter_map(|j| j.path.clone().map(|p| (j.position, p, format!("{} - {}", j.track.artist_line(), j.track.title))))
            .collect();
        entries.sort_by_key(|(p, _, _)| *p);
        let mut out = String::from("#EXTM3U\n");
        for (_, path, label) in entries {
            let rel = path.strip_prefix(&plan.directory).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string_lossy().into_owned());
            out.push_str(&format!("#EXTINF:-1,{label}\n{rel}\n"));
        }
        let dest = plan.directory.join(format!("{}.m3u8", name::folder_name(&title, "playlist")));
        if let Err(e) = std::fs::write(&dest, out) {
            tracing::debug!(error = %e, "could not write the playlist file");
        }
    }

    // ─── Persistence ─────────────────────────────────────────────────────

    async fn clear_rate(&self, id: DownloadId) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.bytes_per_second = 0;
            job.eta_seconds = None;
        }
    }

    async fn update(&self, id: DownloadId, f: impl FnOnce(&mut MusicJob)) {
        let job = {
            let mut jobs = self.jobs.write().await;
            jobs.get_mut(&id).map(|job| {
                f(job);
                job.updated_at = Utc::now();
                job.clone()
            })
        };
        if let Some(job) = job {
            if let Err(e) = self.persist(&job).await {
                tracing::warn!(error = %e, "failed to persist music job");
            }
        }
    }

    async fn load_plan(&self, id: DownloadId) -> Result<Plan> {
        let row = sqlx::query("SELECT track, options, directory, stem, collection_id, collection_title, collection_cover, position FROM music_jobs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound(id))?;
        let track: MusicTrack = serde_json::from_str(&row.try_get::<String, _>("track")?).map_err(|e| Error::Corrupt(format!("stored music track: {e}")))?;
        let options: OutputOptions = serde_json::from_str(&row.try_get::<String, _>("options")?).map_err(|e| Error::Corrupt(format!("stored music options: {e}")))?;
        Ok(Plan {
            track,
            options,
            directory: PathBuf::from(row.try_get::<String, _>("directory")?),
            stem: row.try_get("stem")?,
            collection_id: row.try_get("collection_id")?,
            collection_title: row.try_get("collection_title")?,
            collection_cover: row.try_get("collection_cover")?,
            position: row.try_get::<i64, _>("position")? as u32,
        })
    }

    async fn persist_new(&self, job: &MusicJob, plan: &Plan) -> Result<()> {
        sqlx::query(
            "INSERT INTO music_jobs \
               (id, collection_id, collection_title, collection_url, collection_cover, position, track, options, directory, stem, \
                state, stage, matched_url, matched_title, downloaded_bytes, total_bytes, error, path, created_at, updated_at, finished_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(job.id.to_string())
        .bind(&job.collection_id)
        .bind(&job.collection_title)
        .bind(&job.collection_url)
        .bind(&plan.collection_cover)
        .bind(plan.position as i64)
        .bind(serde_json::to_string(&plan.track).map_err(|e| Error::InvalidInput(e.to_string()))?)
        .bind(serde_json::to_string(&plan.options).map_err(|e| Error::InvalidInput(e.to_string()))?)
        .bind(plan.directory.to_string_lossy().to_string())
        .bind(&plan.stem)
        .bind(job.state.as_str())
        .bind(&job.stage)
        .bind(&job.matched_url)
        .bind(&job.matched_title)
        .bind(job.downloaded_bytes as i64)
        .bind(job.total_bytes.map(|n| n as i64))
        .bind(&job.error)
        .bind(job.path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(job.created_at.to_rfc3339())
        .bind(job.updated_at.to_rfc3339())
        .bind(job.finished_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn persist(&self, job: &MusicJob) -> Result<()> {
        sqlx::query(
            "UPDATE music_jobs SET state = ?, stage = ?, matched_url = ?, matched_title = ?, downloaded_bytes = ?, total_bytes = ?, \
             error = ?, path = ?, track = ?, updated_at = ?, finished_at = ? WHERE id = ?",
        )
        .bind(job.state.as_str())
        .bind(&job.stage)
        .bind(&job.matched_url)
        .bind(&job.matched_title)
        .bind(job.downloaded_bytes as i64)
        .bind(job.total_bytes.map(|n| n as i64))
        .bind(&job.error)
        .bind(job.path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(serde_json::to_string(&job.track).map_err(|e| Error::InvalidInput(e.to_string()))?)
        .bind(job.updated_at.to_rfc3339())
        .bind(job.finished_at.map(|t| t.to_rfc3339()))
        .bind(job.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// `stream` headers with `session` filled in behind them: the stream keeps
/// every header it named, and the session supplies only what it did not.
///
/// The order is the whole point. A format's headers are the client identity
/// its URLs were signed for — a googlevideo link minted for the Android VR app
/// is refused when a desktop browser asks for it — so they must not be
/// overwritten. The session is what lifts the CDN's limit on a visitor it
/// does not recognise, so it must not be dropped.
fn merge_session_under(stream: Headers, session: Headers) -> Headers {
    let mut merged = stream;
    for (name, value) in session {
        if !merged.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(&name)) {
            merged.push((name, value));
        }
    }
    merged
}

/// The extension for an image, read from its first bytes.
///
/// The URL's own extension is not evidence: photo CDNs routinely serve WebP
/// from a `.jpg` path, and a saved cover that lies about its format is one
/// that library scanners skip.
fn sniff_image_extension(path: &Path) -> &'static str {
    let mut head = [0u8; 12];
    let Ok(mut file) = std::fs::File::open(path) else { return "jpg" };
    use std::io::Read;
    let read = file.read(&mut head).unwrap_or(0);
    let head = &head[..read];
    if head.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else if head.starts_with(b"\x89PNG") {
        "png"
    } else if head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WEBP" {
        "webp"
    } else if head.starts_with(b"GIF8") {
        "gif"
    } else {
        "jpg"
    }
}

fn youtube_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// The tags a track's metadata becomes.
pub fn tags_for(track: &MusicTrack, lyrics: Option<String>) -> Tags {
    Tags {
        title: Some(track.title.clone()),
        artist: (!track.artists.is_empty()).then(|| track.artist_line()),
        album: track.album.clone(),
        album_artist: track.album_artist.clone().or_else(|| track.primary_artist().map(str::to_string)),
        track: track.track_number,
        track_total: track.track_total,
        disc: track.disc_number,
        date: track.date.clone().or_else(|| track.year.map(|y| y.to_string())),
        genre: track.genre.clone(),
        comment: Some(track.source_url.clone()),
        lyrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::Service;

    fn track(title: &str) -> MusicTrack {
        MusicTrack {
            id: title.into(),
            title: title.into(),
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            album_artist: None,
            track_number: None,
            track_total: None,
            disc_number: None,
            year: Some(2020),
            date: None,
            genre: None,
            duration: None,
            cover_url: None,
            isrc: None,
            source_url: "https://example/t".into(),
            service: Service::Spotify,
            source: AudioSource::Match,
            explicit: false,
            unavailable: None,
        }
    }

    fn source(kind: SourceKind, tracks: Vec<MusicTrack>) -> MusicSource {
        MusicSource { kind, service: Service::Spotify, title: "My Mix: Best / Of".into(), owner: None, cover_url: None, source_url: "https://example/p".into(), tracks, total: None, note: None }
    }

    #[tokio::test]
    async fn bandcamp_refreshes_stale_links_without_splicing_partial_audio() {
        use axum::response::IntoResponse;
        use std::sync::atomic::AtomicUsize;
        let requests = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            async move {
                match uri.path() {
                    "/stale.mp3" => axum::http::StatusCode::GONE.into_response(),
                    "/fresh.mp3" => ([("Content-Type","audio/mpeg")],"fresh audio bytes").into_response(),
                    _ => axum::http::StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        // Install a page whose stream URL knows the chosen ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}",listener.local_addr().unwrap());
        let page_data = serde_json::json!({"trackinfo":[{"id":7,"file":{"mp3-128":format!("{base}/fresh.mp3")}}]});
        let page = format!("<div data-tralbum=\"{}\"></div>",page_data.to_string().replace('"',"&quot;"));
        let calls = requests.clone();
        let app = app.route("/track",axum::routing::get(move || { let page=page.clone(); let calls=calls.clone(); async move { calls.fetch_add(1,Ordering::SeqCst);axum::response::Html(page) } }));
        let server = tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
        let queue = crate::Queue::open_in_memory().await.unwrap();
        let manager = MusicManager::new(Client::new(),queue.pool());
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("audio.part");
        std::fs::write(&dest,b"old audio must not survive").unwrap();
        let mut track=track("Song");
        track.source=AudioSource::Bandcamp{page_url:format!("{base}/track"),track_id:7};
        let stream=AudioStream{url:format!("{base}/stale.mp3"),headers:vec![],container:Some("mp3".into()),has_video:false,filesize:None};
        manager.fetch_source(DownloadId::new(),&track,stream,None,&dest,&CancellationToken::new(),&AtomicBool::new(false)).await.unwrap();
        assert_eq!(std::fs::read(dest).unwrap(),b"fresh audio bytes");
        assert_eq!(requests.load(Ordering::SeqCst),1);
        server.abort();
    }

    #[tokio::test]
    async fn stale_source_renewal_is_bounded_and_cancelled_jobs_do_not_refresh() {
        use axum::response::IntoResponse;
        use std::sync::atomic::AtomicUsize;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let page_data=serde_json::json!({"trackinfo":[{"id":7,"file":{"mp3-128":format!("{base}/stale.mp3")}}]});
        let page=format!("<div data-tralbum=\"{}\"></div>",page_data.to_string().replace('"',"&quot;"));
        let requests=Arc::new(AtomicUsize::new(0));let count=requests.clone();
        let app=axum::Router::new().fallback(move |uri:axum::http::Uri|{let page=page.clone();let count=count.clone();async move{
            count.fetch_add(1,Ordering::SeqCst);
            if uri.path()=="/track" { axum::response::Html(page).into_response() }
            else {axum::http::StatusCode::NOT_FOUND.into_response()}
        }});
        let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
        let queue=crate::Queue::open_in_memory().await.unwrap();
        let manager=MusicManager::new(Client::new(),queue.pool());
        let dir=tempfile::tempdir().unwrap();let dest=dir.path().join("audio.part");
        let mut track=track("Song");track.source=AudioSource::Bandcamp{page_url:format!("{base}/track"),track_id:7};
        let stream=AudioStream{url:format!("{base}/stale.mp3"),headers:vec![],container:Some("mp3".into()),has_video:false,filesize:None};
        assert!(manager.fetch_source(DownloadId::new(),&track,stream.clone(),None,&dest,&CancellationToken::new(),&AtomicBool::new(false)).await.is_err());
        assert_eq!(requests.load(Ordering::SeqCst),5,"three fetch attempts and two page refreshes");
        let cancel=CancellationToken::new();cancel.cancel();
        assert!(manager.fetch_source(DownloadId::new(),&track,stream,None,&dest,&cancel,&AtomicBool::new(false)).await.is_err());
        assert_eq!(requests.load(Ordering::SeqCst),5,"no request after cancellation");
        server.abort();
    }

    #[tokio::test]
    async fn generic_music_pages_try_alternative_renditions_and_update_the_container() {
        use axum::response::IntoResponse;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let page=format!("<title>Music</title><audio><source src=\"{base}/old.mp3\"><source src=\"{base}/fresh.m4a\"></audio>");
        let app=axum::Router::new().fallback(move |uri:axum::http::Uri|{let page=page.clone();async move{
            match uri.path(){
                "/page"=>axum::response::Html(page).into_response(),
                "/fresh.m4a"=>([("Content-Type","audio/mp4")],"fresh AAC bytes").into_response(),
                _=>axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }});
        let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
        let queue=crate::Queue::open_in_memory().await.unwrap();
        let manager=MusicManager::new(Client::new(),queue.pool());
        let mut track=track("Song");track.source_url=format!("{base}/page");
        track.source=AudioSource::Direct{url:format!("{base}/old.mp3"),headers:vec![],container:Some("mp3".into())};
        let dir=tempfile::tempdir().unwrap();let dest=dir.path().join("audio.part");
        let stream=AudioStream{url:format!("{base}/old.mp3"),headers:vec![],container:Some("mp3".into()),has_video:false,filesize:None};
        let fresh=manager.fetch_source(DownloadId::new(),&track,stream,None,&dest,&CancellationToken::new(),&AtomicBool::new(false)).await.unwrap();
        assert_eq!(fresh.container.as_deref(),Some("m4a"));
        assert_eq!(std::fs::read(dest).unwrap(),b"fresh AAC bytes");
        server.abort();
    }

    #[tokio::test]
    async fn queued_generic_pages_refresh_before_fetching_but_raw_links_are_not_retried() {
        use axum::response::IntoResponse;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base=format!("http://{}",listener.local_addr().unwrap());
        let page=format!("<title>Music</title><audio src=\"{base}/current.mp3\"></audio>");
        let requests=Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=requests.clone();
        let app=axum::Router::new().fallback(move|uri:axum::http::Uri|{let page=page.clone();let count=count.clone();async move{
            count.fetch_add(1,Ordering::SeqCst);
            if uri.path()=="/page"{axum::response::Html(page).into_response()}else{axum::http::StatusCode::FORBIDDEN.into_response()}
        }});
        let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
        let queue=crate::Queue::open_in_memory().await.unwrap();let manager=MusicManager::new(Client::new(),queue.pool());
        let mut track=track("Song");track.source_url=format!("{base}/page");
        track.source=AudioSource::Direct{url:format!("{base}/expired.mp3"),headers:vec![],container:Some("mp3".into())};
        let (fresh,_)=manager.resolve_stream(DownloadId::new(),&mut track,&CancellationToken::new()).await.unwrap();
        assert_eq!(fresh.url,format!("{base}/current.mp3"));
        let before=requests.load(Ordering::SeqCst);
        track.source_url=fresh.url.clone();track.source=AudioSource::Direct{url:fresh.url.clone(),headers:vec![],container:fresh.container.clone()};
        let dir=tempfile::tempdir().unwrap();
        assert!(manager.fetch_source(DownloadId::new(),&track,fresh,None,&dir.path().join("audio.part"),&CancellationToken::new(),&AtomicBool::new(false)).await.is_err());
        assert_eq!(requests.load(Ordering::SeqCst)-before,1,"a raw CDN URL has no page to renew from");
        server.abort();
    }

    #[tokio::test]
    async fn file_removal_respects_choice_and_preserves_entries_on_failure() {
        for mode in ["keep", "delete", "missing", "blocked"] {
            let dir = tempfile::tempdir().unwrap();
            let queue = crate::Queue::open_in_memory().await.unwrap();
            let manager = MusicManager::new(Client::new(), queue.pool());
            let path = dir.path().join("song.mp3");
            if mode == "blocked" { std::fs::create_dir(&path).unwrap(); }
            else if mode != "missing" { std::fs::write(&path, b"downloaded audio").unwrap(); }
            let id = DownloadId::new();
            let job = MusicJob {
                id, collection_id: None, collection_title: None, collection_url: None,
                position: 1, track: track("Song"), format: AudioFormat::Mp3,
                state: MusicJobState::Finished, stage: None, matched_url: None, matched_title: None,
                downloaded_bytes: 16, total_bytes: Some(16), bytes_per_second: 0, eta_seconds: None,
                error: None, path: Some(path.clone()), directory: dir.path().into(),
                created_at: Utc::now(), updated_at: Utc::now(), finished_at: Some(Utc::now()),
            };
            let plan = Plan { track: job.track.clone(), options: OutputOptions::default(), directory: dir.path().into(),
                stem: "song".into(), collection_id: None, collection_title: None, collection_cover: None, position: 1 };
            manager.persist_new(&job, &plan).await.unwrap();
            manager.jobs.write().await.insert(id, job);
            if mode == "keep" {
                let moved = dir.path().join("moved.mp3");
                std::fs::rename(&path, &moved).unwrap();
                manager.locate_file(id, &moved).await.unwrap();
                assert!(manager.locate_file(id, dir.path()).await.is_err());
                assert!(manager.restart_missing(id).await.is_err(), "existing track must not restart");
                let restored = MusicManager::new(Client::new(), queue.pool());
                restored.recover().await.unwrap();
                assert_eq!(restored.get(id).await.unwrap().path, Some(moved.clone()));
                std::fs::rename(&moved, &path).unwrap();
                manager.locate_file(id, &path).await.unwrap();
            }
            if mode == "blocked" {
                assert!(manager.remove_with_file(id, true).await.is_err());
                assert!(manager.get(id).await.is_some());
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM music_jobs WHERE id = ?").bind(id.to_string()).fetch_one(&queue.pool()).await.unwrap();
                assert_eq!(count, 1);
                assert!(path.is_dir());
            } else {
                assert!(manager.remove_with_file(id, mode != "keep").await.unwrap());
                assert!(manager.get(id).await.is_none());
                assert_eq!(path.exists(), mode == "keep");
            }
        }
    }

    #[test]
    fn a_collection_lands_in_its_own_folder_unless_told_otherwise() {
        let base = PathBuf::from("/music");
        let mut new = NewMusicDownload { source: source(SourceKind::Playlist, vec![track("a")]), options: OutputOptions::default(), base_directory: base.clone() };
        assert_eq!(MusicManager::destination(&new), base.join("My Mix_ Best - Of"));

        new.options.folder_mode = FolderMode::None;
        assert_eq!(MusicManager::destination(&new), base);

        new.options.folder_mode = FolderMode::Custom;
        new.options.folder_name = Some("Road trip".into());
        assert_eq!(MusicManager::destination(&new), base.join("Road trip"));

        new.options.directory = Some(PathBuf::from("/elsewhere"));
        assert_eq!(MusicManager::destination(&new), PathBuf::from("/elsewhere").join("Road trip"));

        // A single track never gets a folder named after itself.
        new.options = OutputOptions::default();
        new.source = source(SourceKind::Track, vec![track("a")]);
        assert_eq!(MusicManager::destination(&new), base);
    }

    #[test]
    fn tags_are_built_from_the_track() {
        let mut t = track("Song");
        t.track_number = Some(3);
        t.track_total = Some(10);
        t.date = Some("2020-05-01".into());
        let tags = tags_for(&t, Some("la la".into()));
        assert_eq!(tags.title.as_deref(), Some("Song"));
        assert_eq!(tags.artist.as_deref(), Some("Artist"));
        assert_eq!(tags.album_artist.as_deref(), Some("Artist"), "falls back to the first artist");
        assert_eq!(tags.track, Some(3));
        assert_eq!(tags.date.as_deref(), Some("2020-05-01"));
        assert_eq!(tags.comment.as_deref(), Some("https://example/t"));
        assert_eq!(tags.lyrics.as_deref(), Some("la la"));
        t.date = None;
        assert_eq!(tags_for(&t, None).date.as_deref(), Some("2020"), "the year stands in for a missing date");
    }

    #[test]
    fn a_streams_own_headers_win_and_the_session_fills_in_behind_them() {
        let stream: Headers = vec![("User-Agent".into(), "com.google.android.apps.youtube.vr.oculus/1.65.10".into())];
        let session: Headers = vec![
            // The identity the URL was signed for must survive: a googlevideo
            // link minted for the VR app is refused if a browser asks for it.
            ("user-agent".into(), "Mozilla/5.0".into()),
            ("Cookie".into(), "LOGIN_INFO=abc; SAPISID=def".into()),
            ("X-Youtube-Potoken".into(), "tok".into()),
        ];
        let merged = merge_session_under(stream, session);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].1, "com.google.android.apps.youtube.vr.oculus/1.65.10", "the stream keeps its own client identity");
        assert!(merged.iter().any(|(k, v)| k == "Cookie" && v.contains("LOGIN_INFO")), "the sign-in has to reach the byte fetch, not only the analysis");
        assert!(merged.iter().any(|(k, _)| k == "X-Youtube-Potoken"));
    }

    #[test]
    fn merging_an_empty_session_changes_nothing() {
        let stream: Headers = vec![("User-Agent".into(), "ua".into())];
        assert_eq!(merge_session_under(stream.clone(), Headers::new()), stream);
    }

    #[test]
    fn states_round_trip() {
        for s in [MusicJobState::Queued, MusicJobState::Resolving, MusicJobState::Downloading, MusicJobState::Converting, MusicJobState::Finished, MusicJobState::Failed, MusicJobState::Skipped, MusicJobState::Paused] {
            assert_eq!(MusicJobState::parse(s.as_str()), s);
        }
        assert!(MusicJobState::Queued.is_active());
        assert!(MusicJobState::Skipped.is_terminal());
    }
}
