//! The delivery worker, end to end, for every path that resolves without
//! talking to Discord.
//!
//! These are the outcomes a user hits most often and the ones most likely to
//! regress quietly: a file too large to send, a file that vanished between
//! finishing and being delivered, and the feature being switched off. Each one
//! must leave the delivery in a terminal state with a reason worth reading —
//! a delivery stuck at `pending` forever would be invisible.


use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zoltraak_core::discord::{DeliverySource, DeliveryState, DiscordService, NewDelivery};
use zoltraak_core::queue::Queue;
use zoltraak_core::settings::{DiscordMode, DiscordSettings};
use zoltraak_core::vault::{SecretVault, keys};

/// A service wired to an in-memory queue and vault, already "connected" to a
/// webhook. The URL is never reached: every case here is decided before the
/// upload would start.
async fn service() -> (DiscordService, tempfile::TempDir) {
    let queue = Queue::open_in_memory().await.unwrap();
    let vault = Arc::new(SecretVault::in_memory());
    vault
        .set(keys::DISCORD_WEBHOOK_URL, "https://discord.com/api/webhooks/123/never-used-in-these-tests")
        .await
        .unwrap();
    let service = DiscordService::new(reqwest::Client::new(), queue, vault);
    (service, tempfile::tempdir().unwrap())
}

async fn configure(service: &DiscordService, edit: impl FnOnce(&mut DiscordSettings)) {
    let mut settings = service.settings().await.unwrap();
    settings.enabled = true;
    settings.mode = DiscordMode::Webhook;
    edit(&mut settings);
    service.update_settings(settings).await.unwrap();
}

/// Wait for the worker to finish with a delivery. Polling beats a fixed sleep:
/// the assertion fails on a hang instead of on a slow machine.
async fn settled(service: &DiscordService, id: &str) -> zoltraak_core::discord::Delivery {
    for _ in 0..200 {
        let found = service.deliveries(50).await.unwrap().into_iter().find(|d| d.id == id);
        if let Some(delivery) = found {
            if delivery.state.is_terminal() {
                return delivery;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("delivery {id} never reached a terminal state");
}

fn write(dir: &tempfile::TempDir, name: &str, bytes: usize) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, vec![b'x'; bytes]).unwrap();
    path
}

#[tokio::test]
async fn an_oversized_file_is_skipped_with_both_sizes_named() {
    let (service, dir) = service().await;
    configure(&service, |d| {
        d.compress_to_fit = false;
        d.upload_limit_bytes = Some(1024 * 1024);
    })
    .await;
    tokio::spawn(service.clone().run_worker());

    let path = write(&dir, "big.bin", 4 * 1024 * 1024);
    let queued = service
        .enqueue(NewDelivery::from_path(DeliverySource::Download, Some("d1".into()), path).unwrap())
        .await
        .unwrap()
        .expect("a configured service accepts the delivery");

    let done = settled(&service, &queued.id).await;
    assert_eq!(done.state, DeliveryState::Skipped, "reason: {:?}", done.reason);
    let reason = done.reason.unwrap();
    assert!(reason.contains("4.0 MiB"), "the file's size should be named: {reason}");
    assert!(reason.contains("1.0 MiB"), "the server's limit should be named: {reason}");
    assert!(done.message_url.is_none());
}

#[tokio::test]
async fn an_archive_too_large_to_send_is_never_re_encoded() {
    let (service, dir) = service().await;
    // Fitting is on: an archive must still be refused rather than mangled,
    // and without ffmpeg being invoked at all.
    configure(&service, |d| {
        d.compress_to_fit = true;
        d.upload_limit_bytes = Some(1024 * 1024);
    })
    .await;
    tokio::spawn(service.clone().run_worker());

    let path = write(&dir, "release.zip", 3 * 1024 * 1024);
    let queued = service.enqueue(NewDelivery::from_path(DeliverySource::Manual, None, path.clone()).unwrap()).await.unwrap().unwrap();

    let done = settled(&service, &queued.id).await;
    assert_eq!(done.state, DeliveryState::Skipped);
    assert!(done.reason.unwrap().contains("without damaging them"));
    assert!(path.exists(), "the original download must survive a failed delivery");
}

#[tokio::test]
async fn a_file_that_vanished_before_delivery_is_skipped_not_failed() {
    let (service, dir) = service().await;
    configure(&service, |_| {}).await;
    tokio::spawn(service.clone().run_worker());

    let path = write(&dir, "gone.bin", 16);
    let queued = service.enqueue(NewDelivery::from_path(DeliverySource::Download, None, path.clone()).unwrap()).await.unwrap().unwrap();
    std::fs::remove_file(&path).unwrap();

    let done = settled(&service, &queued.id).await;
    assert_eq!(done.state, DeliveryState::Skipped, "a missing file is not a fault to report as an error");
    assert!(done.reason.unwrap().contains("moved or deleted"));
}

#[tokio::test]
async fn nothing_is_queued_while_discord_is_switched_off() {
    let (service, dir) = service().await;
    let path = write(&dir, "a.bin", 16);

    // Auto-send quietly does nothing...
    let auto = service.enqueue(NewDelivery::from_path(DeliverySource::Download, None, path.clone()).unwrap()).await.unwrap();
    assert!(auto.is_none(), "an unconfigured service must not collect deliveries it will never send");

    // ...but a manual send says why, because someone is watching.
    let manual = service.send_paths(vec![path]).await;
    let message = manual.unwrap_err().to_string();
    assert!(message.contains("Discord"), "the error should say where to fix it: {message}");
    assert!(service.deliveries(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_manual_send_of_a_missing_file_fails_immediately_rather_than_queueing() {
    let (service, _dir) = service().await;
    configure(&service, |_| {}).await;

    let error = service.send_paths(vec![PathBuf::from("/definitely/not/here.bin")]).await.unwrap_err();
    assert!(error.to_string().contains("no longer where the download left it"), "{error}");
    assert!(service.deliveries(10).await.unwrap().is_empty(), "a rejected send leaves no row behind");
}

#[tokio::test]
async fn a_retry_puts_a_skipped_delivery_back_through_the_worker() {
    let (service, dir) = service().await;
    configure(&service, |d| {
        d.compress_to_fit = false;
        d.upload_limit_bytes = Some(1024 * 1024);
    })
    .await;
    tokio::spawn(service.clone().run_worker());

    let path = write(&dir, "big.bin", 4 * 1024 * 1024);
    let queued = service.enqueue(NewDelivery::from_path(DeliverySource::Download, None, path).unwrap()).await.unwrap().unwrap();
    settled(&service, &queued.id).await;

    assert!(service.retry(&queued.id).await.unwrap());
    let again = settled(&service, &queued.id).await;
    assert_eq!(again.state, DeliveryState::Skipped, "the same limit produces the same answer");
    assert_eq!(again.attempts, 1, "the retry is counted");
}

#[tokio::test]
async fn the_status_never_carries_the_credential_it_reports_on() {
    let (service, _dir) = service().await;
    configure(&service, |_| {}).await;

    let status = service.status().await.unwrap();
    assert!(status.has_credential);
    let hint = status.credential_hint.clone().unwrap();
    assert!(!hint.contains("never-used"), "the hint must not reproduce the URL: {hint}");

    let rendered = serde_json::to_string(&status).unwrap();
    assert!(!rendered.contains("discord.com/api/webhooks"), "a status crossing IPC must not carry the webhook: {rendered}");
    assert_eq!(status.upload_limit_label, "19.1 MiB", "an unconfigured webhook uses the 20 MB preparation budget");
    assert!(!status.upload_limit_detected);
}

#[tokio::test]
async fn disconnecting_forgets_the_credential_and_stops_delivery() {
    let (service, dir) = service().await;
    configure(&service, |_| {}).await;
    service.disconnect().await.unwrap();

    let status = service.status().await.unwrap();
    assert!(!status.has_credential);
    assert!(!status.enabled);
    assert!(!status.connected);

    let path = write(&dir, "a.bin", 16);
    assert!(service.enqueue(NewDelivery::from_path(DeliverySource::Download, None, path).unwrap()).await.unwrap().is_none());
}

/// Grouping is a grouping of uploads, never of records.
///
/// Several files claimed in one batch share a message, but each still gets its
/// own row and its own outcome — so a mixed batch must not let one file's
/// verdict stand in for another's. Every one of these is decided before any
/// upload, which is the only part of the batch path reachable without a real
/// Discord behind it.
#[tokio::test]
async fn each_file_in_a_batch_keeps_its_own_outcome() {
    let (service, dir) = service().await;
    configure(&service, |d| {
        d.compress_to_fit = false;
        d.upload_limit_bytes = Some(1024 * 1024);
    })
    .await;
    tokio::spawn(service.clone().run_worker());

    let big = write(&dir, "big.bin", 4 * 1024 * 1024);
    let vanished = write(&dir, "gone.bin", 16);
    let queued_big = service
        .enqueue(NewDelivery::from_path(DeliverySource::Download, Some("d1".into()), big).unwrap())
        .await
        .unwrap()
        .unwrap();
    let queued_gone = service
        .enqueue(NewDelivery::from_path(DeliverySource::Download, Some("d2".into()), vanished.clone()).unwrap())
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(&vanished).unwrap();

    let big_done = settled(&service, &queued_big.id).await;
    let gone_done = settled(&service, &queued_gone.id).await;

    assert_eq!(big_done.state, DeliveryState::Skipped);
    assert!(big_done.reason.unwrap().contains("1.0 MiB"), "the size verdict belongs to the large file");
    assert_eq!(gone_done.state, DeliveryState::Skipped);
    assert!(gone_done.reason.unwrap().contains("moved or deleted"), "and the missing one keeps its own");
}
