use super::*;
use axum::{Json, Router, http::{Method, StatusCode, Uri}, response::IntoResponse};
use std::sync::Mutex;

async fn fixture(mode: DiscordMode, status: u16, missing_channel: bool) -> (DiscordService, tokio::task::JoinHandle<()>, Arc<Mutex<Vec<Method>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let received = requests.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri| {
        let received = received.clone();
        async move {
            received.lock().unwrap().push(method);
            if status != 200 {
                return (StatusCode::from_u16(status).unwrap(), Json(serde_json::json!({"message":"fixture rejection"}))).into_response();
            }
            let data = match uri.path() {
                "/api/v10/users/@me" => serde_json::json!({"id":"bot", "username":"Fresh bot"}),
                "/api/v10/guilds/g" => serde_json::json!({"id":"g", "name":"Fresh server", "premium_tier":2}),
                "/api/v10/guilds/g/channels" => if missing_channel { serde_json::json!([]) } else { serde_json::json!([{"id":"c", "name":"fresh-channel", "type":0}]) },
                "/api/webhooks/123/saved-secret" => serde_json::json!({"id":"123", "name":"Fresh webhook", "guild_id":"g", "channel_id":"c"}),
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            Json(data).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let vault = Arc::new(SecretVault::in_memory());
    vault.set(keys::DISCORD_WEBHOOK_URL, "https://discord.com/api/webhooks/123/saved-secret").await.unwrap();
    vault.set(keys::DISCORD_BOT_TOKEN, "saved-bot-secret").await.unwrap();
    let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(2)).build().unwrap();
    let mut service = DiscordService::new(client, Queue::open_in_memory().await.unwrap(), vault);
    Arc::get_mut(&mut service.inner).unwrap().rest.test_origin = Some(origin);
    service.update_settings(DiscordSettings {
        mode, enabled: true, auto_send: false, guild_id: Some("g".into()), channel_id: Some("c".into()),
        upload_limit_bytes: Some(1_234_567), bot_upload_limit_bytes: 32_500_000, message_template: "keep caption".into(), ..Default::default()
    }).await.unwrap();
    (service, server, requests)
}

#[tokio::test]
async fn reconnect_validates_both_modes_without_enabling_delivery_or_posting() {
    for mode in [DiscordMode::Webhook, DiscordMode::Bot] {
        let (service, server, requests) = fixture(mode, 200, false).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("queued.txt");
        std::fs::write(&path, "pending delivery").unwrap();
        service.send_paths(vec![path]).await.unwrap();
        let before_queue = serde_json::to_value(service.deliveries(10).await.unwrap()).unwrap();
        let mut before = service.settings().await.unwrap();
        before.enabled = false;
        service.update_settings(before.clone()).await.unwrap();
        let secret = service.inner.vault.get(credential_key(mode)).await;

        let result = service.reconnect().await.unwrap();

        let after = service.settings().await.unwrap();
        assert!(!after.enabled);
        assert_eq!(after.auto_send, before.auto_send);
        assert_eq!(after.message_template, before.message_template);
        assert_eq!(after.compress_to_fit, before.compress_to_fit);
        assert_eq!(service.inner.vault.get(credential_key(mode)).await, secret);
        assert_eq!(serde_json::to_value(service.deliveries(10).await.unwrap()).unwrap(), before_queue);
        assert!(requests.lock().unwrap().iter().all(|method| *method == Method::GET));
        assert!(service.status().await.unwrap().last_error.is_none());
        assert!(!serde_json::to_string(&result).unwrap().contains("secret"));
        if mode == DiscordMode::Bot {
            assert_eq!(after.bot_upload_limit_bytes, 32_500_000);
            assert_eq!(after.upload_limit(), 32_500_000, "reconnecting preserves the custom bot budget");
            let target = service.set_target("g", "c").await.unwrap();
            assert_eq!(target.upload_limit(), 32_500_000, "channel selection preserves the custom bot budget");
            assert_eq!(after.channel_name.as_deref(), Some("fresh-channel"));
            assert_eq!(after.guild_name.as_deref(), Some("Fresh server"));
            assert_eq!(after.upload_limit_bytes, before.upload_limit_bytes, "bot discovery preserves the webhook budget");
        } else {
            assert_eq!(after.upload_limit_bytes, before.upload_limit_bytes);
        }
        server.abort();
    }
}

#[tokio::test]
async fn reconnect_failures_preserve_configuration_and_credentials() {
    for (mode, status, missing) in [
        (DiscordMode::Webhook, 401, false), (DiscordMode::Webhook, 404, false),
        (DiscordMode::Bot, 403, false), (DiscordMode::Bot, 500, false), (DiscordMode::Bot, 200, true),
    ] {
        let (service, server, _) = fixture(mode, status, missing).await;
        let before = service.settings().await.unwrap();
        let secret = service.inner.vault.get(credential_key(mode)).await;
        assert!(service.reconnect().await.is_err());
        assert_eq!(service.settings().await.unwrap(), before);
        assert_eq!(service.inner.vault.get(credential_key(mode)).await, secret);
        assert!(service.status().await.unwrap().last_error.is_some());
        server.abort();
    }
}

#[tokio::test]
async fn reconnect_handles_missing_credentials_and_network_failure_without_leaking_secrets() {
    let (service, server, _) = fixture(DiscordMode::Webhook, 200, false).await;
    server.abort();
    let _ = server.await;
    let before = service.settings().await.unwrap();
    let error = service.reconnect().await.unwrap_err().to_string();
    assert!(!error.contains("saved-secret"));
    assert_eq!(service.settings().await.unwrap(), before);
    service.inner.vault.remove(keys::DISCORD_WEBHOOK_URL).await.unwrap();
    assert!(service.reconnect().await.unwrap_err().to_string().contains("No saved Discord credential"));
}
