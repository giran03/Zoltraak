//! Drives the queue: claims runnable items and downloads them with bounded
//! concurrency, mirroring progress back into the database.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::downloader::{Downloader, Outcome};
use crate::error::Result;
use crate::model::{DownloadState, Progress};
use crate::queue::{Queue, QueueItem};

#[derive(Debug, Clone, Copy)]
pub struct RunnerConfig {
    /// Downloads running at once. Each still opens its own connections, so
    /// this multiplies rather than replaces the per-download count.
    pub max_concurrent: usize,
    /// How often in-flight progress is flushed to SQLite. Per-buffer writes
    /// would make the database the bottleneck.
    pub progress_every: Duration,
    /// Longest wait before re-checking an idle queue. In-process changes wake
    /// the loop at once; this only bounds how late a change made by another
    /// process is noticed, so it can be slow.
    pub idle_poll: Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 3,
            progress_every: Duration::from_millis(500),
            idle_poll: Duration::from_secs(10),
        }
    }
}

pub struct Runner {
    queue: Queue,
    downloader: Arc<Downloader>,
    config: RunnerConfig,
}

impl Runner {
    pub fn new(queue: Queue, downloader: Downloader, config: RunnerConfig) -> Self {
        Self {
            queue,
            downloader: Arc::new(downloader),
            config,
        }
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Run until the queue is drained, then return.
    ///
    /// Cancelling stops accepting new work and pauses what is in flight; the
    /// sidecars and queue rows are left ready to resume.
    pub async fn run_until_empty(&self, cancel: CancellationToken) -> Result<RunSummary> {
        // Anything left `downloading` belongs to a process that is gone.
        let recovered = self.queue.recover_interrupted().await?;
        if recovered > 0 {
            tracing::info!(recovered, "requeued downloads interrupted by a previous run");
        }

        let mut summary = RunSummary::default();
        let mut set = tokio::task::JoinSet::new();

        loop {
            // Top up to the concurrency limit.
            while set.len() < self.config.max_concurrent && !cancel.is_cancelled() {
                match self.queue.claim_next().await? {
                    Some(item) => {
                        tracing::info!(id = %item.id, url = %item.url, "starting");
                        set.spawn(run_one(
                            item,
                            Arc::clone(&self.downloader),
                            self.queue.clone(),
                            cancel.clone(),
                            self.config,
                            None,
                            None,
                        ));
                    }
                    None => break,
                }
            }

            if set.is_empty() {
                // Nothing running and nothing claimable: we are done. Polling
                // further would spin forever on an idle queue.
                break;
            }

            match set.join_next().await {
                Some(Ok(result)) => summary.record(result),
                Some(Err(join_err)) => {
                    tracing::error!(error = %join_err, "download task panicked");
                    summary.failed += 1;
                }
                None => break,
            }

            if cancel.is_cancelled() && set.is_empty() {
                break;
            }
        }

        Ok(summary)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    pub completed: usize,
    pub paused: usize,
    pub failed: usize,
}

impl RunSummary {
    fn record(&mut self, outcome: ItemResult) {
        match outcome {
            ItemResult::Completed => self.completed += 1,
            ItemResult::Paused => self.paused += 1,
            ItemResult::Failed => self.failed += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.completed + self.paused + self.failed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemResult {
    Completed,
    Paused,
    Failed,
}

/// Called on every progress sample, unthrottled.
///
/// The database write is throttled because it is expensive; a live observer
/// (the desktop UI) wants every sample so its rate readout stays smooth.
pub(crate) type ProgressHook = Arc<dyn Fn(Progress) + Send + Sync>;

/// Called once, with the final path, when a download finishes successfully.
///
/// This is the only point at which a queued download's resolved path is known:
/// the queue row carries the name it *guessed*, and the downloader may have
/// picked another from `Content-Disposition`. Anything that wants to act on
/// the finished file — Discord delivery today — hangs off here.
///
/// Must not block: it runs inline on the worker that just finished a download.
pub type FinishedHook = Arc<dyn Fn(crate::model::DownloadId, std::path::PathBuf) + Send + Sync>;

pub(crate) async fn run_one(
    item: QueueItem,
    downloader: Arc<Downloader>,
    queue: Queue,
    cancel: CancellationToken,
    config: RunnerConfig,
    hook: Option<ProgressHook>,
    on_finished: Option<FinishedHook>,
) -> ItemResult {
    let (tx, rx) = crate::downloader::progress_channel(item.id);

    // Mirror progress into SQLite on a timer, independent of the download loop.
    let mirror = tokio::spawn(mirror_progress(
        rx,
        queue.clone(),
        item.id,
        config.progress_every,
        hook,
    ));

    let path_queue = queue.clone();
    let result = downloader.download_with_resolved_path(&item.request(), cancel, &tx, |path| async move {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            path_queue.set_filename(item.id, name).await?;
        }
        Ok(())
    }).await;
    mirror.abort();

    match result {
        Ok(Outcome::Completed { path, bytes }) => {
            // The engine may have resolved a different name than the queue guessed.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name != item.filename {
                    let _ = queue.set_filename(item.id, name).await;
                }
            }
            let _ = queue.update_progress(item.id, bytes, Some(bytes)).await;
            let _ = queue.set_state(item.id, DownloadState::Finished).await;
            tracing::info!(id = %item.id, path = %path.display(), "finished");
            if let Some(finished) = on_finished {
                finished(item.id, path);
            }
            ItemResult::Completed
        }
        Ok(Outcome::Paused {
            downloaded, total, ..
        }) => {
            let _ = queue.update_progress(item.id, downloaded, total).await;
            let _ = queue.set_state(item.id, DownloadState::Paused).await;
            tracing::info!(id = %item.id, downloaded, "paused");
            ItemResult::Paused
        }
        Err(e) => {
            tracing::error!(id = %item.id, error = %e, "failed");
            let _ = queue.fail(item.id, &e.to_string()).await;
            ItemResult::Failed
        }
    }
}

async fn mirror_progress(
    mut rx: watch::Receiver<Progress>,
    queue: Queue,
    id: crate::model::DownloadId,
    every: Duration,
    hook: Option<ProgressHook>,
) {
    let mut last = Instant::now();
    while rx.changed().await.is_ok() {
        let snapshot = *rx.borrow();

        // The observer runs first and on every sample: it is an in-memory map
        // write, so throttling it would only make the UI choppier for nothing.
        if let Some(hook) = &hook {
            hook(snapshot);
        }

        if last.elapsed() < every {
            continue;
        }
        last = Instant::now();

        if let Err(e) = queue
            .update_progress(id, snapshot.downloaded_bytes, snapshot.total_bytes)
            .await
        {
            // Losing a progress sample costs display accuracy, not data.
            tracing::debug!(error = %e, "progress mirror write failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_each_outcome() {
        let mut s = RunSummary::default();
        s.record(ItemResult::Completed);
        s.record(ItemResult::Completed);
        s.record(ItemResult::Paused);
        s.record(ItemResult::Failed);

        assert_eq!(s.completed, 2);
        assert_eq!(s.paused, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.total(), 4);
    }
}
