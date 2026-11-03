//! Engine-wide configuration: bandwidth limit, connection/folder defaults,
//! and the scheduler's time window and completion action.
//!
//! Stored as a single JSON blob rather than one column per field — this is
//! edited as a whole from one settings screen, never partially, so there is
//! no transactional reason to split it into columns, and it is one place to
//! extend instead of a migration per new field.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A daily start/stop window during which the supervisor is allowed to run
/// downloads. Outside it, in-flight downloads are stopped (left `waiting`,
/// same as a shutdown) and no new work is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TimeWindow {
    pub enabled: bool,
    /// Minutes since local midnight, `0..1440`.
    pub start_minutes: u16,
    pub stop_minutes: u16,
}

impl TimeWindow {
    /// Whether downloads may run when local time is `now_minutes` past
    /// midnight. Handles a window that wraps past midnight (`start >
    /// stop`, e.g. 22:00–06:00) by treating it as "outside the gap"
    /// instead.
    pub fn allows(&self, now_minutes: u16) -> bool {
        if !self.enabled {
            return true;
        }
        // A zero-width window is almost certainly an unconfigured default,
        // not an intent to block downloads forever.
        if self.start_minutes == self.stop_minutes {
            return true;
        }
        if self.start_minutes < self.stop_minutes {
            (self.start_minutes..self.stop_minutes).contains(&now_minutes)
        } else {
            now_minutes >= self.start_minutes || now_minutes < self.stop_minutes
        }
    }
}

/// What to do once the queue has nothing left to run.
///
/// Deliberately excludes sleep/hibernate/shutdown/VPN-disconnect: those are
/// hard to undo if triggered by a bug, and need a confirmation-gated design
/// of their own rather than riding along with the rest of settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CompletionAction {
    #[default]
    None,
    /// Open `EngineSettings::default_directory` in the OS file manager.
    OpenFolder,
    /// Run a shell command. No output is captured — this is fire-and-forget,
    /// the same trust level as running it in a terminal yourself.
    RunCommand { command: String },
    /// Exit the process. For the CLI's `serve`/`run`, this ends the
    /// invocation; for the desktop app, the engine and the app share a
    /// process, so this closes the app too.
    ExitApp,
}

/// How Zoltraak reaches Discord.
///
/// There is deliberately no "post as me" mode: Discord exposes no OAuth scope
/// that lets an application send messages as a human account, and the only
/// mechanism that would — a user account token — is against their terms and
/// gets accounts terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordMode {
    /// A webhook URL pasted from a channel's settings. No Discord application
    /// needed, but the webhook token cannot read guild metadata, so the upload
    /// limit has to be assumed rather than detected.
    #[default]
    Webhook,
    /// A bot token from the user's own Discord application. Costs one extra
    /// setup step and buys server/channel discovery, a real upload limit, and
    /// a bot Zoltraak can rename and invite.
    Bot,
}

impl DiscordMode {
    pub fn is_bot(self) -> bool {
        matches!(self, Self::Bot)
    }
}

/// Everything about Discord delivery that is *not* a credential. The bot token
/// and webhook URL live in [`crate::vault::SecretVault`] instead — they are
/// credentials, and this struct is handed out over IPC and REST in the clear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordSettings {
    /// Master switch. Off means no delivery happens by any route, manual
    /// included, so there is one place to stop everything.
    pub enabled: bool,
    pub mode: DiscordMode,
    /// Deliver every finished download without being asked.
    pub auto_send: bool,
    pub webhook_username: Option<String>,
    pub webhook_avatar_url: Option<String>,
    pub webhook_avatar_asset_id: Option<String>,
    pub ping_message_template: String,
    pub discord_user_id: Option<String>,
    pub ping_sender: bool,
    /// Bot-mode target. Webhook mode carries its channel inside the URL.
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    /// Last-known display names, cached so the settings screen can render the
    /// current target before any Discord call comes back — or at all, when
    /// the token has since been revoked.
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
    pub bot_username: Option<String>,
    /// Detected from the guild's boost tier in bot mode, set by the user in
    /// webhook mode, and lowered whenever Discord actually rejects an upload.
    /// `None` means [`DEFAULT_UPLOAD_LIMIT`].
    pub upload_limit_bytes: Option<u64>,
    /// User-selected bot upload budget, independent of guild discovery.
    /// Stored in bytes; the application edits this in decimal megabytes.
    pub bot_upload_limit_bytes: u64,
    /// Re-encode oversized media to fit under the limit instead of skipping.
    pub compress_to_fit: bool,
    /// Where the transcode ladder starts. It steps down on its own when the
    /// bitrate a file's duration allows would be too low for this height.
    pub compress_max_height: u32,
    /// Caption posted alongside the files. Empty — the default — means none:
    /// the attachments speak for themselves, and a line of
    /// `MOONDROP_-_Finally_EVO_2….jpg · 133 KiB` above every picture is just
    /// the filename read back at you.
    ///
    /// Supports `{filename}`, `{size}`, `{host}`, `{url}` and `{category}`.
    pub message_template: String,
    pub include_source_url: bool,
    pub notify_on_delivery: bool,
}

/// Default application preparation budget; Discord may enforce a lower limit.
pub const DEFAULT_UPLOAD_LIMIT: u64 = 20_000_000;
pub const DEFAULT_BOT_UPLOAD_LIMIT: u64 = 20_000_000;

impl Default for DiscordSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: DiscordMode::Webhook,
            auto_send: false,
            webhook_username: None,
            webhook_avatar_url: None,
            webhook_avatar_asset_id: None,
            ping_message_template: "{discord user id}".into(),
            discord_user_id: None,
            ping_sender: false,
            guild_id: None,
            channel_id: None,
            guild_name: None,
            channel_name: None,
            bot_username: None,
            upload_limit_bytes: None,
            bot_upload_limit_bytes: DEFAULT_BOT_UPLOAD_LIMIT,
            compress_to_fit: true,
            compress_max_height: 720,
            message_template: String::new(),
            include_source_url: false,
            notify_on_delivery: true,
        }
    }
}

/// The caption Zoltraak used to default to, before files were sent uncaptioned.
///
/// Kept only so it can be recognised and dropped: it was stored verbatim in
/// every install that never touched the field, so changing the default alone
/// would have left existing users with a caption they never chose.
pub const LEGACY_CAPTION_TEMPLATE: &str = "{filename} · {size}";

impl DiscordSettings {
    /// Bring settings written by an older version up to date.
    ///
    /// Only the *untouched* old default is cleared — a template the user
    /// edited, even to something similar, is theirs and is left alone.
    pub fn migrate(&mut self) {
        if self.message_template == LEGACY_CAPTION_TEMPLATE {
            self.message_template.clear();
        }
    }

    /// The limit to plan against: what we know, or the unboosted default.
    pub fn upload_limit(&self) -> u64 {
        if self.mode.is_bot() {
            return if self.bot_upload_limit_bytes > 0 { self.bot_upload_limit_bytes } else { DEFAULT_BOT_UPLOAD_LIMIT };
        }
        self.upload_limit_bytes.filter(|n| *n > 0).unwrap_or(DEFAULT_UPLOAD_LIMIT)
    }

    /// Whether a delivery may be attempted at all. Says nothing about whether
    /// the credential behind it still works — only Discord can answer that.
    pub fn is_configured(&self) -> bool {
        self.enabled && (!self.mode.is_bot() || self.channel_id.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EngineSettings {
    /// `None` or `Some(0)` means unlimited. Shared across every concurrent
    /// download, not per-download.
    pub bandwidth_limit_bytes_per_sec: Option<u64>,
    /// Overrides the size-based tier from [`crate::default_connections`] for
    /// any download that does not specify its own connection count.
    pub default_connections: Option<u8>,
    /// Where a download lands when nothing more specific claims it — a
    /// category rule still wins; see [`crate::queue::Queue::enqueue`].
    pub default_directory: Option<PathBuf>,
    pub desktop_notifications: bool,
    pub play_sound_on_complete: bool,
    pub start_on_launch: bool,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    /// Per-entry trash buttons also delete the downloaded file.
    pub delete_files_on_remove: bool,
    pub schedule: TimeWindow,
    pub on_complete: CompletionAction,
    pub discord: DiscordSettings,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            bandwidth_limit_bytes_per_sec: None,
            default_connections: None,
            default_directory: None,
            desktop_notifications: true,
            play_sound_on_complete: true,
            start_on_launch: false,
            close_to_tray: true,
            minimize_to_tray: false,
            delete_files_on_remove: true,
            schedule: TimeWindow::default(),
            on_complete: CompletionAction::None,
            discord: DiscordSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(start: u16, stop: u16) -> TimeWindow {
        TimeWindow {
            enabled: true,
            start_minutes: start,
            stop_minutes: stop,
        }
    }

    #[test]
    fn a_disabled_window_always_allows() {
        let w = TimeWindow {
            enabled: false,
            start_minutes: 60,
            stop_minutes: 120,
        };
        assert!(w.allows(0));
        assert!(w.allows(90));
        assert!(w.allows(1439));
    }

    #[test]
    fn an_ordinary_same_day_window() {
        let w = window(60, 120); // 01:00–02:00
        assert!(!w.allows(0));
        assert!(w.allows(60));
        assert!(w.allows(90));
        assert!(!w.allows(120), "the stop minute itself is exclusive");
        assert!(!w.allows(1000));
    }

    #[test]
    fn a_window_that_wraps_past_midnight() {
        let w = window(22 * 60, 6 * 60); // 22:00–06:00
        assert!(w.allows(23 * 60), "23:00 is inside the overnight window");
        assert!(w.allows(0), "midnight is inside the overnight window");
        assert!(w.allows(5 * 60 + 59));
        assert!(!w.allows(6 * 60), "06:00 itself is the stop minute");
        assert!(!w.allows(12 * 60), "noon is outside an overnight window");
    }

    #[test]
    fn a_zero_width_window_never_blocks() {
        let w = window(300, 300);
        assert!(w.allows(0));
        assert!(w.allows(300));
        assert!(w.allows(1439));
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = EngineSettings {
            bandwidth_limit_bytes_per_sec: Some(500_000),
            default_connections: Some(8),
            default_directory: Some(PathBuf::from("/downloads")),
            desktop_notifications: true,
            start_on_launch: true,
            close_to_tray: true,
            minimize_to_tray: false,
            delete_files_on_remove: false,
            play_sound_on_complete: true,
            schedule: window(60, 120),
            on_complete: CompletionAction::RunCommand {
                command: "notify-send done".into(),
            },
            discord: DiscordSettings {
                enabled: true,
                mode: DiscordMode::Bot,
                channel_id: Some("123".into()),
                upload_limit_bytes: Some(50 * 1024 * 1024),
                ..DiscordSettings::default()
            },
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: EngineSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, back);
    }

    #[test]
    fn defaults_are_unlimited_and_unscheduled() {
        let s = EngineSettings::default();
        assert!(s.delete_files_on_remove);
        assert_eq!(s.bandwidth_limit_bytes_per_sec, None);
        assert_eq!(s.default_connections, None);
        assert_eq!(s.default_directory, None);
        assert!(s.desktop_notifications);
        assert!(!s.start_on_launch);
        assert!(s.close_to_tray);
        assert!(!s.minimize_to_tray);
        assert!(s.play_sound_on_complete);
        assert!(!s.schedule.enabled);
        assert_eq!(s.on_complete, CompletionAction::None);
        assert!(!s.discord.enabled);
        assert!(!s.discord.auto_send);
    }

    #[test]
    fn settings_written_before_discord_existed_still_load() {
        // The blob is stored whole, so every older build's JSON must keep
        // deserialising as new fields are added.
        let older = r#"{"bandwidthLimitBytesPerSec":null,"defaultConnections":8,"closeToTray":true}"#;
        let s: EngineSettings = serde_json::from_str(older).unwrap();
        assert!(s.delete_files_on_remove);
        assert_eq!(s.default_connections, Some(8));
        assert_eq!(s.discord, DiscordSettings::default());
    }

    #[test]
    fn the_upload_limit_falls_back_to_the_unboosted_default() {
        let mut d = DiscordSettings::default();
        assert_eq!(d.upload_limit(), DEFAULT_UPLOAD_LIMIT);
        d.upload_limit_bytes = Some(0);
        assert_eq!(d.upload_limit(), DEFAULT_UPLOAD_LIMIT, "zero is an unset field, not a ban on uploads");
        d.upload_limit_bytes = Some(100 * 1024 * 1024);
        assert_eq!(d.upload_limit(), 100 * 1024 * 1024);
    }

    #[test]
    fn bot_upload_limit_defaults_to_twenty_mb_and_roundtrips_custom_values() {
        let mut d: DiscordSettings = serde_json::from_str(r#"{"mode":"bot","uploadLimitBytes":10485760}"#).unwrap();
        assert_eq!(d.upload_limit(), 20_000_000, "older settings receive the bot default");
        d.bot_upload_limit_bytes = 32_500_000;
        let restored: DiscordSettings = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(restored.upload_limit(), 32_500_000);
        assert_eq!(restored.upload_limit_bytes, Some(10485760), "guild discovery is stored separately");
    }

    #[test]
    fn bot_mode_is_unconfigured_until_a_channel_is_chosen() {
        let mut d = DiscordSettings { enabled: true, mode: DiscordMode::Bot, ..Default::default() };
        assert!(!d.is_configured());
        d.channel_id = Some("42".into());
        assert!(d.is_configured());

        let webhook = DiscordSettings { enabled: true, ..Default::default() };
        assert!(webhook.is_configured(), "a webhook URL carries its own channel");
        assert!(!DiscordSettings::default().is_configured(), "disabled is disabled");
    }
}
