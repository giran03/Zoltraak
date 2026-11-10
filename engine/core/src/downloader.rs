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

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
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
    /// A ranged connection that delivers nothing for this long is dropped and
    /// its range re-requested. Its peers prove the network is still up, so a
    /// fresh connection — often a different route or CDN edge — beats waiting.
    pub stall_timeout: Duration,
    /// How long a connection must run before it can be judged slow against its
    /// peers (see [`SLOW_PEER_RATIO`]). Also the minimum gap between two such
    /// judgements of the same range, so a reconnect gets a fair chance.
    pub slow_connection_grace: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connections: None,
            retry: RetryPolicy::default(),
            min_steal_bytes: 1024 * 1024,
            persist_every: Duration::from_millis(500),
            scaler_interval: Duration::from_secs(2),
            stall_timeout: Duration::from_secs(15),
            slow_connection_grace: Duration::from_secs(20),
        }
    }
}

/// Fail a connection attempt after this long instead of the OS default
/// (about 21 s on Windows), so a dead route is retried on a new one quickly.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP keepalive probes keep idle pooled connections from being silently
/// dropped by NATs and home routers between ranges.
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);
/// A connection running below `1 / SLOW_PEER_RATIO` of its peers' median
/// speed is reconnected. Loose on purpose: links share bandwidth unevenly,
/// and only a connection stuck on a bad path is this far behind.
const SLOW_PEER_RATIO: u64 = 8;
/// Slow-peer judgements need a few peers to compare against.
const SLOW_PEER_MIN_PEERS: usize = 3;
/// The single-stream fallback has no peers to prove the network is alive and
/// a reconnect restarts it from zero, so it waits much longer before giving up.
const SINGLE_STREAM_STALL: Duration = Duration::from_secs(60);

/// The HTTP client every download should use.
///
/// HTTP/1.1 only, deliberately. Over HTTP/2 the connection pool keeps one
/// connection per host and multiplexes every request onto it, so N ranged
/// workers become N streams on a single TCP socket: one lost packet stalls all
/// of them, and servers that cap speed per connection see one connection.
/// Measured against an HTTP/2 CDN, 8 requested connections held 1 socket.
pub fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        // Redirects are normal for download links (CDN hops, signed URLs).
        .redirect(reqwest::redirect::Policy::limited(10))
        .http1_only()
        .connect_timeout(CONNECT_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE)
        .tcp_nodelay(true)
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
    /// Current speed of each connection that has been running for at least
    /// [`Options::slow_connection_grace`], keyed by worker id. What a
    /// connection's speed is compared against to decide it is stuck.
    worker_rates: HashMap<u32, u64>,
    /// Lowered when the server refuses an extra connection while others are
    /// working, so the scaler stops re-adding connections it will refuse.
    connection_cap: u8,
}

impl Shared {
    fn downloaded(&self) -> u64 {
        self.chunks.iter().map(|c| c.downloaded).sum()
    }

    /// Median speed of every judged connection except `worker`, when there are
    /// enough of them to compare against.
    fn peer_median(&self, worker: u32) -> Option<u64> {
        let mut peers: Vec<u64> = self
            .worker_rates
            .iter()
            .filter(|(id, _)| **id != worker)
            .map(|(_, rate)| *rate)
            .collect();
        if peers.len() < SLOW_PEER_MIN_PEERS {
            return None;
        }
        peers.sort_unstable();
        Some(peers[peers.len() / 2])
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
    /// Mirrors whether [`Self::limiter`] has a cap, so unthrottled transfers —
    /// the common case — skip the shared lock entirely instead of serializing
    /// every received buffer of every connection through it.
    limited: Arc<AtomicBool>,
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
            limited: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_defaults() -> Result<Self> {
        Ok(Self::new(client_builder().build()?, Options::default()))
    }

    /// The client downloads go through. Probing a link with it before queuing
    /// leaves a warm keep-alive connection the download then reuses, saving a
    /// TCP and TLS handshake.
    pub fn client(&self) -> &Client {
        &self.client
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
        let mut limiter = self.limiter.lock().await;
        limiter.set_limit(bytes_per_second, Instant::now());
        self.limited.store(!limiter.is_unlimited(), Ordering::Release);
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
            worker_rates: HashMap::new(),
            connection_cap: connections,
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
        let (persist, persister) = spawn_persister(path.to_path_buf());
        let next_worker = AtomicU32::new(0);
        let mut set = tokio::task::JoinSet::new();
        let spawn = |set: &mut tokio::task::JoinSet<Result<()>>, slot| {
            let ctx = WorkerCtx {
                worker: next_worker.fetch_add(1, Ordering::Relaxed),
                slot,
                shared: Arc::clone(shared),
                client: self.client.clone(),
                url: request.url.clone(),
                headers: request.headers.clone(),
                limiter: Arc::clone(&self.limiter),
                limited: Arc::clone(&self.limited),
                persist: persist.clone(),
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
        loop {
            if set.is_empty() {
                // Workers can retire while work remains (a server refusing an
                // extra connection hands its range back). If every one of them
                // did, start one more rather than mistaking that for the end.
                let slot = (first_error.is_none() && !workers_cancel.is_cancelled())
                    .then(|| shared.lock().expect("engine mutex poisoned").pending_slots.pop())
                    .flatten();
                match slot {
                    Some(slot) => spawn(&mut set, slot),
                    None => break,
                }
            }
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
                    let slot = {
                        let mut guard = shared.lock().expect("engine mutex poisoned");
                        let limit = max_connections.min(guard.connection_cap);
                        (active_workers.load(Ordering::Relaxed) < limit)
                            .then(|| guard.pending_slots.pop().or_else(|| chunk::steal_from(&mut guard.chunks, self.options.min_steal_bytes)))
                            .flatten()
                    };
                    if let Some(slot) = slot { spawn(&mut set, slot); }
                }
            }
        }
        // Every worker's sender is gone with its task; dropping this last one
        // lets the persister write the final checkpoint and stop. Waiting for
        // it keeps a late background write from racing the caller's own
        // save or discard of the sidecar.
        drop(persist);
        if let Err(e) = persister.await {
            tracing::warn!(error = %e, "resume checkpoint task failed");
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
                item = tokio::time::timeout(SINGLE_STREAM_STALL, stream.next()) =>
                    item.map_err(|_| Error::NetworkTimeout)?,
            };
            let Some(item) = item else {
                break;
            };
            let bytes = item?;

            // See the comment in `fetch_range`: the lock must stay held across
            // the sleep, not just across the delay computation.
            if self.limited.load(Ordering::Acquire) {
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

/// Writes resume checkpoints off the download threads.
///
/// Workers publish the latest state and move on; a slow disk then delays only
/// the checkpoint, never the next network read. A watch channel keeps just the
/// newest state, so a backlog can never write an older one over a newer one.
fn spawn_persister(
    path: PathBuf,
) -> (watch::Sender<Option<ResumeState>>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = watch::channel(None::<ResumeState>);
    let task = tokio::spawn(async move {
        // `changed` still reports a value sent just before the last sender
        // dropped, so the final checkpoint is always written.
        while rx.changed().await.is_ok() {
            let Some(state) = rx.borrow_and_update().clone() else {
                continue;
            };
            let path = path.clone();
            match tokio::task::spawn_blocking(move || state.save(&path)).await {
                Ok(Ok(())) => {}
                // A failed checkpoint costs resume accuracy, not correctness.
                Ok(Err(e)) => tracing::warn!(error = %e, "failed to checkpoint resume state"),
                Err(e) => tracing::warn!(error = %e, "resume checkpoint write panicked"),
            }
        }
    });
    (tx, task)
}

/// How often a running connection is compared against its peers.
const JUDGE_EVERY: Duration = Duration::from_secs(1);
/// Bounds of the quick retry used while peers are still receiving data.
const QUICK_RETRY_MIN_MS: u64 = 250;
const QUICK_RETRY_SPREAD_MS: u64 = 750;

/// What a worker does after a retryable range failure.
enum Recovery {
    Retry(Duration),
    /// The server refused this connection while others work: hand the range
    /// back and run with one connection fewer.
    Retire,
    GiveUp,
}

struct WorkerCtx {
    /// Unique within one download attempt; keys [`Shared::worker_rates`].
    worker: u32,
    slot: usize,
    shared: Arc<Mutex<Shared>>,
    client: Client,
    url: String,
    headers: Headers,
    limiter: Arc<tokio::sync::Mutex<RateLimit>>,
    limited: Arc<AtomicBool>,
    persist: watch::Sender<Option<ResumeState>>,
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
    // Download-wide bytes as of this worker's last failure: whether anything
    // arrived since is what tells a flaky connection from a dead network.
    let mut baseline = ctx.shared.lock().expect("engine mutex poisoned").downloaded();

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

        let result = fetch_range(&mut ctx, &mut file, cursor, end).await;
        // A fresh request earns a fresh grace period before it is judged.
        ctx.shared
            .lock()
            .expect("engine mutex poisoned")
            .worker_rates
            .remove(&ctx.worker);
        match result {
            Ok(()) => {
                attempt = 0;
                baseline = ctx.shared.lock().expect("engine mutex poisoned").downloaded();
            }
            Err(e) if e.is_retryable() => match recover(&ctx, &e, &mut attempt, &mut baseline) {
                Recovery::Retry(delay) => {
                    tracing::warn!(
                        error = %e,
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        "range failed; retrying"
                    );
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
                Recovery::Retire => {
                    tracing::info!(error = %e, "server refused an extra connection; continuing with fewer");
                    return Ok(());
                }
                Recovery::GiveUp => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
}

/// Decide how to recover from a retryable failure of this worker's range.
///
/// When other bytes have arrived since this worker last failed, the network
/// is up and only this connection is in trouble: retry it within a second,
/// and do not spend the retry budget. That budget, with its long backoff, is
/// kept for failures with no progress anywhere — a link that is actually down.
/// A server that answers "busy" while other connections work is asking for
/// fewer connections, so the worker gives its range back instead of retrying.
fn recover(ctx: &WorkerCtx, error: &Error, attempt: &mut u32, baseline: &mut u64) -> Recovery {
    let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
    let downloaded = guard.downloaded();
    let progressed = downloaded > *baseline;
    *baseline = downloaded;

    let refused = matches!(error, Error::UnexpectedStatus { .. });
    let active = guard.active_workers.load(Ordering::Relaxed);
    if progressed && refused && active > 1 {
        guard.pending_slots.push(ctx.slot);
        guard.connection_cap = guard.connection_cap.min(active - 1).max(1);
        return Recovery::Retire;
    }
    if progressed && !refused {
        *attempt = 1;
        return Recovery::Retry(quick_retry_delay(ctx, *attempt));
    }
    *attempt += 1;
    match ctx.options.retry.delay_for(*attempt) {
        Some(delay) => Recovery::Retry(delay),
        None => Recovery::GiveUp,
    }
}

/// A short delay, spread per worker so several connections that dropped
/// together do not reconnect in lockstep. Never longer than the policy's own
/// first backoff.
fn quick_retry_delay(ctx: &WorkerCtx, attempt: u32) -> Duration {
    let spread = (u64::from(ctx.worker) * 211 + u64::from(attempt) * 97) % QUICK_RETRY_SPREAD_MS;
    Duration::from_millis(QUICK_RETRY_MIN_MS + spread).min(ctx.options.retry.base)
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
    let started = Instant::now();
    let mut meter = SpeedMeter::default();
    let mut last_judged = started;
    let judge_every = ctx.options.slow_connection_grace.min(JUDGE_EVERY);

    loop {
        let item = tokio::select! {
            _ = ctx.cancel.cancelled() => break,
            item = tokio::time::timeout(ctx.options.stall_timeout, stream.next()) =>
                item.map_err(|_| {
                    tracing::warn!(stall_secs = ctx.options.stall_timeout.as_secs_f32(), "connection stalled; reconnecting");
                    Error::NetworkTimeout
                })?,
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
        if ctx.limited.load(Ordering::Acquire) {
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

        let now = Instant::now();
        meter.record(claim, now);
        let mut too_slow = false;
        let snapshot = {
            let mut guard = ctx.shared.lock().expect("engine mutex poisoned");
            let c = &mut guard.chunks[ctx.slot];
            // Promote the reservation to written now that it is on disk.
            c.claimed -= claim;
            c.downloaded += claim;
            let remaining = c.remaining();
            guard.meter.record(claim, now);

            if now.duration_since(started) >= ctx.options.slow_connection_grace {
                let rate = meter.bytes_per_second();
                guard.worker_rates.insert(ctx.worker, rate);
                if now.duration_since(last_judged) >= judge_every {
                    last_judged = now;
                    // A throttled transfer's speeds say nothing about paths,
                    // and a nearly finished range is not worth a reconnect.
                    too_slow = !ctx.limited.load(Ordering::Acquire)
                        && remaining >= ctx.options.min_steal_bytes
                        && guard
                            .peer_median(ctx.worker)
                            .is_some_and(|median| rate.saturating_mul(SLOW_PEER_RATIO) < median);
                }
            }

            let downloaded = guard.downloaded();
            let rate = guard.meter.bytes_per_second();
            // Read active_connections before the optional guard drop below, since
            // the guard may be released early when the persist timer fires.
            let active_connections = guard.active_workers.load(Ordering::Relaxed);
            let emit = guard.last_emit.elapsed() >= PROGRESS_INTERVAL;
            if emit {
                guard.last_emit = Instant::now();
            }

            // Checkpoint resume state on a timer, not per data chunk. The
            // write itself happens on the persister task (see
            // [`spawn_persister`]), never on this network loop.
            if guard.last_persist.elapsed() >= ctx.options.persist_every {
                guard.last_persist = Instant::now();
                let state = ResumeState {
                    url: ctx.url.clone(),
                    total: ctx.total,
                    validator: ctx.validator.clone(),
                    chunks: guard.chunks.clone(),
                };
                drop(guard);
                ctx.persist.send_replace(Some(state));
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

        if too_slow {
            // Treated like a stall: retried at once, since peers are healthy.
            tracing::warn!(
                bytes_per_second = meter.bytes_per_second(),
                "connection far slower than its peers; reconnecting"
            );
            return Err(Error::NetworkTimeout);
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
