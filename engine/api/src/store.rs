//! Where the per-install API token lives between runs.
//!
//! M2's `serve` generated a fresh token on every launch, which is fine for a
//! one-off `curl` and useless for a browser extension: the user would have to
//! re-pair after every restart, and would quickly learn to paste whatever
//! token a prompt asked for. A stable secret is what makes the pairing step
//! meaningful.
//!
//! The app and the CLI resolve the *same* path deliberately. Only one process
//! can hold port 1450, but which one it is should not change what the
//! extension has to be told.

use std::io;
use std::path::{Path, PathBuf};

use crate::auth::ApiToken;

/// `<config>/Zoltraak/api-token`.
///
/// Resolved from the environment rather than a crate so the headless binary
/// keeps its "no extra dependencies" property, and so a container can point
/// the whole thing somewhere else with one variable.
pub fn default_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ZOLTRAAK_CONFIG_DIR").or_else(|| std::env::var_os("OPENDM_CONFIG_DIR")) {
        return Some(PathBuf::from(dir).join("api-token"));
    }

    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };

    Some(base?.join("Zoltraak").join("api-token"))
}

/// Non-secret preferred browser-profile metadata, beside the API token so the
/// desktop app and headless server agree on the same cookie source.
pub fn extension_source_path() -> Option<PathBuf> {
    default_path().and_then(|path| path.parent().map(|dir| dir.join("extension-source.json")))
}

/// Read the token at `path`, generating and persisting one if it is absent.
///
/// A file that exists but is empty or whitespace is treated as absent: a
/// half-written token can never authenticate anything, so failing the launch
/// over it would strand the user with no way back except finding the file.
pub fn load_or_create(path: &Path) -> io::Result<ApiToken> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(ApiToken::from_existing(trimmed));
        }
        tracing::warn!(path = %path.display(), "empty token file; generating a new token");
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let token = ApiToken::generate();
    write_private(path, token.expose())?;
    tracing::info!(path = %path.display(), "generated a new API token");
    Ok(token)
}

/// Write with an owner-only mode where the OS has one.
#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // 0600 is set at create time, not with a chmod afterwards: between the two
    // there would be a window where the token is world-readable.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

/// Windows has no mode bits to set here. The file lands under the user's
/// roaming profile, which is already ACL'd to that user; any process running
/// as them can read it, but such a process could equally read the browser's
/// own cookie store, so this is not the weak link.
#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_created_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("api-token");

        let first = load_or_create(&path).unwrap();
        assert!(path.exists(), "the token was not persisted");

        let second = load_or_create(&path).unwrap();
        assert_eq!(
            first.expose(),
            second.expose(),
            "a restart must not invalidate the extension's pairing"
        );
    }

    #[test]
    fn an_empty_file_is_replaced_rather_than_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token");
        std::fs::write(&path, "   \n").unwrap();

        let token = load_or_create(&path).unwrap();
        assert_eq!(token.expose().len(), 64);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token.expose());
    }

    #[test]
    fn surrounding_whitespace_is_not_part_of_the_secret() {
        // Hand-edited files and shell redirections both leave a trailing
        // newline; the user should not have to notice.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token");
        std::fs::write(&path, "abc123\n").unwrap();

        assert!(load_or_create(&path).unwrap().verify("abc123"));
    }

    #[test]
    fn the_override_wins_over_the_platform_location() {
        // SAFETY: single-threaded test process; no other thread reads the env.
        unsafe { std::env::set_var("ZOLTRAAK_CONFIG_DIR", "/somewhere/else") };
        let path = default_path().unwrap();
        unsafe { std::env::remove_var("ZOLTRAAK_CONFIG_DIR") };

        assert_eq!(path, PathBuf::from("/somewhere/else").join("api-token"));
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_not_readable_by_other_users() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token");
        load_or_create(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "group/other can read the token: {mode:o}");
    }
}
