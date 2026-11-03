//! Tracks video-download jobs: fetch one or two streams, merge if needed,
//! land the final file — independent of the segmented-download queue, since
//! a video job is a different shape of work (an analysis step up front, a
//! possible merge at the end) and forcing it through `Queue`/`Supervisor`
//! would mean bending both around a case they were never designed for.
//!
//! A job can be paused and resumed. The unit of resumption is a whole segment
//! for a segmented source and a byte offset for a single file — see
//! [`super::stream::StreamProgress`]. Because analysis is stateless, resuming
//! needs the chosen format, the headers it was resolved with, and the resolved
//! filename to be on the row rather than in the caller's memory; migration
//! `0006` is where they live.
//!
//! An interrupted job (a crash, a kill) comes back as `paused` rather than
//! `failed`: the partial file and its position are both still on disk, so the
//! honest state is "stopped, continue when you like" rather than "lost".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::stream::{StreamOptions, StreamOutcome, StreamProgress, StreamTick};
use super::{VideoFormat, VideoMetadata, youtube};
use crate::error::{Error, Result};
use crate::headers::{self, Headers};
use crate::model::DownloadId;
use crate::session::{self, SessionSummary, SessionVault, SiteSession};
use crate::speed::{SharedRateLimit, SpeedMeter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoJobState {
    Downloading,
    Merging,
    /// Stopped at a resumable boundary, by the user or by a restart. Distinct
    /// from `Failed`: nothing went wrong and the bytes already fetched are
    /// still good.
    Paused,
    Finished,
    Failed,
}

impl VideoJobState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Merging => "merging",
            Self::Paused => "paused",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoJob {
    #[serde(default)]
    pub thumbnail: Option<String>,
    pub id: DownloadId,
    pub title: String,
    pub webpage_url: String,
    pub format_id: Option<String>,
    pub total_bytes: Option<u64>,
    pub state: VideoJobState,
    pub downloaded_bytes: u64,
    /// Live throughput. In memory only, like `Supervisor`'s — a rate written to
    /// SQLite is a rate that is already stale, and reloading a stopped job at
    /// its last speed would be a lie.
    pub bytes_per_second: u64,
    /// Seconds remaining at the current rate, when both a rate and a total are
    /// known.
    pub eta_seconds: Option<u64>,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub validation: Option<super::validation::VideoValidation>,
}

impl VideoJob {
    /// Whether [`VideoManager::resume`] can do anything with this job.
    pub fn is_resumable(&self) -> bool {
        matches!(self.state, VideoJobState::Paused | VideoJobState::Failed)
    }
}

/// Everything a job needs to run, whether it is starting or continuing.
///
/// Kept together because a resume has to reproduce the original run exactly:
/// the same format, the same session, and above all the same filename.
#[derive(Debug, Clone)]
struct JobPlan {
    title: String,
    webpage_url: String,
    format: VideoFormat,
    directory: PathBuf,
    headers: Headers,
    /// Resolved once at `start`, then reused verbatim.
    filename: String,
}

/// Per-stream position within a job.
///
/// A merge job fetches two streams in sequence, so "where did this job stop"
/// is two positions and two done-flags, not one. The flags are separate from
/// the positions because a finished stream has no meaningful position left —
/// only the fact that it need not be fetched again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobResume {
    video: StreamProgress,
    video_done: bool,
    audio: StreamProgress,
    audio_done: bool,
    #[serde(default)]
    validation: Option<super::validation::VideoValidation>,
}

fn job_from_row(row: &SqliteRow) -> Result<VideoJob> {
    let state: String = row.try_get("state")?;
    let state = match state.as_str() {
        "downloading" => VideoJobState::Downloading,
        "merging" => VideoJobState::Merging,
        "paused" => VideoJobState::Paused,
        "finished" => VideoJobState::Finished,
        _ => VideoJobState::Failed,
    };

    // The stored format is what lets a reloaded job show the quality it was
    // started at, instead of a blank where the label should be.
    let format: Option<VideoFormat> = row
        .try_get::<Option<String>, _>("format")
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok());

    Ok(VideoJob {
        id: DownloadId(
            uuid::Uuid::parse_str(row.try_get::<String, _>("id")?.as_str()).map_err(|e| Error::Corrupt(e.to_string()))?,
        ),
        title: row.try_get("title")?,
        webpage_url: row.try_get("webpage_url")?,
        format_id: format.as_ref().map(|f| f.id.clone()),
        thumbnail: format.as_ref().and_then(|f| f.download_options.as_ref()).and_then(|o| o.thumbnail.clone()),
        total_bytes: format.as_ref().and_then(|f| f.filesize),
        state,
        downloaded_bytes: row.try_get::<i64, _>("downloaded_bytes")? as u64,
        // A job loaded from disk is not running, so it has no rate yet.
        bytes_per_second: 0,
        eta_seconds: None,
        error: row.try_get("error")?,
        path: row.try_get::<Option<String>, _>("path")?.map(PathBuf::from),
        validation: row.try_get::<Option<String>, _>("resume_state").ok().flatten()
            .and_then(|json| serde_json::from_str::<JobResume>(&json).ok()).and_then(|r| r.validation),
        created_at: DateTime::parse_from_rfc3339(&row.try_get::<String, _>("created_at")?)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| Error::Corrupt(e.to_string()))?,
    })
}

/// Whether a stream source is an HLS media playlist (so its concatenated
/// MPEG-TS segments should be remuxed into `.mp4`). Query strings are common on
/// these URLs, so the extension check ignores them.
fn is_hls_source(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
        .ends_with(".m3u8")
}

/// Whether a partial stream file is really on disk with bytes in it.
///
/// Resume flags live in the row and the files live in the user's download
/// folder, so the two can disagree: a folder the user cleaned out, or a merge
/// that deleted its inputs before failing, leaves a job that believes both
/// streams are finished and has nothing to merge.
async fn file_has_bytes(path: &Path) -> bool {
    tokio::fs::metadata(path).await.map(|meta| meta.len() > 0).unwrap_or(false)
}

/// A chosen format, ready to start. The client (extension/app) is expected to
/// pass back the exact `VideoFormat` an earlier `analyze` call returned —
/// analysis is stateless, so nothing is cached server-side between the two.
#[derive(Debug, Clone)]
pub struct NewVideoDownload {
    pub title: String,
    pub webpage_url: String,
    pub format: VideoFormat,
    pub directory: PathBuf,
    pub headers: Headers,
}

#[derive(Clone)]
pub struct VideoManager {
    client: Client,
    pool: SqlitePool,
    jobs: Arc<RwLock<HashMap<DownloadId, VideoJob>>>,
    /// One token per running job. Present only while a job is in flight;
    /// `cancel` looks here, and a missing entry means there is nothing running
    /// to stop.
    running: Arc<RwLock<HashMap<DownloadId, CancellationToken>>>,
    /// Browser sessions the extension has shared, keyed by
    /// [`super::site::cookie_domain_for`]'s canonical domain (plus one entry
    /// per job for sites without a known domain). This is what lets a URL
    /// *typed or pasted* straight into the Video view resolve without login
    /// errors, and what a resumed job re-attaches its session from — job
    /// rows never carry cookies themselves.
    vault: Arc<SessionVault>,
    /// Short-lived attachment metadata from the selected browser profile.
    /// Kept out of session/job persistence because signed CDN URLs expire.
    facebook_posts: Arc<RwLock<HashMap<String, (std::time::Instant, crate::image::ImageAlbum)>>>,
    /// Segment concurrency, per-segment retry, and the bandwidth cap.
    stream_options: StreamOptions,
    /// Runs when a job finishes, with the muxed file's final path — the video
    /// equivalent of the queue's hook in [`crate::runner::run_one`]. Video
    /// jobs live outside the queue, so they need their own.
    on_finished: Option<crate::runner::FinishedHook>,
}

/// What to tell the user when googlevideo stops serving a stream partway.
///
/// Report observed status and session state without claiming that a refusal
/// proves a preview allowance, or that signing in guarantees recovery.
fn stream_refused(status: u16, on_disk: u64, headers: &Headers) -> String {
    let session = youtube::Session::from_headers(headers);
    let mb = on_disk / (1 << 20);
    let head = format!("YouTube's video host refused the stream (HTTP {status}) after {mb} MiB. URL refresh did not recover it. A 403/410 alone does not identify the cause");
    let tail = if !session.signed_in() {
        "the shared session is not signed in. Try analyzing the link again and selecting an HLS format. \
         If the video requires an account, share that browser session with Zoltraak"
    } else if session.po_token.is_none() {
        "the shared session is signed in but carries no GVS proof-of-origin token. Try an HLS format, or play \
         this exact video with the updated Zoltraak extension and analyze it again; token capture is not guaranteed"
    } else {
        "a GVS token is stored, but it may be expired, bound to another video, or incompatible with the client. \
         Analyze again and try an HLS format, or refresh playback of this exact video in the browser"
    };
    format!("{head}; {tail}")
}

impl VideoManager {
    pub fn new(client: Client, pool: SqlitePool) -> Self {
        Self {
            client,
            pool,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashMap::new())),
            vault: Arc::new(SessionVault::in_memory()),
            facebook_posts: Arc::new(RwLock::new(HashMap::new())),
            stream_options: StreamOptions::default(),
            on_finished: None,
        }
    }

    /// Keep shared browser sessions in this vault — persistent and encrypted
    /// when opened with [`SessionVault::open`] — instead of the default
    /// in-memory one that forgets everything at exit.
    pub fn with_session_vault(mut self, vault: Arc<SessionVault>) -> Self {
        self.vault = vault;
        self
    }

    /// Call `hook` with the finished file whenever a job completes.
    pub fn with_completion_hook(mut self, hook: crate::runner::FinishedHook) -> Self {
        self.on_finished = Some(hook);
        self
    }

    pub fn session_vault(&self) -> &Arc<SessionVault> {
        &self.vault
    }

    /// Share the engine's bandwidth cap with this manager, so a video download
    /// counts against the same limit as a queued file download.
    ///
    /// Hand it [`crate::Downloader::rate_limiter`] from the same `Supervisor`
    /// that drives the queue. Without this, video jobs fetch unthrottled — a
    /// setting the user set and the app then ignored on one of its two
    /// download paths.
    pub fn with_rate_limiter(mut self, limiter: SharedRateLimit) -> Self {
        self.stream_options.limiter = Some(limiter);
        self
    }

    /// Override segment concurrency and per-segment retry.
    pub fn with_segment_options(mut self, segments: super::stream::SegmentOptions) -> Self {
        self.stream_options.segments = segments;
        self
    }

    /// Record the extension's freshest session for a login-gated site.
    /// `domain` should already be the canonical key (`"x.com"`,
    /// `"instagram.com"`, ...); an unrecognized one is stored verbatim
    /// (lowercased) so a future platform still works without an engine change,
    /// it just won't be looked up by `cookie_domain_for` until it's added
    /// there too. The session is minimised on the way in and, with a
    /// persistent vault, encrypted on disk.
    pub async fn store_session(&self, domain: &str, session: SiteSession) -> Result<()> {
        self.vault
            .set(domain, session)
            .await
            .map_err(|source| Error::Io { path: PathBuf::from("sessions.vault"), source })
    }

    /// What is stored, for a settings screen: sites and dates, never values.
    pub async fn sessions(&self) -> Vec<SessionSummary> {
        self.vault.summaries().await
    }

    pub async fn forget_session(&self, domain: &str) -> Result<bool> {
        if domain == "facebook.com" { self.facebook_posts.write().await.clear(); }
        self.vault
            .remove(domain)
            .await
            .map_err(|source| Error::Io { path: PathBuf::from("sessions.vault"), source })
    }

    pub async fn forget_all_sessions(&self) -> Result<()> {
        self.facebook_posts.write().await.clear();
        self.vault
            .clear()
            .await
            .map_err(|source| Error::Io { path: PathBuf::from("sessions.vault"), source })
    }

    pub async fn clear_shared_sessions(&self) -> Result<()> {
        self.facebook_posts.write().await.clear();
        self.vault
            .clear_shared()
            .await
            .map_err(|source| Error::Io { path: PathBuf::from("sessions.vault"), source })
    }

    /// Header names that are credentials rather than request shaping. They
    /// live in the vault, never in a job row.
    fn is_secret_header(name: &str) -> bool {
        name.eq_ignore_ascii_case("cookie")
            || name.eq_ignore_ascii_case("authorization")
            || name.eq_ignore_ascii_case(youtube::POTOKEN_HEADER)
            || name.eq_ignore_ascii_case(youtube::VISITOR_HEADER)
            || name.eq_ignore_ascii_case(youtube::POTOKEN_VIDEO_HEADER)
    }

    fn strip_secrets(headers: &Headers) -> Headers {
        headers.iter().filter(|(name, _)| !Self::is_secret_header(name)).cloned().collect()
    }

    fn has_secrets(headers: &Headers) -> bool {
        headers.iter().any(|(name, _)| Self::is_secret_header(name))
    }

    /// A caller that sends a session with its request (the extension's
    /// per-link flow, or the app relaying a prefill) has the freshest one
    /// there is; remember it for the site so a later pasted URL gets it too.
    async fn learn_session(&self, url: &str, headers: &Headers) {
        if !Self::has_secrets(headers) {
            return;
        }
        let Some(domain) = super::site::cookie_domain_for(url) else { return };
        if let Err(e) = self.store_session(domain, SiteSession::from_headers(headers)).await {
            tracing::warn!(error = %e, domain, "could not store the shared session");
        }
    }

    /// `headers`, with the stored session for `url`'s site filled in wherever
    /// the caller did not supply the corresponding header themselves — an
    /// explicit, freshly-captured session always wins over the vault. For a
    /// site with no known domain, `job` names the job-scoped entry to use.
    async fn with_session(&self, url: &str, headers: &Headers, job: Option<DownloadId>) -> Headers {
        let stored = match super::site::cookie_domain_for(url) {
            Some(domain) => self.vault.get(domain).await,
            None => match job {
                Some(id) => self.vault.get(&session::job_key(&id)).await,
                None => None,
            },
        };
        let Some(stored) = stored else { return headers.clone() };
        let mut merged = headers.clone();
        for (name, value) in stored.to_headers() {
            if !merged.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(&name)) {
                merged.push((name, value));
            }
        }
        merged
    }

    pub async fn analyze(&self, url: &str, headers: &Headers) -> Result<VideoMetadata> {
        self.analyze_with_fallback(url, headers, false).await
    }

    pub async fn analyze_with_fallback(&self, url: &str, headers: &Headers, use_fallback: bool) -> Result<VideoMetadata> {
        self.analyze_source(url, headers, use_fallback, None, None).await
    }

    pub async fn analyze_source(&self, url: &str, headers: &Headers, use_fallback: bool, source_kind: Option<&str>, media_type: Option<&str>) -> Result<VideoMetadata> {
        self.learn_session(url, headers).await;
        let headers = self.with_session(url, headers, None).await;
        super::analyze_source(&self.client, url, &headers, use_fallback, source_kind, media_type).await
    }

    /// Analyze a post URL into the pictures it holds.
    ///
    /// This lives here, rather than in a manager of its own, because the two
    /// things a photo extractor needs from the outside are exactly the two this
    /// type already owns: the HTTP client, and the session vault. A photo post
    /// on Instagram or Facebook is gated by the same login as a video on the
    /// same site, so it must learn and re-attach a session identically — and a
    /// second manager holding a second copy of that logic would be a second
    /// place for the vault rules to drift.
    ///
    /// Unlike a video, the result needs no job of its own: each picture is a
    /// plain file the ordinary download queue can fetch.
    pub async fn analyze_images(&self, url: &str, headers: &Headers) -> Result<crate::image::ImageAlbum> {
        self.learn_session(url, headers).await;
        let headers = self.with_session(url, headers, None).await;
        let mut posts = self.facebook_posts.write().await;
        posts.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(300));
        let browser_id = crate::image::facebook::post_id(url);
        let browser_at = browser_id.as_ref().and_then(|id| posts.get(id).map(|(at, _)| *at));
        let mut browser = browser_id.as_ref().and_then(|id| posts.get(id).map(|(_, a)| a.clone()));
        drop(posts);
        if let Some(album) = &mut browser { crate::image::facebook::browser_headers(album, &headers); }
        let browser_complete = browser.as_ref().is_some_and(|a| a.completeness == Some(crate::image::Completeness::Complete));
        let result = crate::image::analyze_with_browser(&self.client, url, &headers, browser).await;
        // A full browser list with expired/refused CDN links must not keep
        // winning subsequent analysis over freshly fetched engine results.
        if browser_complete && !result.as_ref().is_ok_and(|a| a.completeness == Some(crate::image::Completeness::Complete)) {
            if let Some(id) = browser_id {
                let mut posts = self.facebook_posts.write().await;
                if posts.get(&id).is_some_and(|(at, _)| Some(*at) == browser_at) { posts.remove(&id); }
            }
        }
        result
    }

    pub async fn store_facebook_post(&self, url: &str, documents: &[serde_json::Value]) -> Result<()> {
        if documents.len() > 32 || serde_json::to_vec(documents).map_or(true, |v| v.len() > 512 * 1024) {
            return Err(Error::InvalidInput("browser attachment metadata exceeds its size limit".into()));
        }
        let album = crate::image::facebook::browser_album(url, documents)?;
        let id = crate::image::facebook::post_id(url).unwrap();
        let mut posts = self.facebook_posts.write().await;
        posts.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(300));
        if posts.len() >= 16 && !posts.contains_key(&id) {
            if let Some(oldest) = posts.iter().min_by_key(|(_, (at, _))| at).map(|(id, _)| id.clone()) { posts.remove(&oldest); }
        }
        posts.insert(id, (std::time::Instant::now(), album));
        Ok(())
    }

    /// Load history from disk and park anything a previous run left mid-flight.
    /// Call once at startup, alongside `Queue`'s own recovery.
    ///
    /// Interrupted work is marked `paused`, not `failed`. The partial file and
    /// the last recorded boundary both survived the crash, so the job is
    /// continuable — and calling that a failure would push the user to start a
    /// multi-gigabyte download over for no reason.
    pub async fn recover(&self) -> Result<()> {
        sqlx::query(
            "UPDATE video_jobs SET state = 'paused', error = NULL \
             WHERE state IN ('downloading', 'merging')",
        )
        .execute(&self.pool)
        .await?;

        let rows = sqlx::query(
            "SELECT id, title, webpage_url, state, downloaded_bytes, error, path, created_at, format, resume_state \
             FROM video_jobs ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut jobs = self.jobs.write().await;
        for row in &rows {
            if let Ok(job) = job_from_row(row) {
                jobs.insert(job.id, job);
            }
        }
        Ok(())
    }

    pub async fn start(&self, mut new: NewVideoDownload) -> Result<DownloadId> {
        if let Some(options) = &new.format.download_options {
            if let Some(subtitle) = &options.subtitle { super::subtitles::validate(subtitle)?; }
            if let Some(seconds) = options.live_seconds {
                if !(5..=1800).contains(&seconds) || new.format.requires_merge || !new.format.video_url.as_deref().is_some_and(is_hls_source) {
                    return Err(Error::InvalidInput("Timed recording requires a muxed HLS media stream and 5–1800 seconds".into()));
                }
                new.format.filesize = None;
                if let Some(validation) = &mut new.format.validation { validation.duration = None; }
            }
        }
        new.title = super::title::ensure_valid_title(&new.title, &new.webpage_url);
        let id = DownloadId::new();

        // Credentials go to the vault, not the row. A site with a known
        // session domain shares them with every later request for that site;
        // anything else keeps them under the job, and only as long as the job.
        self.learn_session(&new.webpage_url, &new.headers).await;
        if super::site::cookie_domain_for(&new.webpage_url).is_none() && Self::has_secrets(&new.headers) {
            if let Err(e) = self.store_session(&session::job_key(&id), SiteSession::from_headers(&new.headers)).await {
                tracing::warn!(error = %e, %id, "could not store the job session");
            }
        }
        let request_headers = self.with_session(&new.webpage_url, &new.headers, Some(id)).await;
        let expects_video = !new.format.is_audio_only();
        let expects_audio = new.format.requires_merge || new.format.container == "m4a" || new.format.has_audio == Some(true);
        super::validation::inspect_format(&self.client, &mut new.format, &request_headers).await;
        if let Some(validation) = &mut new.format.validation {
            let page_id = super::validation::content_identity(&new.webpage_url);
            if validation.content_id.is_some() && page_id.is_some() && validation.content_id != page_id {
                let _ = self.vault.remove(&session::job_key(&id)).await;
                return Err(Error::InvalidInput("The selected format belongs to a different video. Analyze the selected page again.".into()));
            }
            if validation.content_id.is_none() { validation.content_id = page_id; }
            if validation.status == super::validation::ValidationStatus::Verified
                && ((expects_video && validation.has_video != Some(true)) || (expects_audio && validation.has_audio != Some(true))) {
                let _ = self.vault.remove(&session::job_key(&id)).await;
                return Err(Error::InvalidInput("The source no longer contains the selected video/audio tracks. Analyze it again before downloading.".into()));
            }
        }
        new.headers = Self::strip_secrets(&new.headers);

        // The filename is resolved once, here, and never re-derived. Doing it
        // again on resume would run the collision numbering against a directory
        // that by then holds this job's own partial file, so the resumed run
        // would pick "clip (2).mp4" and abandon the bytes in "clip.mp4".
        let raw_name = crate::probe::sanitize_filename(&new.title);
        let base_filename = crate::probe::ensure_extension(&raw_name, Some(&new.format.container));
        // Two different videos can easily land on the same title-derived name
        // (a generic caption, a retried job, the same clip re-downloaded) —
        // number it instead of letting the second finish overwrite the first.
        let filename = crate::probe::dedupe_filename(&base_filename, |candidate| {
            new.directory.join(candidate).exists()
        });

        let plan = JobPlan {
            title: new.title.clone(),
            webpage_url: new.webpage_url.clone(),
            format: new.format,
            directory: new.directory,
            headers: new.headers,
            filename,
        };

        let job = VideoJob {
            id,
            title: plan.title.clone(),
            webpage_url: plan.webpage_url.clone(),
            format_id: Some(plan.format.id.clone()),
            thumbnail: plan.format.download_options.as_ref().and_then(|o| o.thumbnail.clone()),
            total_bytes: plan.format.filesize,
            state: VideoJobState::Downloading,
            downloaded_bytes: 0,
            bytes_per_second: 0,
            eta_seconds: None,
            error: None,
            path: None,
            created_at: Utc::now(),
            validation: None,
        };
        self.persist_new(&job, &plan).await?;
        self.jobs.write().await.insert(id, job);

        self.spawn(id, plan, JobResume::default()).await;
        Ok(id)
    }

    /// Stop a running job at its next resumable boundary.
    ///
    /// Returns `false` when nothing was running — an already-paused or finished
    /// job is not an error to cancel, it is simply a no-op.
    pub async fn cancel(&self, id: DownloadId) -> bool {
        // The entry is left in place rather than removed: the task is still
        // unwinding to its boundary and still owns the file. It removes its own
        // entry when it exits, which is what makes that entry a truthful answer
        // to "is anything still writing?" — see `await_stopped`.
        let token = self.running.read().await.get(&id).cloned();
        match token {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Wait for a cancelled job's task to actually finish writing.
    ///
    /// Cancelling only *asks*; the task still has a segment to finish and a
    /// file to flush. Deleting its files before it lets go would either fail on
    /// Windows or, worse, let the task recreate them a moment later.
    async fn await_stopped(&self, id: DownloadId) {
        for _ in 0..100 {
            if !self.running.read().await.contains_key(&id) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tracing::warn!(%id, "video job did not stop within two seconds; proceeding anyway");
    }

    /// Continue a paused or failed job from where it stopped.
    ///
    /// Everything needed is on the row, so this works after a restart as well
    /// as within a session.
    pub async fn resume(&self, id: DownloadId) -> Result<()> {
        if self.running.read().await.contains_key(&id) {
            // Already going. Resuming twice would run two writers over one file.
            return Ok(());
        }

        let (plan, resume) = self.load_plan(id).await?;
        self.update(id, |job| {
            job.state = VideoJobState::Downloading;
            job.error = None;
        })
        .await;
        self.spawn(id, plan, resume).await;
        Ok(())
    }

    pub async fn list(&self) -> Vec<VideoJob> {
        let mut jobs: Vec<_> = self.jobs.read().await.values().cloned().collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs
    }

    pub async fn get(&self, id: DownloadId) -> Option<VideoJob> {
        self.jobs.read().await.get(&id).cloned()
    }

    pub async fn locate_file(&self, id: DownloadId, path: &Path) -> Result<()> {
        crate::file_status::require_file(path).await?;
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&id).ok_or(Error::NotFound(id))?;
        if job.state != VideoJobState::Finished { return Err(Error::InvalidInput("Only completed videos can be located.".into())); }
        let mut next = job.clone();
        next.path = Some(path.into());
        // Verification belongs to the original output, not an arbitrary file chosen later.
        next.validation = None;
        sqlx::query("UPDATE video_jobs SET resume_state = NULL WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        self.persist(&next).await?;
        *job = next;
        Ok(())
    }

    pub async fn restart_missing(&self, id: DownloadId) -> Result<()> {
        let recorded = self.get(id).await.ok_or(Error::NotFound(id))?;
        let job = &recorded;
        if job.state != VideoJobState::Finished { return Err(Error::InvalidInput("Only completed videos can be downloaded again.".into())); }
        let path = job.path.as_ref().ok_or_else(|| Error::InvalidInput("This entry has no recorded output.".into()))?;
        crate::file_status::require_missing(path).await?;
        let (plan, _) = self.load_plan(id).await?;
        // The saved plan may name the old folder after Locate file. Never overwrite a file there.
        crate::file_status::require_missing(&plan.directory.join(&plan.filename)).await?;
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&id).ok_or(Error::NotFound(id))?;
        if job.state != VideoJobState::Finished || job.path != recorded.path {
            return Err(Error::InvalidInput("The video entry changed; check it again before restarting.".into()));
        }
        sqlx::query("UPDATE video_jobs SET resume_state = NULL WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        let mut next = job.clone();
        next.state = VideoJobState::Downloading; next.path = None; next.downloaded_bytes = 0; next.total_bytes = None; next.validation = None; next.error = None;
        self.persist(&next).await?;
        *job = next;
        drop(jobs);
        self.spawn(id, plan, JobResume::default()).await;
        Ok(())
    }

    /// Forget a job, stopping it first if it is running.
    ///
    /// Cancelling before deleting is the whole point: the previous version
    /// dropped the row while the spawned task kept fetching and kept writing to
    /// disk, so "Remove" appeared to stop a download that was in fact still
    /// running to completion.
    pub async fn remove(&self, id: DownloadId) -> Result<bool> {
        self.remove_with_file(id, false).await
    }

    pub async fn remove_with_file(&self, id: DownloadId, delete_file: bool) -> Result<bool> {
        if self.cancel(id).await {
            self.await_stopped(id).await;
        }

        if delete_file {
            if self.running.read().await.contains_key(&id) {
                return Err(Error::InvalidInput("Video is still stopping; try deleting it again shortly".into()));
            }
            if let Some(path) = self.get(id).await.and_then(|job| job.path) {
                crate::file_removal::remove_output(&path).await?;
            }
        }

        // Partial files are this job's alone (the filename was deduped against
        // the directory when the job was created), so removing the job removes
        // them. A finished job's real output is left alone — the user asked to
        // clear a row, not to delete a video they already have.
        if let Ok((plan, _)) = self.load_plan(id).await {
            let state = self.jobs.read().await.get(&id).map(|job| job.state);
            if state != Some(VideoJobState::Finished) {
                for path in Self::partial_paths(&plan) {
                    super::groups::cleanup(&path).await;
                    super::live::cleanup(&path).await;
                    if delete_file {
                        crate::file_removal::remove_output(&path).await?;
                    } else {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                }
            }
        }

        let existed = self.jobs.write().await.remove(&id).is_some();
        sqlx::query("DELETE FROM video_jobs WHERE id = ?").bind(id.to_string()).execute(&self.pool).await?;
        // The job's own session, if it had one, goes with it.
        let _ = self.vault.remove(&session::job_key(&id)).await;
        Ok(existed)
    }

    /// Files an unfinished job may have left behind, for cleanup.
    ///
    /// The destination is on this list too. On the direct path there is no
    /// separate temp file — the partial video *is* the destination — so
    /// leaving it behind would drop a half-written `.mp4` in the user's folder
    /// that looks like a real download and plays as a truncated one. Callers
    /// only use this list for jobs that did not finish.
    fn partial_paths(plan: &JobPlan) -> Vec<PathBuf> {
        let stem = Path::new(&plan.filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| plan.filename.clone());
        vec![
            plan.directory.join(format!("{stem}.video.tmp")),
            plan.directory.join(format!("{stem}.audio.tmp")),
            plan.directory.join(format!("{stem}.ts.part")),
            plan.directory.join(&plan.filename),
        ]
    }

    async fn spawn(&self, id: DownloadId, plan: JobPlan, resume: JobResume) {
        let token = CancellationToken::new();
        self.running.write().await.insert(id, token.clone());

        let manager = self.clone();
        tokio::spawn(async move {
            manager.run(id, plan, resume, token).await;
            manager.running.write().await.remove(&id);
        });
    }

    async fn run(&self, id: DownloadId, plan: JobPlan, resume: JobResume, cancel: CancellationToken) {
        match self.run_inner(id, plan, resume, &cancel).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(error = %e, %id, "video job failed");
                self.clear_rate(id).await;
                self.update(id, |job| {
                    job.state = VideoJobState::Failed;
                    job.error = Some(e.to_string());
                })
                .await;
            }
        }
    }

    /// Options for one stream: shared config, plus this stream's own position
    /// and the job's cancellation token.
    fn stream_options(&self, resume: &StreamProgress, cancel: &CancellationToken, concurrency: usize) -> StreamOptions {
        let mut segments = self.stream_options.segments.clone();
        segments.concurrency = segments.concurrency.min(concurrency).max(1);
        StreamOptions {
            live_seconds: None,
            segments,
            limiter: self.stream_options.limiter.clone(),
            resume: resume.clone(),
            cancel: cancel.clone(),
        }
    }

    async fn run_inner(&self, id: DownloadId, mut plan: JobPlan, resume: JobResume, cancel: &CancellationToken) -> Result<()> {
        std::fs::create_dir_all(&plan.directory).map_err(|source| Error::Io {
            path: plan.directory.clone(),
            source,
        })?;

        // The row carries no credentials; the session comes from the vault —
        // the site's, or this job's own — so the bytes are fetched with the
        // same session the format was resolved with.
        let mut headers = self.with_session(&plan.webpage_url, &plan.headers, Some(id)).await;

        let mut resume = resume;

        // Stream links from a site that signs them expire. A job resumed after
        // a long pause, or one that outlived its links, is re-resolved to the
        // same format rather than failed with a 403 the user cannot act on.
        if youtube::format_expired(&plan.format, Utc::now().timestamp()) {
            self.refresh_format(id, &mut plan, &mut resume, &headers).await?;
        }

        // Refresh expired links with a bounded recovery budget. Repeated
        // extraction cannot repair missing attestation or an IP restriction.
        const MAX_REFRESHES: usize = 2;
        let mut refreshes = 0;
        let mut rate_retries = 0;
        let mut concurrency = self.stream_options.segments.concurrency.max(1);
        loop {
            let before = resume.video.bytes + resume.audio.bytes + (resume.video_done as u64);
            match self.fetch_streams(id, &plan, &mut resume, cancel, &headers, concurrency).await {
                Err(Error::UnexpectedStatus { status: status @ (401 | 403 | 410), url }) if youtube::is_stream_url(&url) || super::validation::content_identity(&plan.webpage_url).is_some() => {
                    let after = resume.video.bytes + resume.audio.bytes + (resume.video_done as u64);
                    let advanced = after > before;
                    if refreshes >= MAX_REFRESHES || (refreshes > 0 && !advanced) {
                        let on_disk = resume.video.bytes + resume.audio.bytes;
                        return Err(Error::InvalidInput(if youtube::is_stream_url(&url) {
                            stream_refused(status, on_disk, &headers)
                        } else { format!("The source refused the stream (HTTP {status}). Refresh the site's browser session and analyze this video again.") }));
                    }
                    refreshes += 1;
                    tracing::info!(%id, refreshes, on_disk = after, "stream link refused; re-resolving and continuing");
                    headers = self.with_session(&plan.webpage_url, &plan.headers, Some(id)).await;
                    self.refresh_format(id, &mut plan, &mut resume, &headers).await?;
                }
                Err(Error::UnexpectedStatus { status: 429, .. }) => {
                    if rate_retries >= 2 {
                        return Err(Error::InvalidInput("The website is rate-limiting downloads. Wait a while and retry.".into()));
                    }
                    concurrency = (concurrency / 2).max(1);
                    let delay = std::time::Duration::from_secs(1 << rate_retries);
                    rate_retries += 1;
                    tokio::select! {
                        _ = cancel.cancelled() => return self.park(id, &plan, &resume).await,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                other => return other,
            }
        }
    }

    /// Refresh only compatible content; changed source identities restart the
    /// job's partials instead of appending a different representation.
    async fn refresh_format(&self, id: DownloadId, plan: &mut JobPlan, resume: &mut JobResume, headers: &Headers) -> Result<()> {
        crate::instagram::invalidate(&plan.webpage_url).await;
        let meta = super::analyze_with_fallback(&self.client, &plan.webpage_url, headers, plan.format.id.starts_with("yt-dlp-")).await?;
        let mut format = meta.formats.into_iter().find(|f| f.id == plan.format.id)
            .ok_or_else(|| Error::InvalidInput("The selected format is no longer available. Analyze the video again.".into()))?;
        super::validation::check_refresh(&plan.format, &format, &plan.webpage_url)?;
        format.download_options = plan.format.download_options.clone();
        let sources_changed = [(&plan.format.video_url, &format.video_url), (&plan.format.audio_url, &format.audio_url)]
            .iter().any(|(old, new)| match (old, new) {
                (Some(a), Some(b)) => super::validation::source_identity(a) != super::validation::source_identity(b),
                (None, None) => false,
                _ => true,
            });
        if sources_changed {
            if plan.format.download_options.as_ref().is_some_and(|o| o.live_seconds.is_some()) {
                return Err(Error::InvalidInput("Live stream URL changed; captured segments were retained. Start a new recording.".into()));
            }
            // Keep the same destination, but never keep bytes whose identity is
            // uncertain. This also clears completed-track flags before a merge.
            for path in Self::partial_paths(plan) {
                super::groups::cleanup(&path).await;
                    super::live::cleanup(&path).await;
                if path.exists() { tokio::fs::remove_file(&path).await.map_err(|source| Error::Io { path, source })?; }
            }
            *resume = JobResume::default();
            self.save_resume(id, resume).await;
        }
        plan.format = format;
        let json = serde_json::to_string(&plan.format).map_err(|e| Error::Corrupt(e.to_string()))?;
        sqlx::query("UPDATE video_jobs SET format = ? WHERE id = ?").bind(json).bind(id.to_string()).execute(&self.pool).await?;
        Ok(())
    }

    async fn finish_checked(&self, id: DownloadId, path: PathBuf, plan: &JobPlan, resume: &mut JobResume,
        cancel: &CancellationToken, cleanup: &[PathBuf]) -> Result<()> {
        self.update(id, |job| job.path = Some(path.clone())).await;
        let mut expected = plan.format.clone();
        if expected.download_options.as_ref().is_some_and(|o| o.live_seconds.is_some()) {
            let input = cleanup.first().unwrap_or(&path);
            let duration = super::live::recorded_duration(input).await
                .ok_or_else(|| Error::InvalidInput("Live recording duration ledger is unavailable; the video was retained".into()))?;
            expected.validation.get_or_insert_with(Default::default).duration = Some(duration);
        }
        let report = tokio::select! {
            biased;
            _ = cancel.cancelled() => return self.park(id, plan, resume).await,
            result = super::validation::inspect_output(&path, &expected) => result?,
        };
        if cancel.is_cancelled() { return self.park(id, plan, resume).await; }
        if let Some(subtitle) = plan.format.download_options.as_ref().and_then(|o| o.subtitle.as_ref()) {
            let mut headers = self.with_session(&plan.webpage_url, &plan.headers, Some(id)).await;
            for (key, value) in &plan.format.headers {
                if !headers.iter().any(|(k,_)| k.eq_ignore_ascii_case(key)) { headers.push((key.clone(), value.clone())); }
            }
            tokio::select! {
                _ = cancel.cancelled() => return self.park(id, plan, resume).await,
                result = super::subtitles::save(&self.client, subtitle, &plan.webpage_url, &headers, &path) => { result?; },
            }
        }
        for temporary in cleanup {
            super::groups::cleanup(temporary).await;
            super::live::cleanup(temporary).await;
            let _ = tokio::fs::remove_file(temporary).await;
        }
        super::groups::cleanup(&path).await;
        super::live::cleanup(&path).await;
        resume.validation = Some(report.clone());
        // Clear positions while retaining the report in the same durable record.
        self.save_resume(id, &JobResume { validation: Some(report.clone()), ..Default::default() }).await;
        self.update(id, |job| job.validation = Some(report)).await;
        self.finish(id, path).await;
        Ok(())
    }

    async fn fetch_streams(
        &self,
        id: DownloadId,
        plan: &JobPlan,
        resume: &mut JobResume,
        cancel: &CancellationToken,
        headers: &Headers,
        concurrency: usize,
    ) -> Result<()> {
        let format = &plan.format;
        // YouTube's resolved client identity wins over the browser's identity.
        // Other sites keep their existing caller-first header behavior.
        let mut headers = headers.clone();
        for (name, value) in &format.headers {
            if youtube::is_youtube_url(&plan.webpage_url) {
                headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
                headers.push((name.clone(), value.clone()));
            } else if !headers.iter().any(|(existing, _)| existing.eq_ignore_ascii_case(name)) {
                headers.push((name.clone(), value.clone()));
            }
        }
        // Internal session metadata is consumed by analysis, never the CDN.
        headers.retain(|(name, _)| !name.to_ascii_lowercase().starts_with("x-youtube-"));
        let filename = &plan.filename;
        let stem = Path::new(filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| filename.clone());

        let video_url = format
            .video_url
            .as_ref()
            .ok_or_else(|| Error::InvalidInput("format has no video stream".into()))?;

        // Progress is reported against the whole job, so a merge job's audio
        // stream continues counting from where the video stream stopped rather
        // than restarting the bar at zero.
        //
        // The speed meter lives here rather than on `VideoJob` because it needs
        // `&mut` on every sample and the job is cloned out to every reader. Each
        // stream gets its own, which is right: throughput across the boundary
        // between a video stream and an audio stream is not one continuous
        // measurement.
        let make_progress_cb = |base_bytes: u64| {
            struct Meter {
                speed: SpeedMeter,
                last_push: std::time::Instant,
                last_bytes: u64,
            }
            let meter = std::sync::Arc::new(std::sync::Mutex::new(Meter {
                speed: SpeedMeter::default(),
                last_push: std::time::Instant::now(),
                last_bytes: 0,
            }));
            let manager = self.clone();
            move |tick: StreamTick| {
                let now = std::time::Instant::now();
                let (rate, downloaded, total) = {
                    let mut meter = meter.lock().unwrap();

                    // The callback reports a running total; the meter wants the
                    // increment since the last sample.
                    let delta = tick.bytes.saturating_sub(meter.last_bytes);
                    meter.last_bytes = tick.bytes;
                    meter.speed.record(delta, now);

                    // Sampling is continuous, publishing is throttled: dropping
                    // the samples in between would leave the meter measuring
                    // only every tenth chunk.
                    if now.duration_since(meter.last_push) < std::time::Duration::from_millis(100) {
                        return;
                    }
                    meter.last_push = now;

                    (
                        meter.speed.bytes_per_second(),
                        base_bytes + tick.bytes,
                        tick.total.map(|total| base_bytes + total),
                    )
                };

                let manager = manager.clone();
                tokio::spawn(async move {
                    manager.update_live(id, downloaded, total, rate).await;
                });
            }
        };

        if format.requires_merge {
            let audio_url = format
                .audio_url
                .as_ref()
                .ok_or_else(|| Error::InvalidInput("format needs a merge but has no audio stream".into()))?;

            // The deduped stem keeps two concurrent downloads of the same title
            // from sharing (and corrupting) the same temporary files.
            let video_tmp = plan.directory.join(format!("{stem}.video.tmp"));
            let audio_tmp = plan.directory.join(format!("{stem}.audio.tmp"));

            // A "done" flag is only as good as the bytes it points at. When the
            // file behind it is gone, fetching the stream again is the honest
            // repair; carrying the flag forward hands ffmpeg a path that does
            // not exist and reports a missing input instead of the real fault.
            let mut corrected = false;
            if resume.video_done && !file_has_bytes(&video_tmp).await {
                tracing::info!(%id, "the finished video stream is no longer on disk; fetching it again");
                resume.video_done = false;
                resume.video = StreamProgress::default();
                corrected = true;
            }
            if resume.audio_done && !file_has_bytes(&audio_tmp).await {
                tracing::info!(%id, "the finished audio stream is no longer on disk; fetching it again");
                resume.audio_done = false;
                resume.audio = StreamProgress::default();
                corrected = true;
            }
            if corrected {
                self.save_resume(id, resume).await;
            }

            if !resume.video_done {
                let cb = make_progress_cb(0);
                let mut options = self.stream_options(&resume.video, cancel, concurrency);
                options.live_seconds = format.download_options.as_ref().and_then(|o| o.live_seconds);
                let outcome =
                    super::stream::fetch_with_progress(&self.client, video_url, &headers, &video_tmp, Some(&cb), &options).await?;

                match outcome {
                    StreamOutcome::Paused(progress) => {
                        resume.video = progress;
                        return self.park(id, plan, resume).await;
                    }
                    StreamOutcome::Refused { progress, status, url } => {
                        resume.video = progress;
                        self.save_resume(id, resume).await;
                        return Err(Error::UnexpectedStatus { status, url });
                    }
                    StreamOutcome::Completed { .. } => {}
                }
                if outcome.bytes() == 0 {
                    let _ = tokio::fs::remove_file(&video_tmp).await;
                    return Err(Error::InvalidInput(
                        "Downloaded 0 bytes — video stream may have expired or requires login".into(),
                    ));
                }
                resume.video = StreamProgress::default();
                resume.video_done = true;
                self.update(id, |job| job.downloaded_bytes = outcome.bytes()).await;
                self.save_resume(id, resume).await;
            }

            let video_bytes = tokio::fs::metadata(&video_tmp).await.map(|m| m.len()).unwrap_or(0);

            if !resume.audio_done {
                let cb = make_progress_cb(video_bytes);
                let options = self.stream_options(&resume.audio, cancel, concurrency);
                let outcome =
                    super::stream::fetch_with_progress(&self.client, audio_url, &headers, &audio_tmp, Some(&cb), &options).await?;

                match outcome {
                    StreamOutcome::Paused(progress) => {
                        resume.audio = progress;
                        return self.park(id, plan, resume).await;
                    }
                    StreamOutcome::Refused { progress, status, url } => {
                        resume.audio = progress;
                        self.save_resume(id, resume).await;
                        return Err(Error::UnexpectedStatus { status, url });
                    }
                    StreamOutcome::Completed { .. } => {}
                }
                if outcome.bytes() == 0 {
                    let _ = tokio::fs::remove_file(&video_tmp).await;
                    let _ = tokio::fs::remove_file(&audio_tmp).await;
                    return Err(Error::InvalidInput(
                        "Downloaded 0 bytes — audio stream may have expired or requires login".into(),
                    ));
                }
                resume.audio = StreamProgress::default();
                resume.audio_done = true;
                self.update(id, |job| job.downloaded_bytes = video_bytes + outcome.bytes()).await;
                self.save_resume(id, resume).await;
            }

            self.update(id, |job| job.state = VideoJobState::Merging).await;

            let dest = plan.directory.join(filename);
            // Only a merge that worked retires the temporary streams. Deleting
            // them either way meant a second attempt found both resume flags
            // set and both inputs missing, so every retry reported a missing
            // file rather than whatever actually failed the first time.
            super::merge::mux(&video_tmp, &audio_tmp, &dest).await?;
            self.finish_checked(id, dest, plan, resume, cancel, &[video_tmp, audio_tmp]).await?;

            Ok(())
        } else if is_hls_source(video_url) {
            // A single muxed HLS rendition: concatenated MPEG-TS segments, which
            // we remux into a clean .mp4 so the result is universally playable
            // rather than a raw .ts. If ffmpeg is unavailable, fall back to
            // keeping the .ts rather than failing the whole download.
            let ts_tmp = plan.directory.join(format!("{stem}.ts.part"));

            let cb = make_progress_cb(0);
            let mut options = self.stream_options(&resume.video, cancel, concurrency);
                options.live_seconds = format.download_options.as_ref().and_then(|o| o.live_seconds);
            let outcome =
                super::stream::fetch_with_progress(&self.client, video_url, &headers, &ts_tmp, Some(&cb), &options).await?;

            match outcome {
                StreamOutcome::Paused(progress) => {
                    resume.video = progress;
                    return self.park(id, plan, resume).await;
                }
                StreamOutcome::Refused { progress, status, url } => {
                    resume.video = progress;
                    self.save_resume(id, resume).await;
                    return Err(Error::UnexpectedStatus { status, url });
                }
                StreamOutcome::Completed { .. } => {}
            }
            if outcome.bytes() == 0 {
                let _ = tokio::fs::remove_file(&ts_tmp).await;
                return Err(Error::InvalidInput(
                    "Downloaded 0 bytes — video URL may have expired or requires a logged-in browser session".into(),
                ));
            }
            self.update(id, |job| job.downloaded_bytes = outcome.bytes()).await;
            self.update(id, |job| job.state = VideoJobState::Merging).await;

            let dest = plan.directory.join(filename);
            match super::merge::remux(&ts_tmp, &dest).await {
                Ok(()) => {
                    self.finish_checked(id, dest, plan, resume, cancel, &[ts_tmp]).await?;
                }
                Err(e) => {
                    // No ffmpeg (or it choked): keep the raw stream as .ts so the
                    // user still gets a file that plays in VLC/most players.
                    // Derive the fallback path from the deduped stem so the .ts
                    // doesn't collide with an already-existing file of the same name.
                    tracing::warn!(error = %e, "HLS remux failed; keeping raw .ts");
                    let ts_dest = plan.directory.join(format!("{stem}.ts"));
                    tokio::fs::rename(&ts_tmp, &ts_dest).await.map_err(|source| Error::Io { path: ts_dest.clone(), source })?;
                    self.finish_checked(id, ts_dest, plan, resume, cancel, &[ts_tmp]).await?;
                }
            }
            Ok(())
        } else {
            let dest = plan.directory.join(filename);
            let cb = make_progress_cb(0);
            let mut options = self.stream_options(&resume.video, cancel, concurrency);
                options.live_seconds = format.download_options.as_ref().and_then(|o| o.live_seconds);
            let outcome =
                super::stream::fetch_with_progress(&self.client, video_url, &headers, &dest, Some(&cb), &options).await?;

            match outcome {
                StreamOutcome::Paused(progress) => {
                    resume.video = progress;
                    return self.park(id, plan, resume).await;
                }
                StreamOutcome::Refused { progress, status, url } => {
                    resume.video = progress;
                    self.save_resume(id, resume).await;
                    return Err(Error::UnexpectedStatus { status, url });
                }
                StreamOutcome::Completed { .. } => {}
            }
            if outcome.bytes() == 0 {
                let _ = tokio::fs::remove_file(&dest).await;
                return Err(Error::InvalidInput(
                    "Downloaded 0 bytes — video URL may have expired or requires a logged-in browser session".into(),
                ));
            }
            self.update(id, |job| job.downloaded_bytes = outcome.bytes()).await;
            self.finish_checked(id, dest, plan, resume, cancel, &[]).await?;
            Ok(())
        }
    }

    /// Record a clean stop: the state on disk, and the state in the row.
    async fn park(&self, id: DownloadId, _plan: &JobPlan, resume: &JobResume) -> Result<()> {
        self.save_resume(id, resume).await;
        self.clear_rate(id).await;
        let on_disk = resume.video.bytes + resume.audio.bytes;
        self.update(id, |job| {
            job.state = VideoJobState::Paused;
            job.error = None;
            // Report what is actually on disk, not what the live counter reached
            // — in-flight segments were dropped when the job stopped.
            if on_disk > 0 {
                job.downloaded_bytes = on_disk;
            }
        })
        .await;
        Ok(())
    }

    pub async fn update_downloaded_bytes(&self, id: DownloadId, downloaded_bytes: u64) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.downloaded_bytes = downloaded_bytes;
        }
    }

    /// Publish a live progress sample.
    ///
    /// Deliberately does not touch SQLite. This runs several times a second per
    /// job; persisting each sample would turn a progress bar into a write
    /// storm, and none of it is worth keeping — a reloaded job is not running,
    /// so its last known speed is meaningless. Durable position lives in
    /// `resume_state`, which is written at segment boundaries instead.
    async fn update_live(&self, id: DownloadId, downloaded: u64, total: Option<u64>, rate: u64) {
        let mut jobs = self.jobs.write().await;
        let Some(job) = jobs.get_mut(&id) else { return };

        job.downloaded_bytes = downloaded;
        // Only ever replace a total with a better one. An estimate that
        // vanished for a tick would make the bar fall back to indeterminate
        // and then reappear, which reads as a glitch.
        if total.is_some() {
            job.total_bytes = total;
        }
        job.bytes_per_second = rate;
        job.eta_seconds = match (job.total_bytes, rate) {
            (Some(total), rate) if rate > 0 && total > downloaded => Some((total - downloaded) / rate),
            _ => None,
        };
    }

    /// Clear the live readings for a job that is no longer moving.
    ///
    /// A paused job showing the speed it had when it stopped is worse than
    /// showing nothing: the number looks live and never changes.
    async fn clear_rate(&self, id: DownloadId) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.bytes_per_second = 0;
            job.eta_seconds = None;
        }
    }

    async fn finish(&self, id: DownloadId, path: PathBuf) {
        self.clear_rate(id).await;
        self.update(id, |job| {
            job.state = VideoJobState::Finished;
            job.path = Some(path.clone());
            job.error = None;
            // A finished file's size is known exactly; stop showing an estimate.
            job.total_bytes = Some(job.downloaded_bytes);
        })
        .await;
        // finish_checked already cleared positions and retained verification.
        if let Some(finished) = &self.on_finished {
            finished(id, path);
        }
    }

    async fn update(&self, id: DownloadId, f: impl FnOnce(&mut VideoJob)) {
        let job = {
            let mut jobs = self.jobs.write().await;
            jobs.get_mut(&id).map(|job| {
                f(job);
                job.clone()
            })
        };
        if let Some(job) = job {
            if let Err(e) = self.persist(&job).await {
                tracing::warn!(error = %e, "failed to persist video job");
            }
        }
    }

    async fn save_resume(&self, id: DownloadId, resume: &JobResume) {
        let json = match serde_json::to_string(resume) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(error = %e, "failed to encode video resume state");
                return;
            }
        };
        if let Err(e) = sqlx::query("UPDATE video_jobs SET resume_state = ? WHERE id = ?")
            .bind(json)
            .bind(id.to_string())
            .execute(&self.pool)
            .await
        {
            tracing::warn!(error = %e, "failed to persist video resume state");
        }
    }

    /// Rebuild a job's plan and position from its row.
    async fn load_plan(&self, id: DownloadId) -> Result<(JobPlan, JobResume)> {
        let row = sqlx::query(
            "SELECT title, webpage_url, format, headers, directory, filename, resume_state \
             FROM video_jobs WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound(id))?;

        let format_json: Option<String> = row.try_get("format")?;
        let directory: Option<String> = row.try_get("directory")?;
        let filename: Option<String> = row.try_get("filename")?;

        // A job created before migration 0006 has none of this, and there is no
        // way to reconstruct it — the format came from a stateless analysis that
        // is long gone. Say so plainly rather than resuming into a panic.
        let (Some(format_json), Some(directory), Some(filename)) = (format_json, directory, filename) else {
            return Err(Error::InvalidInput(
                "this job predates resume support — start it again to make it resumable".into(),
            ));
        };

        let format: VideoFormat =
            serde_json::from_str(&format_json).map_err(|e| Error::Corrupt(format!("stored video format: {e}")))?;

        let resume: JobResume = row
            .try_get::<Option<String>, _>("resume_state")?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        Ok((
            JobPlan {
                title: row.try_get("title")?,
                webpage_url: row.try_get("webpage_url")?,
                format,
                directory: PathBuf::from(directory),
                headers: headers::from_json(row.try_get("headers")?),
                filename,
            },
            resume,
        ))
    }

    /// Insert a brand-new job together with everything a later resume needs.
    async fn persist_new(&self, job: &VideoJob, plan: &JobPlan) -> Result<()> {
        sqlx::query(
            "INSERT INTO video_jobs \
               (id, title, webpage_url, state, downloaded_bytes, error, path, created_at, \
                format, headers, directory, filename, resume_state) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(job.id.to_string())
        .bind(&job.title)
        .bind(&job.webpage_url)
        .bind(job.state.as_str())
        .bind(job.downloaded_bytes as i64)
        .bind(&job.error)
        .bind(job.path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(job.created_at.to_rfc3339())
        .bind(serde_json::to_string(&plan.format).ok())
        .bind(headers::to_json(&plan.headers))
        .bind(plan.directory.to_string_lossy().to_string())
        .bind(&plan.filename)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the mutable half of a row. The plan columns are written once, at
    /// insert, and never change — re-deriving the filename mid-job is exactly
    /// the bug `start`'s comment warns about.
    async fn persist(&self, job: &VideoJob) -> Result<()> {
        sqlx::query(
            "INSERT INTO video_jobs (id, title, webpage_url, state, downloaded_bytes, error, path, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
               state = excluded.state, downloaded_bytes = excluded.downloaded_bytes, \
               error = excluded.error, path = excluded.path",
        )
        .bind(job.id.to_string())
        .bind(&job.title)
        .bind(&job.webpage_url)
        .bind(job.state.as_str())
        .bind(job.downloaded_bytes as i64)
        .bind(&job.error)
        .bind(job.path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(job.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browser_gallery_cache_is_bounded_expires_and_clears_with_sessions() {
        let queue = crate::Queue::open_in_memory().await.unwrap();
        let manager = VideoManager::new(Client::new(), queue.pool());
        let docs = |id: u32| vec![serde_json::json!({"post_id":id.to_string(), "attachments":[{"all_subattachments":{"count":1,"nodes":[{
            "__typename":"Photo", "id":"123", "image":{"uri":"https://scontent.fbcdn.net/photo.jpg"}
        }]}}]})];
        for id in 1..=17 { manager.store_facebook_post(&format!("https://facebook.com/posts/{id}"), &docs(id)).await.unwrap(); }
        assert_eq!(manager.facebook_posts.read().await.len(), 16);
        assert!(!manager.facebook_posts.read().await.contains_key("1"));
        manager.facebook_posts.write().await.get_mut("2").unwrap().0 -= std::time::Duration::from_secs(301);
        manager.store_facebook_post("https://facebook.com/posts/18", &docs(18)).await.unwrap();
        assert!(!manager.facebook_posts.read().await.contains_key("2"));
        manager.clear_shared_sessions().await.unwrap();
        assert!(manager.facebook_posts.read().await.is_empty());
        assert!(manager.store_facebook_post("https://facebook.com/posts/18", &vec![serde_json::json!({}); 33]).await.is_err());
    }

    #[test]
    fn a_refusal_is_explained_against_the_session_that_earned_it() {
        // Enough of a cookie for `cookie_signs_in`: the login marker plus one
        // of the API session ids.
        let signed_in = ("Cookie".to_string(), "LOGIN_INFO=x; SAPISID=y".to_string());
        let pot = (youtube::POTOKEN_HEADER.to_string(), "tok".to_string());

        let anonymous = stream_refused(403, 23 << 20, &Headers::new());
        assert!(anonymous.contains("23 MiB"), "{anonymous}");
        assert!(anonymous.contains("not signed in") && anonymous.contains("HLS"), "{anonymous}");
        assert!(!anonymous.contains("preview"), "{anonymous}");

        // The case that sent the user to sign in when they already had: the
        // advice has to move on to the token instead.
        let no_token = stream_refused(403, 23 << 20, &vec![signed_in.clone()]);
        assert!(no_token.contains("no GVS proof-of-origin token"), "{no_token}");
        assert!(!no_token.contains("Sign in"), "already signed in: {no_token}");

        // Nothing left to suggest, so it says so rather than inventing a step.
        let both = stream_refused(403, 23 << 20, &vec![signed_in, pot]);
        assert!(both.contains("bound to another video"), "{both}");
        assert!(!both.contains("Sign in"), "{both}");
    }
}
