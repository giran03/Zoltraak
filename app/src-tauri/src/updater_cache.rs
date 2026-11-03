//! One signed installer in the application's cache. Metadata commits last so
//! incomplete writes can never be mistaken for a downloaded release.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use std::{fs, path::Path};

const PACKAGE: &str = "installer.bin";
const METADATA: &str = "release.json";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CachedRelease {
    pub version: String,
    pub target: String,
    pub signature: String,
    pub download_url: String,
    pub body: Option<String>,
    pub size: u64,
}

pub fn verify(bytes: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
    let decode = |encoded: &str| -> Result<String, String> {
        String::from_utf8(STANDARD.decode(encoded).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    };
    let key = PublicKey::decode(&decode(pubkey)?).map_err(|e| e.to_string())?;
    let signature = Signature::decode(&decode(signature)?).map_err(|e| e.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|e| e.to_string())
}

pub fn save(dir: &Path, release: &CachedRelease, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || release.size != bytes.len() as u64 {
        return Err("Invalid installer size".into());
    }
    semver::Version::parse(&release.version).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // A marker is never committed while the package is being replaced.
    clear(dir)?;
    fs::write(dir.join("installer.part"), bytes).map_err(|e| e.to_string())?;
    fs::rename(dir.join("installer.part"), dir.join(PACKAGE)).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("release.part"),
        serde_json::to_vec(release).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(dir.join("release.part"), dir.join(METADATA)).map_err(|e| e.to_string())
}

pub fn metadata(dir: &Path) -> Result<Option<CachedRelease>, String> {
    match fs::read(dir.join(METADATA)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Invalid cached update metadata: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn load(dir: &Path, release: &CachedRelease, pubkey: &str) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(dir.join(PACKAGE)).map_err(|e| format!("Cached installer unavailable: {e}"))?;
    if bytes.is_empty() || bytes.len() as u64 != release.size {
        return Err("Cached installer is incomplete".into());
    }
    verify(&bytes, &release.signature, pubkey)
        .map_err(|e| format!("Cached installer signature verification failed: {e}"))?;
    Ok(bytes)
}

pub fn is_installed(current: &str, cached: &str) -> Result<bool, String> {
    let current = semver::Version::parse(current).map_err(|e| e.to_string())?;
    let cached = semver::Version::parse(cached).map_err(|e| e.to_string())?;
    Ok(current >= cached)
}

// Delete only our fixed cache files, never recursively remove app/user data.
pub fn clear(dir: &Path) -> Result<(), String> {
    for name in [PACKAGE, "installer.part", "release.part", METADATA] {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    // Public minisign test vector from minisign-verify. No signing secrets.
    fn signed_release() -> (CachedRelease, String) {
        let key = STANDARD.encode("untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3");
        let signature = STANDARD.encode("untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==");
        (
            CachedRelease {
                version: "1.2.3".into(),
                target: "windows-x86_64".into(),
                signature,
                download_url: "https://updates.test/app.exe".into(),
                body: Some("A release".into()),
                size: 4,
            },
            key,
        )
    }
    fn fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zoltraak-update-cache-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn installer_survives_restart_and_cleanup_preserves_unrelated_files() {
        let dir = fixture();
        let (release, key) = signed_release();
        save(&dir, &release, b"test").unwrap();
        // Read from disk as a new process would, with no in-memory bytes.
        let restored = metadata(&dir).unwrap().unwrap();
        assert_eq!(restored.version, "1.2.3");
        assert_eq!(load(&dir, &restored, &key).unwrap(), b"test");
        assert!(!is_installed("1.2.2", &restored.version).unwrap());
        assert!(is_installed("1.2.3", &restored.version).unwrap());
        assert!(is_installed("1.2.10", &restored.version).unwrap());
        assert!(!is_installed("1.2.3-beta.1", &restored.version).unwrap());
        fs::write(dir.join("unrelated.txt"), b"keep").unwrap();
        clear(&dir).unwrap();
        assert!(metadata(&dir).unwrap().is_none());
        assert!(!dir.join(PACKAGE).exists());
        assert_eq!(fs::read(dir.join("unrelated.txt")).unwrap(), b"keep");
        fs::remove_file(dir.join("unrelated.txt")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn corruption_is_rejected_and_failed_install_can_reuse_the_same_cache() {
        let dir = fixture();
        let (release, key) = signed_release();
        save(&dir, &release, b"test").unwrap();
        // A failed installation does not consume the package; retries read it.
        assert_eq!(
            load(&dir, &release, &key).unwrap(),
            load(&dir, &release, &key).unwrap()
        );
        fs::write(dir.join(PACKAGE), b"Test").unwrap();
        assert!(
            load(&dir, &release, &key)
                .unwrap_err()
                .contains("signature")
        );
        fs::write(dir.join(PACKAGE), b"tes").unwrap();
        assert!(
            load(&dir, &release, &key)
                .unwrap_err()
                .contains("incomplete")
        );
        clear(&dir).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn interrupted_write_never_marks_an_update_ready() {
        let dir = fixture();
        let (release, _) = signed_release();
        fs::write(dir.join("installer.part"), b"tes").unwrap();
        assert!(metadata(&dir).unwrap().is_none());
        assert!(save(&dir, &release, b"tes").is_err());
        save(&dir, &release, b"test").unwrap();
        assert!(metadata(&dir).unwrap().is_some());
        assert!(!dir.join("installer.part").exists());
        clear(&dir).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
