use super::*;
use axum::{body::Bytes, http::{Method, StatusCode}, response::IntoResponse, Router};
use std::sync::Mutex;

async fn service() -> DiscordService {
    let vault = Arc::new(SecretVault::in_memory());
    vault.set(keys::DISCORD_WEBHOOK_URL, "https://discord.com/api/webhooks/123/secret").await.unwrap();
    let service = DiscordService::new(reqwest::Client::new(), Queue::open_in_memory().await.unwrap(), vault);
    service.update_settings(DiscordSettings { enabled: true, discord_user_id: Some("123456789012345678".into()),
        webhook_username: Some("My sender".into()), webhook_avatar_url: Some("https://example.com/avatar.png".into()), ..Default::default() }).await.unwrap();
    service
}

#[test]
fn webhook_avatar_links_refresh_discord_attachments_without_altering_other_hosts() {
    let target = Target::Webhook { url: "https://discord.com/api/webhooks/123/secret".into() };
    for (input, expected) in [
        ("https://cdn.discordapp.com/attachments/123/456/avatar.png?ex=abcd&is=1234&hm=signature", "https://cdn.discordapp.com/attachments/123/456/avatar.png"),
        ("https://images.example/avatar.png?token=keep", "https://images.example/avatar.png?token=keep"),
        ("https://cdn.discordapp.com.example/attachments/a.png?token=keep", "https://cdn.discordapp.com.example/attachments/a.png?token=keep"),
        ("https://cdn.discordapp.com/avatars/123/image.png?size=256", "https://cdn.discordapp.com/avatars/123/image.png?size=256"),
    ] {
        let options = rest::MessageOptions { avatar_url: Some(input.into()), ..Default::default() };
        assert_eq!(rest::message_payload(&target, "", &options).unwrap()["avatar_url"], expected);
        assert_eq!(options.avatar_url.as_deref(), Some(input), "saved URL stays available for previews");
    }
    assert!(rest::message_payload(&target, "", &rest::MessageOptions::default()).unwrap().get("avatar_url").is_none());
}

#[test]
fn payload_restricts_mentions_and_omits_webhook_identity_in_bot_mode() {
    let settings = DiscordSettings { webhook_username: Some("Sender".into()), webhook_avatar_url: Some("https://example.com/a.png".into()), ..Default::default() };
    let options = message_options(&settings, Some("123456789012345678".into()));
    let target = Target::Webhook { url: "https://discord.com/api/webhooks/123/secret".into() };
    let body = rest::message_payload(&target, "@everyone <@999>", &options).unwrap();
    assert_eq!(body["username"], "Sender");
    assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
    assert_eq!(body["allowed_mentions"]["users"], serde_json::json!(["123456789012345678"]));
    assert!(body["content"].as_str().unwrap().starts_with("<@123456789012345678>\n"));
    assert!(!target.reference().contains("secret"));
    let body = rest::message_payload(&Target::Channel { token: "secret".into(), channel_id: "c".into() }, "", &options).unwrap();
    assert!(body.get("username").is_none());
    assert!(body.get("avatar_url").is_none());
    let body = rest::message_payload(&target, "", &rest::MessageOptions::default()).unwrap();
    assert!(body.get("content").is_none());
    assert_eq!(body["allowed_mentions"]["users"], serde_json::json!([]));
    assert!(rest::message_payload(&target, &"a".repeat(2000), &options).is_err());
}

#[tokio::test]
async fn settings_validate_ids_defaults_and_preserve_old_limits() {
    let service = service().await;
    assert_eq!(DiscordSettings::default().upload_limit(), 20_000_000);
    let old: DiscordSettings = serde_json::from_str("{}").unwrap();
    assert!(!old.ping_sender);
    assert!(old.webhook_username.is_none());
    for invalid in ["0", "-1", "123x", "18446744073709551616"] {
        let mut settings = service.settings().await.unwrap();
        settings.discord_user_id = Some(invalid.into());
        assert!(service.update_settings(settings).await.is_err());
    }
    let mut settings = service.settings().await.unwrap();
    settings.ping_sender = true;
    settings.discord_user_id = Some(" ".into());
    settings.webhook_username = Some("  Sender  ".into());
    let settings = service.update_settings(settings).await.unwrap();
    assert!(!settings.ping_sender);
    assert_eq!(settings.webhook_username.as_deref(), Some("Sender"));
    let mut settings = settings;
    settings.webhook_avatar_url = Some("file:///avatar.png".into());
    assert!(service.update_settings(settings).await.is_err());
}

#[tokio::test]
async fn manual_options_survive_recovery_and_keep_submissions_separate() {
    let service = service().await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file.txt");
    std::fs::write(&file, "hello").unwrap();
    let options = DiscordSendOptions { caption: Some("literal {filename}".into()), ping_sender: Some(true), include_source_url: Some(true), compress_to_fit: Some(false), expected_destination: Some("webhook:123".into()) };
    let first = service.send_entries(vec![(file.clone(), Some("https://example.com/source".into()))], options.clone()).await.unwrap();
    service.send_entries(vec![(file.clone(), None)], DiscordSendOptions::default()).await.unwrap();
    let batch = delivery::next_pending_batch(&service.inner.queue.pool(), 10).await.unwrap();
    assert_eq!(batch.len(), 1);
    let stored = batch[0].send_options.as_ref().unwrap();
    assert_eq!(stored.caption, "literal {filename}");
    assert_eq!(stored.ping_user.as_deref(), Some("123456789012345678"));
    assert!(!stored.compress_to_fit);
    delivery::set_state(&service.inner.queue.pool(), &first[0].id, DeliveryState::Uploading).await.unwrap();
    delivery::recover_and_prune(&service.inner.queue.pool()).await.unwrap();
    let recovered = delivery::get(&service.inner.queue.pool(), &first[0].id).await.unwrap().unwrap();
    assert_eq!(recovered.state, DeliveryState::Pending);
    assert_eq!(recovered.send_options.unwrap().source_url.as_deref(), Some("https://example.com/source"));
    let mut settings = service.settings().await.unwrap();
    settings.mode = DiscordMode::Bot;
    settings.channel_id = Some("different".into());
    service.inner.vault.set(keys::DISCORD_BOT_TOKEN, "bot-secret").await.unwrap();
    service.update_settings(settings).await.unwrap();
    service.deliver_batch(first.clone()).await;
    let failed = delivery::get(&service.inner.queue.pool(), &first[0].id).await.unwrap().unwrap();
    assert_eq!(failed.state, DeliveryState::Failed);
    assert!(failed.reason.unwrap().contains("destination changed"));
    let before = service.deliveries(20).await.unwrap().len();
    assert!(service.send_entries(vec![(file, None)], options).await.is_err());
    assert_eq!(service.deliveries(20).await.unwrap().len(), before);
}

#[tokio::test]
async fn json_and_multipart_requests_preserve_identity_through_rate_limit_retry() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String)>::new()));
    let received = requests.clone();
    let app = Router::new().fallback(move |method: Method, body: Bytes| {
        let received = received.clone();
        async move {
            let mut received = received.lock().unwrap();
            received.push((method, String::from_utf8_lossy(&body).into_owned()));
            if received.len() == 2 { return (StatusCode::TOO_MANY_REQUESTS, axum::Json(serde_json::json!({"retry_after": 0.001}))).into_response(); }
            axum::Json(serde_json::json!({"id":"1", "channel_id":"2", "guild_id":"3"})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let mut service = service().await;
    Arc::get_mut(&mut service.inner).unwrap().rest.test_origin = Some(origin);
    service.test().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("hello.txt");
    std::fs::write(&file, "hello").unwrap();
    let entries = service.send_entries(vec![(file, None)], DiscordSendOptions { caption: Some("".into()), ping_sender: Some(true), ..Default::default() }).await.unwrap();
    service.deliver_batch(entries).await;
    let sent = service.deliveries(10).await.unwrap();
    assert_eq!(sent[0].state, DeliveryState::Sent, "{:?}", sent[0].reason);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (method, body) in requests.iter() {
        assert_eq!(*method, Method::POST);
        assert!(body.contains("My sender"));
        assert!(body.contains("https://example.com/avatar.png"));
        assert!(body.contains("allowed_mentions"));
    }
    assert!(requests[1].1.contains("payload_json"));
    assert!(requests[2].1.contains("123456789012345678"));
    server.abort();
}

#[test]
fn ping_templates_render_mentions_and_restrict_every_other_ping() {
    let target = Target::Webhook { url: "https://discord.com/api/webhooks/1/secret".into() };
    let mut settings = DiscordSettings { ping_message_template: "sent by {discord user id}".into(), ..Default::default() };
    let body = rest::message_payload(&target, "caption @everyone", &message_options(&settings, Some("123456".into()))).unwrap();
    assert_eq!(body["content"], "sent by <@123456>\ncaption @everyone");
    assert_eq!(body["allowed_mentions"]["users"], serde_json::json!(["123456"]));
    assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
    for (template, expected) in [("sent by {discord_user_id}", "sent by <@123456>"), ("uploaded", "uploaded <@123456>"), ("", "<@123456>")] {
        settings.ping_message_template = template.into();
        let body = rest::message_payload(&target, "", &message_options(&settings, Some("123456".into()))).unwrap();
        assert_eq!(body["content"], expected);
        let body = rest::message_payload(&target, "caption", &message_options(&settings, None)).unwrap();
        assert_eq!(body["content"], "caption");
    }
    settings.ping_message_template = "a".repeat(500);
    assert!(rest::message_payload(&target, &"b".repeat(1500), &message_options(&settings, Some("123456".into()))).is_err());
}

#[tokio::test]
async fn deletion_updates_grouped_history_and_is_idempotent() {
    use axum::http::Uri;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let received = calls.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri| {
        let calls = received.clone();
        async move {
            let mut calls = calls.lock().unwrap();calls.push((method,uri.path().to_string()));
            if calls.len() == 1 { return (StatusCode::TOO_MANY_REQUESTS,axum::Json(serde_json::json!({"retry_after":0.001}))).into_response(); }
            StatusCode::NO_CONTENT.into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}",listener.local_addr().unwrap());
    let server = tokio::spawn(async move {axum::serve(listener,app).await.unwrap();});
    let mut service=service().await;Arc::get_mut(&mut service.inner).unwrap().rest.test_origin=Some(origin);
    let dir=tempfile::tempdir().unwrap();let file=dir.path().join("kept.txt");std::fs::write(&file,"keep me").unwrap();
    let rows=service.send_paths(vec![file.clone(),file.clone()]).await.unwrap();
    let pool=service.inner.queue.pool();
    for row in &rows {delivery::mark_sent_for_target(&pool,&row.id,"https://discord.com/channels/3/2/1",7,"webhook:123").await.unwrap();}
    assert!(service.delete_message(&rows[0].id).await.unwrap());
    assert!(service.delete_message(&rows[1].id).await.unwrap());
    assert!(service.deliveries(10).await.unwrap().iter().all(|d|d.state==DeliveryState::Deleted));
    assert_eq!(std::fs::read_to_string(file).unwrap(),"keep me");
    assert_eq!(calls.lock().unwrap().len(),2);
    assert!(calls.lock().unwrap().iter().all(|(method,path)|*method==Method::DELETE && path=="/api/webhooks/123/secret/messages/1"));
    server.abort();
}

#[tokio::test]
async fn failed_or_changed_destination_deletion_preserves_sent_history() {
    let app=Router::new().fallback(||async{(StatusCode::UNAUTHORIZED,axum::Json(serde_json::json!({"message":"Invalid webhook token"})))});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let origin=format!("http://{}",listener.local_addr().unwrap());
    let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
    let mut service=service().await;Arc::get_mut(&mut service.inner).unwrap().rest.test_origin=Some(origin);
    let dir=tempfile::tempdir().unwrap();let file=dir.path().join("file.txt");std::fs::write(&file,"hello").unwrap();
    let row=service.send_paths(vec![file]).await.unwrap().remove(0);let pool=service.inner.queue.pool();
    delivery::mark_sent_for_target(&pool,&row.id,"https://discord.com/channels/3/2/1",5,"webhook:999").await.unwrap();
    assert!(service.delete_message(&row.id).await.unwrap_err().to_string().contains("destination changed"));
    delivery::mark_sent_for_target(&pool,&row.id,"https://discord.com/channels/3/2/1",5,"webhook:123").await.unwrap();
    let error=service.delete_message(&row.id).await.unwrap_err().to_string();assert!(error.contains("Invalid webhook token"));assert!(!error.contains("secret"));
    assert_eq!(delivery::get(&pool,&row.id).await.unwrap().unwrap().state,DeliveryState::Sent);
    server.abort();
}

#[tokio::test]
async fn deletion_distinguishes_missing_message_from_missing_webhook() {
    let code=Arc::new(Mutex::new(10008));let received=code.clone();
    let app=Router::new().fallback(move ||{let code=received.clone();async move{(StatusCode::NOT_FOUND,axum::Json(serde_json::json!({"code":*code.lock().unwrap(),"message":"Gone"})))}});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let origin=format!("http://{}",listener.local_addr().unwrap());
    let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
    let mut service=service().await;Arc::get_mut(&mut service.inner).unwrap().rest.test_origin=Some(origin);
    let target=Target::Webhook{url:"https://discord.com/api/webhooks/123/secret".into()};
    service.inner.rest.delete_message(&target,"2","1").await.unwrap();
    *code.lock().unwrap()=10015;
    assert!(service.inner.rest.delete_message(&target,"2","1").await.is_err());
    // Bot requests cannot be redirected to a different channel.
    assert!(service.inner.rest.delete_message(&Target::Channel{token:"secret".into(),channel_id:"9".into()},"2","1").await.is_err());
    server.abort();
}

#[tokio::test]
async fn older_webhook_deliveries_require_ownership_verification() {
    use axum::http::Uri;
    let requests=Arc::new(Mutex::new(Vec::new()));let received=requests.clone();
    let app=Router::new().fallback(move |method:Method,uri:Uri|{let requests=received.clone();async move{
        requests.lock().unwrap().push(method.clone());
        if method==Method::GET && uri.path().ends_with("/1") {return axum::Json(serde_json::json!({"id":"1","channel_id":"2"})).into_response();}
        if method==Method::GET {return (StatusCode::NOT_FOUND,axum::Json(serde_json::json!({"code":10008,"message":"Unknown Message"}))).into_response();}
        StatusCode::NO_CONTENT.into_response()
    }});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let origin=format!("http://{}",listener.local_addr().unwrap());
    let server=tokio::spawn(async move{axum::serve(listener,app).await.unwrap();});
    let mut service=service().await;Arc::get_mut(&mut service.inner).unwrap().rest.test_origin=Some(origin);
    let dir=tempfile::tempdir().unwrap();let file=dir.path().join("old.txt");std::fs::write(&file,"old").unwrap();
    let new=delivery::NewDelivery::from_path(DeliverySource::Download,None,file).unwrap();let pool=service.inner.queue.pool();
    let owned=delivery::insert(&pool,&new).await.unwrap();let unknown=delivery::insert(&pool,&new).await.unwrap();
    delivery::mark_sent(&pool,&owned.id,"https://discord.com/channels/3/2/1",3).await.unwrap();
    delivery::mark_sent(&pool,&unknown.id,"https://discord.com/channels/3/2/9",3).await.unwrap();
    service.delete_message(&owned.id).await.unwrap();assert!(service.delete_message(&unknown.id).await.is_err());
    assert_eq!(delivery::get(&pool,&unknown.id).await.unwrap().unwrap().state,DeliveryState::Sent);
    assert_eq!(*requests.lock().unwrap(),vec![Method::GET,Method::DELETE,Method::GET]);
    server.abort();
}
