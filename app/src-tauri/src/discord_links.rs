//! Activity links prefer an already-running Discord client, otherwise the browser.
use std::io;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

fn message_links(value: &str) -> Result<(String, String), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid Discord message link.")?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com"))
        || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        || url.query().is_some() || url.fragment().is_some() {
        return Err("Only Discord message links can be opened here.".into());
    }
    let parts: Vec<_> = url.path().trim_end_matches('/').split('/').collect();
    let id = |value: &str| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) && value.parse::<u64>().is_ok_and(|n| n > 0);
    if parts.len() != 5 || parts[1] != "channels" || !(parts[2] == "@me" || id(parts[2])) || !id(parts[3]) || !id(parts[4]) {
        return Err("Invalid Discord message link.".into());
    }
    let path = format!("/channels/{}/{}/{}", parts[2], parts[3], parts[4]);
    Ok((format!("https://{}{path}", url.host_str().unwrap()), format!("discord://-{path}")))
}

fn discord_process(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().trim_end_matches(".exe"),
        "discord" | "discordptb" | "discordcanary" | "discorddevelopment" | "discord ptb" | "discord canary" | "discord development")
}

fn discord_running() -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system.processes().values().any(|process| discord_process(&process.name().to_string_lossy()))
}

fn open_native(uri: &str) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL};
        let wide: Vec<_> = std::ffi::OsStr::new(uri).encode_wide().chain(Some(0)).collect();
        // Unlike spawning rundll32, ShellExecute reports a missing protocol handler.
        let result = unsafe { ShellExecuteW(std::ptr::null_mut(), std::ptr::null(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) } as isize;
        if result <= 32 { return Err(io::Error::other(format!("Discord link handler failed ({result})."))); }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        if !std::process::Command::new(opener).arg(uri).status()?.success() {
            return Err(io::Error::other("Discord link handler failed."));
        }
    }
    Ok(())
}

fn open_with(value: &str, running: bool, native: impl FnOnce(&str) -> io::Result<()>, web: impl FnOnce(&str) -> io::Result<()>) -> Result<(), String> {
    let (web_url, native_uri) = message_links(value)?;
    if running && native(&native_uri).is_ok() { return Ok(()); }
    web(&web_url).map_err(|e| format!("Could not open the Discord message: {e}"))
}

pub fn open(value: &str) -> Result<(), String> {
    open_with(value, discord_running(), open_native, zoltraak_core::shell::open_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn open_prefers_running_discord_and_falls_back_when_closed_or_handler_fails() {
        let url = "https://discord.com/channels/1/2/3";
        for (running, fail_native, expected) in [
            (true, false, vec!["discord://-/channels/1/2/3"]),
            (false, false, vec![url]),
            (true, true, vec!["discord://-/channels/1/2/3", url]),
        ] {
            let calls = RefCell::new(Vec::new());
            open_with(url, running, |uri| {
                calls.borrow_mut().push(uri.to_owned());
                if fail_native { Err(io::Error::other("missing handler")) } else { Ok(()) }
            }, |url| { calls.borrow_mut().push(url.to_owned()); Ok(()) }).unwrap();
            assert_eq!(*calls.borrow(), expected);
        }
        assert_eq!(message_links("https://discord.com/channels/@me/2/3").unwrap().1, "discord://-/channels/@me/2/3");
        for invalid in ["https://evil.example/channels/1/2/3", "https://discord.com.evil.example/channels/1/2/3", "https://discord.com/invite/abc", "https://discord.com/channels/1/2/3?extra=1", "discord://-/channels/1/2/3"] {
            assert!(open_with(invalid, true, |_| panic!("must not open"), |_| panic!("must not open")).is_err());
        }
        for process in ["Discord.exe", "DiscordPTB.exe", "DiscordCanary", "Discord", "Discord PTB"] { assert!(discord_process(process)); }
        for process in ["Update.exe", "discord-updater", "chrome", "discord-helper"] { assert!(!discord_process(process)); }
    }
}
