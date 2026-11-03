//! User-controlled updater for Zoltraak.
//!
//! Background work only checks for releases. Downloading and installation each
//! require an explicit choice from the user.

use crate::updater_cache::{self, CachedRelease};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const EVENT_UPDATE_AVAILABLE: &str = "update-available";
pub const EVENT_UPDATE_PROGRESS: &str = "update-progress";
pub const EVENT_UPDATE_READY: &str = "update-ready";
pub const EVENT_UPDATE_ERROR: &str = "update-error";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub body: Option<String>,
    pub date: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub phase: String, // "downloading" | "ready" | "error"
    pub percent: u8,
    pub downloaded: u64,
    pub total: u64,
    pub version: String,
    pub message: Option<String>,
}

fn download_percent(downloaded: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 99.0) as u8
    }
}

fn scheduled_version_matches(approved: &str, available: &str) -> bool {
    approved.trim() == available
}

static UPDATE_PROGRESS: OnceLock<Mutex<Option<UpdateProgress>>> = OnceLock::new();
static PENDING_UPDATE: OnceLock<Mutex<Option<(Update, Vec<u8>)>>> = OnceLock::new();
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

const INSTALL_ON_NEXT_START_FILE: &str = "install-update-on-next-start";

struct DownloadGuard;

impl DownloadGuard {
    fn acquire() -> Result<Self, String> {
        DOWNLOADING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "An update is already downloading".to_string())
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::Release);
    }
}

fn install_on_next_start_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(INSTALL_ON_NEXT_START_FILE))
        .map_err(|error| error.to_string())
}

fn cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("updates"))
        .map_err(|e| e.to_string())
}

fn updater_pubkey(app: &AppHandle) -> Result<String, String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "Updater public key is unavailable".into())
}

fn cached_release(app: &AppHandle) -> Result<Option<CachedRelease>, String> {
    let dir = cache_path(app)?;
    let Some(release) = updater_cache::metadata(&dir)? else {
        updater_cache::clear(&dir)?; // Uncommitted partial files are disposable.
        return Ok(None);
    };
    if updater_cache::is_installed(&app.package_info().version.to_string(), &release.version)? {
        updater_cache::clear(&dir)?;
        let marker = install_on_next_start_path(app)?;
        if fs::read_to_string(&marker).is_ok_and(|v| {
            updater_cache::is_installed(&app.package_info().version.to_string(), v.trim())
                .unwrap_or(false)
        }) {
            let _ = fs::remove_file(marker);
        }
        return Ok(None);
    }
    Ok(Some(release))
}

fn publish_ready(app: &AppHandle, version: &str, size: u64) {
    publish_progress(
        app,
        UpdateProgress {
            phase: "ready".into(),
            percent: 100,
            downloaded: size,
            total: size,
            version: version.into(),
            message: None,
        },
    );
    let _ = app.emit(
        EVENT_UPDATE_READY,
        serde_json::json!({ "version": version }),
    );
}

/// Rebuild the plugin's platform installation context with a metadata-only
/// check. The signed package itself always comes from disk, even if a newer
/// release has appeared since the user approved this one.
async fn restore_cached_update(app: &AppHandle) -> Result<bool, String> {
    if PENDING_UPDATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Pending update state is unavailable")?
        .is_some()
    {
        return Ok(true);
    }
    let Some(release) = cached_release(app)? else {
        return Ok(false);
    };
    let bytes = match updater_cache::load(&cache_path(app)?, &release, &updater_pubkey(app)?) {
        Ok(bytes) => bytes,
        Err(error) => {
            updater_cache::clear(&cache_path(app)?)?;
            let marker = install_on_next_start_path(app)?;
            if fs::read_to_string(&marker)
                .is_ok_and(|v| scheduled_version_matches(&v, &release.version))
            {
                let _ = fs::remove_file(marker);
            }
            return Err(format!(
                "{error}. Invalid cached update removed; download the update again from Settings."
            ));
        }
    };
    let mut update = app
        .updater_builder()
        .version_comparator(|_, _| true)
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Cached update retained; could not check installation metadata: {e}"))?
        .ok_or_else(|| {
            "Cached update retained; installation metadata is unavailable".to_string()
        })?;
    if update.target != release.target {
        return Err("Cached update belongs to a different platform".into());
    }
    update.version = release.version.clone();
    update.signature = release.signature;
    update.download_url = release
        .download_url
        .parse()
        .map_err(|e| format!("Invalid cached update URL: {e}"))?;
    update.body = release.body;
    *PENDING_UPDATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Pending update state is unavailable")? = Some((update, bytes));
    publish_ready(app, &release.version, release.size);
    Ok(true)
}

fn emit_available(app: &AppHandle, update: &Update) {
    let _ = app.emit(
        EVENT_UPDATE_AVAILABLE,
        serde_json::json!({
            "version": update.version.clone(),
            "body": update.body.clone()
        }),
    );
}

fn publish_progress(app: &AppHandle, progress: UpdateProgress) {
    if let Ok(mut current) = UPDATE_PROGRESS.get_or_init(Default::default).lock() {
        *current = Some(progress.clone());
    }
    let _ = app.emit(EVENT_UPDATE_PROGRESS, progress);
}

/// Return the last updater phase so Settings can recover state even when the
/// background worker finished before that view was mounted.
#[tauri::command]
pub fn update_progress() -> Option<UpdateProgress> {
    UPDATE_PROGRESS
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|progress| progress.clone())
}

/// Spawn the background update checker.
///
/// Waits 6 seconds after launch to ensure initial database opening and UI
/// rendering are completely settled before issuing network requests.
pub fn spawn_background_updater(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Also runs on the newly installed app's first launch. Windows exits
        // inside the plugin installer, so cleanup cannot depend on its return.
        if let Ok(_guard) = DownloadGuard::acquire() {
            if let Err(e) = cached_release(&app) {
                tracing::warn!(error = %e, "Update cache cleanup failed");
            }
        }
        // Initial delay
        tokio::time::sleep(Duration::from_secs(6)).await;

        if install_on_next_start_path(&app).is_ok_and(|path| path.exists()) {
            if let Err(e) = install_scheduled_update(&app).await {
                tracing::warn!(error = %e, "Scheduled update installation failed");
                let _ = app.emit(
                    EVENT_UPDATE_ERROR,
                    serde_json::json!({ "version": "", "error": e }),
                );
            }
        } else if let Err(e) = check_and_notify(&app).await {
            tracing::debug!(error = %e, "Background update check completed with notice");
        }

        loop {
            // Check again every 4 hours. Checks never start a download.
            tokio::time::sleep(Duration::from_secs(4 * 3600)).await;
            if let Err(e) = check_and_notify(&app).await {
                tracing::debug!(error = %e, "Background update check completed with notice");
            }
        }
    });
}

/// Check for a release and notify the frontend without downloading it.
async fn check_and_notify(app: &AppHandle) -> Result<bool, String> {
    let _guard = DownloadGuard::acquire()?;
    if restore_cached_update(app).await? {
        return Ok(false);
    }

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return Err(format!("Updater plugin unavailable: {e}")),
    };

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(false),
        Err(e) => return Err(format!("Update check failed: {e}")),
    };

    tracing::info!(version = %update.version, "New version found; waiting for download approval");
    emit_available(app, &update);
    Ok(true)
}

async fn download_checked_update(app: &AppHandle, update: Update) -> Result<bool, String> {
    if PENDING_UPDATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Pending update state is unavailable".to_string())?
        .is_some()
    {
        return Ok(false);
    }

    let target_version = update.version.clone();
    tracing::info!(version = %target_version, "Downloading user-approved update");

    publish_progress(
        app,
        UpdateProgress {
            phase: "downloading".into(),
            percent: 0,
            downloaded: 0,
            total: 0,
            version: target_version.clone(),
            message: None,
        },
    );

    let mut downloaded = 0u64;
    let ver_clone = target_version.clone();
    let app_handle = app.clone();

    let download_res = update
        .download(
            move |chunk_length, content_length| {
                downloaded += chunk_length as u64;
                let total = content_length.unwrap_or(0);
                let pct = download_percent(downloaded, total);
                publish_progress(
                    &app_handle,
                    UpdateProgress {
                        phase: "downloading".into(),
                        percent: pct,
                        downloaded,
                        total,
                        version: ver_clone.clone(),
                        message: None,
                    },
                );
            },
            || {},
        )
        .await;

    match download_res {
        Ok(bytes) => {
            let saved = updater_cache::save(
                &cache_path(app)?,
                &CachedRelease {
                    version: update.version.clone(),
                    target: update.target.clone(),
                    signature: update.signature.clone(),
                    download_url: update.download_url.to_string(),
                    body: update.body.clone(),
                    size: bytes.len() as u64,
                },
                &bytes,
            );
            if let Err(error) = saved {
                publish_progress(
                    app,
                    UpdateProgress {
                        phase: "error".into(),
                        percent: 0,
                        downloaded: 0,
                        total: 0,
                        version: target_version.clone(),
                        message: Some(format!("Could not cache the update: {error}")),
                    },
                );
                return Err(error);
            }
            let size = bytes.len() as u64;
            *PENDING_UPDATE
                .get_or_init(Default::default)
                .lock()
                .map_err(|_| "Pending update state is unavailable".to_string())? =
                Some((update, bytes));
            tracing::info!(version = %target_version, "Update downloaded successfully. Waiting for installation approval.");
            publish_ready(app, &target_version, size);
            Ok(true)
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to download update in background");
            publish_progress(
                app,
                UpdateProgress {
                    phase: "error".into(),
                    percent: 0,
                    downloaded: 0,
                    total: 0,
                    version: target_version.clone(),
                    message: Some(e.to_string()),
                },
            );
            let _ = app.emit(
                EVENT_UPDATE_ERROR,
                serde_json::json!({
                    "version": target_version,
                    "error": e.to_string()
                }),
            );
            Err(e.to_string())
        }
    }
}

/// Download the latest release only after the user approves it.
#[tauri::command]
pub async fn download_update(app: AppHandle) -> Result<bool, String> {
    let _guard = DownloadGuard::acquire()?;
    if restore_cached_update(&app).await? {
        return Ok(false);
    }
    let updater = app
        .updater()
        .map_err(|error| format!("Updater plugin unavailable: {error}"))?;
    let update = updater
        .check()
        .await
        .map_err(|error| format!("Update check failed: {error}"))?
        .ok_or_else(|| "No update is currently available".to_string())?;

    download_checked_update(&app, update).await
}

/// Manual check triggered from the Settings page.
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    let current_ver = app.package_info().version.to_string();
    let _guard = DownloadGuard::acquire()?;
    if restore_cached_update(&app).await? {
        let pending = PENDING_UPDATE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Pending update state is unavailable")?;
        let (update, _) = pending.as_ref().ok_or("Pending update is unavailable")?;
        return Ok(UpdateInfo {
            available: true,
            current_version: current_ver,
            latest_version: Some(update.version.clone()),
            body: update.body.clone(),
            date: None,
            error: None,
        });
    }

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            return Ok(UpdateInfo {
                available: false,
                current_version: current_ver,
                latest_version: None,
                body: None,
                date: None,
                error: Some(e.to_string()),
            });
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let target_version = update.version.clone();
            let body = update.body.clone();
            let date = update.date.map(|d| d.to_string());
            emit_available(&app, &update);

            Ok(UpdateInfo {
                available: true,
                current_version: current_ver,
                latest_version: Some(target_version),
                body,
                date,
                error: None,
            })
        }
        Ok(None) => Ok(UpdateInfo {
            available: false,
            current_version: current_ver,
            latest_version: None,
            body: None,
            date: None,
            error: None,
        }),
        Err(e) => Ok(UpdateInfo {
            available: false,
            current_version: current_ver,
            latest_version: None,
            body: None,
            date: None,
            error: Some(e.to_string()),
        }),
    }
}

/// Remember the user's approval to install this release on the next launch.
#[tauri::command]
pub fn install_on_next_start(app: AppHandle) -> Result<(), String> {
    let pending = PENDING_UPDATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Pending update state is unavailable".to_string())?;
    let (update, bytes) = pending
        .as_ref()
        .ok_or_else(|| "No downloaded update is ready to install".to_string())?;
    // Persist before confirming the schedule, even if a cache cleaner removed
    // the package while the app was running.
    updater_cache::save(
        &cache_path(&app)?,
        &CachedRelease {
            version: update.version.clone(),
            target: update.target.clone(),
            signature: update.signature.clone(),
            download_url: update.download_url.to_string(),
            body: update.body.clone(),
            size: bytes.len() as u64,
        },
        bytes,
    )?;
    let version = update.version.clone();

    let path = install_on_next_start_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, version).map_err(|error| error.to_string())
}

async fn install_scheduled_update(app: &AppHandle) -> Result<(), String> {
    let _guard = DownloadGuard::acquire()?;
    let marker = install_on_next_start_path(app)?;
    let approved_version = fs::read_to_string(&marker).map_err(|error| error.to_string())?;
    if updater_cache::is_installed(
        &app.package_info().version.to_string(),
        approved_version.trim(),
    )? {
        let _ = fs::remove_file(marker);
        return Ok(());
    }
    let release = cached_release(app)?
        .ok_or("Scheduled installer is missing from cache. Download it again from Settings.")?;
    if !scheduled_version_matches(&approved_version, &release.version) {
        return Err("Cached update does not match the approved version".into());
    }
    restore_cached_update(app).await?;
    install_pending(app)
}

fn install_pending(app: &AppHandle) -> Result<(), String> {
    let marker = install_on_next_start_path(app)?;
    let pubkey = updater_pubkey(app)?;
    let scheduled_version = fs::read_to_string(&marker).ok();

    let pending = PENDING_UPDATE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Pending update state is unavailable".to_string())?
        .take()
        .ok_or_else(|| "No downloaded update is ready to install".to_string())?;

    tracing::info!("Installing user-approved update...");
    // install() does not verify signatures itself. Verify again immediately
    // before installation, including for cached packages loaded after restart.
    if let Err(error) = updater_cache::verify(&pending.1, &pending.0.signature, &pubkey) {
        *PENDING_UPDATE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Pending update state is unavailable")? = Some(pending);
        return Err(error);
    }
    if let Err(error) = pending.0.install(&pending.1) {
        *PENDING_UPDATE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Pending update state is unavailable".to_string())? = Some(pending);
        if let Some(version) = scheduled_version {
            let _ = fs::write(marker, version);
        }
        return Err(error.to_string());
    }

    if let Err(error) = cache_path(app).and_then(|dir| updater_cache::clear(&dir)) {
        tracing::warn!(error = %error, "Update applied; cache cleanup will retry on next launch");
    }
    let _ = fs::remove_file(marker);

    // Windows exits from `install`; macOS and Linux need an explicit relaunch.
    app.restart();
}

/// Install the downloaded update only after the user approves, then relaunch.
#[tauri::command]
pub async fn install_and_restart(app: tauri::AppHandle) -> Result<(), String> {
    let _guard = DownloadGuard::acquire()?;
    install_pending(&app)
}

#[cfg(test)]
mod tests {
    use super::{download_percent, scheduled_version_matches};

    #[test]
    fn download_percentage_is_indeterminate_without_a_total_and_waits_at_99() {
        assert_eq!(download_percent(50, 0), 0);
        assert_eq!(download_percent(50, 100), 50);
        assert_eq!(download_percent(100, 100), 99);
    }

    #[test]
    fn only_the_explicitly_approved_version_installs_on_next_start() {
        assert!(scheduled_version_matches("1.2.3", "1.2.3"));
        assert!(scheduled_version_matches("1.2.3\n", "1.2.3"));
        assert!(!scheduled_version_matches("1.2.3", "1.2.4"));
    }
}
