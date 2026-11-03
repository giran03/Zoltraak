use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// The extension's Gecko id, and the name browsers use to find our native
/// messaging host. Both are fixed strings the extension also hardcodes — they
/// are the contract between the two halves, not configuration.
const GECKO_ID: &str = "zoltraak@zoltraak.app";
const NATIVE_HOST_NAME: &str = "app.zoltraak.host";
/// Chromium RSA public key to fix the developer extension ID to match CHROMIUM_EXTENSION_ID
const CHROMIUM_PUBLIC_KEY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEApgu/VF91i0vqckIlJTJYAdiiWC91i07cxLjPYr9OsEfPeuKCeqwGUgSXSE3CciSNrY2xxRsV+e9vQzwuc/GW2PGjj32BEIoYLvsaA41Zq1j/9eTKMM9OgHWjkGduwFLe3Mj6YuanRTsvDr8Kmo53eX/ad7sOv22pyufo/WF8jYpv01xr6KXKIXno9gvep/Q2fpMfcbr/C/ZugnH6sHSOM4WdAih6Waqglti0dkAh0bFCcv7PUb4m6LsjTL0znDgZ1/YXHQlEOe3N55wn98JWP6HesW+8UhmAXcBpPKT7PIi7eiJfQf2jLIkdW+Fs/lPdtHRRVVoqs1a/ebjaDwh1XwIDAQAB";
/// Filename of the native-host manifest we drop in app-data. We also use it to
/// recognize when *we* have been launched *as* that host (Firefox passes the
/// manifest path as the first argument), so it must be distinctive.
const NATIVE_HOST_MANIFEST: &str = "zoltraak-native-host.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedBrowser {
    pub id: String,
    pub name: String,
    pub kind: String, // "firefox" | "chromium"
    pub running: bool,
    pub executable_path: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInstallResult {
    pub browser_id: String,
    pub browser_name: String,
    pub success: bool,
    pub message: String,
    pub extension_path: String,
    /// Preparing files is not browser activation. The UI verifies live versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeRegenerateResult {
    pub success: bool,
    pub message: String,
    pub purged_paths: Vec<String>,
    pub regenerated_browsers: Vec<String>,
}

// Embedded extension files so the desktop app is fully self-contained. These
// build the *unpacked* Chromium load and the *unsigned* dev XPI; the shipping
// Zen/Firefox build is a separately signed XPI (see `find_signed_xpi`).
const EXT_MANIFEST: &str = include_str!("../../../extension/manifest.json");
const EXT_BACKGROUND: &str = include_str!("../../../extension/background.js");
const EXT_CONTENT: &str = include_str!("../../../extension/content.js");
const EXT_API: &str = include_str!("../../../extension/api.js");
const EXT_POPUP_HTML: &str = include_str!("../../../extension/popup.html");

/// Version of the extension bundled into this app build. The manifest remains
/// the single source of truth for packaging and every version label in the UI.
pub fn extension_version() -> String {
    serde_json::from_str::<serde_json::Value>(EXT_MANIFEST)
        .ok()
        .and_then(|manifest| manifest.get("version")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

pub fn signed_xpi_version(path: &Path) -> Option<String> {
    let mut archive = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
    let mut manifest = String::new();
    archive
        .by_name("manifest.json")
        .ok()?
        .read_to_string(&mut manifest)
        .ok()?;
    serde_json::from_str::<serde_json::Value>(&manifest)
        .ok()?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

pub fn import_signed_xpi(source: &Path, app_data_dir: &Path) -> Result<(String, PathBuf), String> {
    let file = File::open(source).map_err(|e| format!("Could not open the XPI: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Invalid XPI archive: {e}"))?;
    let mut manifest = String::new();
    archive
        .by_name("manifest.json")
        .map_err(|_| "The XPI has no manifest.json.".to_string())?
        .read_to_string(&mut manifest)
        .map_err(|e| format!("Could not read the XPI manifest: {e}"))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).map_err(|e| format!("Invalid XPI manifest: {e}"))?;
    let id = manifest
        .pointer("/browser_specific_settings/gecko/id")
        .and_then(|value| value.as_str());
    if id != Some(GECKO_ID) {
        return Err("That XPI is not the Zoltraak Firefox extension.".into());
    }
    let version = manifest
        .get("version")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "The XPI manifest has no version.".to_string())?
        .to_owned();
    if version_is_older(&version, &extension_version()) {
        return Err(format!(
            "Signed XPI v{version} is older than the current extension v{}.",
            extension_version()
        ));
    }
    if archive.by_name("META-INF/mozilla.rsa").is_err()
        && archive.by_name("META-INF/cose.sig").is_err()
    {
        return Err("The XPI is not signed by Mozilla.".into());
    }
    drop(archive);

    let target_dir = app_data_dir.join("extension");
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Could not create the extension directory: {e}"))?;
    let target = target_dir.join("zoltraak-signed.xpi");
    let staged = target_dir.join("zoltraak-signed.xpi.new");
    fs::copy(source, &staged).map_err(|e| format!("Could not stage the signed XPI: {e}"))?;
    if target.exists() {
        fs::remove_file(&target)
            .map_err(|e| format!("Could not replace the old signed XPI: {e}"))?;
    }
    fs::rename(&staged, &target).map_err(|e| format!("Could not activate the signed XPI: {e}"))?;
    Ok((version, target))
}

const EXT_POPUP_JS: &str = include_str!("../../../extension/popup.js");

/// The line in `api.js` the token is baked into. Kept as a constant so the
/// two halves of this contract sit next to each other in a search.
const BAKED_TOKEN_DECL: &str = "const BAKED_TOKEN = \"\";";
const BAKED_BROWSER_ID_DECL: &str = "const BAKED_BROWSER_ID = \"\";";

/// Bakes a token into `api.js` source code by replacing `const BAKED_TOKEN = "...";`.
/// Handles both the pristine empty declaration `const BAKED_TOKEN = "";` and
/// replacing any previously baked token.
pub fn bake_token_into_api_js(source: &str, token: &str) -> String {
    let replacement = format!("const BAKED_TOKEN = \"{token}\";");
    if source.contains(BAKED_TOKEN_DECL) {
        source.replace(BAKED_TOKEN_DECL, &replacement)
    } else if let Some(start) = source.find("const BAKED_TOKEN = \"") {
        if let Some(end) = source[start..].find("\";") {
            let mut updated = source.to_string();
            updated.replace_range(start..start + end + 2, &replacement);
            updated
        } else {
            source.to_string()
        }
    } else {
        source.to_string()
    }
}

fn bake_browser_id_into_api_js(source: &str, browser_id: &str) -> String {
    source.replace(
        BAKED_BROWSER_ID_DECL,
        &format!("const BAKED_BROWSER_ID = \"{browser_id}\";"),
    )
}

/// Write the extension source into `dest_dir`.
///
/// `token` is baked into `api.js` only for the **Chromium dev-load** path,
/// whose files are ours to mutate. The signed Zen/Firefox build is immutable,
/// so it cannot be pre-paired this way and pairs over native messaging instead
/// — passing an empty token here leaves `api.js` untouched.
pub fn write_extension_files(
    dest_dir: &Path,
    token: &str,
    browser_id: &str,
) -> std::io::Result<()> {
    fs::create_dir_all(dest_dir)?;

    // Chromium MV3 uses onDeterminingFilename, not Firefox's blocking webRequest.
    let mut manifest: serde_json::Value = serde_json::from_str(EXT_MANIFEST)?;
    if !token.is_empty() {
        if let Some(permissions) = manifest["permissions"].as_array_mut() {
            permissions.retain(|permission| permission != "webRequestBlocking");
        }
        // Inject Chromium-specific manifest properties for unpacked developer loads
        manifest
            .as_object_mut()
            .unwrap()
            .remove("browser_specific_settings");
        manifest["background"] =
            serde_json::json!({"service_worker": "background.js", "type": "module"});
        manifest["key"] = serde_json::json!(CHROMIUM_PUBLIC_KEY);
        manifest["minimum_chrome_version"] = serde_json::json!("119");
    }
    fs::write(
        dest_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    fs::write(dest_dir.join("background.js"), EXT_BACKGROUND)?;
    fs::write(dest_dir.join("content.js"), EXT_CONTENT)?;
    fs::write(
        dest_dir.join("video-identity.js"),
        include_str!("../../../extension/video-identity.js"),
    )?;
    for (name, source) in [
        ("pill-placement.js", include_str!("../../../extension/pill-placement.js")),
        ("facebook-post.js", include_str!("../../../extension/facebook-post.js")),
        ("generic-media.js", include_str!("../../../extension/generic-media.js")),
        ("media-probe.js", include_str!("../../../extension/media-probe.js")),
        ("media-registry.js", include_str!("../../../extension/media-registry.js")),
    ] {
        fs::write(dest_dir.join(name), source)?;
    }
    fs::write(
        dest_dir.join("yt-inject.js"),
        include_str!("../../../extension/yt-inject.js"),
    )?;
    // The manifest declares these as the toolbar and extension-manager icons;
    // a Chromium load fails outright when a declared icon file is missing.
    fs::create_dir_all(dest_dir.join("icons"))?;
    for (name, bytes) in [
        ("16.png", &include_bytes!("../../../extension/icons/16.png")[..]),
        ("32.png", &include_bytes!("../../../extension/icons/32.png")[..]),
        ("48.png", &include_bytes!("../../../extension/icons/48.png")[..]),
        ("128.png", &include_bytes!("../../../extension/icons/128.png")[..]),
    ] {
        fs::write(dest_dir.join("icons").join(name), bytes)?;
    }
    fs::create_dir_all(dest_dir.join("fonts"))?;
    fs::write(
        dest_dir.join("fonts/archivo-subset.woff2"),
        include_bytes!("../../../extension/fonts/archivo-subset.woff2"),
    )?;
    fs::write(
        dest_dir.join("fonts/martian-mono-subset.woff2"),
        include_bytes!("../../../extension/fonts/martian-mono-subset.woff2"),
    )?;
    fs::write(
        dest_dir.join("theme.js"),
        include_str!("../../../extension/theme.js"),
    )?;
    fs::write(
        dest_dir.join("popup.css"),
        include_str!("../../../extension/popup.css"),
    )?;
    fs::write(
        dest_dir.join("intercept.js"),
        include_str!("../../../extension/intercept.js"),
    )?;
    fs::write(
        dest_dir.join("intercept.html"),
        include_str!("../../../extension/intercept.html"),
    )?;
    fs::write(
        dest_dir.join("placement-controller.js"),
        include_str!("../../../extension/placement-controller.js"),
    )?;
    fs::write(
        dest_dir.join("pill-controller.js"),
        include_str!("../../../extension/pill-controller.js"),
    )?;
    fs::write(
        dest_dir.join("placement.css"),
        include_str!("../../../extension/placement.css"),
    )?;
    fs::write(
        dest_dir.join("placement.html"),
        include_str!("../../../extension/placement.html"),
    )?;
    fs::write(
        dest_dir.join("placement.js"),
        include_str!("../../../extension/placement.js"),
    )?;

    let api_js_content = if token.is_empty() {
        bake_token_into_api_js(EXT_API, "")
    } else {
        let baked = bake_token_into_api_js(EXT_API, token);
        debug_assert!(
            baked.contains(&format!("const BAKED_TOKEN = \"{token}\";")),
            "api.js does not contain `const BAKED_TOKEN = \"...\";` — the token was not baked in"
        );
        baked
    };
    let api_js_content = bake_browser_id_into_api_js(&api_js_content, browser_id);
    fs::write(dest_dir.join("api.js"), api_js_content)?;

    fs::write(dest_dir.join("popup.html"), EXT_POPUP_HTML)?;
    fs::write(dest_dir.join("popup.js"), EXT_POPUP_JS)?;

    Ok(())
}

/// Zip an unpacked extension directory into an (unsigned) XPI. Only used for
/// the dev fallback — release Zen/Firefox refuse to *permanently* install an
/// unsigned XPI, so a signed artifact ships instead.
pub fn create_firefox_xpi(ext_dir: &Path, xpi_path: &Path) -> std::io::Result<()> {
    let file = File::create(xpi_path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut pending = vec![ext_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                let name = path
                    .strip_prefix(ext_dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                zip.start_file(name, options)?;
                std::io::copy(&mut File::open(path)?, &mut zip)?;
            }
        }
    }

    zip.finish()?;
    Ok(())
}

pub fn matches_browser_process(browser_id: &str, process_name: &str) -> bool {
    let p_name = process_name.to_lowercase();
    match browser_id {
        "firefox" => p_name.contains("firefox"),
        "chrome" => p_name == "chrome.exe" || p_name == "chrome",
        "edge" => p_name == "msedge.exe" || p_name == "msedge",
        "brave" => p_name == "brave.exe" || p_name == "brave",
        "vivaldi" => p_name == "vivaldi.exe" || p_name == "vivaldi",
        "zen" => {
            p_name == "zen.exe"
                || p_name == "zen"
                || p_name.contains("zen-browser")
                || p_name.contains("zen browser")
        }
        "opera" => p_name == "opera.exe" || p_name == "opera",
        _ => p_name.contains(&browser_id.to_lowercase()),
    }
}

fn browser_processes() -> System {
    // Browser discovery needs names and executable paths, not CPU, memory,
    // environment or disk statistics for every process on the desktop.
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    sys
}

/// Check if the specified browser is currently running.
/// If running, returns `Some(executable_path)` (using the running process's own path if available).
/// If not running, returns `None`.
pub fn find_running_browser_executable(browser: &DetectedBrowser) -> Option<String> {
    let sys = browser_processes();

    let mut matched_without_exe = false;

    for process in sys.processes().values() {
        let p_name = process.name().to_string_lossy();
        if matches_browser_process(&browser.id, &p_name) {
            if let Some(exe) = process.exe() {
                let path_str = exe.to_string_lossy().to_string();
                if !path_str.is_empty() && Path::new(&path_str).exists() {
                    return Some(path_str);
                }
            }
            matched_without_exe = true;
        }
    }

    if matched_without_exe {
        if let Some(ref exe) = browser.executable_path {
            if Path::new(exe).exists() {
                return Some(exe.clone());
            }
        }
    }

    None
}

pub fn scan_browsers(app_data_dir: &Path) -> Vec<DetectedBrowser> {
    let sys = browser_processes();

    let mut running_browsers = std::collections::HashSet::new();
    let mut exe_paths = std::collections::HashMap::new();

    let supported_ids = [
        "firefox", "chrome", "edge", "brave", "vivaldi", "zen", "opera",
    ];

    for process in sys.processes().values() {
        let p_name = process.name().to_string_lossy();
        let exe = process.exe().map(|p| p.to_string_lossy().to_string());

        for &id in &supported_ids {
            if matches_browser_process(id, &p_name) {
                running_browsers.insert(id);
                if let Some(ref path) = exe {
                    exe_paths.insert(id, path.clone());
                }
            }
        }
    }

    let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let zen_path_1 = format!(r"{}\Programs\zen\zen.exe", local_appdata);
    let zen_path_2 = format!(r"{}\Zen\zen.exe", local_appdata);
    let zen_path_3 = format!(r"{}\Programs\Zen Browser\zen.exe", local_appdata);

    let definitions = vec![
        (
            "zen",
            "Zen Browser",
            "firefox",
            vec![
                r"C:\Program Files\Zen Browser\zen.exe",
                r"C:\Program Files\Zen\zen.exe",
                r"C:\Program Files (x86)\Zen Browser\zen.exe",
                zen_path_1.as_str(),
                zen_path_2.as_str(),
                zen_path_3.as_str(),
            ],
        ),
        (
            "firefox",
            "Mozilla Firefox",
            "firefox",
            vec![
                r"C:\Program Files\Mozilla Firefox\firefox.exe",
                r"C:\Program Files (x86)\Mozilla Firefox\firefox.exe",
            ],
        ),
        (
            "chrome",
            "Google Chrome",
            "chromium",
            vec![
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            ],
        ),
        (
            "edge",
            "Microsoft Edge",
            "chromium",
            vec![
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            ],
        ),
        (
            "brave",
            "Brave Browser",
            "chromium",
            vec![r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe"],
        ),
        (
            "vivaldi",
            "Vivaldi",
            "chromium",
            vec![r"C:\Program Files\Vivaldi\Application\vivaldi.exe"],
        ),
        (
            "opera",
            "Opera",
            "chromium",
            vec![r"C:\Program Files\Opera\launcher.exe"],
        ),
    ];

    let mut result = Vec::new();

    for (id, name, kind, paths) in definitions {
        let is_running = running_browsers.contains(id);
        let mut exe_path = exe_paths.get(id).cloned();

        if exe_path.is_none() {
            for p in paths {
                let path = Path::new(p);
                if path.exists() {
                    exe_path = Some(p.to_string());
                    break;
                }
            }
        }

        if is_running || exe_path.is_some() {
            let item = DetectedBrowser {
                id: id.to_string(),
                name: name.to_string(),
                kind: kind.to_string(),
                running: is_running,
                executable_path: exe_path,
                installed: false,
            };
            let is_installed = extension_already_present(&item, app_data_dir);
            result.push(DetectedBrowser {
                installed: is_installed,
                ..item
            });
        }
    }

    if result.is_empty() {
        let item_chrome = DetectedBrowser {
            id: "chrome".to_string(),
            name: "Chromium Browser".to_string(),
            kind: "chromium".to_string(),
            running: false,
            executable_path: None,
            installed: false,
        };
        let chrome_installed = extension_already_present(&item_chrome, app_data_dir);
        result.push(DetectedBrowser {
            installed: chrome_installed,
            ..item_chrome
        });

        let item_ff = DetectedBrowser {
            id: "firefox".to_string(),
            name: "Mozilla Firefox".to_string(),
            kind: "firefox".to_string(),
            running: false,
            executable_path: None,
            installed: false,
        };
        let ff_installed = extension_already_present(&item_ff, app_data_dir);
        result.push(DetectedBrowser {
            installed: ff_installed,
            ..item_ff
        });
    }

    result.sort_by(|a, b| b.running.cmp(&a.running));

    result
}

// ---------------------------------------------------------------------------
// Native messaging: how the immutable signed extension gets its API token.
// ---------------------------------------------------------------------------

/// True when this process was launched *by a browser as the native host*
/// rather than as the desktop app. Firefox appends the host-manifest path as
/// argv, Chromium appends the calling extension's origin; either is a tell.
/// Callers must check this before doing anything that writes to stdout —
/// native messaging owns the stdio pipe and stray bytes corrupt the protocol.
const NATIVE_HOST_MANIFEST_CHROME: &str = "zoltraak-native-host-chrome.json";

pub fn is_native_host_invocation() -> bool {
    std::env::args().skip(1).any(|a| {
        a == "--native-host"
            || a.contains(NATIVE_HOST_MANIFEST)
            || a.contains(NATIVE_HOST_MANIFEST_CHROME)
            || a.starts_with("moz-extension://")
            || a.starts_with("chrome-extension://")
    })
}

/// The current API token, read from the same on-disk store the running app and
/// REST API share, so the value the extension gets is the one the app accepts.
fn current_token() -> String {
    match zoltraak_api::store::default_path() {
        Some(path) => zoltraak_api::store::load_or_create(&path)
            .map(|t| t.expose().to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// Serve the native-messaging protocol on stdio until the browser closes the
/// pipe. Every inbound message is treated as a pairing request — the only
/// thing the host does is hand back the current token and port. Messages are
/// length-prefixed with a 4-byte native-endian count, per the spec.
pub fn run_native_host() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    loop {
        let mut len_buf = [0u8; 4];
        if reader.read_exact(&mut len_buf).is_err() {
            break; // pipe closed — normal shutdown
        }
        let len = u32::from_ne_bytes(len_buf) as usize;
        // Firefox caps host->browser at 1 MB and browser->host at 4 GB; a
        // pairing ping is tiny, so anything large is a framing error, not us.
        if len == 0 || len > 1024 * 1024 {
            break;
        }
        let mut msg = vec![0u8; len];
        if reader.read_exact(&mut msg).is_err() {
            break;
        }

        let reply = serde_json::json!({
            "token": current_token(),
            "port": 1450,
            "browserId": native_host_browser_id(),
        });
        let bytes = match serde_json::to_vec(&reply) {
            Ok(b) => b,
            Err(_) => break,
        };
        if writer
            .write_all(&(bytes.len() as u32).to_ne_bytes())
            .is_err()
            || writer.write_all(&bytes).is_err()
            || writer.flush().is_err()
        {
            break;
        }
    }
}

fn native_host_browser_id() -> &'static str {
    let Ok(mut pid) = sysinfo::get_current_pid() else {
        return "browser";
    };
    // Only names and parent links are read below; a full refresh would also
    // sample CPU, memory, disks and networks every time a browser starts the
    // native host.
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    for _ in 0..4 {
        let Some(process) = system.process(pid) else {
            break;
        };
        let Some(parent) = process.parent() else {
            break;
        };
        let Some(parent_process) = system.process(parent) else {
            break;
        };
        let name = parent_process.name().to_string_lossy();
        for id in [
            "firefox", "zen", "chrome", "edge", "brave", "vivaldi", "opera",
        ] {
            if matches_browser_process(id, &name) {
                return id;
            }
        }
        pid = parent;
    }
    "browser"
}

/// Write native-host manifests into app-data for both Gecko and Chromium browsers.
fn write_native_host_manifests(app_data: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe()?;
    fs::create_dir_all(app_data)?;

    let gecko_path = app_data.join(NATIVE_HOST_MANIFEST);
    let gecko_manifest = serde_json::json!({
        "name": NATIVE_HOST_NAME,
        "description": "Zoltraak extension pairing host",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_extensions": [GECKO_ID],
    });
    fs::write(&gecko_path, serde_json::to_string_pretty(&gecko_manifest)?)?;

    let chrome_path = app_data.join(NATIVE_HOST_MANIFEST_CHROME);
    let mut allowed_origins = vec![format!(
        "chrome-extension://{}/",
        zoltraak_api::auth::CHROMIUM_EXTENSION_ID
    )];
    if let Ok(store_id) = std::env::var("ZOLTRAAK_CHROME_EXTENSION_ID") {
        let store_id = store_id.trim();
        if !store_id.is_empty() {
            allowed_origins.push(format!("chrome-extension://{store_id}/"));
        }
    }
    if let Ok(content) = fs::read_to_string(app_data.join("chrome_extension_id")) {
        let custom_id = content.trim();
        if !custom_id.is_empty() && !allowed_origins.iter().any(|o| o.contains(custom_id)) {
            allowed_origins.push(format!("chrome-extension://{custom_id}/"));
        }
    }
    let chrome_manifest = serde_json::json!({
        "name": NATIVE_HOST_NAME,
        "description": "Zoltraak extension pairing host",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": allowed_origins,
    });
    fs::write(
        &chrome_path,
        serde_json::to_string_pretty(&chrome_manifest)?,
    )?;

    Ok((gecko_path, chrome_path))
}

/// Register the native-messaging host so Chromium and Zen/Firefox can launch it. Idempotent
/// and safe to call on every startup — it just rewrites the manifests (picking
/// up a moved executable) and re-points the registry keys.
pub fn register_native_host(app_data: &Path) -> bool {
    let (gecko_path, chrome_path) = match write_native_host_manifests(app_data) {
        Ok(paths) => paths,
        Err(_) => return false,
    };
    let gecko_str = gecko_path.to_string_lossy().to_string();
    let chrome_str = chrome_path.to_string_lossy().to_string();

    let gecko_keys = [
        format!(r"HKCU\Software\Mozilla\NativeMessagingHosts\{NATIVE_HOST_NAME}"),
        format!(r"HKCU\Software\Zen\NativeMessagingHosts\{NATIVE_HOST_NAME}"),
    ];

    let chrome_keys = [
        format!(r"HKCU\Software\Google\Chrome\NativeMessagingHosts\{NATIVE_HOST_NAME}"),
        format!(r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\{NATIVE_HOST_NAME}"),
        format!(
            r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\{NATIVE_HOST_NAME}"
        ),
    ];

    let mut ok = false;
    for key in &gecko_keys {
        use std::os::windows::process::CommandExt;
        let done = Command::new("reg")
            .creation_flags(0x08000000)
            .args(["add", key, "/ve", "/t", "REG_SZ", "/d", &gecko_str, "/f"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        ok |= done;
    }
    for key in &chrome_keys {
        use std::os::windows::process::CommandExt;
        let done = Command::new("reg")
            .creation_flags(0x08000000)
            .args(["add", key, "/ve", "/t", "REG_SZ", "/d", &chrome_str, "/f"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        ok |= done;
    }
    ok
}

/// Locate the signed, shippable Zen/Firefox XPI. Checks caller-supplied
/// locations first (e.g. Tauri's resolved resource dir), then next to the
/// executable. Returns `None` in a dev tree where no signed artifact exists.
pub fn find_signed_xpi(extra: &[PathBuf]) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = extra.to_vec();
    #[cfg(debug_assertions)]
    {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("zoltraak-signed.xpi"),
        );
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("zoltraak-signed.xpi"));
            candidates.push(dir.join("resources").join("zoltraak-signed.xpi"));
        }
    }
    let mut existing: Vec<PathBuf> = candidates.into_iter().filter(|p| p.is_file()).collect();
    existing.sort_by(|a, b| {
        if let (Some(a_version), Some(b_version)) = (signed_xpi_version(a), signed_xpi_version(b)) {
            let by_version = version_cmp(&b_version, &a_version);
            if by_version != std::cmp::Ordering::Equal {
                return by_version;
            }
        }
        let time_a = a.metadata().and_then(|m| m.modified()).ok();
        let time_b = b.metadata().and_then(|m| m.modified()).ok();
        time_b.cmp(&time_a) // Newest first
    });
    existing.into_iter().next()
}

/// Where `setup_gecko_policies` writes (or would write) its policy file for a
/// given browser executable — factored out so detection can look at the same
/// path without duplicating the "next to the exe" logic.
fn gecko_policy_path(exe_path: &str) -> Option<PathBuf> {
    Path::new(exe_path)
        .parent()
        .map(|parent| parent.join("distribution").join("policies.json"))
}

/// Drop an enterprise-policy file next to the browser binary that force-adds
/// the extension from a local signed XPI. `normal_installed` (not
/// `force_installed`) is deliberate: the extension installs silently on the
/// next launch but the user can still disable or remove it — an unremovable
/// add-on is exactly the malware-shaped behavior we want to avoid.
pub fn setup_gecko_policies(exe_path: &str, xpi_path: &Path) -> bool {
    write_gecko_policy(exe_path, xpi_path).is_ok()
}

fn write_gecko_policy(exe_path: &str, xpi_path: &Path) -> Result<(), String> {
    let Some(policy_path) = gecko_policy_path(exe_path) else {
        return Err("The browser executable has no parent directory.".into());
    };
    let Some(dist_dir) = policy_path.parent() else {
        return Err("The browser policy has no parent directory.".into());
    };
    fs::create_dir_all(dist_dir)
        .map_err(|e| format!("Could not create the browser policy directory: {e}"))?;
    let xpi_url = reqwest::Url::from_file_path(xpi_path)
        .map_err(|_| "The signed package path must be absolute.".to_string())?;
    let mut content: serde_json::Value = match fs::read(&policy_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            "The existing browser policy is invalid JSON. It was left unchanged.".to_string()
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({"policies":{}})
        }
        Err(error) => return Err(format!("Could not read the browser policy: {error}")),
    };
    let root = content
        .as_object_mut()
        .ok_or("The browser policy must be a JSON object.")?;
    let policies = root
        .entry("policies")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("The policies entry must be an object.")?;
    let extensions = policies
        .entry("ExtensionSettings")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("ExtensionSettings must be an object.")?;
    let entry = extensions
        .entry(GECKO_ID)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("The Zoltraak extension policy must be an object.")?;
    entry.insert(
        "installation_mode".into(),
        serde_json::json!("normal_installed"),
    );
    entry.insert("install_url".into(), serde_json::json!(xpi_url.as_str()));
    let staged = policy_path.with_extension("json.new");
    fs::write(
        &staged,
        serde_json::to_vec_pretty(&content).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Could not write the browser policy: {e}"))?;
    fs::rename(staged, policy_path)
        .map_err(|e| format!("Could not activate the browser policy: {e}"))
}

/// A new release gets a new install_url, which makes Gecko re-evaluate its
/// policy rather than continuing to use the original profile-cached XPI.
fn stage_signed_gecko_package(
    source: &Path,
    app_data_dir: &Path,
    version: &str,
) -> Result<PathBuf, String> {
    if version.is_empty()
        || version.len() > 64
        || !version.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return Err("The signed package has an invalid version.".into());
    }
    let directory = app_data_dir.join("extension").join("signed");
    fs::create_dir_all(&directory)
        .map_err(|e| format!("Could not prepare signed package directory: {e}"))?;
    let target = directory.join(format!("zoltraak-{version}.xpi"));
    if source != target {
        let staged = target.with_extension("xpi.new");
        fs::copy(source, &staged).map_err(|e| format!("Could not stage the signed update: {e}"))?;
        fs::rename(staged, &target)
            .map_err(|e| format!("Could not activate the signed package: {e}"))?;
    }
    Ok(target)
}

fn chromium_user_data_dirs(browser_id: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let app_data = std::env::var("APPDATA").unwrap_or_default();

    if !local_app_data.is_empty() {
        let base = Path::new(&local_app_data);
        match browser_id {
            "chrome" => {
                dirs.push(base.join("Google").join("Chrome").join("User Data"));
                dirs.push(base.join("Google").join("Chrome Beta").join("User Data"));
                dirs.push(base.join("Google").join("Chrome Dev").join("User Data"));
                dirs.push(base.join("Google").join("Chrome SxS").join("User Data"));
                dirs.push(base.join("Chromium").join("User Data"));
            }
            "edge" => {
                dirs.push(base.join("Microsoft").join("Edge").join("User Data"));
                dirs.push(base.join("Microsoft").join("Edge Beta").join("User Data"));
                dirs.push(base.join("Microsoft").join("Edge Dev").join("User Data"));
            }
            "brave" => {
                dirs.push(
                    base.join("BraveSoftware")
                        .join("Brave-Browser")
                        .join("User Data"),
                );
            }
            "vivaldi" => {
                dirs.push(base.join("Vivaldi").join("User Data"));
            }
            "opera" => {
                dirs.push(base.join("Programs").join("Opera"));
            }
            _ => {}
        }
    }

    if !app_data.is_empty() {
        let roaming = Path::new(&app_data);
        if browser_id == "opera" {
            dirs.push(roaming.join("Opera Software").join("Opera Stable"));
            dirs.push(roaming.join("Opera Software").join("Opera GX Stable"));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let home_p = Path::new(&home);
            #[cfg(target_os = "macos")]
            {
                let app_support = home_p.join("Library").join("Application Support");
                match browser_id {
                    "chrome" => dirs.push(app_support.join("Google").join("Chrome")),
                    "edge" => dirs.push(app_support.join("Microsoft Edge")),
                    "brave" => dirs.push(app_support.join("BraveSoftware").join("Brave-Browser")),
                    "vivaldi" => dirs.push(app_support.join("Vivaldi")),
                    "opera" => dirs.push(app_support.join("com.operasoftware.Opera")),
                    _ => {}
                }
            }
            #[cfg(target_os = "linux")]
            {
                let config = home_p.join(".config");
                match browser_id {
                    "chrome" => {
                        dirs.push(config.join("google-chrome"));
                        dirs.push(config.join("chromium"));
                    }
                    "edge" => dirs.push(config.join("microsoft-edge")),
                    "brave" => dirs.push(config.join("BraveSoftware").join("Brave-Browser")),
                    "vivaldi" => dirs.push(config.join("vivaldi")),
                    "opera" => dirs.push(config.join("opera")),
                    _ => {}
                }
            }
        }
    }

    dirs
}

fn is_chromium_extension_installed(browser: &DetectedBrowser, app_data_dir: &Path) -> bool {
    // In unit tests or isolated test runs using a temp directory, do not scan host browser profiles.
    if app_data_dir.starts_with(std::env::temp_dir()) {
        return false;
    }

    let user_data_dirs = chromium_user_data_dirs(&browser.id);
    let ext_dir = chromium_extension_dir(browser, app_data_dir);
    let target_id = zoltraak_api::auth::CHROMIUM_EXTENSION_ID;

    for udir in user_data_dirs {
        if !udir.is_dir() {
            continue;
        }

        let mut profile_dirs = vec![udir.clone()];
        if let Ok(entries) = fs::read_dir(&udir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name == "Default"
                            || name.starts_with("Profile ")
                            || name == "Guest Profile"
                            || name == "System Profile"
                        {
                            profile_dirs.push(entry.path());
                        }
                    }
                }
            }
        }

        for prof in profile_dirs {
            for pref_name in &["Secure Preferences", "Preferences"] {
                let pref_file = prof.join(pref_name);
                if !pref_file.is_file() {
                    continue;
                }

                if let Ok(content) = fs::read_to_string(&pref_file) {
                    if !content.contains(target_id)
                        && !content.contains("dev.zoltraak.app")
                        && !content.contains("OpenDM")
                        && !content.contains("Zoltraak")
                    {
                        continue;
                    }

                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(settings) = val
                            .pointer("/extensions/settings")
                            .and_then(|v| v.as_object())
                        {
                            for (id, item) in settings {
                                if valid_chromium_extension_entry(
                                    id, item, &prof, &ext_dir, target_id,
                                ) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    false
}

fn extension_entry_disabled(entry: &serde_json::Value) -> bool {
    if entry.get("state").and_then(|v| v.as_i64()) == Some(0) {
        return true;
    }
    match entry.get("disable_reasons") {
        Some(serde_json::Value::Number(value)) => value.as_u64().unwrap_or(0) != 0,
        Some(serde_json::Value::Array(values)) => !values.is_empty(),
        Some(serde_json::Value::Object(values)) => !values.is_empty(),
        _ => false,
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(a, b)| a == b)
}

fn manifest_identifies_zoltraak(dir: &Path) -> bool {
    let Ok(content) = fs::read_to_string(dir.join("manifest.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    manifest.get("manifest_version").and_then(|v| v.as_u64()) == Some(3)
        && manifest.get("name").and_then(|v| v.as_str()) == Some("Zoltraak")
}

fn valid_chromium_extension_entry(
    id: &str,
    entry: &serde_json::Value,
    profile_dir: &Path,
    expected_dir: &Path,
    target_id: &str,
) -> bool {
    let Some(object) = entry.as_object() else {
        return false;
    };
    if object.is_empty() || extension_entry_disabled(entry) {
        return false;
    }

    if let Some(raw_path) = entry
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    {
        let configured = Path::new(raw_path);
        let resolved = if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            profile_dir.join("Extensions").join(configured)
        };
        return resolved.is_dir()
            && (same_directory(&resolved, expected_dir)
                || manifest_identifies_zoltraak(&resolved));
    }

    // Some packaged Chromium entries embed their manifest instead of a path.
    id == target_id && entry.pointer("/manifest/name").and_then(|v| v.as_str()) == Some("Zoltraak")
}

/// Best-effort check for whether Zoltraak has already been set up for this
/// browser, so "Auto-Install" can tell a first run from a refresh.
///
/// - Gecko: the enterprise policy file we drop already names our extension id.
/// - Chromium: an enabled, valid extension entry in Chromium profile preferences.
pub fn extension_already_present(browser: &DetectedBrowser, app_data_dir: &Path) -> bool {
    if browser.kind == "firefox" {
        browser
            .executable_path
            .as_deref()
            .and_then(gecko_policy_path)
            .and_then(|p| fs::read_to_string(p).ok())
            .is_some_and(|content| content.contains(GECKO_ID))
    } else {
        is_chromium_extension_installed(browser, app_data_dir)
    }
}

pub fn install_browser_extension(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
    token: &str,
    signed_xpi: Option<&Path>,
) -> ExtensionInstallResult {
    install_browser_extension_internal(browser, app_data_dir, token, signed_xpi, true)
}

fn install_browser_extension_internal(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
    token: &str,
    signed_xpi: Option<&Path>,
    reveal: bool,
) -> ExtensionInstallResult {
    if browser.kind == "firefox" {
        install_gecko(browser, app_data_dir, signed_xpi, reveal)
    } else {
        install_chromium(browser, app_data_dir, token, reveal)
    }
}

/// Zen / Firefox: silent install via a bundled *signed* XPI + enterprise
/// policy, with the token delivered afterwards over native messaging.
fn install_gecko(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
    signed_xpi: Option<&Path>,
    reveal: bool,
) -> ExtensionInstallResult {
    // Register the pairing host up front so the token handshake works the
    // instant the extension loads, regardless of which install path we take.
    register_native_host(app_data_dir);

    if let Some(xpi) = signed_xpi {
        let Some(signed_version) = signed_xpi_version(xpi) else {
            return ExtensionInstallResult {
                browser_id: browser.id.clone(),
                browser_name: browser.name.clone(),
                success: false,
                message: "The local Firefox XPI is invalid. Import a Mozilla-signed Zoltraak XPI and try again.".into(),
                extension_path: xpi.to_string_lossy().into_owned(),
                next_step: None,
            };
        };
        if version_is_older(&signed_version, &extension_version()) {
            return ExtensionInstallResult {
                browser_id: browser.id.clone(),
                browser_name: browser.name.clone(),
                success: false,
                message: format!(
                    "Signed Firefox package v{signed_version} is older than the current extension v{}. Import the approved XPI, then update again.",
                    extension_version()
                ),
                extension_path: xpi.to_string_lossy().into_owned(),
                next_step: None,
            };
        }
        let staged = match stage_signed_gecko_package(xpi, app_data_dir, &signed_version) {
            Ok(path) => path,
            Err(message) => {
                return ExtensionInstallResult {
                    browser_id: browser.id.clone(),
                    browser_name: browser.name.clone(),
                    success: false,
                    message,
                    extension_path: xpi.to_string_lossy().into_owned(),
                    next_step: None,
                };
            }
        };
        let policy = browser
            .executable_path
            .as_deref()
            .ok_or_else(|| "Browser executable not found.".to_string())
            .and_then(|exe| write_gecko_policy(exe, &staged));
        // runtime.reload() cannot install a different signed Gecko XPI. Give
        // the running browser the package so its own consent dialog can do it.
        let opened = if browser.running {
            let executable = find_running_browser_executable(browser)
                .or_else(|| browser.executable_path.clone());
            executable
                .and_then(|exe| {
                    reqwest::Url::from_file_path(&staged)
                        .ok()
                        .map(|url| (exe, url))
                })
                .is_some_and(|(exe, url)| {
                    Command::new(exe)
                        .args(["--new-tab", url.as_str()])
                        .spawn()
                        .is_ok()
                })
        } else {
            false
        };
        let (success, message, next_step) = if opened {
            (
                true,
                format!(
                    "Opened signed extension v{signed_version} in {}. Confirm the add-on installation in the browser; Zoltraak will verify its new version when it reconnects.{}",
                    browser.name,
                    policy
                        .as_ref()
                        .err()
                        .map(|e| format!(" Automatic startup deployment was unavailable: {e}"))
                        .unwrap_or_default()
                ),
                "browser_confirmation",
            )
        } else if policy.is_ok() {
            (
                true,
                format!(
                    "Prepared signed extension v{signed_version} for {}. Restart the browser to apply the changed install policy, or install the XPI from its Add-ons page. Waiting for browser confirmation.",
                    browser.name
                ),
                "restart_browser",
            )
        } else {
            let _ = zoltraak_core::shell::reveal(&staged);
            (
                false,
                format!(
                    "Could not open the {} update prompt or update its policy ({}). Install the revealed signed XPI using Add-ons → Install Add-on From File, then check again.",
                    browser.name,
                    policy
                        .err()
                        .unwrap_or_else(|| "Unknown policy error".into())
                ),
                "manual_install",
            )
        };
        return ExtensionInstallResult {
            browser_id: browser.id.clone(),
            browser_name: browser.name.clone(),
            success,
            message,
            extension_path: staged.to_string_lossy().into_owned(),
            next_step: Some(next_step.into()),
        };
    }

    // Dev tree: no signed artifact. Build the unsigned XPI so it can be
    // side-loaded temporarily, and say plainly that it won't survive a restart
    // until a signed build ships.
    let ext_dir = app_data_dir.join("extension");
    let xpi_path = app_data_dir.join("zoltraak_extension.xpi");
    let built = write_extension_files(&ext_dir, "", &browser.id)
        .and_then(|_| create_firefox_xpi(&ext_dir, &xpi_path));
    if let Err(e) = built {
        return ExtensionInstallResult {
            browser_id: browser.id.clone(),
            browser_name: browser.name.clone(),
            success: false,
            message: format!("Failed to build the extension package: {e}"),
            extension_path: ext_dir.to_string_lossy().to_string(),
            next_step: None,
        };
    }

    let running_exe = if reveal {
        find_running_browser_executable(browser)
    } else {
        None
    };

    let browser_opened = if let Some(ref exe) = running_exe {
        Command::new(exe)
            .args(["--new-tab", "about:debugging#/runtime/this-firefox"])
            .spawn()
            .is_ok()
    } else {
        false
    };

    let message = if reveal {
        let _ = zoltraak_core::shell::reveal(&xpi_path);
        if browser_opened {
            format!(
                "Using running {} process: opened about:debugging and revealed extension package. Click 'Load Temporary Add-on' and pick the XPI file.",
                browser.name
            )
        } else {
            format!(
                "Extension package rebuilt for {} (path revealed). Since {} is not running, no new instance was created. When you open {}, go to about:debugging and click 'Load Temporary Add-on'.",
                browser.name, browser.name, browser.name
            )
        }
    } else {
        format!(
            "No signed build available (dev tree). Extension package rebuilt for {}. Reload it in {}'s about:debugging page.",
            browser.name, browser.name
        )
    };

    ExtensionInstallResult {
        browser_id: browser.id.clone(),
        browser_name: browser.name.clone(),
        success: false,
        message,
        extension_path: xpi_path.to_string_lossy().to_string(),
        next_step: None,
    }
}

pub fn chromium_extension_dir_for_id(browser_id: &str, app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("extensions").join(browser_id)
}

pub fn open_extension_manager(browser_id: &str, app_data_dir: &Path) -> Result<(), String> {
    let browser = scan_browsers(app_data_dir)
        .into_iter()
        .find(|browser| browser.id == browser_id)
        .ok_or_else(|| "Browser not found.".to_string())?;
    let exe = find_running_browser_executable(&browser)
        .or(browser.executable_path.clone())
        .ok_or_else(|| format!("Could not find the {} executable.", browser.name))?;
    let url = if browser.kind == "firefox" {
        "about:addons"
    } else if browser.id == "edge" {
        "edge://extensions"
    } else {
        "chrome://extensions"
    };
    let mut command = Command::new(exe);
    if browser.kind == "firefox" {
        command.arg("--new-tab");
    }
    command
        .arg(url)
        .spawn()
        .map_err(|e| format!("Could not open {}'s extension manager: {e}", browser.name))?;
    Ok(())
}

pub fn prepare_chromium_extension_for_id(
    browser_id: &str,
    app_data_dir: &Path,
    token: &str,
) -> std::io::Result<PathBuf> {
    let dir = chromium_extension_dir_for_id(browser_id, app_data_dir);
    write_extension_files(&dir, token, browser_id)?;
    Ok(dir)
}

fn chromium_extension_dir(browser: &DetectedBrowser, app_data_dir: &Path) -> PathBuf {
    chromium_extension_dir_for_id(&browser.id, app_data_dir)
}

fn install_chromium(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
    token: &str,
    reveal: bool,
) -> ExtensionInstallResult {
    let ext_dir = chromium_extension_dir(browser, app_data_dir);
    let outcome = (|| -> Result<String, String> {
        write_extension_files(&ext_dir, token, &browser.id)
            .map_err(|e| format!("Failed to prepare extension: {e}"))?;
        if !reveal {
            return Ok(format!(
                "Updated extension files for {}. Reload requested for connected extensions.",
                browser.name
            ));
        }
        let exe = find_running_browser_executable(browser)
            .or_else(|| browser.executable_path.clone())
            .ok_or_else(|| format!("Could not find executable for {}.", browser.name))?;
        // Chrome 137 removed this switch. Do not close it for a no-op install.
        if browser.id == "chrome" && !exe.to_lowercase().contains("chromium") {
            if is_chromium_extension_installed(browser, app_data_dir) {
                return Ok(format!(
                    "Updated extension files for {}. Reload requested for connected extensions.",
                    browser.name
                ));
            }
            let _ = zoltraak_core::shell::reveal(&ext_dir);
            return Err("Google Chrome requires loading unpacked manually: extension folder opened and path copied. Go to chrome://extensions, enable Developer mode, and click 'Load unpacked'.".into());
        }
        register_native_host(app_data_dir);
        if find_running_browser_executable(browser).is_some() {
            close_chromium(browser)?;
        }
        let mut child = Command::new(&exe)
            .arg(format!("--load-extension={}", ext_dir.display()))
            .arg("--restore-last-session")
            .spawn()
            .map_err(|e| format!("Could not launch {}: {e}", browser.name))?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "{} exited before extension loading could be confirmed ({status}).",
                browser.name
            ));
        }
        Ok(format!(
            "{} launched with the unpacked extension and pairing token. Previous tabs will be restored.",
            browser.name
        ))
    })();
    ExtensionInstallResult {
        browser_id: browser.id.clone(),
        browser_name: browser.name.clone(),
        success: outcome.is_ok(),
        message: outcome.unwrap_or_else(|e| e),
        extension_path: ext_dir.to_string_lossy().into_owned(),
        next_step: None,
    }
}

/// Normal close only: never discard an unsaved browser session with /F.
fn close_chromium(browser: &DetectedBrowser) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let image = match browser.id.as_str() {
            "edge" => "msedge.exe",
            "brave" => "brave.exe",
            "opera" => "opera.exe",
            "vivaldi" => "vivaldi.exe",
            _ => "chrome.exe",
        };
        use std::os::windows::process::CommandExt;
        Command::new("taskkill")
            .args(["/IM", image])
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| format!("Could not close {}: {e}", browser.name))?;
    }
    for _ in 0..30 {
        if find_running_browser_executable(browser).is_none() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!(
        "{} is still running. Finish any browser save/close prompts, then retry installation.",
        browser.name
    ))
}

pub fn relaunch_chromium(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
    token: &str,
) -> ExtensionInstallResult {
    install_chromium(browser, app_data_dir, token, true)
}

pub fn remove_browser_extension(
    browser: &DetectedBrowser,
    app_data_dir: &Path,
) -> ExtensionInstallResult {
    if browser.kind == "firefox" {
        remove_gecko(browser)
    } else {
        remove_chromium(browser, app_data_dir)
    }
}

/// Reload the extension on all detected browsers by re-installing it.
/// This forces the browser to pick up any updated extension files.
pub fn reload_browser_extension(
    browser_id: &str,
    app_data_dir: &Path,
    token: &str,
    signed_xpi: Option<&Path>,
) -> Vec<ExtensionInstallResult> {
    let scanned = scan_browsers(app_data_dir);
    let targets: Vec<DetectedBrowser> =
        if browser_id == "all" || browser_id == "auto" || browser_id.is_empty() {
            scanned
                .into_iter()
                .filter(|browser| browser.installed)
                .collect()
        } else {
            scanned
                .into_iter()
                .filter(|browser| browser.id == browser_id && browser.installed)
                .collect()
        };

    if targets.is_empty() {
        return vec![ExtensionInstallResult {
            browser_id: browser_id.to_string(),
            browser_name: browser_id.to_string(),
            success: false,
            message: "No matching installed browser extension found.".to_string(),
            extension_path: String::new(),
            next_step: None,
        }];
    }

    let mut results = Vec::new();
    for target in &targets {
        let res =
            install_browser_extension_internal(target, app_data_dir, token, signed_xpi, false);
        results.push(res);
    }
    results
}

fn version_is_older(installed: &str, available: &str) -> bool {
    version_cmp(installed, available).is_lt()
}

fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let parts = |version: &str| {
        version
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let mut left = parts(left);
    let mut right = parts(right);
    let len = left.len().max(right.len());
    left.resize(len, 0);
    right.resize(len, 0);
    left.cmp(&right)
}

fn remove_gecko(browser: &DetectedBrowser) -> ExtensionInstallResult {
    if let Some(exe) = browser.executable_path.as_deref() {
        if let Some(policy_path) = gecko_policy_path(exe) {
            if policy_path.exists() {
                match fs::remove_file(&policy_path) {
                    Ok(_) => {
                        return ExtensionInstallResult {
                            browser_id: browser.id.clone(),
                            browser_name: browser.name.clone(),
                            success: true,
                            message: format!(
                                "Extension policy removed for {}. Restart the browser to complete removal.",
                                browser.name
                            ),
                            extension_path: String::new(),
                            next_step: None,
                        };
                    }
                    Err(e) => {
                        return ExtensionInstallResult {
                            browser_id: browser.id.clone(),
                            browser_name: browser.name.clone(),
                            success: false,
                            message: format!(
                                "Could not remove policy file for {} ({e}). Administrative permissions may be required.",
                                browser.name
                            ),
                            extension_path: String::new(),
                            next_step: None,
                        };
                    }
                }
            }
        }
    }

    ExtensionInstallResult {
        browser_id: browser.id.clone(),
        browser_name: browser.name.clone(),
        success: true,
        message: format!("Extension policy was not present for {}.", browser.name),
        extension_path: String::new(),
        next_step: None,
    }
}

fn remove_chromium(browser: &DetectedBrowser, app_data_dir: &Path) -> ExtensionInstallResult {
    let ext_dir = chromium_extension_dir(browser, app_data_dir);
    if ext_dir.exists() {
        match fs::remove_dir_all(&ext_dir) {
            Ok(_) => ExtensionInstallResult {
                browser_id: browser.id.clone(),
                browser_name: browser.name.clone(),
                success: true,
                message: format!(
                    "Extension files removed for {}. You can now remove or turn off the extension in your browser extensions page.",
                    browser.name
                ),
                extension_path: String::new(),
                next_step: None,
            },
            Err(e) => ExtensionInstallResult {
                browser_id: browser.id.clone(),
                browser_name: browser.name.clone(),
                success: false,
                message: format!(
                    "Could not remove extension directory for {} ({e}).",
                    browser.name
                ),
                extension_path: String::new(),
                next_step: None,
            },
        }
    } else {
        ExtensionInstallResult {
            browser_id: browser.id.clone(),
            browser_name: browser.name.clone(),
            success: true,
            message: format!("Extension files were not present for {}.", browser.name),
            extension_path: String::new(),
            next_step: None,
        }
    }
}

/// Purges all generated unpacked extension directories and temporary artifacts,
/// then regenerates fresh unpacked extension directories for detected Chromium browsers.
pub fn purge_and_regenerate_extension_files(
    app_data_dir: &Path,
    token: &str,
) -> Result<PurgeRegenerateResult, String> {
    let mut purged = Vec::new();
    let mut regenerated = Vec::new();

    // 1. Purge the extensions/ directory (unpacked per-browser extensions)
    let extensions_dir = app_data_dir.join("extensions");
    if extensions_dir.exists() {
        if let Ok(entries) = fs::read_dir(&extensions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if fs::remove_dir_all(&path).is_ok() {
                    purged.push(format!("extensions/{name}"));
                }
            }
        }
    }

    // 2. Clean unpacked files from extension/ directory while preserving any signed XPI
    let extension_dir = app_data_dir.join("extension");
    if extension_dir.exists() {
        if let Ok(entries) = fs::read_dir(&extension_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                // Keep signed XPI packages; purge unpacked JS, HTML, CSS, fonts, etc.
                if file_name.ends_with(".xpi") {
                    continue;
                }
                if path.is_dir() {
                    let _ = fs::remove_dir_all(&path);
                } else {
                    let _ = fs::remove_file(&path);
                }
                purged.push(format!("extension/{file_name}"));
            }
        }
    }

    // 3. Remove unsigned dev XPI if present
    let dev_xpi = app_data_dir.join("zoltraak_extension.xpi");
    if dev_xpi.exists() {
        let _ = fs::remove_file(&dev_xpi);
        purged.push("zoltraak_extension.xpi".into());
    }

    // 4. Re-register native host manifests
    let _ = register_native_host(app_data_dir);

    // 5. Regenerate fresh unpacked extensions for detected Chromium browsers
    let scanned = scan_browsers(app_data_dir);
    for browser in scanned.iter().filter(|b| b.kind == "chromium") {
        if prepare_chromium_extension_for_id(&browser.id, app_data_dir, token).is_ok() {
            regenerated.push(browser.name.clone());
        }
    }

    // If no chromium browsers were detected or installed, ensure standard default dirs are ready
    if regenerated.is_empty() {
        for fallback_id in ["brave", "chrome", "edge"] {
            if prepare_chromium_extension_for_id(fallback_id, app_data_dir, token).is_ok() {
                regenerated.push(fallback_id.to_string());
            }
        }
    }

    let message = if regenerated.is_empty() {
        "Purged extension files, but no browsers were ready to regenerate.".to_string()
    } else {
        format!(
            "Purged {} extension artifact(s) and regenerated fresh extensions for {}.",
            purged.len(),
            regenerated.join(", ")
        )
    };

    Ok(PurgeRegenerateResult {
        success: true,
        message,
        purged_paths: purged,
        regenerated_browsers: regenerated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn browser_process_refresh_preserves_names_and_executable_paths() {
        let sys = browser_processes();
        let current = sys.process(sysinfo::Pid::from_u32(std::process::id())).unwrap();
        assert!(!current.name().is_empty());
        assert_eq!(current.exe().unwrap(), std::env::current_exe().unwrap());
    }

    /// Manual benchmark: no timing threshold in the ordinary test suite.
    #[test]
    #[ignore]
    fn compare_browser_discovery_refresh_cost() {
        for round in 1..=3 {
            let start = std::time::Instant::now();
            let mut old = System::new_all();
            old.refresh_all();
            let old_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            let new = browser_processes();
            eprintln!("round={round} full_refresh_ms={old_ms:.2} browser_refresh_ms={:.2} old_processes={} new_processes={}",
                start.elapsed().as_secs_f64() * 1000.0, old.processes().len(), new.processes().len());
            assert!(new.process(sysinfo::Pid::from_u32(std::process::id())).is_some());
        }
    }

    #[test]
    fn chromium_package_contains_every_manifest_resource_and_is_isolated() {
        let root =
            std::env::temp_dir().join(format!("zoltraak-package-test-{}", std::process::id()));
        let browser = DetectedBrowser {
            id: "brave".into(),
            name: "Brave".into(),
            kind: "chromium".into(),
            running: false,
            executable_path: None,
            installed: false,
        };
        let dir = chromium_extension_dir(&browser, &root);
        write_extension_files(&dir, &"a".repeat(64), "brave").unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let api = fs::read_to_string(dir.join("api.js")).unwrap();
        assert!(api.contains("const BAKED_BROWSER_ID = \"brave\";"));
        assert_eq!(manifest["background"]["service_worker"], "background.js");
        assert!(manifest["background"].get("scripts").is_none());
        assert!(manifest.get("browser_specific_settings").is_none());
        assert!(
            !manifest["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "webRequestBlocking")
        );
        for script in manifest["content_scripts"].as_array().unwrap() {
            for file in script["js"].as_array().unwrap() {
                assert!(dir.join(file.as_str().unwrap()).is_file());
            }
        }
        for group in manifest["web_accessible_resources"].as_array().unwrap() {
            for file in group["resources"].as_array().unwrap() {
                assert!(dir.join(file.as_str().unwrap()).is_file());
            }
        }
        for file in [
            "placement-controller.js",
            "placement.css",
            "placement.html",
            "placement.js",
        ] {
            assert!(
                dir.join(file).is_file(),
                "generated extension omitted {file}"
            );
        }
        assert!(!extension_already_present(&browser, &root));
        fs::write(dir.join(".launch-configured"), "1").unwrap();
        assert!(
            !extension_already_present(&browser, &root),
            "a launch marker is not an installed extension"
        );
        let other = DetectedBrowser {
            id: "edge".into(),
            ..browser.clone()
        };
        assert!(!extension_already_present(&other, &root));
        let xpi = root.join("test.xpi");
        create_firefox_xpi(&dir, &xpi).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(xpi).unwrap()).unwrap();
        assert!(archive.by_name("fonts/archivo-subset.woff2").is_ok());
        assert!(archive.by_name("placement-controller.js").is_ok());
        drop(archive);
        fs::remove_dir_all(root).unwrap();
    }

    /// The token substitution is a string match across a language boundary:
    /// `api.js` declares the constant, this file rewrites it. Nothing but a
    /// test notices when one side is edited and the other is not — the
    /// replacement just stops matching, and every Chromium install ships
    /// unpaired with no error anywhere.
    #[test]
    fn the_token_declaration_in_api_js_is_still_the_one_we_rewrite() {
        assert!(
            EXT_API.contains(BAKED_TOKEN_DECL),
            "api.js must contain `{BAKED_TOKEN_DECL}` verbatim for the token to be baked in"
        );
        assert!(
            EXT_API.contains(BAKED_BROWSER_ID_DECL),
            "api.js must contain `{BAKED_BROWSER_ID_DECL}` verbatim for browser identification"
        );
    }

    #[test]
    fn source_manifest_leaves_the_chromium_key_to_the_chromium_packager() {
        let manifest: serde_json::Value = serde_json::from_str(EXT_MANIFEST).unwrap();
        assert!(manifest.get("key").is_none());
    }

    #[test]
    fn semantic_versions_compare_numerically() {
        assert!(version_is_older("0.1.9", "0.1.40"));
        assert!(!version_is_older("0.1.40", "0.1.40"));
        assert!(!version_is_older("0.2.0", "0.1.40"));
    }

    #[test]
    fn gecko_updates_change_install_url_and_preserve_other_policies() {
        let root =
            std::env::temp_dir().join(format!("zoltraak-gecko-policy-{}", std::process::id()));
        fs::create_dir_all(root.join("Browser space/distribution")).unwrap();
        let exe = root.join("Browser space/zen.exe");
        let policy = gecko_policy_path(exe.to_str().unwrap()).unwrap();
        fs::write(&policy, r#"{"policies":{"DisableTelemetry":true,"ExtensionSettings":{"other@example":{"installation_mode":"force_installed","install_url":"https://example.test/other.xpi"},"zoltraak@zoltraak.app":{"private_browsing":true}}}}"#).unwrap();
        let source = root.join("original signed.xpi");
        fs::write(&source, b"immutable signed package").unwrap();
        let first = stage_signed_gecko_package(&source, &root, "0.1.40").unwrap();
        let second = stage_signed_gecko_package(&source, &root, "0.1.41").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&second).unwrap(), fs::read(&source).unwrap());
        write_gecko_policy(exe.to_str().unwrap(), &second).unwrap();
        let content: serde_json::Value =
            serde_json::from_slice(&fs::read(&policy).unwrap()).unwrap();
        assert_eq!(content["policies"]["DisableTelemetry"], true);
        assert_eq!(
            content["policies"]["ExtensionSettings"]["other@example"]["installation_mode"],
            "force_installed"
        );
        assert_eq!(
            content["policies"]["ExtensionSettings"][GECKO_ID]["private_browsing"],
            true
        );
        assert_eq!(
            content["policies"]["ExtensionSettings"][GECKO_ID]["install_url"],
            reqwest::Url::from_file_path(&second).unwrap().as_str()
        );
        fs::write(&policy, b"invalid JSON").unwrap();
        assert!(write_gecko_policy(exe.to_str().unwrap(), &second).is_err());
        assert_eq!(fs::read(&policy).unwrap(), b"invalid JSON");
        assert!(stage_signed_gecko_package(&source, &root, "../escape").is_err());
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_version_comes_from_the_embedded_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(EXT_MANIFEST).unwrap();
        assert_eq!(extension_version(), manifest["version"].as_str().unwrap());
    }

    #[test]
    fn chromium_preference_entries_must_be_enabled_and_point_to_zoltraak() {
        let root = std::env::temp_dir().join(format!("zoltraak-entry-test-{}", std::process::id()));
        let profile = root.join("Default");
        let expected = root.join("expected");
        let workspace = root.join("workspace");
        let unrelated = root.join("opendm").join("extension");
        fs::create_dir_all(&profile).unwrap();
        write_extension_files(&expected, &"a".repeat(64), "chrome").unwrap();
        write_extension_files(&workspace, &"a".repeat(64), "chrome").unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(
            unrelated.join("manifest.json"),
            r#"{"manifest_version":3,"name":"OpenDM"}"#,
        )
        .unwrap();
        let target_id = zoltraak_api::auth::CHROMIUM_EXTENSION_ID;

        assert!(!valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({}),
            &profile,
            &expected,
            target_id
        ));
        assert!(!valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({"state": 0, "path": expected}),
            &profile,
            &expected,
            target_id
        ));
        assert!(!valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({"disable_reasons": [1], "path": expected}),
            &profile,
            &expected,
            target_id
        ));
        assert!(!valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({"path": root.join("missing")}),
            &profile,
            &expected,
            target_id
        ));
        assert!(!valid_chromium_extension_entry(
            "legacy",
            &serde_json::json!({"path": unrelated}),
            &profile,
            &expected,
            target_id
        ));
        assert!(valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({"path": expected}),
            &profile,
            &expected,
            target_id
        ));
        assert!(valid_chromium_extension_entry(
            "legacy",
            &serde_json::json!({"path": workspace}),
            &profile,
            &expected,
            target_id
        ));
        assert!(valid_chromium_extension_entry(
            target_id,
            &serde_json::json!({"manifest": {"name": "Zoltraak"}}),
            &profile,
            &expected,
            target_id
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn baking_a_token_replaces_the_declaration_exactly_once() {
        let token = "a".repeat(64);
        let baked = bake_token_into_api_js(EXT_API, &token);

        assert!(baked.contains(&format!("const BAKED_TOKEN = \"{token}\";")));
        assert!(
            !baked.contains(BAKED_TOKEN_DECL),
            "the empty declaration must be gone"
        );
        assert_eq!(baked.matches("const BAKED_TOKEN =").count(), 1);
    }

    #[test]
    fn baking_over_an_existing_token_replaces_it_cleanly() {
        let token1 = "1".repeat(64);
        let token2 = "2".repeat(64);
        let baked1 = bake_token_into_api_js(EXT_API, &token1);
        let baked2 = bake_token_into_api_js(&baked1, &token2);

        assert!(baked2.contains(&format!("const BAKED_TOKEN = \"{token2}\";")));
        assert!(!baked2.contains(&token1));
        assert_eq!(baked2.matches("const BAKED_TOKEN =").count(), 1);
    }

    #[test]
    fn test_matches_browser_process() {
        assert!(matches_browser_process("chrome", "chrome.exe"));
        assert!(matches_browser_process("edge", "msedge.exe"));
        assert!(matches_browser_process("brave", "brave.exe"));
        assert!(matches_browser_process("firefox", "firefox.exe"));
        assert!(matches_browser_process("zen", "zen.exe"));
        assert!(matches_browser_process("zen", "zen-browser.exe"));
        assert!(matches_browser_process("opera", "opera.exe"));
        assert!(matches_browser_process("vivaldi", "vivaldi.exe"));

        assert!(!matches_browser_process(
            "chrome",
            "chrome_crashpad_handler.exe"
        ));
        assert!(!matches_browser_process("chrome", "firefox.exe"));
        assert!(!matches_browser_process("firefox", "chrome.exe"));
    }

    #[test]
    fn test_detect_installed_chromium_extension_from_preferences() {
        let fake_profile =
            std::env::temp_dir().join(format!("fake-chrome-profile-{}", std::process::id()));
        fs::create_dir_all(&fake_profile).unwrap();
        let target_id = zoltraak_api::auth::CHROMIUM_EXTENSION_ID;
        let ext_dir = fake_profile.join("extensions").join("chrome");
        fs::create_dir_all(&ext_dir).unwrap();

        let pref_content = serde_json::json!({
            "extensions": {
                "settings": {
                    target_id: {
                        "path": ext_dir.to_string_lossy(),
                    }
                }
            }
        });
        fs::write(
            fake_profile.join("Secure Preferences"),
            serde_json::to_string(&pref_content).unwrap(),
        )
        .unwrap();

        let content = fs::read_to_string(fake_profile.join("Secure Preferences")).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        let settings = val
            .pointer("/extensions/settings")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(settings.contains_key(target_id));
        let path = settings[target_id].get("path").unwrap().as_str().unwrap();
        assert_eq!(path, ext_dir.to_string_lossy());

        fs::remove_dir_all(fake_profile).unwrap();
    }

    #[test]
    fn purge_and_regenerate_cleans_stale_files_and_regenerates_cleanly() {
        let temp_dir = std::env::temp_dir().join(format!("purge-test-{}", std::process::id()));
        let extensions_dir = temp_dir.join("extensions");
        let stale_browser_dir = extensions_dir.join("stale-browser");
        fs::create_dir_all(&stale_browser_dir).unwrap();
        fs::write(stale_browser_dir.join("junk.txt"), "junk").unwrap();

        let extension_dir = temp_dir.join("extension");
        fs::create_dir_all(&extension_dir).unwrap();
        fs::write(extension_dir.join("unpacked.js"), "console.log(1);").unwrap();
        fs::write(extension_dir.join("zoltraak-signed.xpi"), "xpi-bytes").unwrap();

        let token = "a".repeat(64);
        let result = purge_and_regenerate_extension_files(&temp_dir, &token).unwrap();
        assert!(result.success);
        assert!(
            !stale_browser_dir.exists(),
            "stale browser dir must be removed"
        );
        assert!(
            !extension_dir.join("unpacked.js").exists(),
            "unpacked files must be removed"
        );
        assert!(
            extension_dir.join("zoltraak-signed.xpi").exists(),
            "signed XPI must be preserved"
        );

        // Default Chromium dirs should be regenerated
        assert!(extensions_dir.join("brave").join("manifest.json").exists());
        assert!(extensions_dir.join("chrome").join("manifest.json").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
