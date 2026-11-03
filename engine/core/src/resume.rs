//! Resume state, persisted beside the partial file.
//!
//! M0 keeps this in a JSON sidecar rather than SQLite so the engine is usable
//! (and testable) without a database. M1 moves the queue into SQLite; the
//! sidecar stays as the crash-durable record of *byte ranges*, which is the
//! part that must survive a hard kill.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chunk::Chunk;
use crate::error::{Error, Result};

/// Appended to the destination filename to form the sidecar path.
const SIDECAR_SUFFIX: &str = ".zoltraak";
const LEGACY_SIDECAR_SUFFIX: &str = ".opendm";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeState {
    pub url: String,
    pub total: u64,
    /// `ETag`, else `Last-Modified`. `None` when the server offered neither.
    pub validator: Option<String>,
    pub chunks: Vec<Chunk>,
}

impl ResumeState {
    pub fn sidecar_path(destination: &Path) -> PathBuf {
        let mut name = destination.as_os_str().to_os_string();
        name.push(SIDECAR_SUFFIX);
        PathBuf::from(name)
    }

    pub fn downloaded(&self) -> u64 {
        self.chunks.iter().map(|c| c.downloaded).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.chunks.iter().all(Chunk::is_complete)
    }

    /// Whether saved state may be reused for `url` against a freshly probed
    /// server response.
    ///
    /// A server that offers no validator at all is treated as *not* resumable.
    /// Silently continuing into a file that may have changed is the one
    /// failure mode that corrupts data without any error surfacing.
    pub fn can_resume(&self, url: &str, total: u64, validator: Option<&str>) -> bool {
        if self.url != url || self.total != total {
            return false;
        }
        // A valid JSON sidecar can still describe gaps, overlap or offsets
        // outside the file. Validate it before handing it to worker arithmetic.
        let mut chunks = self.chunks.clone();
        chunks.sort_unstable_by_key(|c| c.start);
        let mut end = 0;
        for chunk in chunks {
            if chunk.start != end
                || chunk.end <= chunk.start
                || chunk.end > total
                || chunk.downloaded > chunk.end - chunk.start
            {
                return false;
            }
            end = chunk.end;
        }
        if end != total {
            return false;
        }
        match (&self.validator, validator) {
            (Some(saved), Some(fresh)) => !fresh.starts_with("W/") && saved == fresh,
            _ => false,
        }
    }

    pub fn load(destination: &Path) -> Option<Self> {
        let path = Self::sidecar_path(destination);
        let bytes = std::fs::read(&path)
            .or_else(|_| {
                let mut legacy = destination.as_os_str().to_os_string();
                legacy.push(LEGACY_SIDECAR_SUFFIX);
                std::fs::read(PathBuf::from(legacy))
            })
            .ok()?;
        // A corrupt sidecar must not be fatal — it just means starting over,
        // which is always safe.
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, destination: &Path) -> Result<()> {
        let path = Self::sidecar_path(destination);
        let json = serde_json::to_vec(self).expect("ResumeState is always serializable");

        // Write to a temp file and rename, so a crash mid-write cannot leave a
        // truncated sidecar that would strand a large partial download.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })
    }

    pub fn discard(destination: &Path) {
        // Best-effort: a leftover sidecar is harmless, and the download has
        // already succeeded by the time this runs.
        let _ = std::fs::remove_file(Self::sidecar_path(destination));
        let mut legacy = destination.as_os_str().to_os_string();
        legacy.push(LEGACY_SIDECAR_SUFFIX);
        let _ = std::fs::remove_file(PathBuf::from(legacy));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ResumeState {
        ResumeState {
            url: "https://example.test/f.bin".into(),
            total: 1000,
            validator: Some("\"abc\"".into()),
            chunks: vec![Chunk::new(0, 500), Chunk::new(500, 1000)],
        }
    }

    #[test]
    fn sidecar_sits_next_to_the_target_and_keeps_the_extension() {
        let p = ResumeState::sidecar_path(Path::new("/tmp/movie.mkv"));
        assert_eq!(p, PathBuf::from("/tmp/movie.mkv.zoltraak"));
    }

    #[test]
    fn resume_requires_matching_url_size_and_validator() {
        let s = state();
        assert!(s.can_resume("https://example.test/f.bin", 1000, Some("\"abc\"")));
        assert!(!s.can_resume("https://example.test/other.bin", 1000, Some("\"abc\"")));
        assert!(!s.can_resume("https://example.test/f.bin", 999, Some("\"abc\"")));
        assert!(!s.can_resume("https://example.test/f.bin", 1000, Some("\"xyz\"")));
    }

    #[test]
    fn missing_validator_blocks_resume() {
        // Resuming without a validator risks splicing two different files
        // together, which produces a corrupt result with no error at all.
        let s = state();
        assert!(!s.can_resume("https://example.test/f.bin", 1000, None));

        let unvalidated = ResumeState {
            validator: None,
            ..state()
        };
        assert!(!unvalidated.can_resume("https://example.test/f.bin", 1000, Some("\"abc\"")));
    }

    #[test]
    fn invalid_chunk_layouts_and_weak_validators_cannot_resume() {
        for chunks in [
            vec![],
            vec![Chunk::new(0, 999)],
            vec![Chunk::new(1, 1000)],
            vec![Chunk::new(0, 600), Chunk::new(500, 1000)],
            vec![Chunk {
                start: 0,
                end: 1000,
                downloaded: 1001,
                claimed: 0,
            }],
            vec![Chunk {
                start: u64::MAX,
                end: 0,
                downloaded: 1,
                claimed: 0,
            }],
        ] {
            let s = ResumeState { chunks, ..state() };
            assert!(!s.can_resume(&s.url, s.total, s.validator.as_deref()));
        }
        let s = ResumeState {
            validator: Some("W/\"weak\"".into()),
            ..state()
        };
        assert!(!s.can_resume(&s.url, s.total, s.validator.as_deref()));
        let mut s = state();
        s.chunks.reverse();
        assert!(s.can_resume(&s.url, s.total, s.validator.as_deref()));
    }

    #[test]
    fn downloaded_sums_across_chunks() {
        let mut s = state();
        s.chunks[0].downloaded = 200;
        s.chunks[1].downloaded = 50;
        assert_eq!(s.downloaded(), 250);
        assert!(!s.is_complete());

        s.chunks[0].downloaded = 500;
        s.chunks[1].downloaded = 500;
        assert!(s.is_complete());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");

        let mut s = state();
        s.chunks[0].downloaded = 123;
        s.save(&dest).unwrap();

        let loaded = ResumeState::load(&dest).expect("sidecar was just written");
        assert_eq!(loaded.downloaded(), 123);
        assert_eq!(loaded.url, s.url);
        assert_eq!(loaded.chunks.len(), 2);
    }

    #[test]
    fn corrupt_sidecar_is_ignored_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");
        std::fs::write(ResumeState::sidecar_path(&dest), b"{ not json").unwrap();

        assert!(ResumeState::load(&dest).is_none());
    }

    #[test]
    fn missing_sidecar_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ResumeState::load(&dir.path().join("absent.bin")).is_none());
    }

    #[test]
    fn discard_removes_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");
        state().save(&dest).unwrap();
        assert!(ResumeState::sidecar_path(&dest).exists());

        ResumeState::discard(&dest);
        assert!(!ResumeState::sidecar_path(&dest).exists());
    }
}
