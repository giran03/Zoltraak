//! Delivering finished files to Discord.
//!
//! The shape of the feature is set by two facts about Discord:
//!
//! 1. **There is no way to post as the signed-in user.** No OAuth scope grants
//!    it; the only mechanism is a user account token, which is against
//!    Discord's terms and gets accounts terminated. So a delivery is always
//!    from a webhook or from a bot the user owns.
//! 2. **The upload ceiling belongs to the server**, and it is small — 10 MiB
//!    unboosted. A download manager mostly produces files larger than that, so
//!    fitting the file to the limit is the normal path, not an edge case. See
//!    [`limits`] and [`fit`].
//!
//! Everything lives in the engine rather than the desktop app so the REST API
//! and the CLI get the same feature, and so the credential never has to travel
//! to a client to be used.

pub mod delivery;
pub mod fit;
pub mod limits;
pub mod profile;
pub mod rest;
#[cfg(test)]
mod reconnect_tests;
#[cfg(test)]
mod send_tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::queue::Queue;
use crate::settings::{DiscordMode, DiscordSettings};
use crate::vault::{SecretVault, keys};

pub use delivery::{Delivery, DeliverySource, DeliveryState, NewDelivery};
pub use rest::{Channel, Guild, Target};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscordSendOptions {
    pub caption: Option<String>,
    pub ping_sender: Option<bool>,
    pub include_source_url: Option<bool>,
    pub compress_to_fit: Option<bool>,
    pub expected_destination: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSendOptions {
    pub caption: String,
    #[serde(default = "default_ping_template")]
    pub ping_message_template: String,
    pub ping_user: Option<String>,
    pub source_url: Option<String>,
    pub compress_to_fit: bool,
    pub destination: String,
}
fn default_ping_template() -> String { "{discord user id}".into() }

pub fn valid_user_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) && value.parse::<u64>().is_ok_and(|n| n > 0)
}
fn message_options(settings: &DiscordSettings, ping_user: Option<String>) -> rest::MessageOptions {
    rest::MessageOptions { username: settings.webhook_username.clone(), avatar_url: settings.webhook_avatar_url.clone(), ping_user, ping_message_template: Some(settings.ping_message_template.clone()) }
}

/// What the settings screen needs to draw itself, and nothing a caller could
/// authenticate with. The credentials stay in the vault; only their presence
/// and a masked tail leave the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordStatus {
    pub enabled: bool,
    pub mode: DiscordMode,
    pub auto_send: bool,
    /// Whether a credential for the *current* mode is stored.
    pub has_credential: bool,
    /// Last four characters of that credential, for telling two apart.
    pub credential_hint: Option<String>,
    pub destination_ref: Option<String>,
    /// A credential and target are configured; this is not a live health check.
    pub connected: bool,
    pub bot_username: Option<String>,
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub upload_limit_bytes: u64,
    pub upload_limit_label: String,
    /// Whether the limit was read from the server or is being assumed.
    pub upload_limit_detected: bool,
    pub compress_to_fit: bool,
    /// Set when the last attempt to reach Discord failed, so the settings
    /// screen can show why without the user pressing Test again.
    pub last_error: Option<String>,
}

/// The result of connecting a credential: what Zoltraak found on the other end.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub bot_username: Option<String>,
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
    pub channel_id: Option<String>,
    pub guild_id: Option<String>,
    /// Present in bot mode: the link that adds this bot to a server.
    pub invite_url: Option<String>,
    pub upload_limit_bytes: u64,
}

struct Inner {
    delete_lock: Mutex<()>,
    rest: rest::Rest,
    queue: Queue,
    vault: Arc<SecretVault>,
    /// Woken when a delivery is added, so the worker does not poll.
    wake: Notify,
    /// Cancels the in-flight delivery — used on shutdown and when the user
    /// disconnects mid-upload.
    cancel: CancellationToken,
    /// Surfaced in [`DiscordStatus`] so a failure is visible without the user
    /// having to go and find it in the delivery list.
    last_error: RwLock<Option<String>>,
    /// Whether the upload limit currently held came from a guild rather than
    /// from a guess. Not persisted: it is only worth knowing within a run.
    limit_detected: RwLock<bool>,
}

/// Sends finished files to Discord, one at a time.
///
/// Cloneable over shared state, like [`crate::video::manager::VideoManager`].
/// Deliveries run strictly sequentially: parallel uploads would only race each
/// other into Discord's per-channel rate limit, and re-encoding two files at
/// once would take CPU away from the downloads that are the point of the app.
#[derive(Clone)]
pub struct DiscordService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DiscordService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DiscordService")
    }
}

impl DiscordService {
    pub fn new(client: reqwest::Client, queue: Queue, vault: Arc<SecretVault>) -> Self {
        Self {
            inner: Arc::new(Inner {
                delete_lock: Mutex::new(()),
                rest: rest::Rest::new(client),
                queue,
                vault,
                wake: Notify::new(),
                cancel: CancellationToken::new(),
                last_error: RwLock::new(None),
                limit_detected: RwLock::new(false),
            }),
        }
    }

    // ---- settings -------------------------------------------------------

    pub async fn settings(&self) -> Result<DiscordSettings> {
        Ok(self.inner.queue.load_settings().await?.discord)
    }

    /// Replace the Discord section of the engine settings, leaving the rest
    /// untouched. Read-modify-write because settings are one JSON blob.
    pub async fn update_settings(&self, mut discord: DiscordSettings) -> Result<DiscordSettings> {
        if discord.mode.is_bot() && !(1_000_000..=1_000_000_000).contains(&discord.bot_upload_limit_bytes) {
            return Err(Error::InvalidInput("Bot upload limit must be between 1 and 1000 megabytes.".into()));
        }
        for value in [&mut discord.webhook_username, &mut discord.webhook_avatar_url, &mut discord.discord_user_id] {
            *value = value.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        }
        if discord.webhook_username.as_ref().is_some_and(|s| s.chars().count() > 80) { return Err(Error::InvalidInput("Sender name must be at most 80 characters.".into())); }
        if let Some(url) = &discord.webhook_avatar_url {
            if !reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some()) { return Err(Error::InvalidInput("Avatar image must be an HTTP or HTTPS URL.".into())); }
        }
        if discord.discord_user_id.as_ref().is_some_and(|s| !valid_user_id(s)) { return Err(Error::InvalidInput("Discord user ID must be a positive decimal snowflake.".into())); }
        if discord.discord_user_id.is_none() { discord.ping_sender = false; }
        if discord.ping_message_template.chars().count() > 500 { return Err(Error::InvalidInput("Ping message template must be at most 500 characters.".into())); }
        let current = self.settings().await?;
        if discord.webhook_avatar_url != current.webhook_avatar_url { discord.webhook_avatar_asset_id = None; }
        if discord.upload_limit_bytes != current.upload_limit_bytes && discord.upload_limit_bytes.is_some_and(|n| !(1_000_000..=1_000_000_000).contains(&n)) { return Err(Error::InvalidInput("Webhook upload limit must be between 1 and 1000 MB.".into())); }
        let mut settings = self.inner.queue.load_settings().await?;
        settings.discord = discord.clone();
        self.inner.queue.save_settings(&settings).await?;
        // A newly-enabled service may have deliveries waiting from before.
        self.inner.wake.notify_one();
        Ok(discord)
    }

    async fn mutate_settings(&self, edit: impl FnOnce(&mut DiscordSettings)) -> Result<DiscordSettings> {
        let mut settings = self.inner.queue.load_settings().await?;
        edit(&mut settings.discord);
        self.inner.queue.save_settings(&settings).await?;
        Ok(settings.discord)
    }

    pub async fn status(&self) -> Result<DiscordStatus> {
        let settings = self.settings().await?;
        let key = credential_key(settings.mode);
        let hint = self.inner.vault.masked(key).await;
        let has_credential = hint.is_some();
        let target_chosen = !settings.mode.is_bot() || settings.channel_id.is_some();
        let destination_ref = self.target().await.ok().map(|t| t.reference());
        Ok(DiscordStatus {
            destination_ref,
            enabled: settings.enabled,
            mode: settings.mode,
            auto_send: settings.auto_send,
            has_credential,
            credential_hint: hint,
            connected: has_credential && target_chosen,
            bot_username: settings.bot_username.clone(),
            guild_name: settings.guild_name.clone(),
            channel_name: settings.channel_name.clone(),
            guild_id: settings.guild_id.clone(),
            channel_id: settings.channel_id.clone(),
            upload_limit_bytes: settings.upload_limit(),
            upload_limit_label: limits::human_bytes(settings.upload_limit()),
            upload_limit_detected: *self.inner.limit_detected.read().await,
            compress_to_fit: settings.compress_to_fit,
            last_error: self.inner.last_error.read().await.clone(),
        })
    }

    // ---- connecting -----------------------------------------------------

    /// Revalidate the saved credential without enabling delivery or replacing
    /// preferences. All network checks finish before any metadata is committed.
    pub async fn reconnect(&self) -> Result<Connection> {
        let result = self.reconnect_saved().await;
        if let Err(error) = &result {
            *self.inner.last_error.write().await = Some(error.to_string());
        }
        result
    }

    async fn reconnect_saved(&self) -> Result<Connection> {
        let settings = self.settings().await?;
        let credential = self.inner.vault.get(credential_key(settings.mode)).await
            .ok_or_else(|| Error::InvalidInput("No saved Discord credential. Connect a webhook or bot first.".into()))?;
        let mut refreshed = settings.clone();
        let mut detected = false;
        match settings.mode {
            DiscordMode::Webhook => {
                let info = self.inner.rest.webhook_info(&credential).await?;
                refreshed.bot_username = info.name;
                refreshed.channel_id = info.channel_id;
                refreshed.guild_id = info.guild_id;
                if refreshed.channel_id != settings.channel_id { refreshed.channel_name = None; }
                if refreshed.guild_id != settings.guild_id { refreshed.guild_name = None; }
            }
            DiscordMode::Bot => {
                let identity = self.inner.rest.identity(&credential).await?;
                refreshed.bot_username = Some(identity.username);
                let guild_id = settings.guild_id.as_deref().ok_or_else(|| Error::InvalidInput("Choose a Discord server and channel, then reconnect.".into()))?;
                let channel_id = settings.channel_id.as_deref().ok_or_else(|| Error::InvalidInput("Choose a Discord channel, then reconnect.".into()))?;
                let guild = self.inner.rest.guild(&credential, guild_id).await?;
                let channel = self.inner.rest.channels(&credential, guild_id).await?.into_iter()
                    .find(|channel| channel.id == channel_id)
                    .ok_or_else(|| Error::InvalidInput("The saved Discord channel is no longer available. Choose another channel.".into()))?;
                refreshed.guild_name = Some(guild.name);
                refreshed.channel_name = channel.name;
                refreshed.upload_limit_bytes = Some(limits::for_premium_tier(guild.premium_tier));
                detected = true;
            }
        }
        // Do not overwrite changes made while the network calls were in flight.
        let mut current = self.inner.queue.load_settings().await?;
        if current.discord.mode != settings.mode || current.discord.guild_id != settings.guild_id
            || current.discord.channel_id != settings.channel_id
            || self.inner.vault.get(credential_key(settings.mode)).await.as_deref() != Some(credential.as_str()) {
            return Err(Error::InvalidInput("Discord configuration changed. Reconnect again.".into()));
        }
        current.discord.bot_username = refreshed.bot_username;
        current.discord.channel_id = refreshed.channel_id;
        current.discord.guild_id = refreshed.guild_id;
        current.discord.channel_name = refreshed.channel_name;
        current.discord.guild_name = refreshed.guild_name;
        if detected && !current.discord.mode.is_bot() { current.discord.upload_limit_bytes = refreshed.upload_limit_bytes; }
        self.inner.queue.save_settings(&current).await?;
        *self.inner.limit_detected.write().await = detected;
        self.clear_error().await;
        let d = current.discord;
        Ok(Connection {
            upload_limit_bytes: d.upload_limit(),
            bot_username: d.bot_username, guild_name: d.guild_name, channel_name: d.channel_name,
            channel_id: d.channel_id, guild_id: d.guild_id, invite_url: None,
        })
    }

    /// Store a webhook URL, after checking it points at something real.
    ///
    /// Validating before storing means a typo is reported while the user is
    /// still looking at the field, rather than as a failed delivery later.
    pub async fn connect_webhook(&self, url: &str) -> Result<Connection> {
        let info = self.inner.rest.webhook_info(url).await.inspect_err(|e| self.remember_error(e))?;
        self.inner.vault.set(keys::DISCORD_WEBHOOK_URL, url).await.map_err(vault_error)?;
        let settings = self
            .mutate_settings(|d| {
                d.mode = DiscordMode::Webhook;
                d.enabled = true;
                d.channel_id = info.channel_id.clone();
                d.guild_id = info.guild_id.clone();
                d.channel_name = None;
                d.guild_name = None;
                d.bot_username = info.name.clone();
                // Preserve the independent webhook upload budget.
            })
            .await?;
        *self.inner.limit_detected.write().await = false;
        self.clear_error().await;
        Ok(Connection {
            bot_username: info.name,
            guild_name: None,
            channel_name: None,
            channel_id: info.channel_id,
            guild_id: info.guild_id,
            invite_url: None,
            upload_limit_bytes: settings.upload_limit(),
        })
    }

    /// Store a bot token, after checking Discord accepts it.
    ///
    /// The target channel is not set here — the user still has to invite the
    /// bot to a server and pick one, which is what [`Self::set_target`] is for.
    pub async fn connect_bot(&self, token: &str) -> Result<Connection> {
        let identity = self.inner.rest.identity(token).await.inspect_err(|e| self.remember_error(e))?;
        self.inner.vault.set(keys::DISCORD_BOT_TOKEN, token).await.map_err(vault_error)?;
        let application_id = self.inner.rest.application_id(token).await.unwrap_or_else(|_| identity.id.clone());
        let settings = self
            .mutate_settings(|d| {
                d.mode = DiscordMode::Bot;
                d.enabled = true;
                d.bot_username = Some(identity.username.clone());
            })
            .await?;
        self.clear_error().await;
        Ok(Connection {
            bot_username: Some(identity.username),
            guild_name: settings.guild_name.clone(),
            channel_name: settings.channel_name.clone(),
            channel_id: settings.channel_id.clone(),
            guild_id: settings.guild_id.clone(),
            invite_url: Some(rest::Rest::invite_url(&application_id)),
            upload_limit_bytes: settings.upload_limit(),
        })
    }

    /// Forget the credential for the current mode and stop delivering.
    pub async fn disconnect(&self) -> Result<()> {
        let settings = self.settings().await?;
        self.inner.vault.remove(credential_key(settings.mode)).await.map_err(vault_error)?;
        self.mutate_settings(|d| {
            d.enabled = false;
            d.channel_id = None;
            d.guild_id = None;
            d.channel_name = None;
            d.guild_name = None;
            d.bot_username = None;
            // Preserve upload preferences when disconnecting.
        })
        .await?;
        self.clear_error().await;
        Ok(())
    }

    pub async fn invite_url(&self) -> Result<String> {
        let token = self.require_bot_token().await?;
        let identity = self.inner.rest.identity(&token).await?;
        let application_id = self.inner.rest.application_id(&token).await.unwrap_or(identity.id);
        Ok(rest::Rest::invite_url(&application_id))
    }

    pub async fn update_bot(&self, username: Option<&str>, avatar_data_uri: Option<&str>) -> Result<String> {
        let token = self.require_bot_token().await?;
        let identity = self.inner.rest.update_bot(&token, username, avatar_data_uri).await.inspect_err(|e| self.remember_error(e))?;
        self.mutate_settings(|d| d.bot_username = Some(identity.username.clone())).await?;
        Ok(identity.username)
    }

    pub async fn guilds(&self) -> Result<Vec<Guild>> {
        let token = self.require_bot_token().await?;
        self.inner.rest.guilds(&token).await.inspect_err(|e| self.remember_error(e))
    }

    pub async fn channels(&self, guild_id: &str) -> Result<Vec<Channel>> {
        let token = self.require_bot_token().await?;
        self.inner.rest.channels(&token, guild_id).await.inspect_err(|e| self.remember_error(e))
    }

    /// Choose where bot-mode deliveries go, and read the server's real upload
    /// limit while we are there — the one moment the limit can be known for
    /// certain rather than assumed.
    pub async fn set_target(&self, guild_id: &str, channel_id: &str) -> Result<DiscordSettings> {
        let token = self.require_bot_token().await?;
        let guild = self.inner.rest.guild(&token, guild_id).await.inspect_err(|e| self.remember_error(e))?;
        let channels = self.inner.rest.channels(&token, guild_id).await?;
        let channel = channels
            .into_iter()
            .find(|c| c.id == channel_id)
            .ok_or_else(|| Error::InvalidInput("that channel is not one this bot can post files to".into()))?;

        *self.inner.limit_detected.write().await = true;
        self.clear_error().await;
        self.mutate_settings(|d| {
            d.guild_id = Some(guild.id);
            d.guild_name = Some(guild.name);
            d.channel_id = Some(channel.id);
            d.channel_name = channel.name;
            // Bot upload budget is independent of the webhook budget.
        })
        .await
    }

    /// Post a short message to prove the whole path works, without waiting for
    /// a download to finish.
    pub async fn test(&self) -> Result<String> {
        let target = self.target().await?;
        let settings = self.settings().await?;
        let options = message_options(&settings, if settings.ping_sender { settings.discord_user_id.clone() } else { None });
        let sent = self
            .inner
            .rest
            .send_message_with_options(&target, "Zoltraak is connected. Finished downloads will arrive here.", &options)
            .await
            .inspect_err(|e| self.remember_error(e))?;
        self.clear_error().await;
        Ok(sent.url)
    }

    // ---- deliveries -----------------------------------------------------

    /// Queue a file for delivery. Returns `None` when Discord is not
    /// configured or the file is gone — neither is an error worth failing a
    /// download over.
    pub async fn enqueue(&self, new: NewDelivery) -> Result<Option<Delivery>> {
        if !self.settings().await?.is_configured() {
            return Ok(None);
        }
        let delivery = delivery::insert(&self.inner.queue.pool(), &new).await?;
        self.inner.wake.notify_one();
        Ok(Some(delivery))
    }

    /// Queue files the user picked by hand. Unlike auto-send this reports what
    /// it could not do: the user is watching, and silence would read as a bug.
    pub async fn send_paths(&self, paths: Vec<PathBuf>) -> Result<Vec<Delivery>> {
        self.send_entries(paths.into_iter().map(|path| (path, None)).collect(), DiscordSendOptions::default()).await
    }

    pub async fn send_entries(&self, entries: Vec<(PathBuf, Option<String>)>, options: DiscordSendOptions) -> Result<Vec<Delivery>> {
        let settings = self.settings().await?;
        if !settings.is_configured() { return Err(Error::InvalidInput("Enable and configure Discord delivery in Discord settings first.".into())); }
        let target = self.target().await?;
        let destination = target.reference();
        if options.expected_destination.as_ref().is_some_and(|d| d != &destination) { return Err(Error::InvalidInput("Discord destination changed. Review it and submit again.".into())); }
        let ping_user = if options.ping_sender.unwrap_or(settings.ping_sender) {
            Some(settings.discord_user_id.clone().filter(|id| valid_user_id(id)).ok_or_else(|| Error::InvalidInput("Set a valid Discord user ID before enabling ping.".into()))?)
        } else { None };
        let submission = uuid::Uuid::new_v4().to_string();
        let mut prepared = Vec::new();
        for (path, source) in entries {
            let new = NewDelivery::from_path(DeliverySource::Manual, None, path).ok_or_else(|| Error::InvalidInput("The selected file is no longer where the download left it.".into()))?;
            let caption = options.caption.clone().unwrap_or_else(|| settings.message_template.replace("{filename}", &new.filename).replace("{size}", &limits::human_bytes(new.bytes)).replace("{url}", source.as_deref().unwrap_or("")).replace("{host}", "").replace("{category}", ""));
            let stored = StoredSendOptions { caption, ping_message_template: settings.ping_message_template.clone(), ping_user: ping_user.clone(), source_url: if options.include_source_url.unwrap_or(settings.include_source_url) { source } else { None }, compress_to_fit: options.compress_to_fit.unwrap_or(settings.compress_to_fit), destination: destination.clone() };
            rest::message_payload(&target, &manual_message(&stored), &message_options(&settings, ping_user.clone()))?;
            prepared.push((new, stored));
        }
        let mut tx = self.inner.queue.pool().begin().await?;
        let mut ids = Vec::new();
        for (new, stored) in &prepared {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO discord_deliveries (id, source, path, filename, bytes, state, created_at, updated_at, send_options, submission_id) VALUES (?, 'manual', ?, ?, ?, 'pending', ?, ?, ?, ?)")
                .bind(&id).bind(new.path.to_string_lossy().to_string()).bind(&new.filename).bind(new.bytes as i64)
                .bind(chrono::Utc::now().to_rfc3339()).bind(chrono::Utc::now().to_rfc3339())
                .bind(serde_json::to_string(stored).map_err(|e| Error::Corrupt(e.to_string()))?).bind(&submission).execute(&mut *tx).await?;
            ids.push(id);
        }
        tx.commit().await?;
        let mut created = Vec::new();
        for id in ids { if let Some(d) = delivery::get(&self.inner.queue.pool(), &id).await? { created.push(d); } }
        self.inner.wake.notify_one();
        Ok(created)
    }

    pub async fn deliveries(&self, limit: i64) -> Result<Vec<Delivery>> {
        delivery::list(&self.inner.queue.pool(), limit).await
    }

    pub async fn retry(&self, id: &str) -> Result<bool> {
        let requeued = delivery::requeue(&self.inner.queue.pool(), id).await?;
        if requeued {
            self.inner.wake.notify_one();
        }
        Ok(requeued)
    }

    /// Delete the remote message, keeping grouped history rows together.
    pub async fn delete_message(&self, id: &str) -> Result<bool> {
        let _guard = self.inner.delete_lock.lock().await;
        let pool = self.inner.queue.pool();
        let item = delivery::get(&pool, id).await?.ok_or_else(|| Error::InvalidInput("Delivery no longer exists in Discord history.".into()))?;
        if item.state == DeliveryState::Deleted { return Ok(true); }
        if item.state != DeliveryState::Sent { return Err(Error::InvalidInput("Only sent Discord messages can be deleted.".into())); }
        let url = item.message_url.as_deref().ok_or_else(|| Error::InvalidInput("This delivery has no recorded Discord message.".into()))?;
        let parsed = reqwest::Url::parse(url).map_err(|_| Error::InvalidInput("Invalid recorded Discord message link.".into()))?;
        let parts: Vec<_> = parsed.path().split('/').filter(|p| !p.is_empty()).collect();
        if parsed.scheme() != "https" || parsed.host_str() != Some("discord.com") || parts.len() != 4 || parts[0] != "channels" || !valid_user_id(parts[2]) || !valid_user_id(parts[3]) {
            return Err(Error::InvalidInput("Invalid recorded Discord message link.".into()));
        }
        let target = self.target().await?;
        let destination = item.message_destination.as_deref().or_else(|| item.send_options.as_ref().map(|o| o.destination.as_str()));
        if destination.is_some_and(|d| d != target.reference()) {
            return Err(Error::InvalidInput("Discord destination changed. Reconnect the original webhook or bot destination to delete this message.".into()));
        }
        // Older webhook sends can be verified by the webhook token itself.
        // Older bot sends lack proof of the original bot identity/destination.
        if destination.is_none() && matches!(target, Target::Channel { .. }) {
            return Err(Error::InvalidInput("This older delivery has no saved sender reference. Delete it directly in Discord.".into()));
        }
        if destination.is_none() {
            self.inner.rest.verify_legacy_webhook_message(&target, parts[2], parts[3]).await?;
        }
        self.inner.rest.delete_message(&target, parts[2], parts[3]).await?;
        delivery::mark_message_deleted(&pool, url).await?;
        Ok(true)
    }

    pub async fn clear_history(&self) -> Result<u64> {
        delivery::clear_finished(&self.inner.queue.pool()).await
    }

    /// The delivery worker: tidies up after the last run, then sends whatever
    /// is queued until [`Self::shutdown`] or the process ends.
    ///
    /// Returns the future rather than spawning it, because the engine has no
    /// business deciding whose runtime it belongs on. The desktop app hands it
    /// to `tauri::async_runtime::spawn`, the CLI to `tokio::spawn`; spawning
    /// here would have meant calling `tokio::spawn` from Tauri's `setup`,
    /// which runs on the main thread with no runtime installed and panics.
    pub async fn run_worker(self) {
        if let Err(e) = delivery::recover_and_prune(&self.inner.queue.pool()).await {
            tracing::warn!(error = %e, "could not tidy the Discord delivery history");
        }
        self.work().await;
    }

    pub fn shutdown(&self) {
        self.inner.cancel.cancel();
        self.inner.wake.notify_waiters();
    }

    async fn work(&self) {
        loop {
            if self.inner.cancel.is_cancelled() {
                return;
            }
            match delivery::next_pending_batch(&self.inner.queue.pool(), limits::MAX_ATTACHMENTS).await {
                Ok(batch) if !batch.is_empty() => self.deliver_batch(batch).await,
                Ok(_) => {
                    tokio::select! {
                        _ = self.inner.wake.notified() => {
                            // Let the rest of the burst land before claiming.
                            // Downloads that finish together are queued one at
                            // a time, so waking on the first would send each
                            // as its own message — which is exactly what
                            // grouping is meant to avoid. The wait is short
                            // enough to be invisible for a lone file.
                            tokio::select! {
                                _ = tokio::time::sleep(BATCH_SETTLE) => {}
                                _ = self.inner.cancel.cancelled() => return,
                            }
                        }
                        _ = self.inner.cancel.cancelled() => return,
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "could not read the Discord delivery queue");
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                        _ = self.inner.cancel.cancelled() => return,
                    }
                }
            }
        }
    }

    /// Carry a claimed batch as far as it will go. Every exit path leaves each
    /// row in a terminal state, so nothing can be silently stuck at `pending`.
    ///
    /// The batch exists for one reason: several files that finished together
    /// belong in one Discord message, not in a run of near-identical ones. It
    /// is a grouping of *uploads*, never of records — each delivery still gets
    /// its own outcome, so a group where one file was too large reports that
    /// on the file it was true of.
    async fn deliver_batch(&self, items: Vec<Delivery>) {
        let mut settings = match self.settings().await {
            Ok(settings) => settings,
            Err(e) => return self.record_all(&items, Err(&e)).await,
        };
        if !settings.is_configured() {
            let reason = "Discord delivery was switched off before this could be sent";
            return self.record_all(&items, Ok(reason)).await;
        }
        let target = match self.target().await {
            Ok(target) => target,
            Err(e) => return self.record_all(&items, Err(&e)).await,
        };

        if let Some(options) = items.first().and_then(|d| d.send_options.as_ref()) {
            if options.destination != target.reference() { return self.record_all(&items, Err(&Error::InvalidInput("Discord destination changed. Submit these files again from Downloads.".into()))).await; }
            settings.compress_to_fit = options.compress_to_fit;
        }
        // Prepare everything first: preparing can transcode, and a file that
        // turns out not to be sendable should not hold up the ones that are.
        let budget = limits::budget(settings.upload_limit());
        let mut ready: Vec<(Delivery, Upload)> = Vec::new();
        for item in items {
            match self.prepare(&item, &settings, budget).await {
                Ok(Prepared::Ready(upload)) => ready.push((item, upload)),
                Ok(Prepared::Skipped(reason)) => self.record_skipped(&item, &reason).await,
                Err(e) => self.record_failed(&item, &e).await,
            }
        }

        for group in pack(ready, budget) {
            self.send_group(&target, &settings, group).await;
        }
    }

    /// Get one delivery to the point where it is a file that can be uploaded:
    /// small enough as it stands, or re-encoded until it is.
    async fn prepare(&self, item: &Delivery, settings: &DiscordSettings, budget: u64) -> Result<Prepared> {
        let pool = self.inner.queue.pool();
        if !item.path.exists() {
            return Ok(Prepared::Skipped("the file was moved or deleted before it could be sent".into()));
        }

        // Re-stat rather than trusting the row: the file may have been
        // replaced since the delivery was queued.
        let actual_bytes = std::fs::metadata(&item.path).map(|m| m.len()).unwrap_or(item.bytes);

        let (path, bytes, scratch) = if actual_bytes <= budget {
            (item.path.clone(), actual_bytes, None)
        } else if !settings.compress_to_fit {
            return Ok(Prepared::Skipped(format!(
                "{} is over this server's {} limit, and fitting files is switched off",
                limits::human_bytes(actual_bytes),
                limits::human_bytes(settings.upload_limit())
            )));
        } else {
            delivery::set_state(&pool, &item.id, DeliveryState::Preparing).await?;
            match fit::fit_to_budget(&item.path, budget, settings.compress_max_height, &self.inner.cancel).await? {
                fit::Fitted::Ready { path, bytes, scratch } => (path, bytes, Some(scratch)),
                fit::Fitted::Impossible(reason) => {
                    return Ok(Prepared::Skipped(format!(
                        "{} is over this server's {} limit — {reason}",
                        limits::human_bytes(actual_bytes),
                        limits::human_bytes(settings.upload_limit())
                    )));
                }
            }
        };

        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or(&item.filename).to_string();
        Ok(Prepared::Ready(Upload { path, filename, bytes, _scratch: scratch }))
    }

    /// Upload one packed group as a single message and record what happened to
    /// each delivery in it.
    async fn send_group(&self, target: &Target, settings: &DiscordSettings, group: Vec<(Delivery, Upload)>) {
        let pool = self.inner.queue.pool();
        for (item, _) in &group {
            if let Err(e) = delivery::set_state(&pool, &item.id, DeliveryState::Uploading).await {
                tracing::warn!(id = %item.id, error = %e, "could not mark a Discord delivery as uploading");
            }
        }

        let stored = group.first().and_then(|(d, _)| d.send_options.as_ref());
        let message = if let Some(stored) = stored {
            let mut lines = vec![stored.caption.clone()];
            for (item, _) in &group {
                if let Some(o) = &item.send_options {
                    if !o.caption.is_empty() && !lines.contains(&o.caption) { lines.push(o.caption.clone()); }
                    if let Some(url) = &o.source_url { if !lines.contains(url) { lines.push(url.clone()); } }
                }
            }
            lines.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
        } else { group_message(&settings.message_template, &group) };
        let ping = stored.map(|o| o.ping_user.clone()).unwrap_or_else(|| if settings.ping_sender { settings.discord_user_id.clone() } else { None });
        let mut options = message_options(settings, ping);
        if let Some(stored) = stored { options.ping_message_template = Some(stored.ping_message_template.clone()); }
        let files: Vec<(PathBuf, String)> =
            group.iter().map(|(_, upload)| (upload.path.clone(), upload.filename.clone())).collect();

        match self.inner.rest.send_files_with_options(target, &message, &files, &options).await {
            Ok(sent) => {
                self.clear_error().await;
                for (item, upload) in &group {
                    if let Err(e) = delivery::mark_sent_for_target(&pool, &item.id, &sent.url, upload.bytes, &target.reference()).await {
                        tracing::error!(id = %item.id, error = %e, "could not record a sent Discord delivery");
                    }
                }
            }
            // A rejection on a group of several is ambiguous — one file may be
            // over the ceiling, or only their total is. Splitting answers the
            // question honestly, and costs an extra request only in the case
            // that already went wrong.
            Err(Error::ApiRejected { status: 413, .. }) if group.len() > 1 => {
                tracing::info!(files = group.len(), "Discord rejected a grouped upload; sending them singly");
                for one in group {
                    Box::pin(self.send_group(target, settings, vec![one])).await;
                }
            }
            Err(Error::ApiRejected { status: 413, .. }) => {
                // Discord has just told us the real ceiling is lower than we
                // believed. Record it so the next attempt aims lower, and let
                // the user retry rather than looping on it here.
                let (item, upload) = &group[0];
                let learned = limits::learn_from_rejection(settings.upload_limit(), upload.bytes);
                if let Err(e) = self.mutate_settings(|d| {
                    if d.mode.is_bot() { d.bot_upload_limit_bytes = d.bot_upload_limit_bytes.min(learned); }
                    else { d.upload_limit_bytes = Some(learned); }
                }).await {
                    tracing::warn!(error = %e, "could not record the upload limit Discord just taught us");
                }
                *self.inner.limit_detected.write().await = true;
                self.record_skipped(
                    item,
                    &format!(
                        "the server rejected {} as too large; the limit is now taken to be {} — retry to send it at that size",
                        limits::human_bytes(upload.bytes),
                        limits::human_bytes(learned)
                    ),
                )
                .await;
            }
            Err(e) => {
                self.remember_error(&e);
                for (item, _) in &group {
                    self.record_failed(item, &e).await;
                }
            }
        }
    }

    async fn record_skipped(&self, item: &Delivery, reason: &str) {
        tracing::info!(file = %item.filename, %reason, "Discord delivery skipped");
        let pool = self.inner.queue.pool();
        if let Err(e) = delivery::finish_with_reason(&pool, &item.id, DeliveryState::Skipped, reason).await {
            tracing::error!(id = %item.id, error = %e, "could not record a skipped Discord delivery");
        }
    }

    async fn record_failed(&self, item: &Delivery, error: &Error) {
        tracing::warn!(file = %item.filename, %error, "Discord delivery failed");
        let pool = self.inner.queue.pool();
        if let Err(e) =
            delivery::finish_with_reason(&pool, &item.id, DeliveryState::Failed, &error.to_string()).await
        {
            tracing::error!(id = %item.id, error = %e, "could not record a failed Discord delivery");
        }
    }

    /// Finish a whole batch the same way, for the conditions that are about
    /// the configuration rather than about any one file.
    async fn record_all(&self, items: &[Delivery], outcome: std::result::Result<&str, &Error>) {
        if let Err(e) = outcome {
            self.remember_error(e);
        }
        for item in items {
            match outcome {
                Ok(reason) => self.record_skipped(item, reason).await,
                Err(e) => self.record_failed(item, e).await,
            }
        }
    }

    // ---- plumbing -------------------------------------------------------

    /// Resolve the credential and destination into something sendable.
    async fn target(&self) -> Result<Target> {
        let settings = self.settings().await?;
        match settings.mode {
            DiscordMode::Webhook => {
                let url = self
                    .inner
                    .vault
                    .get(keys::DISCORD_WEBHOOK_URL)
                    .await
                    .ok_or_else(|| Error::InvalidInput("no Discord webhook is set up — paste one in the Discord tab".into()))?;
                Ok(Target::Webhook { url })
            }
            DiscordMode::Bot => {
                let token = self.require_bot_token().await?;
                let channel_id = settings
                    .channel_id
                    .ok_or_else(|| Error::InvalidInput("no Discord channel has been chosen for the bot to post in".into()))?;
                Ok(Target::Channel { token, channel_id })
            }
        }
    }

    async fn require_bot_token(&self) -> Result<String> {
        self.inner
            .vault
            .get(keys::DISCORD_BOT_TOKEN)
            .await
            .ok_or_else(|| Error::InvalidInput("no Discord bot token is stored — add one in the Discord tab".into()))
    }

    /// Record why the last attempt to reach Discord failed.
    ///
    /// Spawns rather than awaits because every call site is an
    /// `inspect_err`, whose closure cannot be async. The store is only read
    /// by the settings screen, which polls, so a few microseconds of lag
    /// costs nothing.
    fn remember_error(&self, error: &Error) {
        let message = error.to_string();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            *inner.last_error.write().await = Some(message);
        });
    }

    async fn clear_error(&self) {
        *self.inner.last_error.write().await = None;
    }
}

/// Turn download ids into the files they produced.
///
/// A queue item and a video job are different rows in different tables, and a
/// caller holding an id from a merged list has no way to know which. Resolving
/// both here keeps that knowledge out of the Tauri commands and the REST
/// handlers, which would otherwise each carry their own copy.
pub async fn resolve_paths(
    supervisor: &crate::supervisor::Supervisor,
    video: &crate::video::manager::VideoManager,
    music: Option<&crate::music::manager::MusicManager>,
    ids: &[String],
) -> Result<Vec<PathBuf>> {
    Ok(resolve_entries(supervisor, video, music, ids).await?.into_iter().map(|(path, _)| path).collect())
}
pub async fn resolve_entries(supervisor: &crate::supervisor::Supervisor, video: &crate::video::manager::VideoManager, music: Option<&crate::music::manager::MusicManager>, ids: &[String]) -> Result<Vec<(PathBuf, Option<String>)>> {
    let snapshot = supervisor.snapshot().await?;
    let jobs = video.list().await;
    let tracks = match music {
        Some(music) => music.list().await,
        None => Vec::new(),
    };
    let mut paths = Vec::with_capacity(ids.len());
    for id in ids {
        let from_queue = snapshot
            .iter()
             .find(|item| item.id.to_string() == *id && item.state == crate::model::DownloadState::Finished)
            .map(|item| std::path::Path::new(&item.directory).join(&item.filename));
        let path = match from_queue {
            Some(path) => Some(path),
            None => jobs
                .iter()
                .find(|job| job.id.to_string() == *id && job.state == crate::video::manager::VideoJobState::Finished)
                .and_then(|job| job.path.clone())
                .or_else(|| tracks.iter().find(|job| job.id.to_string() == *id && matches!(job.state, crate::music::manager::MusicJobState::Finished | crate::music::manager::MusicJobState::Skipped)).and_then(|job| job.path.clone())),
        };
        match path {
            Some(path) => {
                let source = snapshot.iter().find(|i| i.id.to_string() == *id).map(|i| i.url.clone())
                    .or_else(|| jobs.iter().find(|j| j.id.to_string() == *id).map(|j| j.webpage_url.clone()))
                    .or_else(|| tracks.iter().find(|j| j.id.to_string() == *id).map(|j| j.track.source_url.clone()));
                paths.push((path, source.filter(|s| s.starts_with("https://") || s.starts_with("http://"))));
            },
            None => return Err(Error::InvalidInput(format!("download {id} was not found"))),
        }
    }
    Ok(paths)
}

/// How long the worker lets a burst settle after being woken.
///
/// Deliveries are queued one at a time as each download finishes, so a post's
/// eight pictures arrive as eight separate notifications a few milliseconds
/// apart. Claiming on the first would send eight messages. This is the whole
/// mechanism behind grouping, and it is short enough that a single file is not
/// noticeably delayed by it.
const BATCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(1200);

/// A file that is ready to be uploaded, and the transcode scratch that has to
/// outlive the upload for it to still be there when the body is streamed.
struct Upload {
    path: PathBuf,
    filename: String,
    bytes: u64,
    _scratch: Option<fit::Scratch>,
}

enum Prepared {
    Ready(Upload),
    /// Not sent, and not a failure: nothing further will help.
    Skipped(String),
}

/// Split prepared uploads into the messages they will be sent as.
///
/// Two ceilings apply and both are Discord's: at most
/// [`limits::MAX_ATTACHMENTS`] files on a message, and a total request size
/// under the server's limit — the budget is for the *request*, not for each
/// attachment, so a group's sizes add up. Order is preserved, so a carousel
/// stays in the order it was posted in even when it spills into a second
/// message.
fn pack(ready: Vec<(Delivery, Upload)>, budget: u64) -> Vec<Vec<(Delivery, Upload)>> {
    let mut groups: Vec<Vec<(Delivery, Upload)>> = Vec::new();
    let mut current: Vec<(Delivery, Upload)> = Vec::new();
    let mut used: u64 = 0;

    for (item, upload) in ready {
        let would_be = used.saturating_add(upload.bytes);
        // A lone file that is over budget still goes out on its own rather
        // than being dropped here: `prepare` already accepted it, and letting
        // Discord answer is better than second-guessing it silently.
        if !current.is_empty() && (current.len() >= limits::MAX_ATTACHMENTS || would_be > budget || (item.send_options.is_some() && manual_group_length(&current, &item) > 2000)) {
            groups.push(std::mem::take(&mut current));
            used = 0;
        }
        used = used.saturating_add(upload.bytes);
        current.push((item, upload));
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// The caption for a group.
///
/// Empty by default, and usually empty in practice — the files say what they
/// are. When a template *is* set it is rendered per file, but identical lines
/// are collapsed: a template of `{filename}` over eight pictures should read
/// as eight lines, while one of "from Zoltraak" should read as one.
fn manual_message(options: &StoredSendOptions) -> String {
    [Some(options.caption.as_str()), options.source_url.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
}
fn manual_group_length(current: &[(Delivery, Upload)], next: &Delivery) -> usize {
    let Some(options) = next.send_options.as_ref() else { return 0; };
    let mut lines = vec![options.caption.clone()];
    for item in current.iter().map(|(d, _)| d).chain(std::iter::once(next)) {
        if let Some(o) = &item.send_options {
            if !o.caption.is_empty() && !lines.contains(&o.caption) { lines.push(o.caption.clone()); }
            if let Some(url) = &o.source_url { if !lines.contains(url) { lines.push(url.clone()); } }
        }
    }
    lines.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n").chars().count() + options.ping_user.as_ref().map_or(0, |id| {
        let mention = format!("<@{id}>");
        let text = options.ping_message_template.replace("{discord user id}", &mention).replace("{discord_user_id}", &mention);
        text.chars().count() + if text.contains(&mention) { 1 } else { mention.len() + 2 }
    })
}
fn group_message(template: &str, group: &[(Delivery, Upload)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (item, _) in group {
        let rendered = render_message(template, item);
        if !rendered.is_empty() && !lines.contains(&rendered) {
            lines.push(rendered);
        }
    }
    let joined = lines.join("\n");
    joined.chars().take(1900).collect()
}

fn credential_key(mode: DiscordMode) -> &'static str {
    match mode {
        DiscordMode::Webhook => keys::DISCORD_WEBHOOK_URL,
        DiscordMode::Bot => keys::DISCORD_BOT_TOKEN,
    }
}

fn vault_error(source: std::io::Error) -> Error {
    Error::Io { path: PathBuf::from("secrets.vault"), source }
}

/// Fill in the user's message template.
///
/// Unknown placeholders are left alone rather than blanked: a user who typed
/// `{filenam}` should see their typo, not an empty message.
///
/// An empty result is returned as such. It used to fall back to the filename,
/// on the reasoning that Discord rejects an empty message — but that is only
/// true of a message with *nothing* in it, and these carry attachments. The
/// fallback meant every file arrived under a line repeating its own name.
pub fn render_message(template: &str, item: &Delivery) -> String {
    let host = item
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let values: HashMap<&str, String> = HashMap::from([
        ("{filename}", item.filename.clone()),
        ("{size}", limits::human_bytes(item.bytes)),
        ("{kind}", host),
        ("{url}", String::new()),
        ("{category}", String::new()),
        ("{host}", String::new()),
    ]);
    let mut out = template.to_string();
    for (token, value) in values {
        out = out.replace(token, &value);
    }
    out.trim().chars().take(1900).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LEGACY_CAPTION_TEMPLATE;
    use chrono::Utc;

    fn delivery(filename: &str, bytes: u64) -> Delivery {
        Delivery {
            id: "1".into(),
            source: DeliverySource::Download,
            source_id: None,
            path: PathBuf::from(format!("/downloads/{filename}")),
            filename: filename.into(),
            bytes,
            state: DeliveryState::Pending,
            reason: None,
            message_url: None,
            message_destination: None,
            sent_bytes: None,
            attempts: 0,
            send_options: None,
            submission_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_template_that_asks_for_the_name_and_size_still_gets_them() {
        let rendered = render_message("{filename} · {size}", &delivery("clip.mp4", 10 * 1024 * 1024));
        assert_eq!(rendered, "clip.mp4 · 10.0 MiB");
    }

    #[test]
    fn nothing_is_sent_by_default_and_an_empty_template_stays_empty() {
        // The regression this guards: every file used to arrive under a line
        // repeating its own filename and size, because an empty render fell
        // back to the filename. Attachments are a message on their own.
        assert_eq!(DiscordSettings::default().message_template, "");
        assert_eq!(render_message("", &delivery("clip.mp4", 1)), "");
        assert_eq!(render_message("   ", &delivery("clip.mp4", 1)), "");
        assert_eq!(render_message("{url}", &delivery("clip.mp4", 1)), "");
    }

    #[test]
    fn the_caption_a_previous_version_defaulted_to_is_dropped_but_a_chosen_one_is_kept() {
        let mut stored = DiscordSettings { message_template: LEGACY_CAPTION_TEMPLATE.into(), ..Default::default() };
        stored.migrate();
        assert_eq!(stored.message_template, "", "an untouched old default is not a choice");

        let mut chosen = DiscordSettings { message_template: "from Zoltraak".into(), ..Default::default() };
        chosen.migrate();
        assert_eq!(chosen.message_template, "from Zoltraak");
    }

    fn upload(bytes: u64) -> Upload {
        Upload { path: PathBuf::from("/x"), filename: "x".into(), bytes, _scratch: None }
    }

    #[test]
    fn files_that_fit_together_travel_as_one_message() {
        let ready: Vec<_> = (0..4).map(|i| (delivery(&format!("{i}.jpg"), 100), upload(100))).collect();

        let groups = pack(ready, 10_000);

        assert_eq!(groups.len(), 1, "four small pictures are one gallery, not four messages");
        assert_eq!(groups[0].len(), 4);
    }

    #[test]
    fn a_group_never_exceeds_discords_own_two_ceilings() {
        // Twelve files: the attachment count splits them before their sizes do.
        let many: Vec<_> = (0..12).map(|i| (delivery(&format!("{i}.jpg"), 1), upload(1))).collect();
        let groups = pack(many, 10_000);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), limits::MAX_ATTACHMENTS);
        assert_eq!(groups[1].len(), 2);

        // Three files whose total is over the request budget: the sizes split
        // them even though the count would allow one message.
        let heavy: Vec<_> = (0..3).map(|i| (delivery(&format!("{i}.mp4"), 600), upload(600))).collect();
        let groups = pack(heavy, 1_000);
        assert_eq!(groups.len(), 3, "600 + 600 is over a 1000-byte request budget");
    }

    #[test]
    fn a_single_oversized_file_is_still_sent_rather_than_dropped() {
        // `prepare` already accepted it — either it fit or it was transcoded
        // until it did. Packing is not the place to overrule that.
        let groups = pack(vec![(delivery("big.mp4", 5_000), upload(5_000))], 1_000);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 1);
    }

    #[test]
    fn a_groups_caption_says_each_distinct_thing_once() {
        let group =
            vec![(delivery("a.jpg", 1), upload(1)), (delivery("b.jpg", 1), upload(1))];

        // Nothing set: nothing sent, however many files there are.
        assert_eq!(group_message("", &group), "");
        // Per-file: one line each.
        assert_eq!(group_message("{filename}", &group), "a.jpg\nb.jpg");
        // Fixed text: one line, not one per attachment.
        assert_eq!(group_message("from Zoltraak", &group), "from Zoltraak");
    }

    #[test]
    fn an_unknown_placeholder_is_left_visible_rather_than_blanked() {
        let rendered = render_message("{filenam}", &delivery("clip.mp4", 1));
        assert_eq!(rendered, "{filenam}", "a typo should look like a typo");
    }

    #[test]
    fn a_very_long_template_is_cut_to_something_discord_will_accept() {
        let rendered = render_message(&"x".repeat(4000), &delivery("clip.mp4", 1));
        assert!(rendered.chars().count() <= 1900);
    }

    #[test]
    fn each_mode_reads_its_own_credential() {
        assert_eq!(credential_key(DiscordMode::Bot), keys::DISCORD_BOT_TOKEN);
        assert_eq!(credential_key(DiscordMode::Webhook), keys::DISCORD_WEBHOOK_URL);
    }
}
