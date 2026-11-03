//! The slice of Discord's HTTP API that Zoltraak needs.
//!
//! Hand-rolled rather than pulled from `serenity` or `twilight`: this is eight
//! endpoints and no gateway connection, and the rest of the engine already
//! talks to remote JSON APIs the same way (see `video/youtube/innertube.rs`).
//! A gateway library would add a websocket, a cache and a sharding model that
//! nothing here would use.
//!
//! Rate limits are absorbed *inside* this module. Discord answers a 429 with
//! the exact seconds to wait, which is far better information than an
//! exponential backoff would invent, so a 429 is slept through and retried
//! here; everything else is returned for the caller to decide about.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const API: &str = "https://discord.com/api/v10";

/// Discord's own ceiling on how long we will sit on a rate limit before
/// giving up and letting the delivery retry later on its own schedule.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);
const RATE_LIMIT_ATTEMPTS: u32 = 3;

/// Permissions the invite link asks for: read a channel, post in it, attach
/// files, embed links, and do the same inside threads. Nothing else — an
/// invite that asks for Administrator is a red flag to whoever accepts it.
pub const INVITE_PERMISSIONS: u64 = (1 << 10) | (1 << 11) | (1 << 14) | (1 << 15) | (1 << 38);

/// Channel types a file can be posted to: text, announcement, forum.
const POSTABLE_CHANNEL_TYPES: [u8; 3] = [0, 5, 15];

/// Where a delivery is going. Webhook mode carries its channel inside the URL;
/// bot mode names one explicitly.
#[derive(Clone)]
pub enum Target {
    Webhook { url: String },
    Channel { token: String, channel_id: String },
}

impl Target {
    pub fn reference(&self) -> String {
        match self {
            Self::Webhook { url } => format!("webhook:{}", url.split("/webhooks/").nth(1).unwrap_or_default().split('/').next().unwrap_or_default()),
            Self::Channel { channel_id, .. } => format!("bot:{channel_id}"),
        }
    }
}
#[derive(Default)]
pub struct MessageOptions {
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub ping_user: Option<String>,
    pub ping_message_template: Option<String>,
}
pub fn message_payload(target: &Target, message: &str, options: &MessageOptions) -> Result<serde_json::Value> {
    let ping_message = options.ping_user.as_ref().map(|id| {
        let mention = format!("<@{id}>");
        let template = options.ping_message_template.as_deref().unwrap_or("{discord user id}");
        let text = template.replace("{discord user id}", &mention).replace("{discord_user_id}", &mention);
        if text.contains(&mention) { text } else if text.trim().is_empty() { mention } else { format!("{text} {mention}") }
    });
    let content = [ping_message.as_deref(), Some(message)].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    if content.chars().count() > 2000 { return Err(Error::InvalidInput("Discord message exceeds 2000 characters including mentions and source links.".into())); }
    let users: Vec<&str> = options.ping_user.iter().map(String::as_str).collect();
    let mut body = serde_json::json!({"allowed_mentions": {"parse": [], "users": users, "roles": [], "replied_user": false}});
    if !content.is_empty() { body["content"] = content.into(); }
    if matches!(target, Target::Webhook { .. }) {
        if let Some(name) = &options.username { body["username"] = name.clone().into(); }
        if let Some(url) = &options.avatar_url {
            // Discord refreshes attachment URLs used as webhook avatars when
            // their expiring signature parameters are omitted. Keep the saved
            // URL intact for previews and preserve other hosts' query strings.
            let avatar = reqwest::Url::parse(url).ok().filter(|parsed|
                parsed.scheme() == "https" && parsed.host_str() == Some("cdn.discordapp.com") && parsed.path().starts_with("/attachments/"));
            body["avatar_url"] = if let Some(mut avatar) = avatar {
                avatar.set_query(None);
                avatar.set_fragment(None);
                avatar.to_string().into()
            } else { url.clone().into() };
        }
    }
    Ok(body)
}
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Both arms hold a credential — a webhook URL *is* its own password.
        match self {
            Self::Webhook { .. } => f.write_str("Target::Webhook(<redacted>)"),
            Self::Channel { channel_id, .. } => write!(f, "Target::Channel({channel_id})"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotIdentity {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildDetail {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub premium_tier: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub position: i32,
}

/// What a webhook URL turns out to point at. `name` is the webhook's own
/// display name, not the channel's.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
}

// Note: the structs above deliberately carry no `rename_all`. They are
// Discord's wire shapes, and Discord speaks snake_case (`premium_tier`,
// `channel_id`); renaming them would leave every such field silently at its
// default. The ones that reach the front end — `Guild` and `Channel` — happen
// to have single-word fields, so nothing is lost by leaving them as they are.

/// A message that landed, and the link a person can click to reach it.
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub id: String,
    pub channel_id: String,
    pub url: String,
}

#[derive(Deserialize)]
struct MessageResponse {
    id: String,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    guild_id: Option<String>,
}

/// Discord's error body. `message` is written for humans and is worth far more
/// than the status code alone when a token is wrong or a channel is gone.
#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    code: Option<u64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    retry_after: Option<f64>,
}

#[derive(Clone)]
pub struct Rest {
    client: reqwest::Client,
    #[cfg(test)]
    pub(super) test_origin: Option<String>,
}

impl std::fmt::Debug for Rest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rest")
    }
}

impl Rest {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client, #[cfg(test)] test_origin: None }
    }

    pub(super) async fn profile_request(&self, token: &str, path: &str, fields: Option<&serde_json::Map<String, serde_json::Value>>) -> Result<serde_json::Value> {
        let url = format!("{API}/{path}");
        // Profile edits fail promptly on rate limits, preserving the draft for an explicit retry.
        let build = || {
            let request = if let Some(fields) = fields { self.client.patch(&url).json(fields) } else { self.client.get(&url) };
            request.header("Authorization", format!("Bot {token}")).timeout(Duration::from_secs(20))
        };
        #[cfg(test)]
        let response = {
            let mut request = build().build()?;
            if let Some(origin) = &self.test_origin {
                let mut target = reqwest::Url::parse(origin).unwrap();
                target.set_path(request.url().path());
                *request.url_mut() = target;
            }
            self.client.execute(request).await
        };
        #[cfg(not(test))]
        let response = build().send().await;
        let response = response.map_err(|e| Error::Http(e.without_url()))?;
        let bytes = self.handle(response, &url, false).await.map_err(|e| e.into_error(&url))?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt("Discord returned an unreadable profile.".into()))
    }

    // ---- bot management -------------------------------------------------

    /// Confirm a bot token works and learn who it belongs to. This is what
    /// "connect" means in bot mode: everything else is discovery on top.
    pub async fn identity(&self, token: &str) -> Result<BotIdentity> {
        self.get_json(token, &format!("{API}/users/@me")).await
    }

    /// Rename the bot, set its avatar, or both. Discord rate-limits username
    /// changes hard (two per hour), and says so in the 429 it returns.
    pub async fn update_bot(&self, token: &str, username: Option<&str>, avatar_data_uri: Option<&str>) -> Result<BotIdentity> {
        let mut body = serde_json::Map::new();
        if let Some(name) = username {
            body.insert("username".into(), serde_json::Value::String(name.to_string()));
        }
        if let Some(avatar) = avatar_data_uri {
            body.insert("avatar".into(), serde_json::Value::String(avatar.to_string()));
        }
        if body.is_empty() {
            return self.identity(token).await;
        }
        let url = format!("{API}/users/@me");
        self.send_with_rate_limit(&url, || {
            self.client.patch(&url).header("Authorization", format!("Bot {token}")).json(&body)
        })
        .await
    }

    /// The bot's application id, used to build the invite link.
    ///
    /// For a bot the application id and the bot user id are the same value, so
    /// this is a fallback rather than a required call — but asking is free and
    /// keeps the link correct if that ever stops being true.
    pub async fn application_id(&self, token: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct App {
            id: String,
        }
        let app: App = self.get_json(token, &format!("{API}/applications/@me")).await?;
        Ok(app.id)
    }

    /// The URL that adds this bot to a server. Built locally — there is no
    /// endpoint for it, and nothing about it is secret.
    pub fn invite_url(application_id: &str) -> String {
        format!("https://discord.com/oauth2/authorize?client_id={application_id}&scope=bot&permissions={INVITE_PERMISSIONS}")
    }

    pub async fn guilds(&self, token: &str) -> Result<Vec<Guild>> {
        self.get_json(token, &format!("{API}/users/@me/guilds")).await
    }

    pub async fn guild(&self, token: &str, guild_id: &str) -> Result<GuildDetail> {
        self.get_json(token, &format!("{API}/guilds/{guild_id}")).await
    }

    /// Channels a file can actually be posted to, in the order the server
    /// shows them. Voice channels, categories and stage channels are dropped:
    /// offering them would only produce a 403 later.
    pub async fn channels(&self, token: &str, guild_id: &str) -> Result<Vec<Channel>> {
        let mut channels: Vec<Channel> = self.get_json(token, &format!("{API}/guilds/{guild_id}/channels")).await?;
        channels.retain(|c| POSTABLE_CHANNEL_TYPES.contains(&c.kind));
        channels.sort_by_key(|c| (c.position, c.id.clone()));
        Ok(channels)
    }

    // ---- webhooks -------------------------------------------------------

    /// Validate a webhook URL and learn what it points at. A webhook URL is
    /// itself the credential, so this doubles as the "connect" step.
    pub async fn webhook_info(&self, webhook_url: &str) -> Result<WebhookInfo> {
        let url = normalize_webhook_url(webhook_url)?;
        self.send_with_rate_limit(&url, || self.client.get(&url)).await
    }

    // ---- sending --------------------------------------------------------

    /// Post `message` with `path` attached, and return the link to it.
    /// The single-file case of [`Self::send_files`].
    pub async fn send_file(&self, target: &Target, message: &str, path: &Path, filename: &str) -> Result<SentMessage> {
        let files = [(path.to_path_buf(), filename.to_string())];
        self.send_files(target, message, &files).await
    }

    /// Post one message carrying every file in `files`, and return the link
    /// to it.
    ///
    /// Discord renders several attachments on one message as a gallery, so a
    /// post's eight pictures arrive as one thing to scroll past rather than
    /// eight. At most [`MAX_ATTACHMENTS`] may ride together; the caller does
    /// the packing, because it is also the one that knows the size budget.
    ///
    /// The files are streamed rather than read into memory: a 100 MiB upload
    /// on a boosted server should not cost 100 MiB of resident memory. That
    /// means the body cannot be replayed, so each rate-limit retry reopens
    /// them and rebuilds the form.
    pub async fn send_files(&self, target: &Target, message: &str, files: &[(std::path::PathBuf, String)]) -> Result<SentMessage> {
        self.send_files_with_options(target, message, files, &MessageOptions::default()).await
    }
    pub async fn send_files_with_options(&self, target: &Target, message: &str, files: &[(std::path::PathBuf, String)], options: &MessageOptions) -> Result<SentMessage> {
        if files.is_empty() {
            return Err(Error::InvalidInput("nothing to send".into()));
        }
        let url = match target {
            // `wait=true` makes Discord return the created message instead of
            // a bare 204, which is the only way to get a link to it.
            Target::Webhook { url } => format!("{}?wait=true", normalize_webhook_url(url)?),
            Target::Channel { channel_id, .. } => format!("{API}/channels/{channel_id}/messages"),
        };
        // An empty caption is a real choice, not a missing value: the files
        // already say what they are, and Discord accepts a message that is
        // nothing but its attachments. Sending `"content": ""` would work too,
        // but omitting the field is what the API documents.
        let payload = message_payload(target, message, options)?.to_string();

        let mut attempt = 0;
        loop {
            attempt += 1;
            let form = file_form(&payload, files).await?;
            let mut request = self.client.post(&url).multipart(form);
            if let Target::Channel { token, .. } = target {
                request = request.header("Authorization", format!("Bot {token}"));
            }

            #[cfg(test)]
            let response = {
                let mut request = request.build()?;
                if let Some(origin) = &self.test_origin {
                    let mut url = reqwest::Url::parse(origin).unwrap();
                    url.set_path(request.url().path());
                    url.set_query(request.url().query());
                    *request.url_mut() = url;
                }
                self.client.execute(request).await?
            };
            #[cfg(not(test))]
            let response = request.send().await?;
            match self.handle(response, &url, false).await {
                Ok(body) => {
                    let message: MessageResponse = serde_json::from_slice(&body).map_err(|e| Error::Corrupt(format!("Discord sent a message body we could not read: {e}")))?;
                    let channel_id = message.channel_id.clone().unwrap_or_else(|| match target {
                        Target::Channel { channel_id, .. } => channel_id.clone(),
                        Target::Webhook { .. } => String::new(),
                    });
                    let guild = message.guild_id.as_deref().unwrap_or("@me");
                    return Ok(SentMessage {
                        url: format!("https://discord.com/channels/{guild}/{channel_id}/{}", message.id),
                        id: message.id,
                        channel_id,
                    });
                }
                Err(RestFailure::RateLimited(wait)) if attempt < RATE_LIMIT_ATTEMPTS && wait <= MAX_RATE_LIMIT_WAIT => {
                    tracing::info!(?wait, attempt, "Discord rate limited the upload; waiting");
                    tokio::time::sleep(wait).await;
                }
                Err(failure) => return Err(failure.into_error(&url)),
            }
        }
    }

    /// Post a message with no attachment — the "test" button, and the notice
    /// left behind when a file could not be made to fit.
    pub async fn send_message(&self, target: &Target, message: &str) -> Result<SentMessage> {
        self.send_message_with_options(target, message, &MessageOptions::default()).await
    }
    pub async fn send_message_with_options(&self, target: &Target, message: &str, options: &MessageOptions) -> Result<SentMessage> {
        let url = match target {
            Target::Webhook { url } => format!("{}?wait=true", normalize_webhook_url(url)?),
            Target::Channel { channel_id, .. } => format!("{API}/channels/{channel_id}/messages"),
        };
        let body = message_payload(target, message, options)?;
        let message: MessageResponse = self
            .send_with_rate_limit(&url, || {
                let mut request = self.client.post(&url).json(&body);
                if let Target::Channel { token, .. } = target {
                    request = request.header("Authorization", format!("Bot {token}"));
                }
                request
            })
            .await?;
        let channel_id = message.channel_id.unwrap_or_default();
        let guild = message.guild_id.as_deref().unwrap_or("@me");
        Ok(SentMessage {
            url: format!("https://discord.com/channels/{guild}/{channel_id}/{}", message.id),
            id: message.id,
            channel_id,
        })
    }

    // ---- plumbing -------------------------------------------------------

    /// Only the original webhook token or the configured bot can delete.
    pub async fn verify_legacy_webhook_message(&self, target: &Target, channel_id: &str, message_id: &str) -> Result<()> {
        let Target::Webhook { url } = target else { return Err(Error::InvalidInput("Original webhook is required.".into())); };
        let url = format!("{}/messages/{message_id}", normalize_webhook_url(url)?);
        let body: serde_json::Value = self.send_with_rate_limit(&url, || self.client.get(&url).timeout(Duration::from_secs(15))).await?;
        if body["id"].as_str() != Some(message_id) || body["channel_id"].as_str() != Some(channel_id) {
            return Err(Error::InvalidInput("The configured webhook does not own this recorded message. Reconnect the original webhook.".into()));
        }
        Ok(())
    }

    pub async fn delete_message(&self, target: &Target, channel_id: &str, message_id: &str) -> Result<()> {
        let (url, token) = match target {
            Target::Webhook { url } => (format!("{}/messages/{message_id}", normalize_webhook_url(url)?), None),
            Target::Channel { token, channel_id: configured } => {
                if configured != channel_id { return Err(Error::InvalidInput("Discord channel changed. Select the original channel before deleting.".into())); }
                (format!("{API}/channels/{channel_id}/messages/{message_id}"), Some(token.as_str()))
            }
        };
        let _: serde_json::Value = self.send_with_rate_limit_inner(&url, || {
            let request = self.client.delete(&url).timeout(Duration::from_secs(15));
            if let Some(token) = token { request.header("Authorization", format!("Bot {token}")) } else { request }
        }, true).await?;
        Ok(())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, token: &str, url: &str) -> Result<T> {
        self.send_with_rate_limit(url, || self.client.get(url).header("Authorization", format!("Bot {token}"))).await
    }

    /// Run a request, sleeping through a 429 for as long as Discord asks.
    ///
    /// Takes a builder rather than a request so each attempt gets a fresh one.
    async fn send_with_rate_limit<T, F>(&self, url: &str, build: F) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        F: Fn() -> reqwest::RequestBuilder,
    {
        self.send_with_rate_limit_inner(url, build, false).await
    }

    async fn send_with_rate_limit_inner<T, F>(&self, url: &str, build: F, allow_missing_message: bool) -> Result<T>
    where T: serde::de::DeserializeOwned, F: Fn() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let request = build();
            #[cfg(test)]
            let response = {
                let mut request = request.build()?;
                if let Some(origin) = &self.test_origin {
                    let mut url = reqwest::Url::parse(origin).unwrap();
                    url.set_path(request.url().path());
                    url.set_query(request.url().query());
                    *request.url_mut() = url;
                }
                self.client.execute(request).await
            };
            #[cfg(not(test))]
            let response = request.send().await;
            let response = response.map_err(|e| Error::Http(e.without_url()))?;
            match self.handle(response, url, allow_missing_message).await {
                Ok(body) => {
                    return serde_json::from_slice(&body).map_err(|e| Error::Corrupt(format!("Discord sent a response we could not read: {e}")));
                }
                Err(RestFailure::RateLimited(wait)) if attempt < RATE_LIMIT_ATTEMPTS && wait <= MAX_RATE_LIMIT_WAIT => {
                    tracing::info!(?wait, attempt, "Discord rate limited the request; waiting");
                    tokio::time::sleep(wait).await;
                }
                Err(failure) => return Err(failure.into_error(url)),
            }
        }
    }

    /// Split a response into "the bytes" or "why not", reading Discord's own
    /// explanation out of the error body while we still have it.
    async fn handle(&self, response: reqwest::Response, url: &str, allow_missing_message: bool) -> std::result::Result<Vec<u8>, RestFailure> {
        let status = response.status();
        if status.is_success() {
            // A 204 has no body; callers that want a message always pass
            // `wait=true`, so an empty body here means "nothing to report".
            let bytes = response.bytes().await.map_err(RestFailure::Transport)?;
            return Ok(if bytes.is_empty() { b"{}".to_vec() } else { bytes.to_vec() });
        }

        let retry_after_header = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        let body = response.bytes().await.unwrap_or_default();
        let parsed: Option<ApiErrorBody> = serde_json::from_slice(&body).ok();

        if allow_missing_message && status.as_u16() == 404 && parsed.as_ref().and_then(|b| b.code) == Some(10008) { return Ok(b"{}".to_vec()); }

        if status.as_u16() == 429 {
            let seconds = parsed.as_ref().and_then(|b| b.retry_after).or(retry_after_header).unwrap_or(1.0);
            return Err(RestFailure::RateLimited(Duration::from_secs_f64(seconds.clamp(0.0, 3600.0))));
        }

        let message = parsed
            .and_then(|b| b.message)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| describe_status(status.as_u16(), url));
        Err(RestFailure::Rejected { status: status.as_u16(), message })
    }
}

enum RestFailure {
    Transport(reqwest::Error),
    RateLimited(Duration),
    Rejected { status: u16, message: String },
}

impl RestFailure {
    fn into_error(self, _url: &str) -> Error {
        match self {
            RestFailure::Transport(e) => Error::Http(e.without_url()),
            // Only reached once the wait budget is spent; from here the
            // delivery's own retry schedule takes over.
            RestFailure::RateLimited(wait) => Error::ApiRejected { service: "Discord", status: 429, message: format!("Rate limited for {}s. Retry later.", wait.as_secs()) },
            RestFailure::Rejected { status, message } => Error::ApiRejected { service: "Discord", status, message },
        }
    }
}

/// A last-resort explanation when Discord sends a status with no message body.
fn describe_status(status: u16, _url: &str) -> String {
    match status {
        401 => "the token was rejected — it may have been regenerated in the Discord developer portal".into(),
        403 => "the bot is not allowed to post here — check it has been invited to the server and can send messages and attach files in this channel".into(),
        404 => "the channel or webhook no longer exists".into(),
        413 => "the file is larger than this server allows".into(),
        _ => format!("HTTP {status}"),
    }
}

/// Accept a webhook URL in any of the shapes Discord hands out and reduce it
/// to the bare endpoint, so a pasted URL with `?thread_id=` or a trailing
/// slash still works, and a pasted *anything else* fails here rather than
/// halfway through a delivery.
fn normalize_webhook_url(url: &str) -> Result<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let base = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let looks_right = base.starts_with("https://discord.com/api/webhooks/")
        || base.starts_with("https://discordapp.com/api/webhooks/")
        || base.starts_with("https://canary.discord.com/api/webhooks/")
        || base.starts_with("https://ptb.discord.com/api/webhooks/");
    if !looks_right {
        return Err(Error::InvalidInput(
            "that does not look like a Discord webhook URL — copy it from the channel's Integrations settings, and it will start with https://discord.com/api/webhooks/".into(),
        ));
    }
    // id/token, with nothing missing.
    let tail: Vec<&str> = base.trim_start_matches("https://").splitn(2, "/api/webhooks/").nth(1).unwrap_or_default().split('/').collect();
    if tail.len() != 2 || tail.iter().any(|part| part.is_empty()) {
        return Err(Error::InvalidInput("that webhook URL is incomplete — it should end with an id and a token".into()));
    }
    Ok(base.to_string())
}

/// Build the multipart body Discord expects: a JSON part describing the
/// message, and each file streamed from disk under its own `files[n]` part.
///
/// The index in the part name is what Discord uses to order the attachments,
/// so it follows the caller's order rather than whichever future finishes
/// first.
pub(crate) async fn file_form(payload: &str, files: &[(std::path::PathBuf, String)]) -> Result<reqwest::multipart::Form> {
    use tokio_util::io::ReaderStream;

    let mut form = reqwest::multipart::Form::new().text("payload_json", payload.to_string());
    for (index, (path, filename)) in files.iter().enumerate() {
        let file = tokio::fs::File::open(path).await.map_err(|source| Error::Io { path: path.clone(), source })?;
        let len = file.metadata().await.map_err(|source| Error::Io { path: path.clone(), source })?.len();
        let part = reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(ReaderStream::new(file)), len)
            .file_name(filename.clone())
            .mime_str("application/octet-stream")?;
        form = form.part(format!("files[{index}]"), part);
    }
    Ok(form)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_urls_are_accepted_in_the_shapes_people_paste() {
        let canonical = "https://discord.com/api/webhooks/123/abcdef";
        assert_eq!(normalize_webhook_url(canonical).unwrap(), canonical);
        assert_eq!(normalize_webhook_url("  https://discord.com/api/webhooks/123/abcdef/  ").unwrap(), canonical);
        assert_eq!(normalize_webhook_url("https://discord.com/api/webhooks/123/abcdef?thread_id=9").unwrap(), canonical);
        assert!(normalize_webhook_url("https://ptb.discord.com/api/webhooks/1/t").is_ok());
        assert!(normalize_webhook_url("https://discordapp.com/api/webhooks/1/t").is_ok());
    }

    #[test]
    fn anything_that_is_not_a_webhook_is_refused_before_a_delivery_starts() {
        for bad in [
            "",
            "not a url",
            "https://example.com/api/webhooks/1/t",
            "https://discord.com/api/webhooks/",
            "https://discord.com/api/webhooks/123",
            "https://discord.com/api/webhooks/123/",
            "http://discord.com/api/webhooks/123/abc",
        ] {
            assert!(normalize_webhook_url(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn the_error_for_a_bad_webhook_says_where_to_find_a_good_one() {
        let message = normalize_webhook_url("https://example.com/hook").unwrap_err().to_string();
        assert!(message.contains("Integrations"), "{message}");
    }

    #[test]
    fn the_invite_asks_for_posting_rights_and_nothing_more() {
        let url = Rest::invite_url("123456");
        assert!(url.contains("client_id=123456"));
        assert!(url.contains("scope=bot"));
        // Administrator (1 << 3) must never be in there.
        assert_eq!(INVITE_PERMISSIONS & (1 << 3), 0);
        assert_ne!(INVITE_PERMISSIONS & (1 << 15), 0, "attach files is the whole point");
    }

    #[test]
    fn a_target_never_prints_its_credential() {
        let webhook = Target::Webhook { url: "https://discord.com/api/webhooks/1/supersecret".into() };
        assert!(!format!("{webhook:?}").contains("supersecret"));
        let bot = Target::Channel { token: "tok-secret".into(), channel_id: "42".into() };
        let rendered = format!("{bot:?}");
        assert!(!rendered.contains("tok-secret"));
        assert!(rendered.contains("42"), "the channel is not a secret and is worth logging");
    }

    #[test]
    fn discords_own_field_names_are_what_gets_parsed() {
        // A `rename_all` on these structs would leave `premium_tier` at its
        // default, silently reporting every boosted server as unboosted.
        let guild: GuildDetail = serde_json::from_str(r#"{"id":"1","name":"Server","premium_tier":3}"#).unwrap();
        assert_eq!(guild.premium_tier, 3);

        let hook: WebhookInfo =
            serde_json::from_str(r#"{"id":"1","name":"Zoltraak","channel_id":"c1","guild_id":"g1"}"#).unwrap();
        assert_eq!(hook.channel_id.as_deref(), Some("c1"));
        assert_eq!(hook.guild_id.as_deref(), Some("g1"));

        // Channels arrive with `type`, which is the one field we do rename.
        let channel: Channel = serde_json::from_str(r#"{"id":"c1","name":"general","type":0,"position":2}"#).unwrap();
        assert_eq!(channel.kind, 0);
        assert_eq!(channel.position, 2);
    }

    /// The upload body, against a real HTTP server.
    ///
    /// `send_file` itself only talks to discord.com, so the one part of it
    /// that can be exercised locally is the part most likely to be wrong: the
    /// multipart body. Discord ignores a request whose file part is not named
    /// `files[0]`, and streams the file rather than buffering it, so both the
    /// part names and the bytes arriving intact are worth pinning down.
    #[tokio::test]
    async fn the_upload_body_carries_the_message_and_the_file_intact() {
        use axum::extract::Multipart;
        use std::sync::{Arc, Mutex};

        type Seen = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));

        let handler_seen = Arc::clone(&seen);
        let app = axum::Router::new().route(
            "/upload",
            axum::routing::post(move |mut form: Multipart| {
                let seen = Arc::clone(&handler_seen);
                async move {
                    while let Ok(Some(field)) = form.next_field().await {
                        let name = field.name().unwrap_or_default().to_string();
                        let filename = field.file_name().map(str::to_string);
                        let bytes = field.bytes().await.unwrap_or_default().to_vec();
                        seen.lock().unwrap().push((name, filename, bytes));
                    }
                    axum::Json(serde_json::json!({ "id": "42", "channel_id": "c1" }))
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // A payload larger than one buffer, so a streamed body that dropped
        // its tail would show up here.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        let contents: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &contents).unwrap();

        // A second, small file rides along: one message with several
        // attachments is the gallery case, and the part indices are what
        // Discord reads the order from.
        let second = dir.path().join("shot.png");
        std::fs::write(&second, b"png-bytes").unwrap();

        let payload = serde_json::json!({}).to_string();
        let files = [(path.clone(), "clip.mp4".to_string()), (second, "shot.png".to_string())];
        let form = file_form(&payload, &files).await.unwrap();
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/upload"))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());

        let parts = seen.lock().unwrap().clone();
        assert_eq!(parts.len(), 3, "a message part and one part per file");

        let (name, filename, bytes) = &parts[0];
        assert_eq!(name, "payload_json", "Discord reads the message from this exact field");
        assert!(filename.is_none());
        assert_eq!(String::from_utf8_lossy(bytes), payload);

        let (name, filename, bytes) = &parts[1];
        assert_eq!(name, "files[0]", "any other name and Discord silently drops the attachment");
        assert_eq!(filename.as_deref(), Some("clip.mp4"));
        assert_eq!(bytes.len(), contents.len(), "the streamed body must not be truncated");
        assert_eq!(bytes, &contents);

        let (name, filename, bytes) = &parts[2];
        assert_eq!(name, "files[1]", "the second attachment takes the next index, in order");
        assert_eq!(filename.as_deref(), Some("shot.png"));
        assert_eq!(bytes, b"png-bytes");
    }

    #[test]
    fn statuses_without_a_body_still_explain_themselves() {
        assert!(describe_status(403, "u").contains("invited"));
        assert!(describe_status(401, "u").contains("regenerated"));
        assert!(describe_status(404, "u").contains("no longer exists"));
    }
}
