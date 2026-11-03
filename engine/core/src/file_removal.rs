//! Delete only the stored output file; never recurse into a directory.
use std::path::Path;
use crate::{Error, Result};

pub(crate) async fn remove_output(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io { path: path.to_owned(), source }),
    }
}
