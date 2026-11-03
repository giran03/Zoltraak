fn main() {
    // Validate direct cargo/tauri release builds as well as the batch launcher.
    // Development builds need no bundled media tools.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let script = root.join("../../scripts/stage_media_tools.ps1");
        println!("cargo:rerun-if-changed={}", script.display());
        println!("cargo:rerun-if-changed=../../scripts/media_tools.lock.json");
        println!("cargo:rerun-if-changed=resources/bin");
        println!("cargo:rerun-if-changed=resources/zoltraak-signed.xpi");
        assert!(
            root.join("resources/zoltraak-signed.xpi").is_file(),
            "Signed Firefox extension is missing"
        );
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .arg("-ValidateOnly")
            .status()
            .expect("Could not validate complete-install resources");
        assert!(
            status.success(),
            "Media resource validation failed; run scripts/stage_media_tools.ps1"
        );
    }
    tauri_build::build()
}
