//! Bot profile editing. Missing fields preserve remote values; null removes an image/override.
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use crate::error::{Error, Result};
use super::DiscordService;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProfileScope { Global, Application, Server }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRequest {
    pub scope: ProfileScope,
    pub guild_id: Option<String>,
    pub fields: Option<Map<String, Value>>,
}

fn invalid(message: &str) -> Error { Error::InvalidInput(message.into()) }

fn snowflake(value: &str) -> Result<()> {
    if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) || value.parse::<u64>().ok().filter(|n| *n > 0).is_none() {
        return Err(invalid("Choose a valid Discord server or channel."));
    }
    Ok(())
}

fn validate_image(value: &Value) -> Result<()> {
    if value.is_null() { return Ok(()); }
    let uri = value.as_str().ok_or_else(|| invalid("Choose a PNG, JPEG, or WebP image."))?;
    if uri.len() > 7 * 1024 * 1024 { return Err(invalid("Profile images must be at most 5 MiB.")); }
    let (header, encoded) = uri.split_once(',').ok_or_else(|| invalid("Invalid profile image."))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| invalid("Invalid profile image encoding."))?;
    let valid = match header {
        "data:image/png;base64" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "data:image/jpeg;base64" => bytes.starts_with(b"\xff\xd8\xff"),
        "data:image/webp;base64" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !valid || bytes.is_empty() { return Err(invalid("Choose a PNG, JPEG, or WebP image.")); }
    if bytes.len() > 5 * 1024 * 1024 { return Err(invalid("Profile images must be at most 5 MiB.")); }
    Ok(())
}

fn validate_fields(scope: ProfileScope, fields: &Map<String, Value>) -> Result<()> {
    for (key, value) in fields {
        let allowed = match scope {
            ProfileScope::Global => ["username", "avatar", "banner"].contains(&key.as_str()),
            ProfileScope::Application => ["description", "icon", "tags"].contains(&key.as_str()),
            ProfileScope::Server => ["nick", "bio", "avatar", "banner"].contains(&key.as_str()),
        };
        if !allowed { return Err(invalid("This field cannot be changed in the selected profile.")); }
        match key.as_str() {
            "avatar" | "banner" | "icon" => validate_image(value)?,
            "tags" => {
                let tags = value.as_array().ok_or_else(|| invalid("Tags must be a list."))?;
                if tags.len() > 5 || tags.iter().any(|v| v.as_str().is_none_or(|s| s.trim().is_empty() || s.chars().count() > 20)) {
                    return Err(invalid("Use at most five tags, each with 1–20 characters."));
                }
            }
            "nick" | "bio" if value.is_null() => {},
            _ => {
                let text = value.as_str().ok_or_else(|| invalid("Invalid profile text."))?;
                let max = match key.as_str() { "username" | "nick" => 32, "description" => 400, _ => 190 };
                if text.chars().count() > max || (key == "username" && text.trim().chars().count() < 2) {
                    return Err(invalid("Use a name of 2–32 characters, nickname up to 32, description up to 400, and bio up to 190."));
                }
            }
        }
    }
    Ok(())
}

impl DiscordService {
    pub async fn bot_profile(&self, request: ProfileRequest) -> Result<Value> {
        let token = self.require_bot_token().await?;
        let path = match request.scope {
            ProfileScope::Global => "users/@me".to_owned(),
            ProfileScope::Application => "applications/@me".to_owned(),
            ProfileScope::Server => {
                let guild = request.guild_id.as_deref().ok_or_else(|| invalid("Choose a server first."))?;
                snowflake(guild)?;
                if request.fields.is_some() {
                    format!("guilds/{guild}/members/@me")
                } else {
                    let identity = self.inner.rest.identity(&token).await?;
                    format!("guilds/{guild}/members/{}", identity.id)
                }
            }
        };
        if let Some(fields) = &request.fields { validate_fields(request.scope, fields)?; }
        let profile = self.inner.rest.profile_request(&token, &path, request.fields.as_ref())
            .await.inspect_err(|e| self.remember_error(e))?;
        if request.scope == ProfileScope::Global {
            if let Some(name) = profile["username"].as_str() { self.mutate_settings(|d| d.bot_username = Some(name.to_owned())).await?; }
        }
        // Return only the editor's fields, never owner details or other account data.
        let keys: &[&str] = match request.scope {
            ProfileScope::Global => &["id", "username", "avatar", "banner"],
            ProfileScope::Application => &["id", "name", "description", "icon", "tags", "bot_public"],
            ProfileScope::Server => &["nick", "bio", "avatar", "banner"],
        };
        Ok(Value::Object(keys.iter().map(|key| ((*key).to_owned(), profile[*key].clone())).collect()))
    }

    pub async fn bot_diagnostics(&self) -> Result<Value> {
        let token = self.require_bot_token().await?;
        let identity = self.inner.rest.identity(&token).await?;
        let settings = self.settings().await?;
        let (Some(guild), Some(channel)) = (settings.guild_id.as_deref(), settings.channel_id.as_deref()) else {
            return Ok(json!({"credentialValid": true, "destinationSelected": false, "checks": []}));
        };
        snowflake(guild)?; snowflake(channel)?;
        let rest = &self.inner.rest;
        let roles_path=format!("guilds/{guild}/roles");
        let member_path=format!("guilds/{guild}/members/{}", identity.id);
        let channel_path=format!("channels/{channel}");
        let (roles, member, destination) = tokio::try_join!(
            rest.profile_request(&token, &roles_path, None),
            rest.profile_request(&token, &member_path, None),
            rest.profile_request(&token, &channel_path, None),
        )?;
        if destination["guild_id"].as_str() != Some(guild) { return Err(invalid("The channel does not belong to the selected server.")); }
        let permissions = effective_permissions(guild, &identity.id, &roles, &member, &destination)?;
        let view = permissions & (1 << 10) != 0;
        let send = view && permissions & (1 << 11) != 0;
        let postable = matches!(destination["type"].as_u64(), Some(0 | 5));
        let timed_out = member["communication_disabled_until"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).is_some_and(|until| until > chrono::Utc::now());
        let checks = [
            ("View channel", view), ("Send messages", send && !timed_out),
            ("Attach files", send && permissions & (1 << 15) != 0 && !timed_out),
            ("Embed links", send && permissions & (1 << 14) != 0 && !timed_out),
            ("Direct file destination", postable),
        ].map(|(label, allowed)| json!({"label": label, "allowed": allowed}));
        Ok(json!({"credentialValid": true, "destinationSelected": true, "checks": checks}))
    }
}

fn bits(value: &Value) -> Result<u64> {
    value.as_str().and_then(|s| s.parse().ok()).ok_or_else(|| invalid("Discord returned unreadable permissions. Refresh and retry."))
}

fn effective_permissions(guild: &str, user: &str, roles: &Value, member: &Value, channel: &Value) -> Result<u64> {
    let roles = roles.as_array().ok_or_else(|| invalid("Discord returned unreadable roles."))?;
    let assigned = member["roles"].as_array().ok_or_else(|| invalid("Discord returned unreadable member roles."))?;
    let has_role = |id: &str| assigned.iter().any(|r| r.as_str() == Some(id));
    let mut permissions = 0;
    for role in roles {
        if role["id"].as_str().is_some_and(|id| id == guild || has_role(id)) { permissions |= bits(&role["permissions"])?; }
    }
    if permissions & (1 << 3) != 0 { return Ok(u64::MAX); }
    let overwrites = channel["permission_overwrites"].as_array().ok_or_else(|| invalid("Discord returned unreadable channel permissions."))?;
    // Discord applies everyone, all role overwrites together, then member overwrites.
    for pass in 0..3 {
        let (mut allow, mut deny) = (0, 0);
        for overwrite in overwrites {
            let id = overwrite["id"].as_str().unwrap_or_default();
            let kind = overwrite["type"].as_u64();
            let applies = match pass {
                0 => kind == Some(0) && id == guild,
                1 => kind == Some(0) && id != guild && has_role(id),
                _ => kind == Some(1) && id == user,
            };
            if applies { allow |= bits(&overwrite["allow"])?; deny |= bits(&overwrite["deny"])?; }
        }
        permissions = (permissions & !deny) | allow;
    }
    Ok(permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, Json, body::Bytes, http::{Method, StatusCode, Uri}, response::IntoResponse};
    use std::sync::{Arc, Mutex};
    use crate::{Queue, settings::{DiscordMode, DiscordSettings}, vault::{SecretVault, keys}};

    #[test]
    fn profile_validation_preserves_null_and_limits_scope_and_size() {
        let fields = |v: Value| v.as_object().unwrap().clone();
        assert!(validate_fields(ProfileScope::Global, &fields(json!({"username":"New bot", "avatar":null, "banner":null}))).is_ok());
        assert!(validate_fields(ProfileScope::Server, &fields(json!({"nick":null,"bio":null,"avatar":null,"banner":null}))).is_ok());
        for bad in [json!({"username":"a"}), json!({"bio":"Wrong scope"}), json!({"avatar":"https://example.com/a.png"}), json!({"banner":"data:image/png;base64,YmFk"})] {
            assert!(validate_fields(ProfileScope::Global, &fields(bad)).is_err());
        }
        assert!(validate_fields(ProfileScope::Application, &fields(json!({"tags":["one","two","three","four","five","six"]}))).is_err());
        assert!(validate_fields(ProfileScope::Server, &fields(json!({"bio":"a".repeat(191)}))).is_err());
        assert!(snowflake("../roles").is_err());
    }

    #[test]
    fn permissions_apply_role_union_then_member_override_and_admin_bypass() {
        let roles = json!([{"id":"1","permissions":"33792"},{"id":"2","permissions":"2048"},{"id":"3","permissions":"0"}]);
        let member = json!({"roles":["2","3"]});
        let mut channel = json!({"permission_overwrites":[
            {"id":"2","type":0,"deny":"32768","allow":"0"},
            {"id":"3","type":0,"deny":"0","allow":"32768"}
        ]});
        assert_ne!(effective_permissions("1","9",&roles,&member,&channel).unwrap() & (1 << 15), 0);
        channel["permission_overwrites"].as_array_mut().unwrap().push(json!({"id":"9","type":1,"deny":"32768","allow":"0"}));
        assert_eq!(effective_permissions("1","9",&roles,&member,&channel).unwrap() & (1 << 15), 0);
        let admin = json!([{"id":"1","permissions":"8"}]);
        assert_eq!(effective_permissions("1","9",&admin,&member,&channel).unwrap(), u64::MAX);
    }

    #[tokio::test]
    async fn profile_round_trip_scopes_updates_preserves_null_and_surfaces_rate_limits() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let requests = captured.clone();
        let app = Router::new().fallback(move |method: Method, uri: Uri, bytes: Bytes| {
            let requests = requests.clone();
            async move {
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                requests.lock().unwrap().push((method.clone(), uri.path().to_owned(), body.clone()));
                if body["username"] == "Limited" {
                    return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"retry_after":3600,"message":"Rate limited"}))).into_response();
                }
                let mut profile = match uri.path() {
                    "/api/v10/users/@me" => json!({"id":"42","username":"Bot","avatar":"old","banner":"keep","email":"private"}),
                    "/api/v10/applications/@me" => json!({"id":"42","name":"Application","description":"Before","icon":null,"tags":[],"owner":{"id":"secret"}}),
                    "/api/v10/guilds/7/members/@me" | "/api/v10/guilds/7/members/42" => json!({"nick":"Old nick","bio":null,"avatar":null,"banner":null,"roles":["99"]}),
                    "/api/v10/guilds/7/roles" => json!([{"id":"7","permissions":"52224"},{"id":"99","permissions":"0"}]),
                    "/api/v10/channels/8" => json!({"id":"8","guild_id":"7","type":0,"permission_overwrites":[{"id":"42","type":1,"allow":"0","deny":"32768"}]}),
                    _ => return StatusCode::NOT_FOUND.into_response(),
                };
                if method == Method::PATCH { for (k,v) in body.as_object().unwrap() { profile[k] = v.clone(); } }
                Json(profile).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let vault = Arc::new(SecretVault::in_memory());
        vault.set(keys::DISCORD_BOT_TOKEN, "bot-secret").await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut service = DiscordService::new(client, Queue::open_in_memory().await.unwrap(), vault);
        Arc::get_mut(&mut service.inner).unwrap().rest.test_origin = Some(origin);
        service.update_settings(DiscordSettings { mode: DiscordMode::Bot, ..Default::default() }).await.unwrap();
        let request = |scope, fields: Value| ProfileRequest { scope, guild_id: Some("7".into()), fields: fields.as_object().cloned() };
        let user = service.bot_profile(request(ProfileScope::Global, json!({"username":"Changed","avatar":null}))).await.unwrap();
        assert_eq!(user["banner"], "keep", "omitting banner preserves it");
        assert!(user["avatar"].is_null());
        assert!(user.get("email").is_none());
        assert_eq!(service.settings().await.unwrap().bot_username.as_deref(), Some("Changed"));
        let app = service.bot_profile(request(ProfileScope::Application, json!({"description":"After","tags":["download"]}))).await.unwrap();
        assert_eq!(app["description"], "After"); assert!(app.get("owner").is_none());
        let member = service.bot_profile(request(ProfileScope::Server, json!({"nick":null,"banner":null}))).await.unwrap();
        assert!(member["nick"].is_null()); assert!(member.get("roles").is_none());
        let error = service.bot_profile(request(ProfileScope::Global, json!({"username":"Limited"}))).await.unwrap_err();
        assert!(error.to_string().contains("3600"));
        assert_eq!(service.settings().await.unwrap().bot_username.as_deref(), Some("Changed"));
        let count = captured.lock().unwrap().len();
        assert!(service.bot_profile(request(ProfileScope::Global, json!({"token":"bad"}))).await.is_err());
        assert_eq!(captured.lock().unwrap().len(), count, "invalid fields never reach Discord");
        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 4, "a profile rate limit must not retry or sleep");
        assert_eq!(calls[2].1, "/api/v10/guilds/7/members/@me");
        assert_eq!(calls[0].2, json!({"username":"Changed","avatar":null}));
        drop(calls);
        let fetched = service.bot_profile(request(ProfileScope::Server, Value::Null)).await.unwrap();
        assert_eq!(fetched["nick"], "Old nick");
        assert_eq!(captured.lock().unwrap().last().unwrap().1, "/api/v10/guilds/7/members/42");
        let empty = service.bot_diagnostics().await.unwrap();
        assert_eq!(empty["destinationSelected"], false);
        let mut settings = service.settings().await.unwrap();
        settings.guild_id = Some("7".into()); settings.channel_id = Some("8".into());
        service.update_settings(settings).await.unwrap();
        let diagnostics = service.bot_diagnostics().await.unwrap();
        assert_eq!(diagnostics["credentialValid"], true);
        assert_eq!(diagnostics["checks"][1]["allowed"], true);
        assert_eq!(diagnostics["checks"][2]["allowed"], false, "member overwrite blocks attachments");
        server.abort();
    }
}
