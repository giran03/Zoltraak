//! Automatic download and installation of portable FFmpeg and FFprobe binaries.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use tauri::{AppHandle, Emitter, Manager};

pub const FFMPEG_EVENT: &str = "ffmpeg-install-progress";
static INSTALL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaPackage {
    url: String,
    archive_sha256: String,
    directory: String,
    files: Vec<MediaFile>,
}
#[derive(serde::Deserialize)]
struct MediaFile {
    name: String,
    sha256: String,
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    let mut file = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegProgress {
    pub phase: String, // "downloading" | "extracting" | "completed" | "error"
    pub percent: u8,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicToolsResult {
    pub ffmpeg: Option<String>,
    pub ffprobe: bool,
}

pub async fn download_and_install_ffmpeg(app: &AppHandle) -> Result<MusicToolsResult, String> {
    let _install = INSTALL_LOCK.lock().await;
    let package: MediaPackage = serde_json::from_str(
        include_str!("../../../scripts/media_tools.lock.json").trim_start_matches('\u{feff}'),
    )
    .map_err(|e| format!("Invalid bundled media package descriptor: {e}"))?;
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {e}"))?;

    let bin_dir = app_data_dir.join("bin");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("Failed to create bin directory: {e}"))?;

    let temp_zip = app_data_dir.join("ffmpeg-install.tmp.zip");

    let _ = app.emit(
        FFMPEG_EVENT,
        FfmpegProgress {
            phase: "downloading".into(),
            percent: 0,
            message: "Connecting to FFmpeg download mirror...".into(),
        },
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let urls = [&package.url];

    let mut response_opt = None;
    for url in urls {
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                response_opt = Some(resp);
                break;
            }
            Ok(resp) => {
                tracing::warn!(url, status = %resp.status(), "FFmpeg download mirror responded with non-200");
            }
            Err(e) => {
                tracing::warn!(url, error = %e, "Failed to reach FFmpeg download mirror");
            }
        }
    }

    let mut response = response_opt.ok_or_else(|| {
        "Could not connect to any FFmpeg download mirrors. Check internet connection.".to_string()
    })?;

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut last_reported_pct: u8 = 0;

    let mut file =
        File::create(&temp_zip).map_err(|e| format!("Failed to create temporary archive: {e}"))?;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Download stream error: {e}"))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("Disk write error: {e}"))?;
        downloaded += chunk.len() as u64;

        if total_size > 0 {
            let pct = ((downloaded as f64 / total_size as f64) * 100.0).clamp(0.0, 99.0) as u8;
            if pct > last_reported_pct {
                last_reported_pct = pct;
                let mb_downloaded = downloaded as f64 / (1024.0 * 1024.0);
                let mb_total = total_size as f64 / (1024.0 * 1024.0);
                let _ = app.emit(
                    FFMPEG_EVENT,
                    FfmpegProgress {
                        phase: "downloading".into(),
                        percent: pct,
                        message: format!(
                            "Downloading FFmpeg: {:.1} MB / {:.1} MB ({pct}%)",
                            mb_downloaded, mb_total
                        ),
                    },
                );
            }
        }
    }

    // Flush and drop file handle before opening for unzip
    drop(file);

    let _ = app.emit(
        FFMPEG_EVENT,
        FfmpegProgress {
            phase: "extracting".into(),
            percent: 100,
            message: "Extracting ffmpeg.exe and ffprobe.exe...".into(),
        },
    );

    let temp_zip_clone = temp_zip.clone();
    let bin_dir_clone = bin_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let zip_file = File::open(&temp_zip_clone)
            .map_err(|e| format!("Failed to open downloaded archive: {e}"))?;
        let mut archive = zip::ZipArchive::new(BufReader::new(zip_file))
            .map_err(|e| format!("Invalid zip archive: {e}"))?;

        if sha256_file(&temp_zip_clone)? != package.archive_sha256 {
            return Err(
                "Media archive checksum mismatch; existing tools were not replaced.".into(),
            );
        }
        let staging = bin_dir_clone.join(".staging");
        fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        // Verify the entire payload before installing any executable. Archive paths
        // come from our pinned descriptor, never from untrusted archive names.
        for file in &package.files {
            let source = if file.name.ends_with(".exe") || file.name.ends_with(".dll") {
                format!("{}/bin/{}", package.directory, file.name)
            } else {
                format!("{}/{}", package.directory, file.name)
            };
            let mut entry = archive
                .by_name(&source)
                .map_err(|e| format!("Missing media resource {}: {e}", file.name))?;
            let outpath = staging.join(&file.name);
            let mut outfile = File::create(&outpath).map_err(|e| e.to_string())?;
            io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
            drop(outfile);
            if sha256_file(&outpath)? != file.sha256 {
                return Err(format!("Media resource checksum mismatch: {}", file.name));
            }
        }
        // Libraries first, executables last, so new tools never start without their DLLs.
        for executable in [false, true] {
            for file in package
                .files
                .iter()
                .filter(|file| file.name.ends_with(".exe") == executable)
            {
                fs::copy(staging.join(&file.name), bin_dir_clone.join(&file.name))
                    .map_err(|e| e.to_string())?;
            }
        }
        for file in &package.files {
            let _ = fs::remove_file(staging.join(&file.name));
        }
        let _ = fs::remove_dir(&staging);
        drop(archive);
        let _ = fs::remove_file(&temp_zip_clone);
        Ok(())
    })
    .await
    .map_err(|e| format!("Extraction worker thread failed: {e}"))??;

    // Register bin_dir into media_tools search list
    zoltraak_core::media_tools::register_search_dir(bin_dir);

    // Verify newly active tools
    let ffmpeg_ver = zoltraak_core::music::convert::ffmpeg_version().await;
    let ffprobe_avail = zoltraak_core::music::convert::ffprobe_available().await;
    if ffmpeg_ver.is_none() || !ffprobe_avail {
        return Err("Media tools were extracted but could not run; check the installation and shared libraries.".into());
    }

    let _ = app.emit(
        FFMPEG_EVENT,
        FfmpegProgress {
            phase: "completed".into(),
            percent: 100,
            message: format!(
                "FFmpeg {} ready.",
                ffmpeg_ver.as_deref().unwrap_or("binaries are")
            ),
        },
    );

    Ok(MusicToolsResult {
        ffmpeg: ffmpeg_ver,
        ffprobe: ffprobe_avail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_media_package_and_streaming_checksum() {
        let package: MediaPackage = serde_json::from_str(
            include_str!("../../../scripts/media_tools.lock.json").trim_start_matches('\u{feff}'),
        ).unwrap();
        assert!(package.files.iter().any(|file| file.name == "ffmpeg.exe"));
        assert!(package.files.iter().any(|file| file.name == "ffprobe.exe"));
        assert_eq!(package.files.iter().filter(|file| file.name.ends_with(".dll")).count(), 7);
        assert!(package.files.iter().all(|file| !file.name.contains(['/', '\\']) && file.sha256.len() == 64));
        let path = std::env::temp_dir().join(format!("zoltraak-sha256-{}.txt", std::process::id()));
        fs::write(&path, b"abc").unwrap();
        let result = sha256_file(&path);
        fs::remove_file(path).unwrap();
        assert_eq!(result.unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
