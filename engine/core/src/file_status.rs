//! File availability checks shared by the native UI and the download managers.
use std::path::Path;
use crate::{Error, Result};

pub async fn require_file(path: &Path) -> Result<()> {
    let meta = tokio::fs::metadata(path).await.map_err(|source| Error::Io { path: path.into(), source })?;
    if !meta.is_file() { return Err(Error::InvalidInput("Choose a file, not a folder.".into())); }
    Ok(())
}

// Permission failures and disconnected destinations must not authorize a restart.
pub async fn require_missing(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        let meta = tokio::fs::metadata(parent).await.map_err(|source| Error::Io { path: parent.into(), source })?;
        if !meta.is_dir() { return Err(Error::InvalidInput("The destination folder is unavailable.".into())); }
    }
    match tokio::fs::metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io { path: path.into(), source }),
        Ok(_) => Err(Error::InvalidInput("The file is still on disk. Locate it or choose a new download instead.".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn missing_and_regular_files_are_distinguished_from_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("track.mp3");
        assert!(require_missing(&file).await.is_ok());
        assert!(require_file(&file).await.is_err());
        tokio::fs::write(&file, b"fixture").await.unwrap();
        assert!(require_file(&file).await.is_ok());
        assert!(require_missing(&file).await.is_err());
        assert!(require_file(dir.path()).await.is_err());
        assert!(require_missing(dir.path()).await.is_err());
        assert!(require_missing(&dir.path().join("disconnected-drive/file.mp3")).await.is_err());
    }
}
