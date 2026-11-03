//! Tauri shell. The frontend talks to `zoltraak-core` in-process over IPC
//! commands, not the REST API — there is no reason to pay HTTP overhead for a
//! call from the window that owns the engine.
//!
//! The app also *hosts* the REST API on the same engine instance, bound to
//! `127.0.0.1:1450`, so the browser extension has something to talk to
//! without the user running a separate headless process. Both paths share one
//! `Supervisor`, so a download added from either place shows up in both.
//!
//! The engine is started once at setup and lives in Tauri's managed state. The
//! frontend never polls: a single background task emits the queue on a timer,
//! which keeps one snapshot serialization serving every open window.

use std::path::PathBuf;
use std::str::FromStr;

pub mod browser;
pub mod ffmpeg;
pub mod updater;
mod media_preview;
mod discord_links;
mod updater_cache;
mod app_icon;
mod window_diagnostics;
mod window_corners;
mod file_drop;
mod queue_push;
mod webview_power;

use tauri_plugin_dialog::DialogExt;

use zoltraak_core::downloader::Options;
use zoltraak_core::queue::NewDownload;
use zoltraak_core::runner::RunnerConfig;
use zoltraak_core::discord::{DeliverySource, DiscordService};
use zoltraak_core::settings::DiscordSettings;
use zoltraak_core::remux::manager::{NewRemux, RemuxJob, RemuxManager};
use zoltraak_core::remux::{Container, MediaInfo};
use zoltraak_core::{
    DownloadId, DownloadSnapshot, Downloader, MusicEnqueued, MusicJob, MusicManager, MusicSettings, MusicSource, NewMusicDownload,
    NewVideoDownload, OutputOptions, Queue, Supervisor, VideoFormat, VideoJob, VideoManager,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

/// What the UI needs to show the extension-pairing card.
///
/// The token is handed to the frontend deliberately: the window and the engine
/// are the same process and the same trust domain, and pairing is only usable
/// if the user can see and copy the secret. It never leaves the machine.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiStatus {
    listening: bool,
    port: u16,
    token: String,
    /// Why the API is not listening, when it is not.
    detail: Option<String>,
}

/// IDs of queue items or video jobs explicitly marked for auto-delivery to Discord
/// upon download completion, even if global auto-send is disabled.
#[derive(Clone, Default)]
pub struct DiscordAutoSend(pub std::sync::Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>);


const QUEUE_EVENT: &str = "zoltraak://queue";

/// Emitted to the frontend when a client (the browser extension's "Send to
/// Zoltraak") wants the downloads text input filled with a URL.
const DOWNLOAD_PREFILL_EVENT: &str = "zoltraak://download-prefill";

/// Emitted to the frontend when a client (the browser extension's "Send video
/// to Zoltraak") wants the Video view filled with a URL.
const VIDEO_PREFILL_EVENT: &str = "zoltraak://video-prefill";

/// Emitted when the browser extension sends a social post to Images.
const IMAGE_PREFILL_EVENT: &str = "zoltraak://image-prefill";

/// Emitted to the frontend when a client (the browser extension's "Download
/// music with Zoltraak") wants the Music view filled with a link.
const MUSIC_PREFILL_EVENT: &str = "zoltraak://music-prefill";

/// Emitted to the frontend when an external client (e.g. web guide or deep link)
/// requests focus and navigation to a specific section and tab.
const APP_NAVIGATE_EVENT: &str = "zoltraak://navigate";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppNavigatePayload {
    section: String,
    tab: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MusicPrefillPayload {
    url: String,
    headers: zoltraak_core::headers::Headers,
}

/// What the Music view needs to know about the local toolchain.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MusicTools {
    /// The installed ffmpeg's version, or `None` when it is not on PATH.
    ffmpeg: Option<String>,
    ffprobe: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoPrefillPayload {
    url: String,
    /// Sanitized headers (a session cookie, most often) captured by the
    /// extension for `url` — carried along so the Video view's analyze/
    /// download can use the same session the caller browsed with.
    headers: zoltraak_core::headers::Headers,
    /// When set, the Video view should immediately download the best-quality
    /// result after analyzing — the floating "Download this video" button's
    /// one-click flow. Always `false` for the downloads-input prefill.
    auto_download: bool,
    /// The source page's title, used only as a fallback filename when the
    /// engine's extractor has none of its own (a raw CDN link).
    title: Option<String>,
}

/// IPC boundary error.
///
/// The engine's `Error` carries paths and driver detail that belong in a log,
/// not in a UI toast, so it is flattened to a message on the way out.
#[derive(Debug, serde::Serialize)]
struct IpcError {
    message: String,
}

impl From<zoltraak_core::Error> for IpcError {
    fn from(e: zoltraak_core::Error) -> Self {
        tracing::warn!(error = %e, "command failed");
        Self {
            message: e.to_string(),
        }
    }
}

impl From<String> for IpcError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

type IpcResult<T> = std::result::Result<T, IpcError>;

fn parse_id(raw: &str) -> IpcResult<DownloadId> {
    DownloadId::from_str(raw).map_err(IpcError::from)
}

/// Health probe the UI calls on mount to decide what the status pill shows.
#[tauri::command]
fn engine_ready() -> bool {
    true
}

/// Connection count the engine would pick for a file of this size, so the UI
/// can preview the choice before a download starts.
#[tauri::command]
fn planned_connections(total_bytes: Option<u64>) -> u8 {
    zoltraak_core::default_connections(total_bytes)
}

/// Where downloads land when the user does not pick a folder.
#[tauri::command]
fn default_directory(app: tauri::AppHandle, sup: tauri::State<'_, Supervisor>) -> IpcResult<String> {
    let dir = sup
        .settings()
        .default_directory
        .unwrap_or_else(|| downloads_dir(&app));
    Ok(dir.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn ensure_dialog_topmost(title: &'static str) {
    std::thread::spawn(move || {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        let wide: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
        // Poll for up to 4 seconds (80 * 50ms)
        for _ in 0..80 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
            if hwnd != 0 as _ {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                    );
                    SetForegroundWindow(hwnd);
                    BringWindowToTop(hwnd);
                }
                break;
            }
        }
    });
}

#[cfg(not(windows))]
fn ensure_dialog_topmost(_title: &'static str) {}

/// Ask the user for a folder, starting wherever the entry line is currently
/// pointed.
///
/// `Ok(None)` is a cancel, not a failure — the caller keeps the destination it
/// had. The picker is opened from Rust rather than through the plugin's JS
/// bridge, which keeps it in the same shape as every other engine call the
/// frontend makes and leaves the window's capability set untouched.
#[tauri::command]
async fn choose_directory(app: tauri::AppHandle, start: Option<String>) -> IpcResult<Option<String>> {
    const DIALOG_TITLE: &str = "Where should this download land?";
    let mut builder = app.dialog().file().set_title(DIALOG_TITLE);
    if let Some(dir) = start.filter(|d| !d.trim().is_empty()) {
        builder = builder.set_directory(dir);
    }

    ensure_dialog_topmost(DIALOG_TITLE);

    // The callback form, not `blocking_pick_folder`: this runs on a runtime
    // worker, and parking it for as long as someone browses their disk is not
    // this thread's to spend.
    let (tx, rx) = tokio::sync::oneshot::channel();
    builder.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let picked = rx
        .await
        .map_err(|_| IpcError::from("The folder picker closed unexpectedly.".to_string()))?;

    Ok(picked
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn queue_snapshot(sup: tauri::State<'_, Supervisor>) -> IpcResult<Vec<DownloadSnapshot>> {
    Ok(sup.snapshot().await?)
}

/// Queue a URL.
///
/// `filename` exists for the Images view: a photo CDN's path is a content
/// hash, so a gallery saved straight from its URLs lands as a folder of
/// unreadable names with no hint of which post they came from. `headers`
/// likewise — a Facebook `lookaside` link is only served to the crawler
/// identity that found it, and is a login page to anything else.
#[tauri::command]
async fn add_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    auto_send: tauri::State<'_, DiscordAutoSend>,
    url: String,
    directory: Option<String>,
    connections: Option<u8>,
    filename: Option<String>,
    headers: Option<Vec<(String, String)>>,
    auto_discord: Option<bool>,
) -> IpcResult<String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Enter a URL to download.".to_string().into());
    }
    // Reject anything the engine cannot fetch here rather than letting it
    // become a failed queue row the user has to clean up.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http:// and https:// URLs are supported.".to_string().into());
    }

    let directory = directory
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            sup.settings()
                .default_directory
                .unwrap_or_else(|| downloads_dir(&app))
        });

    // The queue outlives the window that created it, so store an absolute path.
    let directory = zoltraak_core::supervisor::absolute_directory(directory)
        .map_err(|e| IpcError::from(format!("invalid destination folder: {e}")))?;

    let mut new = NewDownload::new(url, directory);
    new.connections = connections;
    new.headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    if let Some(name) = filename {
        // Only the last path segment is used, which both preserves a name that
        // arrived as a full path and keeps the write inside the chosen folder.
        if let Some(name) = zoltraak_core::probe::requested_filename(&name) {
            new.filename = name;
        }
    }

    let id = sup.enqueue(new).await?;
    let id_str = id.to_string();
    if auto_discord.unwrap_or(false) {
        auto_send.0.write().await.insert(id_str.clone());
    }
    let _ = push_queue(&app, &sup).await;
    Ok(id_str)
}

#[tauri::command]
async fn pause_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    id: String,
) -> IpcResult<()> {
    sup.pause(parse_id(&id)?).await?;
    let _ = push_queue(&app, &sup).await;
    Ok(())
}

#[tauri::command]
async fn resume_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    id: String,
) -> IpcResult<()> {
    sup.resume(parse_id(&id)?).await?;
    let _ = push_queue(&app, &sup).await;
    Ok(())
}

#[tauri::command]
async fn remove_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    id: String,
    delete_file: Option<bool>,
) -> IpcResult<()> {
    let parsed_id = parse_id(&id)?;
    let delete_file = delete_file.unwrap_or(false);
    match sup.queue().get(parsed_id).await {
        Ok(_) => {
            sup.remove_with_file(parsed_id, delete_file).await?;
            let _ = push_queue(&app, &sup).await;
            return Ok(());
        }
        Err(zoltraak_core::Error::NotFound(_)) => {}
        Err(error) => return Err(error.into()),
    }
    if video.get(parsed_id).await.is_some() {
        video.remove_with_file(parsed_id, delete_file).await?;
        return Ok(());
    }
    if music.get(parsed_id).await.is_some() {
        music.remove_with_file(parsed_id, delete_file).await?;
        return Ok(());
    }
    Err(IpcError::from(format!("download {id} not found")))
}

/// Everything the pairing card renders: whether the extension has something to
/// connect to, and the token to paste into it.
#[tauri::command]
fn api_status(status: tauri::State<'_, ApiStatus>) -> ApiStatus {
    status.inner().clone()
}

/// Reveal a finished download in the OS file manager.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputStatus { path: Option<String>, status: &'static str, detail: Option<String> }

async fn output_path(sup: &Supervisor, video: &VideoManager, music: &MusicManager, id: DownloadId) -> IpcResult<Option<PathBuf>> {
    if let Ok(item) = sup.queue().get(id).await { return Ok(Some(item.path())); }
    if let Some(job) = video.get(id).await { return Ok(job.path); }
    if let Some(job) = music.get(id).await { return Ok(job.path); }
    Err(IpcError::from(format!("download {id} not found")))
}

#[tauri::command]
async fn download_file_status(sup: tauri::State<'_, Supervisor>, video: tauri::State<'_, VideoManager>, music: tauri::State<'_, MusicManager>, id: String) -> IpcResult<OutputStatus> {
    let path = output_path(&sup, &video, &music, parse_id(&id)?).await?;
    let Some(path) = path else { return Ok(OutputStatus { path: None, status: "unknown", detail: Some("No output file recorded yet.".into()) }); };
    let (status, detail) = match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_file() => ("present", None),
        Ok(_) => ("inaccessible", Some("The recorded output is not a regular file.".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::metadata(path.parent().unwrap_or(std::path::Path::new(""))).await {
                Ok(meta) if meta.is_dir() => ("missing", Some("The file was moved or deleted outside the app.".into())),
                _ => ("inaccessible", Some("The destination folder is unavailable. Reconnect the drive or locate the file.".into())),
            }
        }
        Err(e) => ("inaccessible", Some(e.to_string())),
    };
    Ok(OutputStatus { path: Some(path.to_string_lossy().into_owned()), status, detail })
}

#[tauri::command]
async fn download_file_statuses(
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    ids: Vec<String>,
) -> IpcResult<std::collections::HashMap<String, OutputStatus>> {
    let mut map = std::collections::HashMap::with_capacity(ids.len());
    for id in ids {
        if let Ok(parsed) = parse_id(&id) {
            if let Ok(Some(path)) = output_path(&sup, &video, &music, parsed).await {
                let (status, detail) = match tokio::fs::metadata(&path).await {
                    Ok(meta) if meta.is_file() => ("present", None),
                    Ok(_) => ("inaccessible", Some("The recorded output is not a regular file.".into())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        match tokio::fs::metadata(path.parent().unwrap_or(std::path::Path::new(""))).await {
                            Ok(meta) if meta.is_dir() => ("missing", Some("The file was moved or deleted outside the app.".into())),
                            _ => ("inaccessible", Some("The destination folder is unavailable. Reconnect the drive or locate the file.".into())),
                        }
                    }
                    Err(e) => ("inaccessible", Some(e.to_string())),
                };
                map.insert(id, OutputStatus { path: Some(path.to_string_lossy().into_owned()), status, detail });
            }
        }
    }
    Ok(map)
}

#[tauri::command]
async fn download_thumbnail(app: tauri::AppHandle, sup: tauri::State<'_, Supervisor>, video: tauri::State<'_, VideoManager>, music: tauri::State<'_, MusicManager>, id: String) -> IpcResult<Option<String>> {
    use base64::Engine;
    let id = parse_id(&id)?;
    // Resolve only recorded, completed files. The frontend cannot supply an
    // arbitrary filesystem path or ask FFmpeg to decode a remote URL.
    let path = if let Ok(item) = sup.queue().get(id).await {
        (item.state == zoltraak_core::DownloadState::Finished).then(|| item.path())
    } else if let Some(job) = video.get(id).await {
        (job.state == zoltraak_core::VideoJobState::Finished).then_some(job.path).flatten()
    } else if let Some(job) = music.get(id).await {
        matches!(job.state, zoltraak_core::MusicJobState::Finished | zoltraak_core::MusicJobState::Skipped).then_some(job.path).flatten()
    } else { None };
    let Some(path) = path else { return Ok(None); };
    let cache = app.path().app_cache_dir().map_err(|e| IpcError::from(e.to_string()))?.join("media-previews");
    Ok(media_preview::thumbnail(&path, &cache).await.map_err(IpcError::from)?
        .map(|bytes| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
}

#[tauri::command]
async fn locate_download_file(app: tauri::AppHandle, sup: tauri::State<'_, Supervisor>, video: tauri::State<'_, VideoManager>, music: tauri::State<'_, MusicManager>, id: String) -> IpcResult<Option<String>> {
    let id = parse_id(&id)?;
    let old = output_path(&sup, &video, &music, id).await?;
    let title = "Locate downloaded file";
    let mut picker = app.dialog().file().set_title(title);
    if let Some(parent) = old.as_ref().and_then(|p| p.parent()) { picker = picker.set_directory(parent); }
    ensure_dialog_topmost(title);
    let (tx, rx) = tokio::sync::oneshot::channel();
    picker.pick_file(move |picked| { let _ = tx.send(picked); });
    let picked = rx.await.map_err(|_| IpcError::from("File picker closed unexpectedly.".to_string()))?;
    let Some(picked) = picked else { return Ok(None); };
    let path = picked.into_path().map_err(|e| IpcError::from(e.to_string()))?;
    if sup.queue().get(id).await.is_ok() { sup.queue().locate_file(id, &path).await?; }
    else if video.get(id).await.is_some() { video.locate_file(id, &path).await?; }
    else { music.locate_file(id, &path).await?; }
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn redownload_missing(sup: tauri::State<'_, Supervisor>, video: tauri::State<'_, VideoManager>, music: tauri::State<'_, MusicManager>, id: String) -> IpcResult<()> {
    let id = parse_id(&id)?;
    if sup.queue().get(id).await.is_ok() { sup.restart_missing(id).await?; }
    else if video.get(id).await.is_some() { video.restart_missing(id).await?; }
    else { music.restart_missing(id).await?; }
    Ok(())
}

#[derive(serde::Serialize)]
struct DestinationHealth { path: String, writable: bool, detail: Option<String> }

#[tauri::command]
async fn destination_health(app: tauri::AppHandle, sup: tauri::State<'_, Supervisor>) -> IpcResult<DestinationHealth> {
    let directory = sup.settings().default_directory.unwrap_or_else(|| downloads_dir(&app));
    let probe = directory.join(format!(".zoltraak-write-check-{}", DownloadId::new()));
    let result = async {
        let file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&probe).await?;
        drop(file);
        tokio::fs::remove_file(&probe).await
    }.await;
    Ok(DestinationHealth { path: directory.to_string_lossy().into_owned(), writable: result.is_ok(), detail: result.err().map(|e: std::io::Error| e.to_string()) })
}

#[tauri::command]
async fn reveal_download(
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    id: String,
) -> IpcResult<()> {
    let parsed_id = parse_id(&id)?;
    // `reveal` selects the file when it is there and otherwise falls back to
    // the nearest folder that exists, so an in-progress or since-deleted
    // download still lands the user in its destination.
    if let Ok(item) = sup.queue().get(parsed_id).await {
        let path = item.path();
        let result = if path.exists() {
            zoltraak_core::shell::reveal(&path)
        } else {
            zoltraak_core::shell::reveal_dir(&item.directory)
        };
        result.map_err(|e| IpcError::from(format!("could not open: {e}")))?;
        return Ok(());
    }

    if let Some(job) = video.get(parsed_id).await {
        if let Some(path) = job.path {
            zoltraak_core::shell::reveal(&path)
                .map_err(|e| IpcError::from(format!("could not open: {e}")))?;
            return Ok(());
        }
    }

    if let Some(job) = music.get(parsed_id).await {
        // A track that has not landed yet still has a folder to show.
        let result = match job.path.filter(|p| p.exists()) {
            Some(path) => zoltraak_core::shell::reveal(&path),
            None => zoltraak_core::shell::reveal_dir(&job.directory),
        };
        result.map_err(|e| IpcError::from(format!("could not open: {e}")))?;
        return Ok(());
    }

    Err(IpcError::from(format!("download {id} not found")))
}

/// Open a finished download directly with the OS default application (or reveal if not a file).
#[tauri::command]
async fn open_download(
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    id: String,
) -> IpcResult<()> {
    let parsed_id = parse_id(&id)?;
    if let Ok(item) = sup.queue().get(parsed_id).await {
        let path = item.path();
        if path.is_file() {
            zoltraak_core::shell::open_file(&path)
                .map_err(|e| IpcError::from(format!("could not open: {e}")))?;
            return Ok(());
        } else {
            let result = if path.exists() {
                zoltraak_core::shell::reveal(&path)
            } else {
                zoltraak_core::shell::reveal_dir(&item.directory)
            };
            result.map_err(|e| IpcError::from(format!("could not open: {e}")))?;
            return Ok(());
        }
    }

    if let Some(job) = video.get(parsed_id).await {
        if let Some(path) = job.path {
            if path.is_file() {
                zoltraak_core::shell::open_file(&path)
                    .map_err(|e| IpcError::from(format!("could not open: {e}")))?;
                return Ok(());
            } else {
                zoltraak_core::shell::reveal(&path)
                    .map_err(|e| IpcError::from(format!("could not open: {e}")))?;
                return Ok(());
            }
        }
    }

    if let Some(job) = music.get(parsed_id).await {
        let result = match job.path.filter(|p| p.is_file()) {
            Some(path) => zoltraak_core::shell::open_file(&path),
            None => zoltraak_core::shell::reveal_dir(&job.directory),
        };
        result.map_err(|e| IpcError::from(format!("could not open: {e}")))?;
        return Ok(());
    }

    Err(IpcError::from(format!("download {id} not found")))
}

/// Resolve a finished row to an existing file before the webview starts a
/// native copy drag. The frontend never gets to nominate an arbitrary path.
#[tauri::command]
async fn drag_download_path(
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    id: String,
) -> IpcResult<String> {
    let parsed_id = parse_id(&id)?;
    let path = if let Ok(item) = sup.queue().get(parsed_id).await {
        Some(item.path())
    } else if let Some(job) = video.get(parsed_id).await {
        job.path
    } else if let Some(job) = music.get(parsed_id).await {
        job.path
    } else {
        None
    };

    let path = path.ok_or_else(|| IpcError::from(format!("download {id} has no file yet")))?;
    if !path.is_file() {
        return Err(IpcError::from(format!(
            "{} is no longer on disk",
            path.display()
        )));
    }
    Ok(path.to_string_lossy().into_owned())
}

// ---- Music ---------------------------------------------------------------
//
// A link resolves to tracks; the chosen ones become jobs the music manager
// works through. The manager owns the queue, the matching, the fetch and the
// ffmpeg step; these commands only translate between the window and it.

/// Where music lands by default: a `Music` folder inside the download folder.
fn music_dir(app: &tauri::AppHandle, sup: &Supervisor) -> PathBuf {
    sup.settings().default_directory.unwrap_or_else(|| downloads_dir(app)).join("Music")
}

#[tauri::command]
fn music_directory(app: tauri::AppHandle, sup: tauri::State<'_, Supervisor>) -> String {
    music_dir(&app, &sup).to_string_lossy().into_owned()
}

/// Resolve a link, or search by name when the input is not one.
#[tauri::command]
async fn analyze_music(
    music: tauri::State<'_, MusicManager>,
    input: String,
    headers: Option<Vec<(String, String)>>,
) -> IpcResult<MusicSource> {
    let input = input.trim().to_string();
    if input.is_empty() {
        return Err("Paste a link, or type something to search for.".to_string().into());
    }
    let headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    Ok(music.analyze(&input, &headers).await?)
}

/// Queue the tracks of an analyzed source. The source arrives holding only
/// the tracks the user kept.
#[tauri::command]
async fn start_music_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    music: tauri::State<'_, MusicManager>,
    auto_send: tauri::State<'_, DiscordAutoSend>,
    source: MusicSource,
    options: OutputOptions,
    auto_discord: Option<bool>,
) -> IpcResult<MusicEnqueued> {
    let mut options = options;
    if let Some(dir) = &options.directory {
        if dir.as_os_str().is_empty() {
            options.directory = None;
        } else if !dir.is_absolute() {
            return Err("The music folder must be an absolute path.".to_string().into());
        }
    }
    let base_directory = music_dir(&app, &sup);
    let enqueued = music.enqueue(NewMusicDownload { source, options, base_directory }).await?;
    if auto_discord.unwrap_or(false) {
        let mut set = auto_send.0.write().await;
        for id in &enqueued.job_ids {
            set.insert(id.to_string());
        }
    }
    Ok(enqueued)
}

#[tauri::command]
async fn list_music_jobs(music: tauri::State<'_, MusicManager>) -> IpcResult<Vec<MusicJob>> {
    Ok(music.list().await)
}

#[tauri::command]
async fn remove_music_job(music: tauri::State<'_, MusicManager>, id: String, delete_file: Option<bool>) -> IpcResult<()> {
    music.remove_with_file(parse_id(&id)?, delete_file.unwrap_or(false)).await?;
    Ok(())
}

#[tauri::command]
async fn pause_music_job(music: tauri::State<'_, MusicManager>, id: String) -> IpcResult<()> {
    music.pause(parse_id(&id)?).await;
    Ok(())
}

/// Requeue a paused, failed or skipped job. `force` downloads a skipped
/// track even though its file exists.
#[tauri::command]
async fn resume_music_job(music: tauri::State<'_, MusicManager>, id: String, force: Option<bool>) -> IpcResult<()> {
    music.resume(parse_id(&id)?, force.unwrap_or(false)).await?;
    Ok(())
}

#[tauri::command]
async fn remove_music_collection(music: tauri::State<'_, MusicManager>, collection_id: String) -> IpcResult<usize> {
    Ok(music.remove_collection(&collection_id).await?)
}

#[tauri::command]
async fn retry_music_collection(music: tauri::State<'_, MusicManager>, collection_id: String) -> IpcResult<usize> {
    Ok(music.retry_collection(&collection_id).await?)
}

#[tauri::command]
async fn clear_music_done(music: tauri::State<'_, MusicManager>) -> IpcResult<usize> {
    Ok(music.clear_done().await?)
}

#[tauri::command]
async fn music_settings(music: tauri::State<'_, MusicManager>) -> IpcResult<MusicSettings> {
    Ok(music.settings().await)
}

#[tauri::command]
async fn update_music_settings(music: tauri::State<'_, MusicManager>, settings: MusicSettings) -> IpcResult<MusicSettings> {
    if let Some(dir) = &settings.output.directory {
        if !dir.as_os_str().is_empty() && !dir.is_absolute() {
            return Err("The music folder must be an absolute path.".to_string().into());
        }
    }
    let mut settings = settings;
    if settings.output.directory.as_ref().is_some_and(|d| d.as_os_str().is_empty()) {
        settings.output.directory = None;
    }
    Ok(music.update_settings(settings).await?)
}

/// Whether ffmpeg is there to convert and tag with.
#[tauri::command]
async fn music_tools() -> MusicTools {
    MusicTools {
        ffmpeg: zoltraak_core::music::convert::ffmpeg_version().await,
        ffprobe: zoltraak_core::music::convert::ffprobe_available().await,
    }
}

/// Download and set up portable FFmpeg & FFprobe binaries in the app's data directory.
#[tauri::command]
async fn install_ffmpeg(app: tauri::AppHandle) -> Result<ffmpeg::MusicToolsResult, String> {
    ffmpeg::download_and_install_ffmpeg(&app).await
}

/// Reveal a folder (or default downloads directory) in OS file manager.
#[tauri::command]
async fn reveal_folder(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    path: Option<String>,
) -> IpcResult<()> {
    // An explicit path is revealed as-is — it may be a file, and it is not
    // ours to create. Only the app's own download folder gets materialised,
    // for the fresh install where nothing has been saved into it yet.
    let result = match path.filter(|p| !p.trim().is_empty()).map(PathBuf::from) {
        Some(target) => zoltraak_core::shell::reveal(&target),
        None => {
            let default = sup
                .settings()
                .default_directory
                .unwrap_or_else(|| downloads_dir(&app));
            zoltraak_core::shell::reveal_dir(&default)
        }
    };
    result.map_err(|e| IpcError::from(format!("could not open: {e}")))?;
    Ok(())
}

/// Open the detailed user guide in the default web browser.
#[tauri::command]
async fn open_guide(api_status: tauri::State<'_, ApiStatus>) -> IpcResult<()> {
    let port = if api_status.listening {
        api_status.port
    } else {
        zoltraak_api::DEFAULT_ADDR.port()
    };
    let url = format!("http://127.0.0.1:{port}/guide");
    zoltraak_core::shell::open_url(&url)
        .map_err(|e| IpcError::from(format!("could not open guide: {e}")))?;
    Ok(())
}

/// Open the legal notices, privacy policy, and terms in the default web browser.
#[tauri::command]
async fn open_legal(api_status: tauri::State<'_, ApiStatus>) -> IpcResult<()> {
    let port = if api_status.listening {
        api_status.port
    } else {
        zoltraak_api::DEFAULT_ADDR.port()
    };
    let url = format!("http://127.0.0.1:{port}/legal");
    zoltraak_core::shell::open_url(&url)
        .map_err(|e| IpcError::from(format!("could not open legal notices: {e}")))?;
    Ok(())
}

/// Open an external web link in the default web browser.
#[tauri::command]
async fn open_url(url: String) -> IpcResult<()> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(IpcError::from(
            "only http and https URLs can be opened".to_string(),
        ));
    }
    zoltraak_core::shell::open_url(&url)
        .map_err(|e| IpcError::from(format!("could not open url: {e}")))?;
    Ok(())
}

#[tauri::command]
async fn open_discord_message(url: String) -> IpcResult<()> {
    tokio::task::spawn_blocking(move || discord_links::open(&url))
        .await.map_err(|e| IpcError::from(e.to_string()))?
        .map_err(IpcError::from)
}


#[tauri::command]
async fn list_categories(
    sup: tauri::State<'_, Supervisor>,
) -> IpcResult<Vec<zoltraak_core::Category>> {
    Ok(sup.queue().list_categories().await?)
}

#[tauri::command]
async fn add_category(
    sup: tauri::State<'_, Supervisor>,
    name: String,
    kind: zoltraak_core::CategoryKind,
    pattern: String,
    directory: String,
    priority: Option<i64>,
) -> IpcResult<i64> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the category a name.".to_string().into());
    }
    let directory = zoltraak_core::supervisor::absolute_directory(directory)
        .map_err(|e| IpcError::from(format!("invalid destination folder: {e}")))?;

    Ok(sup
        .queue()
        .add_category(zoltraak_core::NewCategory {
            name,
            kind,
            pattern,
            directory,
            priority: priority.unwrap_or(0),
        })
        .await?)
}

#[tauri::command]
async fn set_category_enabled(
    sup: tauri::State<'_, Supervisor>,
    id: i64,
    enabled: bool,
) -> IpcResult<()> {
    sup.queue().set_category_enabled(id, enabled).await?;
    Ok(())
}

#[tauri::command]
async fn remove_category(sup: tauri::State<'_, Supervisor>, id: i64) -> IpcResult<()> {
    sup.queue().remove_category(id).await?;
    Ok(())
}

/// Analyze a page or direct media URL — a paste-a-link entry point to the
/// same extractor the browser extension's "Download this video" button uses.
#[tauri::command]
async fn analyze_video(
    video: tauri::State<'_, VideoManager>,
    url: String,
    headers: Option<Vec<(String, String)>>,
    use_fallback: Option<bool>,
) -> IpcResult<zoltraak_core::VideoMetadata> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http:// and https:// URLs are supported.".to_string().into());
    }
    let headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    Ok(video.analyze_with_fallback(&url, &headers, use_fallback.unwrap_or(false)).await?)
}

/// The pictures a post holds.
///
/// There is no matching `start_image_download`: each item is a plain file, so
/// the UI queues it through `add_download` like any other link — which is also
/// what puts it in the Downloads list with pause, resume, and reveal already
/// working.
#[tauri::command]
async fn analyze_images(
    video: tauri::State<'_, VideoManager>,
    url: String,
    headers: Option<Vec<(String, String)>>,
) -> IpcResult<zoltraak_core::ImageAlbum> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http:// and https:// URLs are supported.".to_string().into());
    }
    let headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    Ok(video.analyze_images(&url, &headers).await?)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkInspection {
    filename: String,
    content_type: Option<String>,
    is_html: bool,
}

/// What a link would be saved as, so a dropped web page can be confirmed
/// before it is queued. A server that does not answer is judged by its URL.
#[tauri::command]
async fn inspect_download_link(
    url: String,
    headers: Option<Vec<(String, String)>>,
) -> IpcResult<LinkInspection> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Only http:// and https:// URLs are supported.".to_string().into());
    }
    let headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| IpcError::from(format!("could not prepare the link check: {e}")))?;
    let caps = zoltraak_core::probe::probe(&client, &url, &headers)
        .await
        .unwrap_or_default();
    let filename = zoltraak_core::probe::resolve_filename(None, &url, &caps);
    let is_html = zoltraak_core::probe::is_html(&filename, caps.content_type.as_deref());
    Ok(LinkInspection { filename, content_type: caps.content_type, is_html })
}

#[tauri::command]
async fn start_video_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    auto_send: tauri::State<'_, DiscordAutoSend>,
    title: String,
    webpage_url: String,
    format: VideoFormat,
    directory: Option<String>,
    headers: Option<Vec<(String, String)>>,
    auto_discord: Option<bool>,
) -> IpcResult<String> {
    let directory = directory
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            sup.settings()
                .default_directory
                .unwrap_or_else(|| downloads_dir(&app))
        });
    let directory = zoltraak_core::supervisor::absolute_directory(directory)
        .map_err(|e| IpcError::from(format!("invalid destination folder: {e}")))?;

    let headers = zoltraak_core::headers::sanitize(headers.unwrap_or_default());
    let id = video
        .start(NewVideoDownload {
            title,
            webpage_url,
            format,
            directory,
            headers,
        })
        .await?;
    let id_str = id.to_string();
    if auto_discord.unwrap_or(false) {
        auto_send.0.write().await.insert(id_str.clone());
    }
    Ok(id_str)
}

/// The browser sessions the extension has shared, for the settings screen:
/// sites, dates and cookie *names* only. Values never leave the vault.
#[tauri::command]
async fn list_sessions(video: tauri::State<'_, VideoManager>) -> IpcResult<Vec<zoltraak_core::SessionSummary>> {
    Ok(video.sessions().await)
}

#[tauri::command]
async fn forget_session(video: tauri::State<'_, VideoManager>, domain: String) -> IpcResult<()> {
    video.forget_session(&domain).await?;
    Ok(())
}

#[tauri::command]
async fn forget_all_sessions(video: tauri::State<'_, VideoManager>) -> IpcResult<()> {
    video.forget_all_sessions().await?;
    Ok(())
}

#[tauri::command]
async fn extension_source_state(
    api: tauri::State<'_, zoltraak_api::ApiState>,
) -> IpcResult<zoltraak_api::extension_source::ExtensionSourceState> {
    api.extension_source_state().await.map_err(IpcError::from)
}

#[tauri::command]
async fn select_extension_source(
    api: tauri::State<'_, zoltraak_api::ApiState>,
    client_id: String,
) -> IpcResult<zoltraak_api::extension_source::ExtensionSourceState> {
    api.select_extension_source(&client_id).await.map_err(IpcError::from)
}

#[tauri::command]
async fn list_video_jobs(video: tauri::State<'_, VideoManager>) -> IpcResult<Vec<VideoJob>> {
    Ok(video.list().await)
}

/// Queue a finished file for Discord, if the user asked for that.
///
/// Every failure here is swallowed deliberately: a download that succeeded
/// must not be reported as broken because a side errand could not be started.
/// Anything that does go wrong is visible in the delivery list instead.
async fn deliver_if_wanted(
    discord: &DiscordService,
    auto_send: &DiscordAutoSend,
    source: DeliverySource,
    source_id: Option<String>,
    path: std::path::PathBuf,
) {
    let explicitly_tagged = if let Some(ref id) = source_id {
        auto_send.0.write().await.remove(id)
    } else {
        false
    };

    if !explicitly_tagged {
        match discord.settings().await {
            Ok(settings) if settings.auto_send => {}
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(error = %e, "could not read Discord settings for auto-send");
                return;
            }
        }
    }
    let Some(new) = zoltraak_core::discord::NewDelivery::from_path(source, source_id, path) else {
        return;
    };
    if let Err(e) = discord.enqueue(new).await {
        tracing::warn!(error = %e, "could not queue a Discord delivery");
    }
}

// ---- Remux ---------------------------------------------------------------
//
// The only part of the engine whose input is a path on this machine rather
// than a URL. Everything here therefore starts by turning something the window
// knows — a dropped path, a typed one, or the id of a finished download — into
// a file the manager can be pointed at.

/// How deep a dropped folder is walked.
///
/// A drop of "my videos" should find the clips inside it; a drop of `C:\` must
/// not walk the whole disk while the window sits there saying nothing. Two
/// levels covers a season folder inside a series folder and stops well short
/// of anything pathological.
const SCAN_DEPTH: usize = 2;

/// Collect the media files under `path`, following folders to [`SCAN_DEPTH`].
fn collect_media(path: &std::path::Path, depth: usize, out: &mut Vec<PathBuf>) {
    // A cap on the batch as well as on the depth: past this the list stops
    // being something a person reads before pressing convert.
    const MAX_FILES: usize = 500;
    if out.len() >= MAX_FILES {
        return;
    }
    if path.is_file() {
        if zoltraak_core::remux::is_media_path(path) {
            out.push(path.to_path_buf());
        }
        return;
    }
    if !path.is_dir() || depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    // Directory order is whatever the filesystem feels like; a stable list is
    // what makes a drop of the same folder twice produce the same rows.
    found.sort();
    for entry in found {
        collect_media(&entry, depth - 1, out);
    }
}

/// Describe local files, expanding any folders among them.
///
/// This is what a drop onto the Remux view runs: it decides what is media,
/// what each file actually contains, and therefore what converting it would
/// cost — all before anything is started.
#[tauri::command]
async fn scan_media(paths: Vec<String>) -> IpcResult<Vec<MediaInfo>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for raw in paths {
        collect_media(std::path::Path::new(raw.trim().trim_matches('"')), SCAN_DEPTH, &mut files);
    }
    files.dedup();

    let mut out = Vec::with_capacity(files.len());
    for file in files {
        match zoltraak_core::conversion::inspect_for_plan(&file).await {
            Ok(info) => out.push(info),
            // One unreadable file in a dropped folder must not cost the user
            // the rest of the batch.
            Err(e) => tracing::debug!(path = %file.display(), error = %e, "could not read a dropped file"),
        }
    }
    Ok(out)
}

/// Ask the user for files to repackage, and describe what they picked.
///
/// Opened from Rust in the same shape as [`choose_directory`], and returning
/// the *probed* files rather than their paths: the view's whole job before a
/// run is to say what converting each one would cost, and a second round trip
/// to find that out would show a list that is briefly lying.
#[tauri::command]
async fn choose_media_files(app: tauri::AppHandle) -> IpcResult<Vec<MediaInfo>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Which files should be converted?")
        .add_filter("Video, audio, images and animations", zoltraak_core::remux::MEDIA_EXTENSIONS)
        .pick_files(move |picked| {
            let _ = tx.send(picked);
        });

    let picked = rx
        .await
        .map_err(|_| IpcError::from("The file picker closed unexpectedly.".to_string()))?;

    let paths: Vec<String> = picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();

    scan_media(paths).await
}

fn parse_container(raw: &str) -> Result<Container, IpcError> {
    Container::parse(raw).ok_or_else(|| IpcError::from(format!("{raw:?} is not a container Zoltraak writes")))
}

#[tauri::command]
async fn media_capabilities() -> IpcResult<zoltraak_core::conversion::Capabilities> {
    Ok(zoltraak_core::conversion::capabilities().await?)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionPreviewPlan {
    path: String,
    plan: Option<zoltraak_core::conversion::ConversionPlan>,
    error: Option<String>,
}

#[tauri::command]
async fn plan_conversion(
    paths: Vec<String>,
    settings: zoltraak_core::conversion::Settings,
) -> IpcResult<Vec<ConversionPreviewPlan>> {
    settings.validate()?;
    if paths.len() > 500 {
        return Err(IpcError::from(
            "Choose up to 500 files per batch".to_string(),
        ));
    }
    let mut plans = Vec::new();
    for path in paths {
        let result = match zoltraak_core::conversion::inspect_for_plan(std::path::Path::new(&path)).await {
            Ok(info) => settings.plan(&info),
            Err(e) => Err(e),
        };
        let (plan, error) = match result {
            Ok(plan) => (Some(plan), None),
            Err(e) => (None, Some(e.to_string())),
        };
        plans.push(ConversionPreviewPlan { path, plan, error });
    }
    Ok(plans)
}

#[tauri::command]
async fn start_conversions(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    remux: tauri::State<'_, RemuxManager>,
    paths: Vec<String>,
    settings: zoltraak_core::conversion::Settings,
    directory: Option<String>,
    replace_source: Option<bool>,
) -> IpcResult<Vec<String>> {
    if paths.len() > 500 {
        return Err(IpcError::from(
            "Choose up to 500 files per batch".to_string(),
        ));
    }
    let request = NewRemux {
        inputs: paths.into_iter().map(PathBuf::from).collect(),
        container: settings.format.container().unwrap_or(Container::Mp4),
        output_directory: directory
            .map(|d| d.trim().to_string())
            .filter(|d| !d.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| Some(conversion_directory(&app, &sup))),
        replace_source: replace_source.unwrap_or(false),
    };
    Ok(remux
        .enqueue_conversion(request, settings)
        .await?
        .into_iter()
        .map(|id| id.to_string())
        .collect())
}

fn conversion_directory(app: &tauri::AppHandle, sup: &Supervisor) -> PathBuf {
    zoltraak_core::conversion::default_output_directory(
        &sup.settings().default_directory.unwrap_or_else(|| downloads_dir(app)),
    )
}

/// Queue a batch of local files for repackaging.
#[tauri::command]
async fn start_remux(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    remux: tauri::State<'_, RemuxManager>,
    paths: Vec<String>,
    container: String,
    directory: Option<String>,
    replace_source: Option<bool>,
) -> IpcResult<Vec<String>> {
    let container = parse_container(&container)?;
    let directory = directory
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(conversion_directory(&app, &sup)));

    let ids = remux
        .enqueue(NewRemux {
            inputs: paths.into_iter().map(PathBuf::from).collect(),
            container,
            output_directory: directory,
            replace_source: replace_source.unwrap_or(false),
        })
        .await?;
    Ok(ids.iter().map(ToString::to_string).collect())
}

/// Repackage the file a finished row in the register points at.
///
/// The id is resolved here rather than in the window for the same reason
/// `open_download` resolves it here: the three subsystems keep a file's real
/// path in three different places, and a UI that guessed at one of them would
/// be wrong for the other two.
#[tauri::command]
async fn remux_download(
    app: tauri::AppHandle,
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    remux: tauri::State<'_, RemuxManager>,
    id: String,
    container: String,
    directory: Option<String>,
) -> IpcResult<String> {
    let container = parse_container(&container)?;
    let parsed_id = parse_id(&id)?;

    let path = if let Ok(item) = sup.queue().get(parsed_id).await {
        Some(item.path())
    } else if let Some(job) = video.get(parsed_id).await {
        job.path
    } else if let Some(job) = music.get(parsed_id).await {
        job.path
    } else {
        None
    };

    let path = path.ok_or_else(|| IpcError::from(format!("download {id} has no file yet")))?;
    if !path.is_file() {
        return Err(IpcError::from(format!("{} is no longer on disk", path.display())));
    }

    let ids = remux
        .enqueue(NewRemux {
            inputs: vec![path],
            container,
            output_directory: directory.map(|dir|dir.trim().to_string()).filter(|dir|!dir.is_empty()).map(PathBuf::from).or_else(||Some(conversion_directory(&app, &sup))),
            replace_source: false,
        })
        .await?;
    ids.first()
        .map(ToString::to_string)
        .ok_or_else(|| IpcError::from("nothing was queued".to_string()))
}

#[tauri::command]
async fn list_remux_jobs(remux: tauri::State<'_, RemuxManager>) -> IpcResult<Vec<RemuxJob>> {
    Ok(remux.list().await)
}

#[tauri::command]
async fn cancel_remux_job(remux: tauri::State<'_, RemuxManager>, id: String) -> IpcResult<()> {
    Ok(remux.cancel(parse_id(&id)?).await?)
}

#[tauri::command]
async fn remove_remux_job(remux: tauri::State<'_, RemuxManager>, id: String) -> IpcResult<()> {
    Ok(remux.remove(parse_id(&id)?).await?)
}

#[tauri::command]
async fn retry_remux_job(
    remux: tauri::State<'_, RemuxManager>,
    id: String,
    directory: Option<String>,
) -> IpcResult<()> {
    let directory = directory
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    Ok(remux.retry(parse_id(&id)?, directory).await?)
}

#[tauri::command]
async fn clear_remux_done(remux: tauri::State<'_, RemuxManager>) -> IpcResult<usize> {
    Ok(remux.clear_done().await)
}

/// Open a finished conversion's own file, or show it in its folder.
#[tauri::command]
async fn open_remux_output(remux: tauri::State<'_, RemuxManager>, id: String) -> IpcResult<()> {
    let job = remux
        .get(parse_id(&id)?)
        .await
        .ok_or_else(|| IpcError::from(format!("no repackaging job {id}")))?;
    let path = job.output.unwrap_or(job.input);
    let result = if path.is_file() {
        zoltraak_core::shell::reveal(&path)
    } else {
        zoltraak_core::shell::reveal_dir(path.parent().unwrap_or(&path))
    };
    result.map_err(|e| IpcError::from(format!("could not open: {e}")))
}

// ---- Convert: queue, previews and watch rules ------------------------------

#[tauri::command]
fn remux_queue_state(remux: tauri::State<'_, RemuxManager>) -> zoltraak_core::remux::manager::QueueState {
    remux.queue_state()
}

#[tauri::command]
fn set_remux_paused(remux: tauri::State<'_, RemuxManager>, paused: bool) {
    remux.set_paused(paused);
}

#[tauri::command]
fn set_remux_parallelism(remux: tauri::State<'_, RemuxManager>, parallelism: usize) -> IpcResult<()> {
    Ok(remux.set_parallelism(parallelism)?)
}

#[tauri::command]
fn move_remux_job(
    remux: tauri::State<'_, RemuxManager>,
    id: String,
    movement: zoltraak_core::remux::manager::QueueMove,
) -> IpcResult<()> {
    Ok(remux.move_job(parse_id(&id)?, movement)?)
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// One frame of a source as a data URL, for the trim and crop editor.
#[tauri::command]
async fn conversion_thumbnail(path: String, at_seconds: f64, max_width: u32) -> IpcResult<String> {
    let bytes = zoltraak_core::conversion_preview::thumbnail(std::path::Path::new(&path), at_seconds, max_width).await?;
    Ok(data_url("image/jpeg", &bytes))
}

/// Loudness peaks of a source's audio, for the trim editor.
#[tauri::command]
async fn conversion_waveform(path: String, buckets: usize) -> IpcResult<Vec<f32>> {
    Ok(zoltraak_core::conversion_preview::waveform(std::path::Path::new(&path), buckets).await?)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewSample {
    url: String,
    format: zoltraak_core::conversion::Format,
    start_seconds: f64,
    sample_seconds: f64,
    bytes: usize,
    width: Option<u32>,
    height: Option<u32>,
}

/// Render a few seconds with the current recipe. Runs only while nothing else is converting.
#[tauri::command]
async fn conversion_preview(
    remux: tauri::State<'_, RemuxManager>,
    path: String,
    settings: zoltraak_core::conversion::Settings,
    start_seconds: f64,
    length_seconds: f64,
) -> IpcResult<PreviewSample> {
    let sample = remux
        .when_idle(zoltraak_core::conversion_preview::sample(
            std::path::Path::new(&path),
            &settings,
            start_seconds,
            length_seconds,
        ))
        .await?;
    Ok(PreviewSample {
        url: data_url(sample.mime, &sample.bytes),
        format: sample.format,
        start_seconds: sample.start_seconds,
        sample_seconds: sample.sample_seconds,
        bytes: sample.bytes.len(),
        width: sample.width,
        height: sample.height,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchRules {
    rules: Vec<zoltraak_core::conversion_watch::WatchRule>,
    statuses: Vec<zoltraak_core::conversion_watch::WatchStatus>,
}

#[tauri::command]
fn list_watch_rules(watcher: tauri::State<'_, zoltraak_core::conversion_watch::ConversionWatcher>) -> WatchRules {
    WatchRules { rules: watcher.rules(), statuses: watcher.statuses() }
}

#[tauri::command]
fn save_watch_rules(
    watcher: tauri::State<'_, zoltraak_core::conversion_watch::ConversionWatcher>,
    rules: Vec<zoltraak_core::conversion_watch::WatchRule>,
) -> IpcResult<()> {
    Ok(watcher.set_rules(rules)?)
}

// ---- Discord delivery ----------------------------------------------------
//
// Credentials never come back out: `discord_status` reports only whether one
// is stored and its last four characters. Everything that needs the real token
// happens inside the engine.

#[tauri::command]
async fn discord_status(discord: tauri::State<'_, DiscordService>) -> IpcResult<zoltraak_core::DiscordStatus> {
    Ok(discord.status().await?)
}

#[tauri::command]
async fn discord_settings(discord: tauri::State<'_, DiscordService>) -> IpcResult<DiscordSettings> {
    Ok(discord.settings().await?)
}

#[tauri::command]
async fn update_discord_settings(
    discord: tauri::State<'_, DiscordService>,
    settings: DiscordSettings,
) -> IpcResult<DiscordSettings> {
    Ok(discord.update_settings(settings).await?)
}

/// Store a webhook URL after checking it points at a real channel, so a typo
/// is caught here rather than as a failed delivery an hour later.
#[tauri::command]
async fn discord_connect_webhook(
    discord: tauri::State<'_, DiscordService>,
    url: String,
) -> IpcResult<zoltraak_core::discord::Connection> {
    Ok(discord.connect_webhook(&url).await?)
}

#[tauri::command]
async fn discord_connect_bot(
    discord: tauri::State<'_, DiscordService>,
    token: String,
) -> IpcResult<zoltraak_core::discord::Connection> {
    Ok(discord.connect_bot(&token).await?)
}

#[tauri::command]
async fn discord_reconnect(discord: tauri::State<'_, DiscordService>) -> IpcResult<zoltraak_core::discord::Connection> {
    Ok(discord.reconnect().await?)
}

#[tauri::command]
async fn discord_disconnect(discord: tauri::State<'_, DiscordService>) -> IpcResult<()> {
    discord.disconnect().await?;
    Ok(())
}

#[tauri::command]
async fn discord_guilds(discord: tauri::State<'_, DiscordService>) -> IpcResult<Vec<zoltraak_core::discord::Guild>> {
    Ok(discord.guilds().await?)
}

#[tauri::command]
async fn discord_channels(
    discord: tauri::State<'_, DiscordService>,
    guild_id: String,
) -> IpcResult<Vec<zoltraak_core::discord::Channel>> {
    Ok(discord.channels(&guild_id).await?)
}

#[tauri::command]
async fn discord_set_target(
    discord: tauri::State<'_, DiscordService>,
    guild_id: String,
    channel_id: String,
) -> IpcResult<DiscordSettings> {
    Ok(discord.set_target(&guild_id, &channel_id).await?)
}

#[tauri::command]
async fn discord_invite_url(discord: tauri::State<'_, DiscordService>) -> IpcResult<String> {
    Ok(discord.invite_url().await?)
}

/// Rename the bot, or give it an avatar. The avatar arrives as a `data:` URI
/// because that is the shape Discord's API wants it in.
#[tauri::command]
async fn discord_update_bot(
    discord: tauri::State<'_, DiscordService>,
    username: Option<String>,
    avatar: Option<String>,
) -> IpcResult<String> {
    Ok(discord.update_bot(username.as_deref(), avatar.as_deref()).await?)
}

#[tauri::command]
async fn discord_bot_profile(
    discord: tauri::State<'_, DiscordService>,
    request: zoltraak_core::discord::profile::ProfileRequest,
) -> IpcResult<serde_json::Value> {
    Ok(discord.bot_profile(request).await?)
}

#[tauri::command]
async fn discord_bot_diagnostics(discord: tauri::State<'_, DiscordService>) -> IpcResult<serde_json::Value> {
    Ok(discord.bot_diagnostics().await?)
}

#[tauri::command]
async fn discord_test(discord: tauri::State<'_, DiscordService>) -> IpcResult<String> {
    Ok(discord.test().await?)
}

/// Send finished downloads by hand. Takes queue and video job ids rather than
/// paths, so the front end never has to know where a file landed.
#[tauri::command]
async fn discord_send(
    discord: tauri::State<'_, DiscordService>,
    sup: tauri::State<'_, Supervisor>,
    video: tauri::State<'_, VideoManager>,
    music: tauri::State<'_, MusicManager>,
    ids: Vec<String>,
    options: Option<zoltraak_core::discord::DiscordSendOptions>,
) -> IpcResult<usize> {
    let paths = zoltraak_core::discord::resolve_entries(&sup, &video, Some(&music), &ids).await?;
    Ok(discord.send_entries(paths, options.unwrap_or_default()).await?.len())
}


#[tauri::command]
async fn discord_deliveries(discord: tauri::State<'_, DiscordService>) -> IpcResult<Vec<zoltraak_core::Delivery>> {
    Ok(discord.deliveries(50).await?)
}

#[tauri::command]
async fn discord_retry_delivery(discord: tauri::State<'_, DiscordService>, id: String) -> IpcResult<bool> {
    Ok(discord.retry(&id).await?)
}

#[tauri::command]
async fn discord_delete_message(discord: tauri::State<'_, DiscordService>, id: String) -> IpcResult<bool> {
    Ok(discord.delete_message(&id).await?)
}

#[tauri::command]
async fn discord_clear_deliveries(discord: tauri::State<'_, DiscordService>) -> IpcResult<u64> {
    Ok(discord.clear_history().await?)
}

#[tauri::command]
async fn discord_tag_auto_send(
    auto_send: tauri::State<'_, DiscordAutoSend>,
    ids: Vec<String>,
) -> IpcResult<()> {
    let mut set = auto_send.0.write().await;
    for id in ids {
        set.insert(id);
    }
    Ok(())
}

#[tauri::command]
async fn remove_video_job(video: tauri::State<'_, VideoManager>, id: String, delete_file: Option<bool>) -> IpcResult<()> {
    video.remove_with_file(parse_id(&id)?, delete_file.unwrap_or(false)).await?;
    Ok(())
}

/// Stop a running video job at its next resumable boundary. Pausing something
/// that is not running is a no-op, not an error.
#[tauri::command]
async fn pause_video_job(video: tauri::State<'_, VideoManager>, id: String) -> IpcResult<()> {
    video.cancel(parse_id(&id)?).await;
    Ok(())
}

#[tauri::command]
async fn resume_video_job(video: tauri::State<'_, VideoManager>, id: String) -> IpcResult<()> {
    video.resume(parse_id(&id)?).await?;
    Ok(())
}

#[tauri::command]
fn set_extension_theme(
    state: tauri::State<'_, zoltraak_api::ApiState>,
    theme: serde_json::Value,
    guide_theme: serde_json::Value,
) {
    *state.extension_theme.write().expect("theme lock poisoned") = theme;
    *state.guide_theme.write().expect("theme lock poisoned") = guide_theme;
}

#[tauri::command]
fn get_settings(sup: tauri::State<'_, Supervisor>) -> zoltraak_core::EngineSettings {
    sup.settings()
}

fn is_silent_arg(a: &str) -> bool {
    let lower = a.to_ascii_lowercase();
    lower == "--silent"
        || lower == "-silent"
        || lower == "/silent"
        || lower == "--minimized"
        || lower == "-minimized"
        || lower == "/minimized"
        || lower == "--hidden"
        || lower == "--background"
        || lower == "--autostart"
}

fn is_silent_launch() -> bool {
    std::env::args().skip(1).any(|a| is_silent_arg(&a))
}

#[cfg(target_os = "windows")]
fn sync_autostart(enable: bool) {
    if let Ok(exe) = std::env::current_exe() {
        let path_str = exe.to_string_lossy();
        let val = format!("\"{}\" --silent", path_str);
        use std::os::windows::process::CommandExt;
        if enable {
            let mut cmd = std::process::Command::new("reg");
            cmd.creation_flags(0x08000000);
            let _ = cmd
                .args(["add", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", "Zoltraak", "/t", "REG_SZ", "/d", &val, "/f"])
                .spawn();
        } else {
            let mut cmd = std::process::Command::new("reg");
            cmd.creation_flags(0x08000000);
            let _ = cmd
                .args(["delete", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", "Zoltraak", "/f"])
                .spawn();
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn sync_autostart(_enable: bool) {}

/// Persist and apply new settings in one call, so the UI never has a state
/// where the form was saved but the running engine has not caught up.
#[tauri::command]
async fn update_settings(
    sup: tauri::State<'_, Supervisor>,
    settings: zoltraak_core::EngineSettings,
) -> IpcResult<()> {
    if let Some(dir) = &settings.default_directory {
        if !dir.is_absolute() {
            return Err("The default folder must be an absolute path.".to_string().into());
        }
    }
    let prev_start = sup.settings().start_on_launch;
    if prev_start != settings.start_on_launch {
        sync_autostart(settings.start_on_launch);
    }
    sup.update_settings(settings).await?;
    Ok(())
}

fn signed_xpi_for_app(app: &tauri::AppHandle, data_dir: &std::path::Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("zoltraak-signed.xpi"));
    }
    candidates.push(data_dir.join("extension").join("zoltraak-signed.xpi"));
    browser::find_signed_xpi(&candidates)
}

#[tauri::command]
async fn detect_browsers(app: tauri::AppHandle) -> IpcResult<Vec<browser::DetectedBrowser>> {
    let data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_default();
    // Focus refreshes call this command. Process/profile discovery must never
    // occupy the window message thread while a native title-bar drag starts.
    tauri::async_runtime::spawn_blocking(move || browser::scan_browsers(&data_dir))
        .await
        .map_err(|error| IpcError::from(format!("browser discovery failed: {error}")))
}

#[tauri::command]
async fn get_firefox_extension_version(app: tauri::AppHandle) -> IpcResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let data_dir = app.path().app_data_dir().ok()?;
        signed_xpi_for_app(&app, &data_dir).and_then(|path| browser::signed_xpi_version(&path))
    })
    .await
    .map_err(|error| IpcError::from(format!("extension version lookup failed: {error}")))
}

#[tauri::command]
async fn import_firefox_extension(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    api_state: tauri::State<'_, zoltraak_api::ApiState>,
) -> IpcResult<Vec<browser::ExtensionInstallResult>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Import Mozilla-signed Zoltraak extension")
        .add_filter("Firefox extension", &["xpi"])
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    let Some(source) = rx
        .await
        .map_err(|_| IpcError::from("The XPI picker closed unexpectedly.".to_string()))?
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(Vec::new());
    };
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;
    let (version, installed_xpi) = browser::import_signed_xpi(&source, &data_dir)
        .map_err(IpcError::from)?;
    let targets = browser::scan_browsers(&data_dir)
        .into_iter()
        .filter(|browser| browser.kind == "firefox" && browser.installed)
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return Ok(vec![browser::ExtensionInstallResult {
            browser_id: "firefox".into(),
            browser_name: "Firefox / Zen".into(),
            success: true,
            message: format!("Imported signed extension v{version}. Click Install for the browser you want to use."),
            extension_path: installed_xpi.to_string_lossy().into_owned(),
            next_step: None,
        }]);
    }
    api_state.enable_pairing_window(std::time::Duration::from_secs(60));
    let mut results = Vec::new();
    for target in targets {
        results.extend(browser::reload_browser_extension(
            &target.id,
            &data_dir,
            &api_status.token,
            Some(&installed_xpi),
        ));
    }
    Ok(results)
}

#[tauri::command]
async fn install_extension(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    api_state: tauri::State<'_, zoltraak_api::ApiState>,
    browser_id: String,
) -> IpcResult<Vec<browser::ExtensionInstallResult>> {
    api_state.enable_pairing_window(std::time::Duration::from_secs(60));
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;
    let token = &api_status.token;

    // The shippable Zen/Firefox artifact is a separately signed XPI, bundled as
    // a Tauri resource. Look there first, then let `find_signed_xpi` fall back
    // to locations next to the executable. Absent in a dev tree — the Gecko
    // installer then degrades to a temporary side-load with an honest message.
    let signed_xpi = signed_xpi_for_app(&app, &data_dir);

    let scanned = browser::scan_browsers(&data_dir);
    let targets: Vec<browser::DetectedBrowser> = if browser_id == "all" || browser_id == "auto" {
        // If Zoltraak has already set itself up for any detected browser, treat
        // this click as a refresh rather than a first install: target every
        // supported browser we found, not just the ones happening to be
        // running right now, so a stale copy never lingers on the others.
        let already_installed = scanned
            .iter()
            .any(|b| browser::extension_already_present(b, &data_dir));

        if already_installed {
            scanned
        } else {
            let running: Vec<_> = scanned.iter().filter(|b| b.running).cloned().collect();
            if !running.is_empty() {
                running
            } else {
                scanned
            }
        }
    } else {
        scanned.into_iter().filter(|b| b.id == browser_id).collect()
    };

    if targets.is_empty() {
        return Err(IpcError::from("No matching target browser found.".to_string()));
    }

    let mut results = Vec::new();
    for target in &targets {
        let res = browser::install_browser_extension(target, &data_dir, token, signed_xpi.as_deref());
        results.push(res);
    }

    api_state.trigger_extension_reload();
    Ok(results)
}

#[tauri::command]
async fn remove_extension(
    app: tauri::AppHandle,
    browser_id: String,
) -> IpcResult<Vec<browser::ExtensionInstallResult>> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;

    let scanned = browser::scan_browsers(&data_dir);
    let targets: Vec<browser::DetectedBrowser> = if browser_id == "all" || browser_id == "auto" {
        scanned
    } else {
        scanned.into_iter().filter(|b| b.id == browser_id).collect()
    };

    if targets.is_empty() {
        return Err(IpcError::from("No matching target browser found.".to_string()));
    }

    let mut results = Vec::new();
    for target in &targets {
        let res = browser::remove_browser_extension(target, &data_dir);
        results.push(res);
    }

    Ok(results)
}

#[tauri::command]
async fn reload_extension(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    api_state: tauri::State<'_, zoltraak_api::ApiState>,
    browser_id: String,
) -> IpcResult<Vec<browser::ExtensionInstallResult>> {
    api_state.enable_pairing_window(std::time::Duration::from_secs(60));
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;
    let _ = browser::register_native_host(&data_dir);
    let token = &api_status.token;

    let signed_xpi = signed_xpi_for_app(&app, &data_dir);

    let results = browser::reload_browser_extension(&browser_id, &data_dir, token, signed_xpi.as_deref());

    let chromium_updated = results.iter().any(|result| {
        result.success
            && browser::scan_browsers(&data_dir)
                .iter()
                .any(|candidate| candidate.id == result.browser_id && candidate.kind == "chromium")
    });
    if chromium_updated {
        api_state.trigger_extension_reload();
    }
    Ok(results)
}

#[tauri::command]
fn open_extension_manager(app: tauri::AppHandle, browser_id: String) -> IpcResult<()> {
    let directory = app.path().app_data_dir().map_err(|e| IpcError::from(e.to_string()))?;
    browser::open_extension_manager(&browser_id, &directory).map_err(IpcError::from)
}

#[tauri::command]
async fn relaunch_browser(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    api_state: tauri::State<'_, zoltraak_api::ApiState>,
    browser_id: String,
) -> IpcResult<browser::ExtensionInstallResult> {
    api_state.enable_pairing_window(std::time::Duration::from_secs(60));
    api_state.trigger_extension_reload();

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;
    let token = &api_status.token;

    let scanned = browser::scan_browsers(&data_dir);
    let target = scanned
        .into_iter()
        .find(|b| b.id == browser_id)
        .ok_or_else(|| IpcError::from("Browser not found.".to_string()))?;

    let result = browser::relaunch_chromium(&target, &data_dir, token);
    Ok(result)
}

#[tauri::command]
fn get_extension_directory(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    browser_id: String,
) -> IpcResult<String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;
    let path = browser::prepare_chromium_extension_for_id(&browser_id, &data_dir, &api_status.token)
        .map_err(|e| IpcError::from(e.to_string()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn purge_and_regenerate_extensions(
    app: tauri::AppHandle,
    api_status: tauri::State<'_, ApiStatus>,
    api_state: tauri::State<'_, zoltraak_api::ApiState>,
) -> IpcResult<browser::PurgeRegenerateResult> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| IpcError::from(e.to_string()))?;

    api_state.reset_reload_version();
    api_state.enable_pairing_window(std::time::Duration::from_secs(60));

    let result = browser::purge_and_regenerate_extension_files(&data_dir, &api_status.token)
        .map_err(IpcError::from)?;
    Ok(result)
}

async fn push_queue(app: &tauri::AppHandle, sup: &Supervisor) -> zoltraak_core::Result<()> {
    let snapshot = sup.snapshot().await?;
    let _ = app.emit(QUEUE_EVENT, snapshot);
    Ok(())
}

/// Bind and start the local REST API on the same engine the UI drives.
///
/// A bind failure (most often the port already held by a headless `zoltraak
/// serve`, or another Zoltraak window) must not stop the app from launching —
/// the desktop UI works fine with the extension simply unavailable, so this
/// degrades into `ApiStatus::listening = false` instead of propagating an
/// error out of `setup`.
async fn start_api(
    handle: &tauri::AppHandle,
    supervisor: &Supervisor,
    video: &VideoManager,
    music: &MusicManager,
    remux: &RemuxManager,
    discord: &DiscordService,
    auto_send: &DiscordAutoSend,
    _data_dir: &std::path::Path,
) -> (ApiStatus, zoltraak_api::ApiState) {
    let token_res = match zoltraak_api::store::default_path() {
        Some(path) => zoltraak_api::store::load_or_create(&path),
        None => Err(std::io::Error::other("no config directory available")),
    };

    let (token, token_err) = match token_res {
        Ok(token) => (token, None),
        Err(e) => {
            tracing::warn!(error = %e, "could not load or create the API token");
            (zoltraak_api::ApiToken::generate(), Some(e.to_string()))
        }
    };
    let exposed = if token_err.is_none() {
        token.expose().to_string()
    } else {
        String::new()
    };

    // The `/download/prefill` route drops URLs onto this channel; a background task
    // pulls them off, brings the window forward, and hands the URL to the downloads
    // text input.
    let (download_prefill_tx, mut download_prefill_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, zoltraak_core::headers::Headers)>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((url, headers)) = download_prefill_rx.recv().await {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_always_on_top(true);
                    let _ = window.set_focus();
                    let _ = window.set_always_on_top(false);
                }
                let _ = handle.emit(
                    DOWNLOAD_PREFILL_EVENT,
                    VideoPrefillPayload { url, headers, auto_download: false, title: None },
                );
            }
        });
    }

    // The `/video/prefill` route drops URLs onto this channel; a background task
    // pulls them off, brings the window forward (unless auto_download is true,
    // e.g. clicking the extension's floating video overlay), and hands the URL to
    // the Video view. Kept off the request path so a slow window op never blocks
    // the HTTP response the extension is waiting on.
    let (prefill_tx, mut prefill_rx) = tokio::sync::mpsc::unbounded_channel::<(
        String,
        zoltraak_core::headers::Headers,
        bool,
        Option<String>,
    )>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((url, headers, auto_download, title)) = prefill_rx.recv().await {
                if !auto_download {
                    if let Some(window) = handle.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_always_on_top(true);
                        let _ = window.set_focus();
                        let _ = window.set_always_on_top(false);
                    }
                }
                let _ = handle.emit(
                    VIDEO_PREFILL_EVENT,
                    VideoPrefillPayload { url, headers, auto_download, title },
                );
            }
        });
    }

    let (image_prefill_tx, mut image_prefill_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, zoltraak_core::headers::Headers)>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((url, headers)) = image_prefill_rx.recv().await {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_always_on_top(true);
                    let _ = window.set_focus();
                    let _ = window.set_always_on_top(false);
                }
                let _ = handle.emit(
                    IMAGE_PREFILL_EVENT,
                    MusicPrefillPayload { url, headers },
                );
            }
        });
    }

    // The `/music/prefill` route: bring the window forward and hand the link
    // to the Music view, same shape as the video prefill.
    let (music_prefill_tx, mut music_prefill_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, zoltraak_core::headers::Headers)>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((url, headers)) = music_prefill_rx.recv().await {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_always_on_top(true);
                    let _ = window.set_focus();
                    let _ = window.set_always_on_top(false);
                }
                let _ = handle.emit(MUSIC_PREFILL_EVENT, MusicPrefillPayload { url, headers });
            }
        });
    }

    let resolved_default_dir = supervisor
        .settings()
        .default_directory
        .unwrap_or_else(|| downloads_dir(handle));

    let (folder_picker_tx, mut folder_picker_rx) =
        tokio::sync::mpsc::unbounded_channel::<zoltraak_api::FolderPickerRequest>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((start, resp_tx)) = folder_picker_rx.recv().await {
                const DIALOG_TITLE: &str = "Where should this download land?";
                let mut builder = handle.dialog().file().set_title(DIALOG_TITLE);
                if let Some(dir) = start.filter(|d| d.is_absolute() && d.exists()) {
                    builder = builder.set_directory(dir);
                }
                ensure_dialog_topmost(DIALOG_TITLE);
                builder.pick_folder(move |picked| {
                    let path = picked.and_then(|p| p.into_path().ok());
                    let _ = resp_tx.send(path);
                });
            }
        });
    }

    let (app_navigate_tx, mut app_navigate_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, Option<String>)>();
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((section, tab)) = app_navigate_rx.recv().await {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_always_on_top(true);
                    let _ = window.set_focus();
                    let _ = window.set_always_on_top(false);
                }
                let _ = handle.emit(
                    APP_NAVIGATE_EVENT,
                    AppNavigatePayload { section, tab },
                );
            }
        });
    }

    let (reload_version, reload_tx) = zoltraak_api::new_reload_channel();
    let extension_sources = zoltraak_api::store::extension_source_path()
        .map(zoltraak_api::extension_source::ExtensionSourceManager::open)
        .unwrap_or_default();
    let state = zoltraak_api::ApiState {
        extension_theme: Default::default(),
        guide_theme: Default::default(),
        token: std::sync::Arc::new(token),
        supervisor: supervisor.clone(),
        video: video.clone(),
        music: Some(music.clone()),
        music_prefill_tx: Some(music_prefill_tx),
        remux: remux.clone(),
        default_directory: std::sync::Arc::new(resolved_default_dir),
        download_prefill_tx: Some(download_prefill_tx),
        image_prefill_tx: Some(image_prefill_tx),
        prefill_tx: Some(prefill_tx),
        folder_picker_tx: Some(folder_picker_tx),
        discord: Some(discord.clone()),
        auto_send: Some(auto_send.0.clone()),
        // Closed at launch: pairing opens only when the user asks the app to pair.
        pairing_window: std::sync::Arc::new(std::sync::RwLock::new(None)),
        reload_version,
        reload_tx,
        extension_sources,
        extension_source_switch: Default::default(),
        app_navigate_tx: Some(app_navigate_tx),
    };

    if let Some(err) = token_err {
        return (
            ApiStatus {
                listening: false,
                port: 0,
                token: String::new(),
                detail: Some(err),
            },
            state,
        );
    }

    match tokio::net::TcpListener::bind(zoltraak_api::DEFAULT_ADDR).await {
        Ok(listener) => {
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            tracing::info!(port, "local API listening for the extension");
            tokio::spawn(zoltraak_api::serve(listener, state.clone()));
            (
                ApiStatus {
                    listening: true,
                    port,
                    token: exposed,
                    detail: None,
                },
                state,
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "local API port unavailable — a headless `zoltraak serve` may already be running");
            (
                ApiStatus {
                    listening: false,
                    port: zoltraak_api::DEFAULT_ADDR.port(),
                    token: exposed,
                    detail: Some(e.to_string()),
                },
                state,
            )
        }
    }
}

fn downloads_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .download_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Zoltraak")
}

#[tauri::command]
fn get_app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn get_extension_version() -> String {
    browser::extension_version()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A browser launched us as the extension's native-messaging host, not as
    // the desktop app. Serve the token handshake on stdio and exit — this must
    // happen before *anything* writes to stdout (tracing included), since the
    // native-messaging pipe is stdout and stray bytes corrupt the protocol.
    if browser::is_native_host_invocation() {
        browser::run_native_host();
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zoltraak=info".into()),
        )
        .init();

    if std::env::var("ZOLTRAAK_WINDOW_PROBE").as_deref() == Ok("1") {
        window_diagnostics::run_probe();
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let is_silent = args.iter().any(|a| is_silent_arg(a));
            if !is_silent {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            if let Some(window) = app.get_webview_window("main") {
                window_diagnostics::install(&window);
                file_drop::install(&window);
            }

            // The queue lives with the app's data, not next to the binary: an
            // installed app cannot assume its own directory is writable.
            let data_dir = handle.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = data_dir.join("zoltraak.db");

            // Register the native-messaging host now so a Zen/Firefox extension
            // can pair itself the moment it loads, without waiting on the user
            // to open the install card. Idempotent; failure is non-fatal.
            if !browser::register_native_host(&data_dir) {
                tracing::warn!("could not register the extension pairing host");
            }

            // Register directories for media tool discovery (FFmpeg & FFprobe)
            zoltraak_core::media_tools::register_search_dir(data_dir.join("bin"));
            if let Ok(res_dir) = handle.path().resource_dir() {
                zoltraak_core::media_tools::register_search_dir(res_dir.join("resources").join("bin"));
                zoltraak_core::media_tools::register_search_dir(res_dir.join("bin"));
                zoltraak_core::media_tools::register_search_dir(res_dir.join("resources"));
                zoltraak_core::media_tools::register_search_dir(res_dir);
            }

            // Check in the background; downloads and installs remain user-controlled.
            updater::spawn_background_updater(handle.clone());

            let supervisor = tauri::async_runtime::block_on(async {
                let queue = Queue::open(&db).await?;
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::limited(10))
                    .build()
                    .expect("failed to build the HTTP client");

                Ok::<_, zoltraak_core::Error>(Supervisor::start(
                    queue,
                    Downloader::new(client, Options::default()),
                    RunnerConfig::default(),
                ))
            })?;

            tracing::info!(db = %db.display(), "engine started");
            app.manage(supervisor.clone());

            // Shares the same SQLite database as the queue, in its own table
            // — a video job is a different shape of work (an analysis step,
            // a possible merge) and has no byte-range resume, so it does not
            // go through `Supervisor`.
            // Browser sessions the extension shares live next to the API
            // token, encrypted, so a pasted link behind a login keeps working
            // across restarts. Losing the vault is not fatal: the extension
            // re-shares sessions on the next visit.
            let config_dir = zoltraak_api::store::default_path()
                .and_then(|token_path| token_path.parent().map(|dir| dir.to_path_buf()));
            let vault = config_dir
                .as_deref()
                .and_then(|dir| match zoltraak_core::SessionVault::open(dir) {
                    Ok(vault) => Some(vault),
                    Err(e) => {
                        tracing::warn!(error = %e, "session vault unavailable; sessions will not persist");
                        None
                    }
                })
                .unwrap_or_else(zoltraak_core::SessionVault::in_memory);
            // Shared between the video and music managers: a YouTube session
            // the extension shares unlocks the same streams for both.
            let vault = std::sync::Arc::new(vault);

            // Standing credentials — today the Discord bot token or webhook —
            // live beside the session vault under the same protection. An
            // in-memory fallback keeps the app usable; the user is simply
            // asked for the credential again next launch.
            let secrets = std::sync::Arc::new(
                config_dir
                    .as_deref()
                    .and_then(|dir| match zoltraak_core::SecretVault::open(dir) {
                        Ok(vault) => Some(vault),
                        Err(e) => {
                            tracing::warn!(error = %e, "secret vault unavailable; Discord credentials will not persist");
                            None
                        }
                    })
                    .unwrap_or_else(zoltraak_core::SecretVault::in_memory),
            );

            let discord = DiscordService::new(
                reqwest::Client::new(),
                supervisor.queue().clone(),
                std::sync::Arc::clone(&secrets),
            );
            tauri::async_runtime::spawn(discord.clone().run_worker());
            app.manage(discord.clone());

            let auto_send = DiscordAutoSend::default();
            app.manage(auto_send.clone());

            // Auto-send: the queue and the video subsystem each report their
            // own finished files, since a video job never passes through the
            // queue. Both hooks run on the worker that just finished a
            // download, so they only queue a row and return.
            let queue_deliveries = discord.clone();
            let queue_auto_send = auto_send.clone();
            supervisor.on_finished(std::sync::Arc::new(move |id, path| {
                let discord = queue_deliveries.clone();
                let auto_send = queue_auto_send.clone();
                let source_id = id.to_string();
                tauri::async_runtime::spawn(async move {
                    deliver_if_wanted(&discord, &auto_send, DeliverySource::Download, Some(source_id), path).await;
                });
            }));

            let video_deliveries = discord.clone();
            let video_auto_send = auto_send.clone();
            let video = tauri::async_runtime::block_on(async {
                let video = VideoManager::new(reqwest::Client::new(), supervisor.queue().pool())
                    .with_rate_limiter(supervisor.rate_limiter())
                    .with_session_vault(std::sync::Arc::clone(&vault))
                    .with_completion_hook(std::sync::Arc::new(move |id, path| {
                        let discord = video_deliveries.clone();
                        let auto_send = video_auto_send.clone();
                        let source_id = id.to_string();
                        tauri::async_runtime::spawn(async move {
                            deliver_if_wanted(&discord, &auto_send, DeliverySource::Video, Some(source_id), path).await;
                        });
                    }));
                video.recover().await?;
                Ok::<_, zoltraak_core::Error>(video)
            })?;
            app.manage(video.clone());

            // Music jobs: a third subsystem beside the queue and video jobs,
            // with its own table and worker. It shares the session vault
            // (YouTube streams need it) and the bandwidth cap.
            let music_deliveries = discord.clone();
            let music_auto_send = auto_send.clone();
            let music = tauri::async_runtime::block_on(async {
                let music = MusicManager::new(reqwest::Client::new(), supervisor.queue().pool())
                    .with_rate_limiter(supervisor.rate_limiter())
                    .with_session_vault(std::sync::Arc::clone(&vault))
                    .with_completion_hook(std::sync::Arc::new(move |id, path| {
                        let discord = music_deliveries.clone();
                        let auto_send = music_auto_send.clone();
                        let source_id = id.to_string();
                        tauri::async_runtime::spawn(async move {
                            deliver_if_wanted(&discord, &auto_send, DeliverySource::Music, Some(source_id), path).await;
                        });
                    }));
                music.recover().await?;
                Ok::<_, zoltraak_core::Error>(music)
            })?;
            app.manage(music.clone());

            // Conversion recipes and history share the engine database; unfinished
            // jobs recover as interrupted and can be retried from the beginning.
            let remux = tauri::async_runtime::block_on(RemuxManager::with_history(supervisor.queue().pool()))?;
            app.manage(remux.clone());

            // Folder rules that convert new files; they only act on files that arrive after launch.
            let watch_file = app.path().app_data_dir().map(|dir| dir.join("conversion-watch.json"));
            let watcher = match watch_file {
                Ok(path) => {
                    let _ = path.parent().map(std::fs::create_dir_all);
                    zoltraak_core::conversion_watch::ConversionWatcher::load(path)
                }
                Err(_) => zoltraak_core::conversion_watch::ConversionWatcher::in_memory(),
            };
            app.manage(watcher.clone());
            tauri::async_runtime::spawn({
                let remux = remux.clone();
                async move { watcher.spawn(remux) }
            });

            // Host the local REST API here too, so the extension works without
            // a terminal open — the app is the only long-lived process a
            // desktop user actually has running. Requests that name no
            // directory land in the same folder the in-app composer defaults
            // to, not the app's internal data directory where the DB lives.
            let default_download_dir = downloads_dir(&handle);
            let (api_status, api_state) = tauri::async_runtime::block_on(start_api(
                &handle,
                &supervisor,
                &video,
                &music,
                &remux,
                &discord,
                &auto_send,
                &default_download_dir,
            ));
            app.manage(api_status);
            app.manage(api_state);

            // One emitter for all windows, rather than each window polling.
            tauri::async_runtime::spawn(queue_push::run(handle.clone(), supervisor.clone(), QUEUE_EVENT));

            // Sync autostart on launch
            if supervisor.settings().start_on_launch {
                sync_autostart(true);
            }

            // System tray icon setup
            let show_i = MenuItem::with_id(app, "show", "Show Zoltraak", true, None::<&str>)?;
            let folder_i = MenuItem::with_id(app, "folder", "Open Downloads Folder", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit Zoltraak", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_i, &folder_i, &quit_i])?;

            let mut tray_builder = TrayIconBuilder::with_id("main")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .tooltip("Zoltraak")
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                        "folder" => {
                            if let Some(sup) = app.try_state::<Supervisor>() {
                                let dir = sup.settings().default_directory.unwrap_or_else(|| downloads_dir(app));
                                let _ = zoltraak_core::shell::reveal_dir(&dir);
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let _ = tray_builder.build(app)?;
            app_icon::restore(app.handle());

            // Show the main window on regular launch; keep hidden if launched silently to tray on boot.
            if !is_silent_launch() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Every way back to the window (tray, second launch, extension
            // hand-off) ends in `set_focus`, so focus is where power comes back.
            if let tauri::WindowEvent::Focused(true) = event {
                if let Some(webview) = window.get_webview_window(window.label()) {
                    webview_power::shown(&webview);
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if let Some(sup) = window.try_state::<Supervisor>() {
                    if sup.settings().close_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                        if let Some(webview) = window.get_webview_window(window.label()) {
                            webview_power::hidden(&webview);
                        }
                        return;
                    }
                }
            }
            if let tauri::WindowEvent::Resized(_) = event {
                if let Ok(is_min) = window.is_minimized() {
                    if is_min {
                        if let Some(sup) = window.try_state::<Supervisor>() {
                            if sup.settings().minimize_to_tray {
                                let _ = window.hide();
                                if let Some(webview) = window.get_webview_window(window.label()) {
                                    webview_power::hidden(&webview);
                                }
                            }
                        }
                    }
                }
            }
            if let tauri::WindowEvent::Destroyed = event {
                // Stop cleanly so in-flight downloads flush their resume state
                // and come back as `waiting` on the next launch.
                if let Some(sup) = window.try_state::<Supervisor>() {
                    let sup = sup.inner().clone();
                    tauri::async_runtime::block_on(sup.shutdown());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_icon::sync_app_icon,
            set_extension_theme,
            window_corners::set_window_corners,
            engine_ready,
            planned_connections,
            default_directory,
            choose_directory,
            queue_snapshot,
            api_status,
            add_download,
            pause_download,
            resume_download,
            remove_download,
            reveal_download,
            download_file_status,
            download_file_statuses,
            download_thumbnail,
            locate_download_file,
            redownload_missing,
            destination_health,
            open_download,
            drag_download_path,
            reveal_folder,
            list_categories,
            add_category,
            set_category_enabled,
            remove_category,
            get_settings,
            update_settings,
            analyze_video,
            analyze_images,
            inspect_download_link,
            start_video_download,
            list_video_jobs,
            remove_video_job,
            pause_video_job,
            resume_video_job,
            music_directory,
            analyze_music,
            start_music_download,
            list_music_jobs,
            remove_music_job,
            pause_music_job,
            resume_music_job,
            remove_music_collection,
            retry_music_collection,
            clear_music_done,
            music_settings,
            update_music_settings,
            music_tools,
            install_ffmpeg,
            updater::check_for_updates,
            updater::download_update,
            updater::update_progress,
            updater::install_on_next_start,
            updater::install_and_restart,
            get_app_version,
            get_extension_version,
            open_extension_manager,
            get_firefox_extension_version,
            scan_media,
            choose_media_files,
            start_remux,
            media_capabilities,
            plan_conversion,
            start_conversions,
            remux_download,
            list_remux_jobs,
            cancel_remux_job,
            remove_remux_job,
            retry_remux_job,
            clear_remux_done,
            open_remux_output,
            remux_queue_state,
            set_remux_paused,
            set_remux_parallelism,
            move_remux_job,
            conversion_thumbnail,
            conversion_waveform,
            conversion_preview,
            list_watch_rules,
            save_watch_rules,
            list_sessions,
            forget_session,
            forget_all_sessions,
            extension_source_state,
            select_extension_source,
            discord_status,
            discord_settings,
            update_discord_settings,
            discord_connect_webhook,
            discord_connect_bot,
            discord_disconnect,
            discord_reconnect,
            discord_guilds,
            discord_channels,
            discord_set_target,
            discord_invite_url,
            discord_update_bot,
            discord_bot_profile,
            discord_bot_diagnostics,
            discord_test,
            discord_send,
            discord_deliveries,
            discord_retry_delivery,
            discord_clear_deliveries,
            discord_delete_message,
            discord_tag_auto_send,
            detect_browsers,
            import_firefox_extension,
            install_extension,
            remove_extension,
            reload_extension,
            relaunch_browser,
            get_extension_directory,
            purge_and_regenerate_extensions,
            open_guide,
            open_legal,
            open_url,
            open_discord_message,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Zoltraak")
        .run(|_app, _event| {});
}
