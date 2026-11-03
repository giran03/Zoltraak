//! Discovery and command construction for media tools (FFmpeg and FFprobe).
//!
//! Zoltraak resolves `ffmpeg` and `ffprobe` binaries in the following order:
//! 1. Explicitly registered directories (e.g. app data bin, Tauri unpacked resources)
//! 2. Sibling paths next to the running executable (`<exe>/bin/`, `<exe>/resources/bin/`, `<exe>/`)
//! 3. Standard application data directories (`%APPDATA%/dev.zoltraak.app/bin/`)
//! 4. System `PATH` (fallback)

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tokio::process::Command;

static SEARCH_DIRS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Register an additional directory to search for media binaries.
pub fn register_search_dir(dir: PathBuf) {
    if let Ok(mut dirs) = SEARCH_DIRS.write() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
}

/// Look for an executable file named `base_name` (appending `.exe` on Windows).
pub fn find_tool(base_name: &str) -> PathBuf {
    let mut names = vec![base_name.to_string()];
    #[cfg(windows)]
    {
        let exe_name = format!("{base_name}.exe");
        if !names.contains(&exe_name) {
            names.insert(0, exe_name);
        }
    }

    // 1. Check registered search directories
    if let Ok(dirs) = SEARCH_DIRS.read() {
        for dir in dirs.iter() {
            for name in &names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    // 2. Check relative to current running executable
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let relative_subdirs = [
                PathBuf::new(),
                PathBuf::from("bin"),
                PathBuf::from("resources").join("bin"),
                PathBuf::from("resources"),
            ];

            for sub in &relative_subdirs {
                let dir = exe_dir.join(sub);
                for name in &names {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }
    }

    // 3. Check standard application data directories
    #[cfg(windows)]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            let bin_dir = Path::new(&app_data).join("dev.zoltraak.app").join("bin");
            for name in &names {
                let candidate = bin_dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let bin_dir = Path::new(&local_app_data).join("dev.zoltraak.app").join("bin");
            for name in &names {
                let candidate = bin_dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let bin_dir = Path::new(&home)
                .join(".local")
                .join("share")
                .join("dev.zoltraak.app")
                .join("bin");
            for name in &names {
                let candidate = bin_dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    // 4. Default to system PATH lookup
    PathBuf::from(base_name)
}

/// Locate the `ffmpeg` binary.
pub fn find_ffmpeg() -> PathBuf {
    find_tool("ffmpeg")
}

/// Locate the `ffprobe` binary.
pub fn find_ffprobe() -> PathBuf {
    find_tool("ffprobe")
}

/// Inputs the engine hands FFmpeg are always local files or pipes. Placed before
/// each `-i`, this stops a crafted playlist or concat list inside a user file
/// from making FFmpeg open network URLs.
pub const LOCAL_PROTOCOLS: &str = "file,pipe";
pub const LOCAL_INPUT: [&str; 2] = ["-protocol_whitelist", LOCAL_PROTOCOLS];

/// No console window, and below-normal CPU priority: an encode may use every
/// idle core, but the window and the download engine always run first.
#[cfg(target_os = "windows")]
const SPAWN_FLAGS: u32 = 0x0800_0000 /* CREATE_NO_WINDOW */ | 0x0000_4000 /* BELOW_NORMAL_PRIORITY_CLASS */;

/// Create a new `tokio::process::Command` pointing to the resolved `ffmpeg`.
pub fn ffmpeg_command() -> Command {
    let mut cmd = Command::new(find_ffmpeg());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(SPAWN_FLAGS);
    cmd
}

/// Create a new `tokio::process::Command` pointing to the resolved `ffprobe`.
pub fn ffprobe_command() -> Command {
    let mut cmd = Command::new(find_ffprobe());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(SPAWN_FLAGS);
    cmd
}

/// Create a new `std::process::Command` pointing to the resolved `ffmpeg`.
pub fn std_ffmpeg_command() -> std::process::Command {
    let mut cmd = std::process::Command::new(find_ffmpeg());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(SPAWN_FLAGS);
    }
    cmd
}

/// Create a new `std::process::Command` pointing to the resolved `ffprobe`.
pub fn std_ffprobe_command() -> std::process::Command {
    let mut cmd = std::process::Command::new(find_ffprobe());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(SPAWN_FLAGS);
    }
    cmd
}


/// The FFmpeg components the engine relies on, from `media_capabilities.json`
/// — the same list the bundling script checks the shipped build against.
#[derive(Debug, serde::Deserialize)]
pub struct RequiredCapabilities {
    pub encoders: Vec<String>,
    pub filters: Vec<String>,
    pub muxers: Vec<String>,
    pub demuxers: Vec<String>,
}

pub fn required_capabilities() -> RequiredCapabilities {
    serde_json::from_str(include_str!("../media_capabilities.json")).expect("media_capabilities.json is valid")
}

#[cfg(test)]
mod capability_tests {
    #[test]
    fn capability_manifest_parses_and_lists_every_kind() {
        let caps = super::required_capabilities();
        assert!(!caps.encoders.is_empty() && !caps.filters.is_empty() && !caps.muxers.is_empty() && !caps.demuxers.is_empty());
    }
}
