//! OS-shell utilities: revealing a path in the desktop file manager and
//! running a user-provided command.
//!
//! These live in the engine, not a UI crate, so the same code backs both the
//! desktop app's "show in folder" action and the scheduler's on-completion
//! actions. On a truly headless install (a NAS with no desktop session) a
//! reveal call simply fails with an I/O error from the missing binary —
//! callers already treat that as best-effort, not fatal.

use std::io;
use std::path::{Path, PathBuf};

/// Walk up from `path` to the nearest ancestor that actually exists on disk.
///
/// A reveal target is often gone or not written yet — a download that hasn't
/// created its file, a history entry the user deleted from disk, a category
/// folder that was moved. Rather than hand the file manager a path it will
/// refuse (or, worse, create a stray directory named after the file), open the
/// closest place that does exist so the user still lands somewhere useful.
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut cursor = Some(path);
    while let Some(candidate) = cursor {
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        cursor = candidate.parent().filter(|p| !p.as_os_str().is_empty());
    }
    None
}

/// Open `path` in the user's file manager, selecting it when it is a file.
///
/// Never creates anything: a path that does not exist resolves to its nearest
/// existing ancestor. Use [`reveal_dir`] for targets the caller knows to be
/// directories the app is allowed to materialise.
pub fn reveal(path: &Path) -> io::Result<()> {
    let target = nearest_existing(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("nothing on disk at or above {}", path.display()),
        )
    })?;

    if target.is_dir() {
        open_directory(&target)
    } else {
        select_in_file_manager(&target)
    }
}

/// Reveal a path the caller knows to be a directory, creating it first if it
/// is missing — the default download folder on a fresh install, for instance.
pub fn reveal_dir(path: &Path) -> io::Result<()> {
    if !path.exists() {
        let _ = std::fs::create_dir_all(path);
    }
    reveal(path)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Render a path the way the Windows shell expects: backslash separators, and
/// no trailing separator except on a bare drive root (`C:\`), where dropping it
/// would turn an absolute path into a drive-relative one.
#[cfg(target_os = "windows")]
fn windows_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    if text.len() > 3 && text.ends_with('\\') {
        text.trim_end_matches('\\').to_string()
    } else {
        text
    }
}

/// Expand the `%SystemRoot%`-style variables the registry stores verbatim.
#[cfg(target_os = "windows")]
fn expand_env(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('%') else {
            out.push('%');
            rest = after;
            break;
        };
        let name = &after[..close];
        match std::env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Split a registry command line into its executable and the remaining raw
/// argument text, honouring the quotes Windows puts around program paths.
#[cfg(target_os = "windows")]
fn split_command_line(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('"') {
        let (program, args) = rest.split_once('"')?;
        Some((expand_env(program), args.trim().to_string()))
    } else {
        let (program, args) = value.split_once(char::is_whitespace).unwrap_or((value, ""));
        Some((expand_env(program), args.trim().to_string()))
    }
}

/// The command line the current user has registered as their handler for
/// opening a folder, when it is something other than Explorer.
///
/// Windows keeps its own default under `HKEY_CLASSES_ROOT`; the per-user
/// override in `HKCU\Software\Classes` is only ever written by a file manager
/// the user installed and chose. An entry here therefore means "this is my file
/// explorer" and we honour it — anything else, including no entry at all, means
/// Explorer. Resolved once per process: it costs a subprocess, and the answer
/// does not change while the app runs.
#[cfg(target_os = "windows")]
fn user_file_manager() -> Option<&'static (String, String)> {
    use std::sync::OnceLock;
    static HANDLER: OnceLock<Option<(String, String)>> = OnceLock::new();

    HANDLER
        .get_or_init(|| {
            use std::os::windows::process::CommandExt;
            let mut cmd = std::process::Command::new("reg");
            cmd.creation_flags(0x08000000);
            let output = cmd
                .args([
                    "query",
                    r"HKCU\Software\Classes\Directory\shell\open\command",
                    "/ve",
                ])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }

            // `    (Default)    REG_SZ    "C:\...\Explorer++.exe" "%1"`
            let text = String::from_utf8_lossy(&output.stdout);
            let value = text.lines().find_map(|line| {
                let rest = line
                    .split_once("REG_EXPAND_SZ")
                    .or_else(|| line.split_once("REG_SZ"))?
                    .1
                    .trim();
                (!rest.is_empty()).then(|| rest.to_string())
            })?;

            let (program, args) = split_command_line(&value)?;
            let stem = Path::new(&program)
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            // Routing Explorer through here would only lose `/select`.
            if stem == "explorer" {
                return None;
            }
            Some((program, args))
        })
        .as_ref()
}

/// Launch the user's own file manager on `directory`, substituting the path
/// into whatever placeholder its registered command line uses.
#[cfg(target_os = "windows")]
fn spawn_user_file_manager(directory: &str) -> Option<io::Result<()>> {
    use std::os::windows::process::CommandExt;

    let (program, args) = user_file_manager()?;
    let quoted = format!("\"{directory}\"");
    let mut line = args.clone();
    for placeholder in ["%1", "%L", "%l", "%V", "%v", "%I", "%i"] {
        line = line.replace(placeholder, &quoted);
    }
    if line.trim().is_empty() {
        line = quoted;
    }

    let mut command = std::process::Command::new(program);
    command.raw_arg(line);
    Some(command.spawn().map(|_| ()))
}

#[cfg(target_os = "windows")]
fn select_in_file_manager(path: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    // Someone who has replaced Explorer wants their own manager here too, and
    // that choice outranks the selection highlight: no replacement exposes a
    // standard "select this item" verb, so we open the containing folder in
    // theirs rather than yanking them into a shell they chose to stop using.
    if let Some(parent) = path.parent().filter(|p| p.exists()) {
        if let Some(Ok(())) = spawn_user_file_manager(&windows_path(parent)) {
            return Ok(());
        }
    }

    // `/select,` and the path form a single command-line token, so this has to
    // be raw text: letting Rust quote the argument yields
    // `"/select,C:\a b\c.mp4"`, which Explorer reads as one opaque path and
    // silently opens Documents instead of the file's folder.
    let target = windows_path(path);
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{target}\""))
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn open_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    let target = windows_path(path);
    if let Some(Ok(())) = spawn_user_file_manager(&target) {
        return Ok(());
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("\"{target}\""))
        .spawn()
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn select_in_file_manager(path: &Path) -> io::Result<()> {
    // `open -R` reveals and selects in the user's default file viewer.
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_directory(path: &Path) -> io::Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// Linux / BSD
// ---------------------------------------------------------------------------

/// Ask the session's registered file manager to show `path` and highlight it.
///
/// `org.freedesktop.FileManager1` is the desktop-neutral way to do this: D-Bus
/// activation resolves to whichever file manager the user actually runs
/// (Nautilus, Dolphin, Nemo, Thunar, PCManFM…), so we neither hardcode one nor
/// lose the selection. Falls back to opening the parent folder when no such
/// service is on the bus.
#[cfg(all(unix, not(target_os = "macos")))]
fn select_in_file_manager(path: &Path) -> io::Result<()> {
    let shown = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .status();

    if matches!(shown, Ok(status) if status.success()) {
        return Ok(());
    }

    match path.parent().filter(|p| p.exists()) {
        Some(parent) => open_directory(parent),
        None => open_directory(path),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_directory(path: &Path) -> io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Percent-encode a path into the `file://` URI that `FileManager1` expects.
#[cfg(all(unix, not(target_os = "macos")))]
fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

// ---------------------------------------------------------------------------
// Opening and running
// ---------------------------------------------------------------------------

/// Open `path` directly with the OS default application (equivalent to double-clicking the file).
pub fn open_file(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("\"{}\"", windows_path(path)))
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// Open a URL directly in the user's default web browser.
pub fn open_url(url: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .raw_arg(format!("url.dll,FileProtocolHandler \"{url}\""))
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

/// Run a shell command, fire-and-forget — no output captured, nothing
/// awaited. This is the same trust level as the user typing it into a
/// terminal themselves; it is their command, configured by them.
pub fn run_command(command: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("cmd");
        cmd.creation_flags(0x08000000);
        cmd.arg("/C").arg(command).spawn()?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("sh").arg("-c").arg(command).spawn()?;
    }
    Ok(())
}

/// Send a native OS desktop notification toast.
pub fn notify_desktop(title: &str, body: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "[reflection.assembly]::loadwithpartialname('System.Windows.Forms') | Out-Null; \
             $notification = New-Object System.Windows.Forms.NotifyIcon; \
             $notification.Icon = [System.Drawing.SystemIcons]::Information; \
             $notification.BalloonTipTitle = '{}'; \
             $notification.BalloonTipText = '{}'; \
             $notification.Visible = $True; \
             $notification.ShowBalloonTip(5000);",
            title.replace('\'', "''"),
            body.replace('\'', "''")
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.creation_flags(0x08000000);
        let _ = cmd
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!("display notification \"{}\" with title \"{}\"", body.replace('"', "\\\""), title.replace('"', "\\\""));
        let _ = std::process::Command::new("osascript").arg("-e").arg(script).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("notify-send").arg(title).arg(body).spawn();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_existing_walks_up_to_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone").join("deeper").join("file.mp4");

        let found = nearest_existing(&missing).unwrap();

        assert_eq!(found, dir.path());
    }

    #[test]
    fn reveal_never_creates_the_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-downloaded-yet.mp4");

        // Best-effort: the spawn may fail on a headless test runner. What must
        // hold either way is that we did not materialise the file as a folder.
        let _ = reveal(&missing);

        assert!(!missing.exists(), "reveal must not create its target");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_keeps_the_drive_root_separator() {
        assert_eq!(windows_path(Path::new(r"C:\")), r"C:\");
        assert_eq!(windows_path(Path::new(r"C:\Users\me\")), r"C:\Users\me");
        assert_eq!(windows_path(Path::new("C:/Users/me")), r"C:\Users\me");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn split_command_line_separates_quoted_programs_from_args() {
        let (program, args) = split_command_line(r#""C:\Apps\Files.exe" "%1""#).unwrap();
        assert_eq!(program, r"C:\Apps\Files.exe");
        assert_eq!(args, r#""%1""#);

        let (program, args) = split_command_line(r"C:\Apps\fm.exe %1").unwrap();
        assert_eq!(program, r"C:\Apps\fm.exe");
        assert_eq!(args, "%1");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn file_uri_encodes_spaces_but_not_separators() {
        assert_eq!(file_uri(Path::new("/home/me/a b.mp4")), "file:///home/me/a%20b.mp4");
    }
}
