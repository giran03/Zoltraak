//! Zoltraak download engine.
//!
//! The engine is UI-agnostic: it is driven by [`Engine`] and reports progress
//! over a broadcast channel. The Tauri app, the REST API, and the CLI are all
//! thin clients over this crate.

pub mod category;
pub mod conversion;
pub mod conversion_preview;
pub mod conversion_watch;
pub mod chunk;
pub mod discord;
pub mod downloader;
mod file_removal;
pub mod file_status;
pub mod error;
pub mod headers;
pub mod image;
mod instagram;
pub mod media_tools;
mod migrations;
pub mod model;
pub mod music;
pub mod probe;
pub mod queue;
pub mod remux;
pub mod resume;
pub mod retry;
pub mod runner;
pub mod session;
pub mod settings;
pub mod shell;
pub mod speed;
pub mod supervisor;
pub mod vault;
pub mod video;

pub use category::{Category, CategoryKind, NewCategory};
pub use discord::{Delivery, DeliveryState, DiscordService, DiscordStatus};
pub use downloader::{Downloader, Options, Outcome, Request, progress_channel};
pub use headers::Headers;
pub use image::{ImageAlbum, ImageItem, MediaKind};
pub use music::manager::{Enqueued as MusicEnqueued, MusicJob, MusicJobState, MusicManager, NewMusicDownload};
pub use music::{AudioFormat, AudioSource, FolderMode, MusicSettings, MusicSource, MusicTrack, OutputOptions, Service as MusicService, SourceKind as MusicSourceKind};
pub use queue::{NewDownload, Queue, QueueItem};
pub use remux::manager::{NewRemux, RemuxJob, RemuxJobState, RemuxManager};
pub use remux::{Container, MediaInfo, Plan as RemuxPlan, StreamAction};
pub use runner::{Runner, RunnerConfig, RunSummary};
pub use session::{SessionSummary, SessionVault, SiteSession};
pub use settings::{CompletionAction, DiscordMode, DiscordSettings, EngineSettings, TimeWindow};
pub use speed::SharedRateLimit;
pub use supervisor::{DownloadSnapshot, Supervisor};
pub use vault::SecretVault;
pub use video::manager::{NewVideoDownload, VideoJob, VideoJobState, VideoManager};
pub use video::stream::{SegmentOptions, StreamOptions};
pub use video::{VideoFormat, VideoMetadata};
pub use error::{Error, Result};
pub use model::{Download, DownloadId, DownloadState, Progress};

/// Connection count chosen for a file of `size` bytes, before user overrides.
///
/// Thresholds mirror the tiers in `PLAN.md`. Servers that do not advertise
/// range support are always forced to a single connection by the caller.
pub fn default_connections(size: Option<u64>) -> u8 {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    match size {
        // An unknown length means no `Content-Length`, so the file cannot be
        // split into ranges at all.
        None => 1,
        Some(n) if n < 20 * MB => 1,
        Some(n) if n < 100 * MB => 4,
        Some(n) if n < GB => 8,
        Some(n) if n < 20 * GB => 16,
        Some(_) => 32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    #[test]
    fn unknown_size_uses_single_connection() {
        assert_eq!(default_connections(None), 1);
    }

    #[test]
    fn connection_tiers_match_plan() {
        assert_eq!(default_connections(Some(5 * MB)), 1);
        assert_eq!(default_connections(Some(50 * MB)), 4);
        assert_eq!(default_connections(Some(500 * MB)), 8);
        assert_eq!(default_connections(Some(5 * GB)), 16);
        assert_eq!(default_connections(Some(50 * GB)), 32);
    }

    #[test]
    fn tier_boundaries_are_inclusive_downward() {
        assert_eq!(default_connections(Some(20 * MB)), 4);
        assert_eq!(default_connections(Some(100 * MB)), 8);
        assert_eq!(default_connections(Some(GB)), 16);
        assert_eq!(default_connections(Some(20 * GB)), 32);
    }
}
