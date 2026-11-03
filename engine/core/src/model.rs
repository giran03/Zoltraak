use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DownloadId(pub uuid::Uuid);

impl DownloadId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DownloadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Keeps `uuid` an implementation detail: callers parse ids without taking a
/// direct dependency on the representation.
impl std::str::FromStr for DownloadId {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        uuid::Uuid::parse_str(s.trim())
            .map(Self)
            .map_err(|e| format!("invalid download id {s:?}: {e}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Waiting,
    Downloading,
    Paused,
    Finished,
    Failed,
    Scheduled,
}

impl DownloadState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Scheduled => "scheduled",
        }
    }

    /// Whether the engine should pick this item up on its own.
    ///
    /// Paused is deliberately excluded: a user who paused something does not
    /// expect a restart to silently resume it.
    pub fn is_runnable(&self) -> bool {
        matches!(self, Self::Waiting)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished)
    }
}

impl std::str::FromStr for DownloadState {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "waiting" => Ok(Self::Waiting),
            "downloading" => Ok(Self::Downloading),
            "paused" => Ok(Self::Paused),
            "finished" => Ok(Self::Finished),
            "failed" => Ok(Self::Failed),
            "scheduled" => Ok(Self::Scheduled),
            other => Err(format!("unknown download state: {other}")),
        }
    }
}

impl std::fmt::Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: DownloadId,
    pub url: String,
    pub filename: String,
    pub destination: PathBuf,
    pub state: DownloadState,
    /// `None` when the server sent no `Content-Length`.
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub connections: u8,
    /// `ETag` or `Last-Modified`, used to detect a changed remote file on resume.
    pub validator: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Progress {
    pub id: DownloadId,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: u64,
    pub state: DownloadState,
    /// Live count of worker goroutines actively fetching byte ranges right now.
    /// This differs from the *planned* connection count, which is fixed at enqueue
    /// time; this one grows as the dynamic scaler spawns extra workers and
    /// shrinks as they finish or are stolen away.
    pub active_connections: u8,
}

impl Progress {
    /// Completion ratio in `0.0..=1.0`, or `None` when the total size is unknown.
    pub fn fraction(&self) -> Option<f64> {
        match self.total_bytes {
            Some(0) | None => None,
            Some(total) => Some((self.downloaded_bytes as f64 / total as f64).clamp(0.0, 1.0)),
        }
    }

    /// Estimated seconds remaining, or `None` when the total size or rate is unknown.
    pub fn eta_secs(&self) -> Option<u64> {
        let total = self.total_bytes?;
        if self.bytes_per_second == 0 {
            return None;
        }
        let remaining = total.saturating_sub(self.downloaded_bytes);
        Some(remaining / self.bytes_per_second)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(downloaded: u64, total: Option<u64>, rate: u64) -> Progress {
        Progress {
            id: DownloadId::new(),
            downloaded_bytes: downloaded,
            total_bytes: total,
            bytes_per_second: rate,
            state: DownloadState::Downloading,
            active_connections: 0,
        }
    }

    #[test]
    fn fraction_is_none_without_a_known_total() {
        assert_eq!(progress(500, None, 100).fraction(), None);
        assert_eq!(progress(0, Some(0), 100).fraction(), None);
    }

    #[test]
    fn fraction_reports_ratio() {
        assert_eq!(progress(50, Some(200), 0).fraction(), Some(0.25));
    }

    #[test]
    fn eta_needs_both_total_and_rate() {
        assert_eq!(progress(50, Some(200), 0).eta_secs(), None);
        assert_eq!(progress(50, None, 10).eta_secs(), None);
        assert_eq!(progress(50, Some(200), 10).eta_secs(), Some(15));
    }
}
