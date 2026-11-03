//! The record of what Zoltraak has tried to put into Discord, and the queries
//! the worker runs against it.
//!
//! Deliveries are persisted rather than kept in memory because the interesting
//! failures outlive a process: a token revoked overnight, a server that lost a
//! boost tier, an upload interrupted by a restart. A row also gives the UI
//! something to show and a retry something to act on.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::{Error, Result};

/// What produced the file. Manual sends have no download behind them, so the
/// source id is optional rather than the source being faked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverySource {
    Download,
    Video,
    /// A music job: a tagged track produced by the music manager.
    Music,
    Manual,
}

impl DeliverySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Video => "video",
            Self::Music => "music",
            Self::Manual => "manual",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "download" => Ok(Self::Download),
            "video" => Ok(Self::Video),
            "music" => Ok(Self::Music),
            "manual" => Ok(Self::Manual),
            other => Err(Error::Corrupt(format!("unknown delivery source {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Waiting for the worker.
    Pending,
    /// Being re-encoded to fit under the upload limit.
    Preparing,
    Uploading,
    Sent,
    Deleted,
    /// Deliberately not sent, and not an error: too large to compress, or the
    /// file went missing between finishing and being delivered.
    Skipped,
    Failed,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Preparing => "preparing",
            Self::Uploading => "uploading",
            Self::Sent => "sent",
            Self::Deleted => "deleted",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "preparing" => Ok(Self::Preparing),
            "uploading" => Ok(Self::Uploading),
            "sent" => Ok(Self::Sent),
            "deleted" => Ok(Self::Deleted),
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            other => Err(Error::Corrupt(format!("unknown delivery state {other}"))),
        }
    }

    /// Whether nothing further will happen to this delivery on its own.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Sent | Self::Deleted | Self::Skipped | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub source: DeliverySource,
    pub source_id: Option<String>,
    pub path: PathBuf,
    pub filename: String,
    pub bytes: u64,
    pub state: DeliveryState,
    pub reason: Option<String>,
    pub message_url: Option<String>,
    pub message_destination: Option<String>,
    /// What was uploaded, when that differs from `bytes` because the file was
    /// re-encoded. `None` until something has actually been sent.
    pub sent_bytes: Option<u64>,
    pub attempts: u32,
    pub send_options: Option<super::StoredSendOptions>,
    pub submission_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A delivery to create. Separate from [`Delivery`] so callers cannot invent
/// an id, a state or a timestamp.
#[derive(Debug, Clone)]
pub struct NewDelivery {
    pub source: DeliverySource,
    pub source_id: Option<String>,
    pub path: PathBuf,
    pub filename: String,
    pub bytes: u64,
}

impl NewDelivery {
    /// Build one from a file that has just finished, reading its size from
    /// disk. Returns `None` when the file is gone — a delivery for a file
    /// that no longer exists is not worth a row.
    pub fn from_path(source: DeliverySource, source_id: Option<String>, path: PathBuf) -> Option<Self> {
        let bytes = std::fs::metadata(&path).ok()?.len();
        let filename = path.file_name()?.to_str()?.to_string();
        Some(Self { source, source_id, path, filename, bytes })
    }
}

/// How many deliveries are kept. This is a log for the user, not an audit
/// trail; the oldest entries stop being interesting almost immediately.
pub const HISTORY_LIMIT: i64 = 200;

pub async fn insert(pool: &SqlitePool, new: &NewDelivery) -> Result<Delivery> {
    insert_with_options(pool, new, None, None).await
}
pub async fn insert_with_options(pool: &SqlitePool, new: &NewDelivery, options: Option<super::StoredSendOptions>, submission: Option<String>) -> Result<Delivery> {
    let now = Utc::now();
    let delivery = Delivery {
        id: uuid::Uuid::new_v4().to_string(),
        source: new.source,
        source_id: new.source_id.clone(),
        path: new.path.clone(),
        filename: new.filename.clone(),
        bytes: new.bytes,
        state: DeliveryState::Pending,
        reason: None,
        message_url: None,
        message_destination: None,
        sent_bytes: None,
        attempts: 0,
        send_options: options,
        submission_id: submission,
        created_at: now,
        updated_at: now,
    };
    sqlx::query(
        "INSERT INTO discord_deliveries (id, source, source_id, path, filename, bytes, state, attempts, created_at, updated_at, send_options, submission_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
    )
    .bind(&delivery.id)
    .bind(delivery.source.as_str())
    .bind(&delivery.source_id)
    .bind(delivery.path.to_string_lossy().to_string())
    .bind(&delivery.filename)
    .bind(delivery.bytes as i64)
    .bind(delivery.state.as_str())
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(delivery.send_options.as_ref().map(serde_json::to_string).transpose().map_err(|e| Error::Corrupt(e.to_string()))?)
    .bind(&delivery.submission_id)
    .execute(pool)
    .await?;
    Ok(delivery)
}

/// The oldest delivery still waiting to be attempted.
pub async fn next_pending(pool: &SqlitePool) -> Result<Option<Delivery>> {
    let row = sqlx::query("SELECT * FROM discord_deliveries WHERE state = 'pending' ORDER BY created_at ASC LIMIT 1")
        .fetch_optional(pool)
        .await?;
    row.map(|r| to_delivery(&r)).transpose()
}

/// The oldest deliveries still waiting, oldest first, up to `limit`.
///
/// Taking several at once is what lets the worker put a post's pictures into
/// one Discord message instead of a run of near-identical ones. The rows are
/// only *read* here; each is still moved through its own states, so a group
/// that partly fails leaves accurate rows behind.
pub async fn next_pending_batch(pool: &SqlitePool, limit: usize) -> Result<Vec<Delivery>> {
    let rows = sqlx::query("SELECT * FROM discord_deliveries WHERE state = 'pending' ORDER BY created_at ASC LIMIT ?")
        .bind(limit.clamp(1, 50) as i64)
        .fetch_all(pool)
        .await?;
    let mut deliveries: Vec<Delivery> = rows.iter().map(to_delivery).collect::<Result<_>>()?;
    if let Some(first) = deliveries.first() {
        let submission = first.submission_id.clone();
        deliveries = deliveries.into_iter().take_while(|d| d.submission_id == submission).collect();
    }
    Ok(deliveries)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Delivery>> {
    let row = sqlx::query("SELECT * FROM discord_deliveries WHERE id = ?").bind(id).fetch_optional(pool).await?;
    row.map(|r| to_delivery(&r)).transpose()
}

/// Newest first — the order the UI shows them in.
pub async fn list(pool: &SqlitePool, limit: i64) -> Result<Vec<Delivery>> {
    let rows = sqlx::query("SELECT * FROM discord_deliveries ORDER BY created_at DESC LIMIT ?")
        .bind(limit.clamp(1, HISTORY_LIMIT))
        .fetch_all(pool)
        .await?;
    rows.iter().map(to_delivery).collect()
}

pub async fn set_state(pool: &SqlitePool, id: &str, state: DeliveryState) -> Result<()> {
    sqlx::query("UPDATE discord_deliveries SET state = ?, updated_at = ? WHERE id = ?")
        .bind(state.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Record a successful send: where the message is, and how big what we
/// actually uploaded turned out to be.
pub async fn mark_sent(pool: &SqlitePool, id: &str, message_url: &str, sent_bytes: u64) -> Result<()> {
    sqlx::query("UPDATE discord_deliveries SET state = 'sent', message_url = ?, sent_bytes = ?, reason = NULL, updated_at = ? WHERE id = ?")
        .bind(message_url)
        .bind(sent_bytes as i64)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Persist the non-secret destination alongside the returned message URL.
pub async fn mark_sent_for_target(pool: &SqlitePool, id: &str, message_url: &str, sent_bytes: u64, destination: &str) -> Result<()> {
    sqlx::query("UPDATE discord_deliveries SET state = 'sent', message_url = ?, message_destination = ?, sent_bytes = ?, reason = NULL, updated_at = ? WHERE id = ?")
        .bind(message_url).bind(destination).bind(sent_bytes as i64).bind(Utc::now().to_rfc3339()).bind(id).execute(pool).await?;
    Ok(())
}
/// One Discord message can contain several delivery rows. Keep all in sync.
pub async fn mark_message_deleted(pool: &SqlitePool, message_url: &str) -> Result<()> {
    sqlx::query("UPDATE discord_deliveries SET state = 'deleted', reason = 'Message deleted from Discord', updated_at = ? WHERE message_url = ? AND state = 'sent'")
        .bind(Utc::now().to_rfc3339()).bind(message_url).execute(pool).await?;
    Ok(())
}

/// Finish a delivery without sending it, with the reason a person would want
/// to read. `state` is `Skipped` when nothing went wrong and `Failed` when
/// something did — the difference matters to whoever reads the list.
pub async fn finish_with_reason(pool: &SqlitePool, id: &str, state: DeliveryState, reason: &str) -> Result<()> {
    sqlx::query("UPDATE discord_deliveries SET state = ?, reason = ?, updated_at = ? WHERE id = ?")
        .bind(state.as_str())
        .bind(reason)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Put a finished delivery back in the queue. Attempts are counted here rather
/// than at claim time so a retry the user asked for is visible as one.
pub async fn requeue(pool: &SqlitePool, id: &str) -> Result<bool> {
    let affected = sqlx::query(
        "UPDATE discord_deliveries SET state = 'pending', reason = NULL, attempts = attempts + 1, updated_at = ?
         WHERE id = ? AND state IN ('sent', 'skipped', 'failed')",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(affected > 0)
}

pub async fn clear_finished(pool: &SqlitePool) -> Result<u64> {
    let affected = sqlx::query("DELETE FROM discord_deliveries WHERE state IN ('sent', 'deleted', 'skipped', 'failed')")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected)
}

/// Recover from an unclean shutdown and trim the history.
///
/// A delivery left `preparing` or `uploading` has no worker behind it any
/// more: the process that owned it is gone. Sending again is safe — at worst
/// a duplicate message, against the certainty of a file silently never
/// arriving — so those go back to `pending`.
pub async fn recover_and_prune(pool: &SqlitePool) -> Result<()> {
    let stranded = sqlx::query("UPDATE discord_deliveries SET state = 'pending', updated_at = ? WHERE state IN ('preparing', 'uploading')")
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await?
        .rows_affected();
    if stranded > 0 {
        tracing::info!(stranded, "requeued Discord deliveries left in flight by a previous run");
    }
    sqlx::query(
        "DELETE FROM discord_deliveries WHERE id NOT IN
           (SELECT id FROM discord_deliveries ORDER BY created_at DESC LIMIT ?)",
    )
    .bind(HISTORY_LIMIT)
    .execute(pool)
    .await?;
    Ok(())
}

fn to_delivery(row: &sqlx::sqlite::SqliteRow) -> Result<Delivery> {
    let parse_time = |value: String| -> Result<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&value)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| Error::Corrupt(format!("unreadable delivery timestamp: {e}")))
    };
    Ok(Delivery {
        id: row.try_get("id")?,
        source: DeliverySource::parse(row.try_get::<String, _>("source")?.as_str())?,
        source_id: row.try_get("source_id")?,
        path: PathBuf::from(row.try_get::<String, _>("path")?),
        filename: row.try_get("filename")?,
        bytes: row.try_get::<i64, _>("bytes")? as u64,
        state: DeliveryState::parse(row.try_get::<String, _>("state")?.as_str())?,
        reason: row.try_get("reason")?,
        message_url: row.try_get("message_url")?,
        message_destination: row.try_get("message_destination")?,
        sent_bytes: row.try_get::<Option<i64>, _>("sent_bytes")?.map(|n| n as u64),
        attempts: row.try_get::<i64, _>("attempts")? as u32,
        send_options: row.try_get::<Option<String>, _>("send_options")?.map(|s| serde_json::from_str(&s).map_err(|e| Error::Corrupt(e.to_string()))).transpose()?,
        submission_id: row.try_get("submission_id")?,
        created_at: parse_time(row.try_get("created_at")?)?,
        updated_at: parse_time(row.try_get("updated_at")?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn sample(name: &str) -> NewDelivery {
        NewDelivery {
            source: DeliverySource::Download,
            source_id: Some("abc".into()),
            path: PathBuf::from(format!("/downloads/{name}")),
            filename: name.into(),
            bytes: 1234,
        }
    }

    #[tokio::test]
    async fn a_delivery_starts_pending_and_is_claimed_oldest_first() {
        let pool = pool().await;
        let first = insert(&pool, &sample("a.mp4")).await.unwrap();
        // Timestamps have sub-second resolution; make the order unambiguous.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        insert(&pool, &sample("b.mp4")).await.unwrap();

        assert_eq!(first.state, DeliveryState::Pending);
        assert_eq!(next_pending(&pool).await.unwrap().unwrap().id, first.id);

        set_state(&pool, &first.id, DeliveryState::Uploading).await.unwrap();
        assert_eq!(next_pending(&pool).await.unwrap().unwrap().filename, "b.mp4");
    }

    #[tokio::test]
    async fn sending_records_the_link_and_what_was_actually_uploaded() {
        let pool = pool().await;
        let d = insert(&pool, &sample("big.mp4")).await.unwrap();
        mark_sent(&pool, &d.id, "https://discord.com/channels/1/2/3", 999).await.unwrap();

        let stored = get(&pool, &d.id).await.unwrap().unwrap();
        assert_eq!(stored.state, DeliveryState::Sent);
        assert_eq!(stored.message_url.as_deref(), Some("https://discord.com/channels/1/2/3"));
        assert_eq!(stored.sent_bytes, Some(999));
        assert_eq!(stored.bytes, 1234, "the original size is still on the row");
        assert!(stored.reason.is_none());
    }

    #[tokio::test]
    async fn skipping_and_failing_both_keep_their_reason() {
        let pool = pool().await;
        let skipped = insert(&pool, &sample("huge.zip")).await.unwrap();
        finish_with_reason(&pool, &skipped.id, DeliveryState::Skipped, "too large to compress").await.unwrap();
        let failed = insert(&pool, &sample("x.mp4")).await.unwrap();
        finish_with_reason(&pool, &failed.id, DeliveryState::Failed, "Discord refused the request").await.unwrap();

        let listed = list(&pool, 10).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, failed.id, "newest first");
        assert!(listed.iter().all(|d| d.reason.is_some() && d.state.is_terminal()));
    }

    #[tokio::test]
    async fn a_retry_puts_a_finished_delivery_back_and_counts_the_attempt() {
        let pool = pool().await;
        let d = insert(&pool, &sample("a.mp4")).await.unwrap();
        assert!(!requeue(&pool, &d.id).await.unwrap(), "a pending delivery is already queued");

        finish_with_reason(&pool, &d.id, DeliveryState::Failed, "boom").await.unwrap();
        assert!(requeue(&pool, &d.id).await.unwrap());

        let stored = get(&pool, &d.id).await.unwrap().unwrap();
        assert_eq!(stored.state, DeliveryState::Pending);
        assert_eq!(stored.attempts, 1);
        assert!(stored.reason.is_none(), "the old failure must not linger on a fresh attempt");
    }

    #[tokio::test]
    async fn a_restart_requeues_whatever_was_in_flight() {
        let pool = pool().await;
        let uploading = insert(&pool, &sample("a.mp4")).await.unwrap();
        let preparing = insert(&pool, &sample("b.mp4")).await.unwrap();
        let done = insert(&pool, &sample("c.mp4")).await.unwrap();
        set_state(&pool, &uploading.id, DeliveryState::Uploading).await.unwrap();
        set_state(&pool, &preparing.id, DeliveryState::Preparing).await.unwrap();
        mark_sent(&pool, &done.id, "https://discord.com/channels/1/2/3", 10).await.unwrap();

        recover_and_prune(&pool).await.unwrap();

        assert_eq!(get(&pool, &uploading.id).await.unwrap().unwrap().state, DeliveryState::Pending);
        assert_eq!(get(&pool, &preparing.id).await.unwrap().unwrap().state, DeliveryState::Pending);
        assert_eq!(get(&pool, &done.id).await.unwrap().unwrap().state, DeliveryState::Sent, "a finished delivery is left alone");
    }

    #[tokio::test]
    async fn clearing_removes_finished_deliveries_and_leaves_the_queue_alone() {
        let pool = pool().await;
        let pending = insert(&pool, &sample("a.mp4")).await.unwrap();
        let sent = insert(&pool, &sample("b.mp4")).await.unwrap();
        mark_sent(&pool, &sent.id, "https://discord.com/channels/1/2/3", 5).await.unwrap();

        assert_eq!(clear_finished(&pool).await.unwrap(), 1);
        let remaining = list(&pool, 10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, pending.id);
    }

    #[tokio::test]
    async fn a_delivery_is_not_created_for_a_file_that_is_gone() {
        assert!(NewDelivery::from_path(DeliverySource::Manual, None, PathBuf::from("/nope/missing.bin")).is_none());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("real.bin");
        std::fs::write(&path, b"hello").unwrap();
        let new = NewDelivery::from_path(DeliverySource::Manual, None, path).unwrap();
        assert_eq!(new.bytes, 5);
        assert_eq!(new.filename, "real.bin");
    }
}
