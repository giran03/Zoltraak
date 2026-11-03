//! The segmented download engine.
//!
//! Each chunk is fetched by its own ranged request and written straight to its
//! offset in a preallocated file, so there is no merge pass. Workers that
//! finish early steal half the remaining work from the busiest peer.
//!
//! A lightweight *dynamic scaler* runs alongside the workers. Every
//! [`Options::scaler_interval`] it checks whether there is still work left in
//! large enough chunks to justify another connection. If the active-worker count
//! is below the configured maximum it calls [`chunk::steal_from`] and spawns a
//! new worker for the stolen range — reproducing the IDM behaviour of adding
//! connections mid-download rather than waiting for one to finish.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::RANGE;
use reqwest::{Client, StatusCode};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::chunk::{self, Chunk};
use crate::error::{Error, Result};
use crate::headers::{self, Headers};
use crate::model::{DownloadId, DownloadState, Progress};
use crate::probe::{self, Capabilities};
use crate::resume::ResumeState;
use crate::retry::RetryPolicy;
use crate::speed::{RateLimit, SharedRateLimit, SpeedMeter};
use crate::{default_connections, model};

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// `None` uses the size-based tier from [`default_connections`].
    pub connections: Option<u8>,
    pub retry: RetryPolicy,
    /// A chunk with less than this left is not worth splitting: the extra
    /// request costs a round trip that outweighs the parallelism gained.
    pub min_steal_bytes: u64,
    /// How often resume state is flushed to the sidecar.
    pub persist_every: Duration,
    /// How often the dynamic scaler checks whether another connection should
    /// be added. Set to `Duration::MAX` to disable proactive scaling entirely
    /// (work-stealing on completion still fires).
    pub scaler_interval: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connections: None,
            retry: RetryPolicy::default(),
            min_steal_bytes: 1024 * 1024,
            persist_every: Duration::from_millis(500),
            scaler_interval: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub url: String,
    pub directory: PathBuf,
    /// Overrides the name derived from `Content-Disposition` or the URL.
    pub filename: Option<String>,
    /// Sent with every request this download makes. Sanitize client-supplied
    /// pairs through [`crate::headers::sanitize`] before putting them here.
    pub headers: Headers,
}

impl Request {
    pub fn new(url: impl Into<String>, directory: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            directory: directory.into(),
            filename: None,
            headers: Headers::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Completed {
        path: PathBuf,
        bytes: u64,
    },
    /// Cancelled cleanly. Resume state is on disk; re-running the same request
    /// continues from here.
    Paused {
        path: PathBuf,
        downloaded: u64,
        total: Option<u64>,
    },
}

/// Minimum gap between progress snapshots. Chunks arrive many times a second
/// per connection; publishing each one only makes the UI redraw at network
/// frequency, and the final snapshot is always sent when the transfer ends.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

struct Shared {
    chunks: Vec<Chunk>,
    pending_slots: Vec<usize>,
    meter: SpeedMeter,
    last_persist: Instant,
    /// When a progress snapshot last went out; see [`PROGRESS_INTERVAL`].
    last_emit: Instant,
    /// Number of worker tasks currently fetching byte ranges. Incremented just
    /// before a worker task is spawned, decremented when it returns (success,
    /// cancel, or error). The dynamic scaler reads this to decide whether
    /// spawning another connection is still worthwhile.
    active_workers: Arc<AtomicU8>,
}

impl Shared {
    fn downloaded(&self) -> u64 {
        self.chunks.iter().map(|c| c.downloaded).sum()
    }
}

pub struct Downloader {
    client: Client,
    options: Options,
    /// Shared across every worker of every concurrent download this instance
    /// runs, so it is a genuine process-wide cap rather than per-download —
    /// matching what a "bandwidth limit" setting means to a user.
    ///
    /// An async mutex, held across the throttling sleep itself rather than
    /// released before it. Concurrent workers computing a delay independently
    /// and then sleeping in parallel would let their waits overlap instead of
    /// compound, letting N connections add up to N times the configured cap.
    /// Holding the lock through the sleep serializes consumption, so the next
    /// waiter's own delay is computed against real elapsed time.
    limiter: SharedRateLimit,
}

/// Progress channel seeded with an empty snapshot for `id`.
pub fn progress_channel(id: DownloadId) -> (watch::Sender<Progress>, watch::Receiver<Progress>) {
    watch::channel(Progress {
        id,
        downloaded_bytes: 0,
        total_bytes: None,
        bytes_per_second: 0,
        state: DownloadState::Waiting,
        active_connections: 0,
    })
}

impl Downloader {
    pub fn new(client: Client, options: Options) -> Self {
        Self {
            client,
            options,
            limiter: Arc::new(tokio::sync::Mutex::new(RateLimit::new(
                None,
                Instant::now(),
            ))),
        }
    }

    pub fn with_defaults() -> Result<Self> {
        let client = Client::builder()
            // Redirects are normal for download links (CDN hops, signed URLs).
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self::new(client, Options::default()))
    }

    /// The bandwidth cap this instance enforces, for handing to a subsystem
    /// that fetches bytes outside the segmented queue — `VideoManager`, in
    /// particular. Sharing the handle is what makes "bandwidth limit" one
    /// number for the whole app rather than one per download path.
    pub fn rate_limiter(&self) -> SharedRateLimit {
        Arc::clone(&self.limiter)
    }

    /// Change the global bandwidth cap live. `None` lifts it. Applies to every
    /// download already in flight, not just ones started afterward.
    pub async fn set_bandwidth_limit(&self, bytes_per_second: Option<u64>) {
        self.limiter
            .lock()
            .await
            .set_limit(bytes_per_second, Instant::now());
    }

    pub async fn download(
        &self,
        request: &Request,
        cancel: CancellationToken,
        progress: &watch::Sender<Progress>,
    ) -> Result<Outcome> {
        self.download_with_resolved_path(request, cancel, progress, |_| async { Ok(()) })
            .await
    }

    /// Persist the actual filename before opening the output, so removal and
    /// retries can find it even when Content-Disposition changes the name.
    pub(crate) async fn download_with_resolved_path<F, Fut>(
        &self,
        request: &Request,
        cancel: CancellationToken,
        progress: &watch::Sender<Progress>,
        on_path: F,
    ) -> Result<Outcome>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let id = progress.borrow().id;
        let mut attempt = 0;
        let caps = loop {
            let result = tokio::select! {
                _ = cancel.cancelled() => break Capabilities::default(),
                result = probe::probe(&self.client, &request.url, &request.headers) => result,
            };
            match result {
                Ok(caps) => break caps,
                Err(e) if e.is_retryable() => {
                    attempt += 1;
                    let Some(delay) = self.options.retry.delay_for(attempt) else {
                        return Err(e);
                    };
                    tokio::select! {
                        _ = cancel.cancelled() => {},
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
                Err(e) => return Err(e),
            }
        };
        let path = self.resolve_path(request, &caps)?;
        on_path(path.clone()).await?;

        if !caps.supports_segmentation() {
            tracing::info!(
                url = %request.url,
                length = ?caps.content_length,
                ranges = caps.accepts_ranges,
                "server does not support segmentation; using a single stream"
            );
            return self
                .stream_whole(request, &path, id, cancel, progress)
                .await;
        }

        let total = caps
            .content_length
            .expect("supports_segmentation() guarantees a length");
        let connections = self
            .options
            .connections
            .unwrap_or_else(|| default_connections(Some(total)))
            .max(1);

        let (chunks, resumed) = self.prepare(request, &path, total, &caps, connections)?;

        if chunks.iter().all(Chunk::is_complete) {
            // The sidecar says everything already landed; nothing to do.
            ResumeState::discard(&path);
            return Ok(Outcome::Completed { path, bytes: total });
        }

        tracing::info!(
            url = %request.url,
            %total,
            connections,
            resumed,
            chunks = chunks.len(),
            "starting segmented download"
        );

        let active_workers = Arc::new(AtomicU8::new(0));
        let shared = Arc::new(Mutex::new(Shared {
            pending_slots: chunks
                .iter()
                .enumerate()
                .filter_map(|(slot, c)| (!c.is_complete()).then_some(slot))
                .collect(),
            chunks,
            meter: SpeedMeter::default(),
            last_persist: Instant::now(),
            last_emit: Instant::now(),
            active_workers: Arc::clone(&active_workers),
        }));

        if let Err(e) = self
            .run_workers(request, &path, total, &caps, &shared, &cancel, progress, id)
            .await
        {
            if matches!(
                e,
                Error::RangeUnsupported | Error::InvalidRange | Error::StaleResume
            ) && !cancel.is_cancelled()
            {
                tracing::warn!(error = %e, "restarting download as a single stream");
                ResumeState::discard(&path);
                return self
                    .stream_whole(request, &path, id, cancel, progress)
                    .await;
            }
            let chunks = shared.lock().expect("engine mutex poisoned").chunks.clone();
            if let Err(save_error) = (ResumeState {
                url: request.url.clone(),
                total,
                validator: caps.validator.clone(),
                chunks,
            })
            .save(&path)
            {
                tracing::warn!(error = %save_error, "failed to save partial download after error");
            }
            return Err(e);
        }

        self.finish(request, path, total, &caps, &shared, &cancel, progress, id)
    }

    fn resolve_path(&self, request: &Request, caps: &Capabilities) -> Result<PathBuf> {
        let name = probe::resolve_filename(request.filename.as_deref(), &request.url, caps);

        std::fs::create_dir_all(&request.directory).map_err(|source| Error::Io {
            path: request.directory.clone(),
            source,
        })?;

        Ok(request.directory.join(name))
    }

    /// Decide between resuming and starting fresh, and make sure the target
    /// file exists at full size so workers can write at arbitrary offsets.
    fn prepare(
        &self,
        request: &Request,
        path: &Path,
        total: u64,
        caps: &Capabilities,
        connections: u8,
    ) -> Result<(Vec<Chunk>, bool)> {
        let saved = ResumeState::load(path)
            .filter(|s| s.can_resume(&request.url, total, caps.validator.as_deref()));

        // A sidecar that no longer matches the remote file is the dangerous
        // case: resuming would splice two different files together.
        if let Some(state) = saved {
            if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == total) {
                return Ok((state.chunks, true));
            }
            // Sidecar without the partial file: nothing to resume into.
            ResumeState::discard(path);
        } else if ResumeState::load(path).is_some() {
            tracing::warn!(
                path = %path.display(),
                "remote file changed since the last attempt; restarting from zero"
            );
            ResumeState::discard(path);
        }

        let file = std::fs::File::create(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        // Preallocate so every worker's offset is already valid.
        file.set_len(total).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;

        Ok((chunk::plan(total, connections), false))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_workers(
        &self,
        request: &Request,
        path: &Path,
        total: u64,
        caps: &Capabilities,
        shared: &Arc<Mutex<Shared>>,
        cancel: &CancellationToken,
        progress: &watch::Sender<Progress>,
        id: DownloadId,
    ) -> Result<()> {
        let max_connections = self
            .options
            .connections
            .unwrap_or_else(|| default_connections(Some(total)))
            .max(1);
        let active_workers = shared
            .lock()
            .expect("engine mutex poisoned")
            .active_workers
            .clone();
        // Worker failure cancels this attempt, never the caller's token: a
        // range refusal can then safely fall back to an ordinary GET.
        let workers_cancel = cancel.child_token();
        let mut set = tokio::task::JoinSet::new();
        let spawn = |set: &mut tokio::task::JoinSet<Result<()>>, slot| {
            let ctx = WorkerCtx {
                slot,
                shared: Arc::clone(shared),
                client: self.client.clone(),
                url: request.url.clone(),
                headers: request.headers.clone(),
                limiter: Arc::clone(&self.limiter),
                path: path.to_path_buf(),
                cancel: workers_cancel.clone(),
                progress: progress.clone(),
                id,
                total,
                validator: caps.validator.clone(),
                options: self.options,
            };
            active_workers.fetch_add(1, Ordering::Relaxed);
            let active = active_workers.clone();
            set.spawn(async move {
                let result = worker(ctx).await;
                active.fetch_sub(1, Ordering::Relaxed);
                result
            });
        };
        let slots: Vec<_> = {
            let mut guard = shared.lock().expect("engine mutex poisoned");
            (0..max_connections)
                .filter_map(|_| guard.pending_slots.pop())
                .collect()
        };
        for slot in slots {
            spawn(&mut set, slot);
        }

        // Every dynamically added worker belongs to this JoinSet. Completion,
        // errors and shutdown all wait for it before reopening the output.
        let scaling = self.options.scaler_interval < Duration::MAX;
        let interval = if scaling {
            self.options.scaler_interval.max(Duration::from_millis(1))
        } else {
            Duration::from_secs(86400)
        };
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut first_error = None;
        while !set.is_empty() {
            tokio::select! {
                joined = set.join_next() => {
                    let error = match joined {
                        Some(Ok(Err(e))) => Some(e),
                        Some(Err(e)) => Some(Error::Io {
                            path: path.to_path_buf(), source: std::io::Error::other(e.to_string()),
                        }),
                        _ => None,
                    };
                    if let Some(e) = error {
                        if first_error.is_none() {
                            tracing::warn!(error = %e, "worker failed; cancelling siblings");
                            first_error = Some(e);
                            workers_cancel.cancel();
                        }
                    }
                }
                _ = ticker.tick(), if scaling && !workers_cancel.is_cancelled() => {
                    if active_workers.load(Ordering::Relaxed) < max_connections {
                        let slot = {
                            let mut guard = shared.lock().expect("engine mutex poisoned");
                            guard.pending_slots.pop().or_else(|| chunk::steal_from(&mut guard.chunks, self.options.min_steal_bytes))
                        };
                        if let Some(slot) = slot { spawn(&mut set, slot); }
                    }
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        request: &Request,
        path: PathBuf,
        total: u64,
        caps: &Capabilities,
        shared: &Arc<Mutex<Shared>>,
        cancel: &CancellationToken,
        progress: &watch::Sender<Progress>,
        id: DownloadId,
    ) -> Result<Outcome> {
        let guard = shared.lock().expect("engine mutex poisoned");
        let downloaded = guard.downloaded();
        let complete = guard.chunks.iter().all(Chunk::is_complete);

        let state = ResumeState {
            url: request.url.clone(),
            total,
            validator: caps.validator.clone(),
            chunks: guard.chunks.clone(),
        };
        drop(guard);

        if complete {
            ResumeState::discard(&path);
            let _ = progress.send(Progress {
                id,
                downloaded_bytes: downloaded,
                total_bytes: Some(total),
                bytes_per_second: 0,
                state: DownloadState::Finished,
                active_connections: 0,
            });
            return Ok(Outcome::Completed {
                path,
                bytes: downloaded,
            });
        }

        // Incomplete without an error means cancellation; keep the state so the
        // next run resumes instead of restarting.
        state.save(&path)?;
        let _ = progress.send(Progress {
            id,
            downloaded_bytes: downloaded,
            total_bytes: Some(total),
            bytes_per_second: 0,
            state: DownloadState::Paused,
            active_connections: 0,
        });

        debug_assert!(cancel.is_cancelled(), "incomplete download without cancel");
        Ok(Outcome::Paused {
            path,
            downloaded,
            total: Some(total),
        })
    }

    /// Single-stream fallback for servers with no range support or no length.
    async fn stream_whole(
        &self,
        request: &Request,
        path: &Path,
        id: DownloadId,
        cancel: CancellationToken,
        progress: &watch::Sender<Progress>,
    ) -> Result<Outcome> {
        let mut attempt = 0;
        loop {
            if cancel.is_cancelled() {
                return Ok(Outcome::Paused {
                    path: path.to_path_buf(),
                    downloaded: 0,
                    total: None,
                });
            }
            let result = self
                .stream_whole_attempt(request, path, id, &cancel, progress)
                .await;
            match result {
                Err(e) if e.is_retryable() => {
                    attempt += 1;
                    let Some(delay) = self.options.retry.delay_for(attempt) else {
                        return Err(e);
                    };
                    tokio::select! {
                        _ = cancel.cancelled() => {},
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
                result => return result,
            }
        }
    }

    async fn stream_whole_attempt(
        &self,
        request: &Request,
        path: &Path,
        id: DownloadId,
        cancel: &CancellationToken,
        progress: &watch::Sender<Progress>,
    ) -> Result<Outcome> {
        let response = tokio::select! {
            _ = cancel.cancelled() => return Ok(Outcome::Paused {
                path: path.to_path_buf(), downloaded: 0, total: None,
            }),
            response = headers::send(headers::apply(self.client.get(&request.url), &request.headers)
                .header(reqwest::header::ACCEPT_ENCODING, "identity")) => response?,
        };
        let status = response.status();
        if status != StatusCode::OK {
            return Err(Error::UnexpectedStatus {
                status: status.as_u16(),
                url: request.url.clone(),
            });
        }
        let total = response.content_length();

        // Never leave a segmented checkpoint attached to a truncated stream.
        ResumeState::discard(path);

        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;

        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;
        let mut meter = SpeedMeter::default();
        let mut last_emit = Instant::now();

        loop {
            let item = tokio::select! {
                _ = cancel.cancelled() => break,
                item = tokio::time::timeout(Duration::from_secs(60), stream.next()) =>
                    item.map_err(|_| Error::NetworkTimeout)?,
            };
            let Some(item) = item else {
                break;
            };
            let bytes = item?;

            // See the comment in `fetch_range`: the lock must stay held across
            // the sleep, not just across the delay computation.
            {
                let mut limiter = tokio::select! {
                    _ = cancel.cancelled() => break,
                    limiter = self.limiter.lock() => limiter,
                };
                let delay = limiter.delay_for(bytes.len() as u64, Instant::now());
                if delay > Duration::ZERO {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
            }

            file.write_all(&bytes).await.map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;

            downloaded += bytes.len() as u64;
            meter.record(bytes.len() as u64, Instant::now());
            if last_emit.elapsed() < PROGRESS_INTERVAL {
                continue;
            }
            last_emit = Instant::now();
            let _ = progress.send(Progress {
                id,
                downloaded_bytes: downloaded,
                total_bytes: total,
                bytes_per_second: meter.bytes_per_second(),
                state: DownloadState::Downloading,
                active_connections: 1, // single-stream fallback
            });
        }

        file.flush().await.map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;

        if cancel.is_cancelled() {
            let _ = progress.send(Progress {
                id,
                downloaded_bytes: downloaded,
                total_bytes: total,
                bytes_per_second: 0,
                state: DownloadState::Paused,
                active_connections: 0,
            });
            return Ok(Outcome::Paused {
                path: path.to_path_buf(),
                downloaded,
                total,
            });
        }
        if total.is_some_and(|total| downloaded != total) {
            return Err(Error::IncompleteResponse);
        }

        let _ = progress.send(Progress {
            id,
            downloaded_bytes: downloaded,
            total_bytes: total,
            bytes_per_second: 0,
            state: DownloadState::Finished,
            active_connections: 0,
        });

        Ok(Outcome::Completed {
            path: path.to_path_buf(),
            bytes: downloaded,
        })
    }
}

struct WorkerCtx {
    slot: usize,
    shared: Arc<Mutex<Shared>>,
    client: Client,
    url: String,
    headers: Headers,
    limiter: Arc<tokio::sync::Mutex<RateLimit>>,
    path: PathBuf,
    cancel: CancellationToken,
    progress: watch::Sender<Progress>,
    id: DownloadId,
    total: u64,
    validator: Option<String>,
    options: Options,
}

async fn worker(mut ctx: WorkerCtx) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&ctx.path)
        .await
        .map_err(|source| Error::Io {
            path: ctx.path.clone(),
            source,
        })?;

    let mut attempt = 0u32;

    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }

        let (cursor, end) = {
            let guard = ctx.shared.lock().expect("engine mutex poisoned");
            let c = guard.chunks[ctx.slot];
            (c.cursor(), c.end)
        };

        if cursor >= end {
            // Take a pending saved range, then split remaining work if useful.
            let stolen = {
                let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
                guard
                    .pending_slots
                    .pop()
                    .or_else(|| chunk::steal_from(&mut guard.chunks, ctx.options.min_steal_bytes))
            };
            match stolen {
                Some(next) => {
                    ctx.slot = next;
                    continue;
                }
                None => return Ok(()),
            }
        }

        match fetch_range(&mut ctx, &mut file, cursor, end).await {
            Ok(()) => attempt = 0,
            Err(e) if e.is_retryable() => {
                attempt += 1;
                match ctx.options.retry.delay_for(attempt) {
                    Some(delay) => {
                        tracing::warn!(
                            error = %e,
                            attempt,
                            delay_secs = delay.as_secs(),
                            "range failed; retrying"
                        );
                        tokio::select! {
                            _ = ctx.cancel.cancelled() => return Ok(()),
                            _ = tokio::time::sleep(delay) => {},
                        }
                    }
                    None => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
}

async fn fetch_range(
    ctx: &mut WorkerCtx,
    file: &mut tokio::fs::File,
    cursor: u64,
    end: u64,
) -> Result<()> {
    // HTTP ranges are inclusive on both ends; chunks are half-open.
    // `RANGE` is set after the client's headers precisely because `sanitize`
    // refuses to pass one through: nothing a caller sends can displace it.
    let mut request = headers::apply(ctx.client.get(&ctx.url), &ctx.headers)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(RANGE, format!("bytes={}-{}", cursor, end - 1));
    if let Some(validator) = ctx.validator.as_deref().filter(|v| !v.starts_with("W/")) {
        request = request.header(reqwest::header::IF_RANGE, validator);
    }
    let response = tokio::select! {
        _ = ctx.cancel.cancelled() => return Ok(()),
        response = headers::send(request) => response?,
    };

    let status = response.status();
    if status == StatusCode::OK {
        // Never write a full response at a chunk offset. Join all writers,
        // then restart with an ordinary GET instead.
        return Err(Error::RangeUnsupported);
    }
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        return Err(Error::InvalidRange);
    }
    if status != StatusCode::PARTIAL_CONTENT {
        return Err(Error::UnexpectedStatus {
            status: status.as_u16(),
            url: ctx.url.clone(),
        });
    }

    let range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(probe::parse_content_range);
    if !matches!(range, Some((start, last, Some(total)))
        if start == cursor && last == end - 1 && total == ctx.total)
    {
        return Err(Error::InvalidRange);
    }

    // The file may have changed between the probe and this request.
    if let Some(expected) = &ctx.validator {
        let fresh = response
            .headers()
            .get(reqwest::header::ETAG)
            .or_else(|| response.headers().get(reqwest::header::LAST_MODIFIED))
            .and_then(|v| v.to_str().ok());
        if let Some(fresh) = fresh {
            if fresh != expected {
                return Err(Error::StaleResume);
            }
        }
    }

    file.seek(SeekFrom::Start(cursor))
        .await
        .map_err(|source| Error::Io {
            path: ctx.path.clone(),
            source,
        })?;

    // Writes advance the file's own cursor from the seek above, and the claim
    // below is computed from shared state, so no local position is needed.
    let mut stream = response.bytes_stream();

    loop {
        let item = tokio::select! {
            _ = ctx.cancel.cancelled() => break,
            item = tokio::time::timeout(Duration::from_secs(60), stream.next()) =>
                item.map_err(|_| Error::NetworkTimeout)?,
        };
        let Some(item) = item else {
            break;
        };
        let bytes = item?;

        // Reserve the region before writing, under the same lock that reads
        // `end`. A peer may steal the tail of this chunk while the request is
        // in flight; claiming atomically is what stops both workers from
        // writing the same bytes and double-counting them.
        let claim = {
            let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
            let c = &mut guard.chunks[ctx.slot];
            let available = c.end.saturating_sub(c.cursor());
            let n = available.min(bytes.len() as u64);
            c.claimed += n;
            n
        };

        if claim == 0 {
            // The rest of this chunk was stolen; the remainder of the response
            // belongs to another worker now.
            break;
        }

        // Throttled after the bytes are already off the wire, not before —
        // delaying the write (and so the next `stream.next()`) is what slows
        // the loop down; there is no way to un-receive a TCP payload that
        // already arrived.
        //
        // The lock is held across the sleep itself: computing a delay and
        // releasing the lock before sleeping would let concurrent workers
        // each compute their delay against the same near-zero elapsed time
        // and then sleep in parallel, so their waits would overlap instead
        // of compound — N connections would add up to N times the cap
        // instead of sharing it. Serializing the sleep is what makes the
        // next waiter's own delay reflect real elapsed time.
        {
            let mut limiter = tokio::select! {
                _ = ctx.cancel.cancelled() => {
                    ctx.shared.lock().expect("engine mutex poisoned").chunks[ctx.slot].claimed -= claim;
                    break;
                },
                limiter = ctx.limiter.lock() => limiter,
            };
            let delay = limiter.delay_for(claim, Instant::now());
            if delay > Duration::ZERO {
                tokio::select! {
                    _ = ctx.cancel.cancelled() => {},
                    _ = tokio::time::sleep(delay) => {},
                }
            }
        }

        if let Err(source) = file.write_all(&bytes[..claim as usize]).await {
            // Release the reservation so the bytes are re-fetched rather than
            // silently skipped.
            let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
            guard.chunks[ctx.slot].claimed -= claim;
            return Err(Error::Io {
                path: ctx.path.clone(),
                source,
            });
        }

        let snapshot = {
            let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
            let c = &mut guard.chunks[ctx.slot];
            // Promote the reservation to written now that it is on disk.
            c.claimed -= claim;
            c.downloaded += claim;
            guard.meter.record(claim, Instant::now());

            let downloaded = guard.downloaded();
            let rate = guard.meter.bytes_per_second();
            // Read active_connections before the optional guard drop below, since
            // the guard may be released early when the persist timer fires.
            let active_connections = guard.active_workers.load(Ordering::Relaxed);
            let emit = guard.last_emit.elapsed() >= PROGRESS_INTERVAL;
            if emit {
                guard.last_emit = Instant::now();
            }

            // Flush resume state on a timer, not per data chunk — the sidecar
            // write is synchronous and would otherwise dominate throughput.
            if guard.last_persist.elapsed() >= ctx.options.persist_every {
                guard.last_persist = Instant::now();
                let state = ResumeState {
                    url: ctx.url.clone(),
                    total: ctx.total,
                    validator: ctx.validator.clone(),
                    chunks: guard.chunks.clone(),
                };
                drop(guard);
                // A failed checkpoint costs resume accuracy, not correctness.
                if let Err(e) = state.save(&ctx.path) {
                    tracing::warn!(error = %e, "failed to checkpoint resume state");
                }
            }

            emit.then_some(Progress {
                id: ctx.id,
                downloaded_bytes: downloaded,
                total_bytes: Some(ctx.total),
                bytes_per_second: rate,
                state: DownloadState::Downloading,
                active_connections,
            })
        };

        if let Some(snapshot) = snapshot {
            let _ = ctx.progress.send(snapshot);
        }

        if claim < bytes.len() as u64 {
            // Hit the shrunken end; the rest of this response is not ours.
            break;
        }
    }

    file.flush().await.map_err(|source| Error::Io {
        path: ctx.path.clone(),
        source,
    })?;

    if !ctx.cancel.is_cancelled()
        && !ctx.shared.lock().expect("engine mutex poisoned").chunks[ctx.slot].is_complete()
    {
        return Err(Error::IncompleteResponse);
    }

    Ok(())
}

// Keep the model import meaningful for downstream users of this module.
pub use model::Progress as ProgressSnapshot;
