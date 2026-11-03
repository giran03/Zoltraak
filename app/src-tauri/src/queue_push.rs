//! The one queue emitter for the window. It sends only the rows that moved,
//! re-reads the whole queue only when the supervisor reports a change (or on
//! a slow resync), and does nothing at all while the window is hidden.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use zoltraak_core::{DownloadSnapshot, DownloadState, Supervisor};

/// Progress cadence while something is downloading.
const PROGRESS_EVERY: Duration = Duration::from_millis(400);
/// A full re-read regardless of revisions. It covers rows changed outside the
/// supervisor (direct SQL, another process) and doubles as the keepalive the
/// window's heartbeat waits for before it polls on its own.
const RESYNC_EVERY: Duration = Duration::from_secs(10);
/// How often a hidden window is checked for having been shown again.
const HIDDEN_CHECK: Duration = Duration::from_secs(1);
/// Lets a burst of changes (twenty pasted links) land as one push.
const SETTLE: Duration = Duration::from_millis(60);

/// What the window applies to its copy of the queue. `count` lets it notice
/// it has drifted and fetch a full snapshot instead.
#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDelta {
    pub upserts: Vec<DownloadSnapshot>,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    pub count: usize,
}

impl QueueDelta {
    fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.removed.is_empty() && self.order.is_none()
    }
}

/// The emitter's copy of what the window was last told.
#[derive(Default)]
struct Published {
    rows: HashMap<String, DownloadSnapshot>,
    order: Vec<String>,
}

impl Published {
    /// Replace everything with a full read and describe the difference.
    fn apply_full(&mut self, fresh: Vec<DownloadSnapshot>) -> QueueDelta {
        let order: Vec<String> = fresh.iter().map(|row| row.id.clone()).collect();
        let mut rows = HashMap::with_capacity(fresh.len());
        let mut upserts = Vec::new();
        for row in fresh {
            if self.rows.get(&row.id) != Some(&row) {
                upserts.push(row.clone());
            }
            rows.insert(row.id.clone(), row);
        }
        let removed = self.order.iter().filter(|id| !rows.contains_key(*id)).cloned().collect();
        let order_changed = order != self.order;
        self.rows = rows;
        self.order = order;
        QueueDelta { upserts, removed, order: order_changed.then(|| self.order.clone()), count: self.order.len() }
    }

    /// Fold in fresh rows for the downloads in flight. Returns `None` when a
    /// row is unknown, so the caller can fall back to a full read rather than
    /// guess where it belongs.
    fn apply_active(&mut self, fresh: Vec<DownloadSnapshot>) -> Option<QueueDelta> {
        let mut upserts = Vec::new();
        for row in fresh {
            let known = self.rows.get_mut(&row.id)?;
            if *known != row {
                *known = row.clone();
                upserts.push(row);
            }
        }
        Some(QueueDelta { upserts, count: self.order.len(), ..QueueDelta::default() })
    }

    fn downloading(&self) -> bool {
        self.rows.values().any(|row| row.state == DownloadState::Downloading)
    }
}

fn window_shown(handle: &AppHandle) -> bool {
    match handle.get_webview_window("main") {
        Some(window) => window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false),
        None => false,
    }
}

pub async fn run(handle: AppHandle, sup: Supervisor, event: &'static str) {
    let mut published = Published::default();
    let mut revisions = sup.revisions();
    let mut seen_revision: Option<u64> = None;
    let mut last_full: Option<Instant> = None;

    loop {
        if !window_shown(&handle) {
            // Nothing is rendered, so nothing is read. Showing the window
            // forces a full read, because the queue may have changed meanwhile.
            last_full = None;
            tokio::time::sleep(HIDDEN_CHECK).await;
            continue;
        }

        let revision = *revisions.borrow_and_update();
        let full_due = seen_revision != Some(revision) || last_full.is_none_or(|at| at.elapsed() >= RESYNC_EVERY);
        let delta = if full_due {
            match sup.snapshot().await {
                Ok(fresh) => {
                    seen_revision = Some(revision);
                    last_full = Some(Instant::now());
                    Some(published.apply_full(fresh))
                }
                Err(e) => {
                    tracing::debug!(error = %e, "queue push failed");
                    None
                }
            }
        } else {
            match sup.active_snapshots().await {
                Ok(fresh) => published.apply_active(fresh).or_else(|| {
                    last_full = None;
                    None
                }),
                Err(e) => {
                    tracing::debug!(error = %e, "queue progress push failed");
                    None
                }
            }
        };

        // A full read is sent even when nothing moved: it is the keepalive.
        if let Some(delta) = delta {
            if full_due || !delta.is_empty() {
                let _ = handle.emit(event, &delta);
            }
        }

        let wait = if published.downloading() {
            PROGRESS_EVERY
        } else {
            last_full.map_or(Duration::ZERO, |at| RESYNC_EVERY.saturating_sub(at.elapsed()))
        };
        tokio::select! {
            changed = revisions.changed() => {
                if changed.is_err() {
                    // The supervisor is gone; fall back to the timer alone.
                    tokio::time::sleep(wait).await;
                } else {
                    tokio::time::sleep(SETTLE).await;
                }
            }
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, state: DownloadState, downloaded: u64) -> DownloadSnapshot {
        DownloadSnapshot {
            id: id.into(),
            url: format!("https://example.test/{id}"),
            filename: format!("{id}.bin"),
            directory: "C:/Downloads".into(),
            state,
            total_bytes: Some(100),
            downloaded_bytes: downloaded,
            bytes_per_second: 0,
            eta_seconds: None,
            connections: 1,
            priority: 0,
            error: None,
            created_at: "2026-11-03T00:00:00Z".into(),
        }
    }

    #[test]
    fn first_full_read_sends_every_row_and_the_order() {
        let mut published = Published::default();
        let delta = published.apply_full(vec![row("a", DownloadState::Finished, 100), row("b", DownloadState::Waiting, 0)]);
        assert_eq!(delta.upserts.len(), 2);
        assert_eq!(delta.order.as_deref(), Some(&["a".to_string(), "b".to_string()][..]));
        assert_eq!(delta.count, 2);
    }

    #[test]
    fn unchanged_full_read_is_an_empty_keepalive() {
        let mut published = Published::default();
        published.apply_full(vec![row("a", DownloadState::Finished, 100)]);
        let delta = published.apply_full(vec![row("a", DownloadState::Finished, 100)]);
        assert!(delta.is_empty());
        assert_eq!(delta.count, 1);
    }

    #[test]
    fn full_read_reports_removals_and_reorders() {
        let mut published = Published::default();
        published.apply_full(vec![row("a", DownloadState::Finished, 100), row("b", DownloadState::Waiting, 0), row("c", DownloadState::Waiting, 0)]);
        let delta = published.apply_full(vec![row("c", DownloadState::Waiting, 0), row("a", DownloadState::Finished, 100)]);
        assert!(delta.upserts.is_empty());
        assert_eq!(delta.removed, vec!["b".to_string()]);
        assert_eq!(delta.order.as_deref(), Some(&["c".to_string(), "a".to_string()][..]));
    }

    #[test]
    fn progress_tick_sends_only_the_rows_that_moved() {
        let mut published = Published::default();
        published.apply_full(vec![row("a", DownloadState::Finished, 100), row("b", DownloadState::Downloading, 10)]);
        let delta = published.apply_active(vec![row("b", DownloadState::Downloading, 40)]).expect("known row");
        assert_eq!(delta.upserts, vec![row("b", DownloadState::Downloading, 40)]);
        assert!(delta.order.is_none() && delta.removed.is_empty());
        assert!(published.downloading());
    }

    #[test]
    fn progress_tick_for_an_unknown_row_asks_for_a_full_read() {
        let mut published = Published::default();
        published.apply_full(vec![row("a", DownloadState::Finished, 100)]);
        assert!(published.apply_active(vec![row("z", DownloadState::Downloading, 1)]).is_none());
    }
}
