//! Theme icons are data files; never rewrite the signed executable's resources.
use image::{
    RgbaImage,
    codecs::ico::{IcoEncoder, IcoFrame},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, sync::Mutex};
use tauri::Manager;

static SYNC: Mutex<()> = Mutex::new(());
const EMBLEM: &[u8] = include_bytes!("../icons/emblem-source.png");
const DEFAULT_FOREGROUND: &str = "#56a2f0";

#[derive(Clone, Serialize, Deserialize)]
struct Palette {
    foreground: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IconResult {
    shortcuts_updated: usize,
    warning: Option<String>,
}

fn color(value: &str) -> Result<[u8; 3], String> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err("Icon colors must use #RRGGBB format".into());
    }
    Ok([
        u8::from_str_radix(&value[1..3], 16).unwrap(),
        u8::from_str_radix(&value[3..5], 16).unwrap(),
        u8::from_str_radix(&value[5..7], 16).unwrap(),
    ])
}
fn source_mask() -> Result<&'static RgbaImage, String> {
    static SOURCE: std::sync::OnceLock<Result<RgbaImage, String>> = std::sync::OnceLock::new();
    SOURCE
        .get_or_init(|| {
            image::load_from_memory(EMBLEM)
                .map(|image| {
                    let image = image.into_rgba8();
                    let mut bounds = (image.width(), image.height(), 0, 0);
                    for (x, y, pixel) in image.enumerate_pixels() {
                        if pixel.0[3] > 0 {
                            bounds.0 = bounds.0.min(x);
                            bounds.1 = bounds.1.min(y);
                            bounds.2 = bounds.2.max(x);
                            bounds.3 = bounds.3.max(y);
                        }
                    }
                    let cropped = image::imageops::crop_imm(
                        &image,
                        bounds.0,
                        bounds.1,
                        bounds.2 - bounds.0 + 1,
                        bounds.3 - bounds.1 + 1,
                    )
                    .to_image();
                    image::imageops::resize(
                        &cropped,
                        512,
                        512,
                        image::imageops::FilterType::Lanczos3,
                    )
                })
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}
fn render(size: u32, fg: [u8; 3]) -> Result<RgbaImage, String> {
    static MASKS: Mutex<std::collections::BTreeMap<u32, RgbaImage>> =
        Mutex::new(std::collections::BTreeMap::new());
    let source = source_mask()?;
    let mut masks = MASKS.lock().map_err(|e| e.to_string())?;
    let mut symbol = masks
        .entry(size)
        .or_insert_with(|| {
            image::imageops::resize(source, size, size, image::imageops::FilterType::Lanczos3)
        })
        .clone();
    drop(masks);
    for pixel in symbol.pixels_mut() {
        pixel.0[..3].copy_from_slice(&fg);
    }
    Ok(symbol)
}
fn encode(palette: &Palette) -> Result<Vec<u8>, String> {
    let fg = color(&palette.foreground)?;
    let frames = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let image = render(size, fg)?;
            IcoFrame::as_png(image.as_raw(), size, size, image::ExtendedColorType::Rgba8)
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut bytes = Vec::new();
    IcoEncoder::new(&mut bytes)
        .encode_images(&frames)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
#[derive(Default, Deserialize)]
struct Shortcuts {
    updated: usize,
    failures: usize,
    references: Vec<String>,
}

#[cfg(windows)]
fn shortcuts(path: &Path) -> Result<Shortcuts, String> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    // Data goes through environment variables, never interpolated as shell code.
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("icon_shortcuts.ps1"),
        ])
        .env("ZOLTRAAK_ICON_PATH", path)
        .env("ZOLTRAAK_ICON_TARGET", executable)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Windows shortcut refresh timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(
            "Windows could not refresh the shortcuts. Check shortcut write permissions.".into(),
        );
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Windows shortcut refresh returned an invalid result".into())
}
#[cfg(not(windows))]
fn shortcuts(_: &Path) -> Result<Shortcuts, String> {
    Ok(Shortcuts::default())
}

fn apply(app: &tauri::AppHandle, palette: Palette, force: bool) -> Result<IconResult, String> {
    let _guard = SYNC.lock().map_err(|e| e.to_string())?;
    color(&palette.foreground)?;
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("theme-icons");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let saved = directory.join("palette.json");
    let name = format!(
        "v2-transparent-{}.ico",
        &palette.foreground[1..].to_lowercase()
    );
    let path = directory.join(name);
    if !path.exists() {
        fs::write(&path, encode(&palette)?).map_err(|e| e.to_string())?;
    }
    let image = render(64, color(&palette.foreground)?)?;
    let icon = tauri::image::Image::new_owned(image.into_raw(), 64, 64);
    for window in app.webview_windows().values() {
        window.set_icon(icon.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_icon(Some(icon)).map_err(|e| e.to_string())?;
    }
    let unchanged = fs::read(&saved)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Palette>(&bytes).ok())
        .is_some_and(|p| p.foreground == palette.foreground);
    // A new process must refresh shortcuts even with an unchanged palette: the
    // installer may have replaced them. Only skip repeated updates in this run.
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let mut last = LAST.lock().map_err(|e| e.to_string())?;
    let signature = path.to_string_lossy().into_owned();
    if !force && unchanged && last.as_ref() == Some(&signature) {
        return Ok(IconResult {
            shortcuts_updated: 0,
            warning: None,
        });
    }
    let result = shortcuts(&path);
    fs::write(
        &saved,
        serde_json::to_vec(&palette).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            return Ok(IconResult {
                shortcuts_updated: 0,
                warning: Some(format!("Window and tray icons updated. {error}")),
            });
        }
    };
    *last = Some(signature);
    // Retain referenced shortcut icons and the newest 32 variants. On failure
    // keep all files, since an inaccessible shortcut may still use an old icon.
    if result.failures == 0 {
        let mut files: Vec<_> = fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                (e.file_name().to_string_lossy().starts_with("v1-")
                    || e.file_name()
                        .to_string_lossy()
                        .starts_with("v2-transparent-"))
                    && e.path().extension().is_some_and(|e| e == "ico")
            })
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        for file in files.into_iter().skip(32) {
            if file.path() != path
                && !result
                    .references
                    .iter()
                    .any(|p| Path::new(p) == file.path())
            {
                let _ = fs::remove_file(file.path());
            }
        }
    }
    Ok(IconResult { shortcuts_updated: result.updated, warning: (result.failures > 0).then(|| format!("App icons updated; {} shortcut(s) could not be written. Try Refresh desktop icon after checking permissions.", result.failures)) })
}

#[tauri::command]
pub async fn sync_app_icon(
    app: tauri::AppHandle,
    foreground: String,
    force: bool,
) -> Result<IconResult, String> {
    tauri::async_runtime::spawn_blocking(move || apply(&app, Palette { foreground }, force))
        .await
        .map_err(|e| e.to_string())?
}
pub fn restore(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let palette = app
            .path()
            .app_local_data_dir()
            .ok()
            .and_then(|dir| fs::read(dir.join("theme-icons/palette.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Palette {
                foreground: DEFAULT_FOREGROUND.into(),
            });
        if let Err(error) = apply(&app, palette, false) {
            tracing::warn!(%error, "could not restore app icon");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_colors_and_renders_transparent_edges() {
        for invalid in ["red", "#fff", "#123456;", "#12zz56", "#１２３"] {
            assert!(color(invalid).is_err());
        }
        let image = render(64, [166, 174, 164]).unwrap();
        assert_eq!(image.get_pixel(0, 0).0[3], 0);
        assert!(image.pixels().filter(|p| p.0[3] == 0).count() > 200);
        assert!(
            image
                .enumerate_pixels()
                .any(|(x, y, p)| (x == 0 || y == 0 || x == 63 || y == 63) && p.0[3] > 0)
        );
        assert!(image.pixels().all(|p| p.0[..3] == [166, 174, 164]));
        assert!(
            image
                .pixels()
                .any(|p| p.0[0] > 100 && p.0[1] > 100 && p.0[3] == 255)
        );
    }
    #[test]
    fn ico_contains_seven_resolutions() {
        let bytes = encode(&Palette {
            foreground: DEFAULT_FOREGROUND.into(),
        })
        .unwrap();
        assert_eq!(&bytes[..6], &[0, 0, 1, 0, 7, 0]);
        for (i, size) in [16, 24, 32, 48, 64, 128, 0].iter().enumerate() {
            assert_eq!(bytes[6 + i * 16], *size);
        }
    }
    /// Explicit asset maintenance utility, never run by ordinary cargo test.
    #[test]
    #[ignore]
    fn regenerate_bundled_icon_assets() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
        let palette = Palette {
            foreground: DEFAULT_FOREGROUND.into(),
        };
        fs::write(directory.join("icon.ico"), encode(&palette).unwrap()).unwrap();
        let mut preview = source_mask().unwrap().clone();
        for pixel in preview.pixels_mut() {
            pixel.0[..3].copy_from_slice(&[255, 255, 255]);
        }
        image::imageops::resize(&preview, 512, 512, image::imageops::FilterType::Lanczos3)
            .save(Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/assets/zoltraak-emblem.png"))
            .unwrap();
        for (name, size) in [
            ("32x32.png", 32),
            ("64x64.png", 64),
            ("128x128.png", 128),
            ("128x128@2x.png", 256),
            ("icon.png", 256),
        ] {
            render(size, color(DEFAULT_FOREGROUND).unwrap())
                .unwrap()
                .save(directory.join(name))
                .unwrap();
        }
    }
}
