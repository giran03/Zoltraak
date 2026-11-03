//! SQLite-backed download queue and history.
//!
//! This is the durable record of *what* to download and in what order. The
//! byte-level resume state stays in the sidecar next to each partial file —
//! that has to survive a hard kill, and a per-chunk database write on every
//! network buffer would dominate throughput.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::category::{Category, CategoryKind, NewCategory};
use crate::downloader::Request;
use crate::error::{Error, Result};
use crate::model::{DownloadId, DownloadState};
use crate::settings::EngineSettings;

/// The single row `settings` ever holds.
const SETTINGS_KEY: &str = "engine";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: DownloadId,
    pub url: String,
    pub filename: String,
    pub directory: PathBuf,
    pub state: DownloadState,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub connections: Option<u8>,
    /// Forwarded by an external client (the browser extension) so the engine
    /// can reach a link that only works with the browser's session.
    pub headers: crate::headers::Headers,
    pub validator: Option<String>,
    pub priority: i64,
    pub position: i64,
    pub error: Option<String>,
    /// The category that routed this download here, kept for display even
    /// after the rule itself is edited or deleted.
    pub category_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl QueueItem {
    pub fn request(&self) -> Request {
        Request {
            url: self.url.clone(),
            directory: self.directory.clone(),
            filename: Some(self.filename.clone()),
            headers: self.headers.clone(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.filename)
    }

    fn from_row(row: &SqliteRow) -> Result<Self> {
        let parse_time = |s: String| {
            DateTime::parse_from_rfc3339(&s)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|e| Error::Corrupt(format!("bad timestamp {s:?}: {e}")))
        };

        let state: String = row.try_get("state")?;

        Ok(Self {
            id: DownloadId(
                uuid::Uuid::parse_str(row.try_get::<String, _>("id")?.as_str())
                    .map_err(|e| Error::Corrupt(format!("bad id: {e}")))?,
            ),
            url: row.try_get("url")?,
            filename: row.try_get("filename")?,
            directory: PathBuf::from(row.try_get::<String, _>("directory")?),
            state: DownloadState::from_str(&state).map_err(Error::Corrupt)?,
            total_bytes: row.try_get::<Option<i64>, _>("total_bytes")?.map(|v| v as u64),
            downloaded_bytes: row.try_get::<i64, _>("downloaded_bytes")? as u64,
            connections: row.try_get::<Option<i64>, _>("connections")?.map(|v| v as u8),
            headers: crate::headers::from_json(row.try_get("headers")?),
            validator: row.try_get("validator")?,
            priority: row.try_get("priority")?,
            position: row.try_get("position")?,
            error: row.try_get("error")?,
            category_name: row.try_get("category_name")?,
            created_at: parse_time(row.try_get("created_at")?)?,
            updated_at: parse_time(row.try_get("updated_at")?)?,
            completed_at: row
                .try_get::<Option<String>, _>("completed_at")?
                .map(parse_time)
                .transpose()?,
        })
    }
}

fn category_from_row(row: &SqliteRow) -> Result<Category> {
    let kind: String = row.try_get("kind")?;
    Ok(Category {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: kind.parse::<CategoryKind>().map_err(Error::Corrupt)?,
        pattern: row.try_get("pattern")?,
        directory: PathBuf::from(row.try_get::<String, _>("directory")?),
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
    })
}

/// A download to add to the queue.
#[derive(Debug, Clone)]
pub struct NewDownload {
    /// Insert as paused atomically, without exposing the job to the runner.
    pub paused: bool,
    pub url: String,
    pub directory: PathBuf,
    /// Resolved lazily by the engine when absent, but the queue needs *a*
    /// name up front for its uniqueness constraint.
    pub filename: String,
    pub connections: Option<u8>,
    /// Already-sanitized request headers. See [`crate::headers`].
    pub headers: crate::headers::Headers,
    pub priority: i64,
    /// Whether a matching category may override `directory`. `false` is for
    /// a caller that chose the destination on purpose — the composer's own
    /// folder picker, an API request that named an explicit `directory` — a
    /// deliberate choice should never be second-guessed by a filing rule.
    pub categorize: bool,
}

impl NewDownload {
    pub fn new(url: impl Into<String>, directory: impl Into<PathBuf>) -> Self {
        let url = url.into();
        let filename = crate::probe::filename_from_url(&url);
        Self {
            paused: false,
            url,
            directory: directory.into(),
            filename,
            connections: None,
            headers: Vec::new(),
            priority: 0,
            categorize: true,
        }
    }
}

#[derive(Clone)]
pub struct Queue {
    pool: SqlitePool,
}

impl Queue {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL lets the UI read progress while a download writes.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .foreign_keys(true)
            // Per-connection settings, so they belong on the options: run once
            // against the pool they only ever reached one of its connections.
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .pragma("temp_store", "MEMORY")
            // 8 MB page cache per connection (negative = kibibytes). The queue
            // is small; a larger cache only grows the working set.
            .pragma("cache_size", "-8000")
            // Enough to map the whole database for reads without reserving a
            // large region of address space per connection.
            .pragma("mmap_size", "33554432");

        Self::from_options(options).await
    }

    /// In-memory database, for tests.
    pub async fn open_in_memory() -> Result<Self> {
        Self::from_options(SqliteConnectOptions::from_str(":memory:").unwrap()).await
    }

    async fn from_options(options: SqliteConnectOptions) -> Result<Self> {
        // One dedicated write connection prevents write-lock contention between
        // the download workers; multiple read connections serve concurrent
        // snapshots / API calls without queuing.
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .connect_with(options)
            .await?;

        crate::migrations::run(&pool).await?;

        Ok(Self { pool })
    }

    /// The underlying connection pool, for subsystems that keep their own
    /// tables in the same database (`VideoManager`'s job history) rather than
    /// duplicating a second SQLite file next to this one.
    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub async fn enqueue(&self, mut new: NewDownload) -> Result<DownloadId> {
        let id = DownloadId::new();
        let now = Utc::now().to_rfc3339();
        let settings = self.load_settings().await?;

        let category_name = if new.categorize {
            match self.match_category(&new.url, &new.filename).await? {
                Some(cat) => {
                    new.directory = cat.directory;
                    Some(cat.name)
                }
                None => {
                    // No rule claimed it, so the global default (if the user
                    // set one) is a better fallback than whatever directory
                    // the caller resolved on its own before this destination
                    // was ever configured.
                    if let Some(dir) = &settings.default_directory {
                        new.directory = dir.clone();
                    }
                    None
                }
            }
        } else {
            None
        };

        if new.connections.is_none() {
            new.connections = settings.default_connections;
        }

        // Prevent a second, *different* download from silently overwriting an
        // existing file — or another queued/finished download — that already
        // claims this exact name in this directory. Number it instead, the
        // same way Windows/most browsers do, rather than the two clobbering
        // each other on disk when they run.
        //
        // Skipped when this exact (url, directory, filename) triple was
        // already enqueued before: that is a genuine re-add (a double-click,
        // most often) and must still hit the unique-index conflict below
        // rather than quietly spawning a numbered duplicate.
        let dir_str = new.directory.to_string_lossy().into_owned();
        let exact_match_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM downloads WHERE url = ? AND directory = ? AND filename = ?",
        )
        .bind(&new.url)
        .bind(&dir_str)
        .bind(&new.filename)
        .fetch_one(&self.pool)
        .await?;

        if exact_match_exists == 0 {
            let existing_names: std::collections::HashSet<String> =
                sqlx::query_scalar("SELECT filename FROM downloads WHERE directory = ?")
                    .bind(&dir_str)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .collect();
            let target_dir = new.directory.clone();
            new.filename = crate::probe::dedupe_filename(&new.filename, |candidate| {
                existing_names.contains(candidate) || target_dir.join(candidate).exists()
            });
        }

        // Append to the end of the queue.
        let next_position: i64 = sqlx::query("SELECT COALESCE(MAX(position), -1) + 1 FROM downloads")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;

        sqlx::query(
            "INSERT INTO downloads
                (id, url, filename, directory, state, connections, headers,
                 priority, position, created_at, updated_at, category_name)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&new.url)
        .bind(&new.filename)
        .bind(new.directory.to_string_lossy().as_ref())
        .bind(if new.paused { DownloadState::Paused } else { DownloadState::Waiting }.as_str())
        .bind(new.connections.map(i64::from))
        .bind(crate::headers::to_json(&new.headers))
        .bind(new.priority)
        .bind(next_position)
        .bind(&now)
        .bind(&now)
        .bind(&category_name)
        .execute(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(ref db) if db.is_unique_violation() => Error::Duplicate {
                url: new.url.clone(),
            },
            other => Error::Db(other),
        })?;

        Ok(id)
    }

    /// The highest-priority enabled category that claims `url`/`filename`,
    /// if any. Ties break in insertion order, same as the download queue.
    async fn match_category(&self, url: &str, filename: &str) -> Result<Option<Category>> {
        let rows = sqlx::query(
            "SELECT * FROM categories WHERE enabled = 1 ORDER BY priority DESC, position ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        for row in &rows {
            let category = category_from_row(row)?;
            if category.matches(url, filename) {
                return Ok(Some(category));
            }
        }
        Ok(None)
    }

    /// Add a filing rule. The pattern is validated before it reaches storage
    /// so a bad regex fails loudly instead of silently never matching.
    pub async fn add_category(&self, new: NewCategory) -> Result<i64> {
        crate::category::validate(new.kind, &new.pattern)?;
        let now = Utc::now().to_rfc3339();

        let next_position: i64 = sqlx::query("SELECT COALESCE(MAX(position), -1) + 1 FROM categories")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;

        let row = sqlx::query(
            "INSERT INTO categories (name, kind, pattern, directory, priority, position, enabled, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 1, ?)
             RETURNING id",
        )
        .bind(&new.name)
        .bind(new.kind.to_string())
        .bind(&new.pattern)
        .bind(new.directory.to_string_lossy().as_ref())
        .bind(new.priority)
        .bind(next_position)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get(0)?)
    }

    pub async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = sqlx::query("SELECT * FROM categories ORDER BY priority DESC, position ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(category_from_row).collect()
    }

    pub async fn set_category_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let result = sqlx::query("UPDATE categories SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidInput(format!("no category with id {id}")));
        }
        Ok(())
    }

    /// Current settings, or the defaults when nothing has been saved yet.
    ///
    /// Corrupt stored JSON degrades to defaults rather than failing every
    /// caller of `enqueue` — a bad settings row should not stop downloads
    /// from working.
    pub async fn load_settings(&self) -> Result<EngineSettings> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
            .bind(SETTINGS_KEY)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = row else {
            return Ok(EngineSettings::default());
        };
        let raw: String = row.try_get(0)?;
        match serde_json::from_str::<EngineSettings>(&raw) {
            Ok(mut settings) => {
                // Stored settings can predate the current defaults; fields an
                // older version wrote verbatim are reconciled on the way in
                // rather than at every use site.
                settings.discord.migrate();
                Ok(settings)
            }
            Err(e) => {
                tracing::warn!(error = %e, "ignoring unparseable settings; using defaults");
                Ok(EngineSettings::default())
            }
        }
    }

    pub async fn save_settings(&self, settings: &EngineSettings) -> Result<()> {
        let json = serde_json::to_string(settings)
            .map_err(|e| Error::InvalidInput(format!("could not serialize settings: {e}")))?;

        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(SETTINGS_KEY)
        .bind(json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn remove_category(&self, id: i64) -> Result<()> {
        let result = sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidInput(format!("no category with id {id}")));
        }
        Ok(())
    }

    pub async fn get(&self, id: DownloadId) -> Result<QueueItem> {
        sqlx::query("SELECT * FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound(id))
            .and_then(|row| QueueItem::from_row(&row))
    }

    pub async fn list(&self) -> Result<Vec<QueueItem>> {
        let rows = sqlx::query(
            "SELECT * FROM downloads ORDER BY priority DESC, position ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(QueueItem::from_row).collect()
    }

    pub async fn list_by_state(&self, state: DownloadState) -> Result<Vec<QueueItem>> {
        let rows = sqlx::query(
            "SELECT * FROM downloads WHERE state = ? ORDER BY priority DESC, position ASC",
        )
        .bind(state.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(QueueItem::from_row).collect()
    }

    /// Claim the highest-priority runnable item, marking it `downloading` in
    /// the same statement.
    ///
    /// The conditional UPDATE is what makes this safe to call from several
    /// workers at once: only one can move a given row out of `waiting`.
    pub async fn claim_next(&self) -> Result<Option<QueueItem>> {
        let now = Utc::now().to_rfc3339();

        let row = sqlx::query(
            "UPDATE downloads
                SET state = ?, updated_at = ?
              WHERE id = (
                    SELECT id FROM downloads
                     WHERE state = ?
                     ORDER BY priority DESC, position ASC
                     LIMIT 1
              )
             RETURNING *",
        )
        .bind(DownloadState::Downloading.as_str())
        .bind(&now)
        .bind(DownloadState::Waiting.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.as_ref().map(QueueItem::from_row).transpose()
    }

    pub async fn set_state(&self, id: DownloadId, state: DownloadState) -> Result<()> {
        let now = Utc::now();
        let completed = state.is_terminal().then(|| now.to_rfc3339());

        sqlx::query(
            "UPDATE downloads
                SET state = ?, updated_at = ?,
                    completed_at = COALESCE(?, completed_at)
              WHERE id = ?",
        )
        .bind(state.as_str())
        .bind(now.to_rfc3339())
        .bind(completed)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn fail(&self, id: DownloadId, message: &str) -> Result<()> {
        sqlx::query(
            "UPDATE downloads SET state = ?, error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(DownloadState::Failed.as_str())
        .bind(message)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn update_progress(
        &self,
        id: DownloadId,
        downloaded: u64,
        total: Option<u64>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE downloads
                SET downloaded_bytes = ?,
                    total_bytes = COALESCE(?, total_bytes),
                    updated_at = ?
              WHERE id = ?",
        )
        .bind(downloaded as i64)
        .bind(total.map(|v| v as i64))
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record the filename the engine actually resolved, which may differ from
    /// the guess made at enqueue time (`Content-Disposition` wins).
    pub async fn set_filename(&self, id: DownloadId, filename: &str) -> Result<()> {
        sqlx::query("UPDATE downloads SET filename = ?, updated_at = ? WHERE id = ?")
            .bind(filename)
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn locate_file(&self, id: DownloadId, path: &Path) -> Result<()> {
        crate::file_status::require_file(path).await?;
        let filename = path.file_name().ok_or_else(|| Error::InvalidInput("Choose a file.".into()))?;
        let result = sqlx::query("UPDATE downloads SET directory = ?, filename = ?, updated_at = ? WHERE id = ? AND state = 'finished'")
            .bind(path.parent().unwrap_or(Path::new("")).to_string_lossy().as_ref())
            .bind(filename.to_string_lossy().as_ref()).bind(Utc::now().to_rfc3339()).bind(id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() != 1 { return Err(Error::InvalidInput("Only completed entries can be located.".into())); }
        Ok(())
    }

    pub async fn restart_missing(&self, id: DownloadId) -> Result<()> {
        let item = self.get(id).await?;
        if item.state != DownloadState::Finished { return Err(Error::InvalidInput("Only completed entries can be downloaded again.".into())); }
        crate::file_status::require_missing(&item.path()).await?;
        let result = sqlx::query("UPDATE downloads SET state = 'waiting', downloaded_bytes = 0, total_bytes = NULL, validator = NULL, error = NULL, completed_at = NULL WHERE id = ? AND state = 'finished'")
            .bind(id.to_string()).execute(&self.pool).await?;
        if result.rows_affected() != 1 { return Err(Error::InvalidInput("The entry changed; check it again before restarting.".into())); }
        Ok(())
    }

    pub async fn remove(&self, id: DownloadId) -> Result<()> {
        sqlx::query("DELETE FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn set_priority(&self, id: DownloadId, priority: i64) -> Result<()> {
        sqlx::query("UPDATE downloads SET priority = ?, updated_at = ? WHERE id = ?")
            .bind(priority)
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Reset rows left mid-flight by a crash.
    ///
    /// A `downloading` row at startup means the process died holding it: no
    /// worker exists any more, so without this the item would sit in the queue
    /// forever, claimed by nobody. Returns how many were recovered.
    pub async fn recover_interrupted(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE downloads SET state = ?, updated_at = ? WHERE state = ?",
        )
        .bind(DownloadState::Waiting.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(DownloadState::Downloading.as_str())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn queue() -> Queue {
        Queue::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn enqueue_then_read_back() {
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        let item = q.get(id).await.unwrap();
        assert_eq!(item.url, "https://host.test/a.zip");
        assert_eq!(item.filename, "a.zip");
        assert_eq!(item.state, DownloadState::Waiting);
        assert_eq!(item.downloaded_bytes, 0);
        assert_eq!(item.total_bytes, None);
    }

    #[tokio::test]
    async fn duplicate_target_is_rejected() {
        let q = queue().await;
        let new = NewDownload::new("https://host.test/a.zip", "/tmp");
        q.enqueue(new.clone()).await.unwrap();

        let err = q.enqueue(new).await.unwrap_err();
        assert!(
            matches!(err, Error::Duplicate { .. }),
            "expected a duplicate error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_different_url_with_the_same_name_is_numbered_not_overwritten() {
        let q = queue().await;
        q.enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        // A different URL that happens to resolve to the same filename in the
        // same directory must not silently target the first download's file.
        let mut second = NewDownload::new("https://other.test/a.zip", "/tmp");
        second.filename = "a.zip".into();
        let id = q.enqueue(second).await.unwrap();

        assert_eq!(q.get(id).await.unwrap().filename, "a (1).zip");
    }

    #[tokio::test]
    async fn re_adding_the_exact_same_target_still_conflicts_rather_than_numbering() {
        let q = queue().await;
        let new = NewDownload::new("https://host.test/a.zip", "/tmp");
        q.enqueue(new.clone()).await.unwrap();

        // Must still hit the duplicate-URL guard, not quietly enqueue "a (1).zip".
        let err = q.enqueue(new).await.unwrap_err();
        assert!(matches!(err, Error::Duplicate { .. }));
    }

    #[tokio::test]
    async fn same_url_to_a_different_directory_is_allowed() {
        let q = queue().await;
        q.enqueue(NewDownload::new("https://host.test/a.zip", "/tmp/one"))
            .await
            .unwrap();
        q.enqueue(NewDownload::new("https://host.test/a.zip", "/tmp/two"))
            .await
            .expect("a different destination is a distinct download");
    }

    #[tokio::test]
    async fn claim_respects_priority_then_insertion_order() {
        let q = queue().await;
        for name in ["first", "second"] {
            q.enqueue(NewDownload::new(
                format!("https://host.test/{name}.zip"),
                "/tmp",
            ))
            .await
            .unwrap();
        }
        let urgent = q
            .enqueue(NewDownload {
                priority: 10,
                ..NewDownload::new("https://host.test/urgent.zip", "/tmp")
            })
            .await
            .unwrap();

        assert_eq!(q.claim_next().await.unwrap().unwrap().id, urgent);
        assert_eq!(
            q.claim_next().await.unwrap().unwrap().filename,
            "first.zip",
            "equal priority should run in insertion order"
        );
        assert_eq!(q.claim_next().await.unwrap().unwrap().filename, "second.zip");
        assert!(q.claim_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn claiming_marks_the_item_downloading() {
        let q = queue().await;
        q.enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        let claimed = q.claim_next().await.unwrap().unwrap();
        assert_eq!(claimed.state, DownloadState::Downloading);
        // And it must not be handed out twice.
        assert!(q.claim_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn paused_items_are_not_claimed() {
        // A user who paused something does not expect it to restart on its own.
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        q.set_state(id, DownloadState::Paused).await.unwrap();

        assert!(q.claim_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn recover_interrupted_requeues_orphaned_rows() {
        let q = queue().await;
        q.enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        let claimed = q.claim_next().await.unwrap().unwrap();

        // Simulate a crash: the row is `downloading` with no worker alive.
        let recovered = q.recover_interrupted().await.unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(
            q.get(claimed.id).await.unwrap().state,
            DownloadState::Waiting
        );
        assert!(q.claim_next().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn recover_leaves_paused_and_finished_alone() {
        let q = queue().await;
        let paused = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        let done = q
            .enqueue(NewDownload::new("https://host.test/b.zip", "/tmp"))
            .await
            .unwrap();
        q.set_state(paused, DownloadState::Paused).await.unwrap();
        q.set_state(done, DownloadState::Finished).await.unwrap();

        assert_eq!(q.recover_interrupted().await.unwrap(), 0);
        assert_eq!(q.get(paused).await.unwrap().state, DownloadState::Paused);
        assert_eq!(q.get(done).await.unwrap().state, DownloadState::Finished);
    }

    #[tokio::test]
    async fn finishing_stamps_completed_at() {
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        assert!(q.get(id).await.unwrap().completed_at.is_none());

        q.set_state(id, DownloadState::Finished).await.unwrap();
        assert!(q.get(id).await.unwrap().completed_at.is_some());
    }

    #[tokio::test]
    async fn progress_updates_persist() {
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        q.update_progress(id, 512, Some(2048)).await.unwrap();
        let item = q.get(id).await.unwrap();
        assert_eq!(item.downloaded_bytes, 512);
        assert_eq!(item.total_bytes, Some(2048));

        // A later update without a known total must not erase the known one.
        q.update_progress(id, 1024, None).await.unwrap();
        let item = q.get(id).await.unwrap();
        assert_eq!(item.downloaded_bytes, 1024);
        assert_eq!(item.total_bytes, Some(2048));
    }

    #[tokio::test]
    async fn failure_records_the_message() {
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        q.fail(id, "connection reset").await.unwrap();
        let item = q.get(id).await.unwrap();
        assert_eq!(item.state, DownloadState::Failed);
        assert_eq!(item.error.as_deref(), Some("connection reset"));
    }

    #[tokio::test]
    async fn missing_id_is_not_found() {
        let q = queue().await;
        let err = q.get(DownloadId::new()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn remove_deletes_the_row() {
        let q = queue().await;
        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        q.remove(id).await.unwrap();
        assert!(q.list().await.unwrap().is_empty());
    }

    /// The M1 exit criterion.
    #[tokio::test]
    async fn queue_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("zoltraak.db");

        let ids = {
            let q = Queue::open(&db).await.unwrap();
            let a = q
                .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
                .await
                .unwrap();
            let b = q
                .enqueue(NewDownload::new("https://host.test/b.zip", "/tmp"))
                .await
                .unwrap();

            q.update_progress(a, 4096, Some(8192)).await.unwrap();
            q.set_state(b, DownloadState::Paused).await.unwrap();
            // Leave `a` mid-flight, as a crash would.
            q.set_state(a, DownloadState::Downloading).await.unwrap();
            q.close().await;
            (a, b)
        };

        // Reopen as a fresh process would.
        let q = Queue::open(&db).await.unwrap();
        assert_eq!(q.list().await.unwrap().len(), 2);

        let a = q.get(ids.0).await.unwrap();
        assert_eq!(a.downloaded_bytes, 4096, "progress must survive restart");
        assert_eq!(a.total_bytes, Some(8192));
        assert_eq!(
            a.state,
            DownloadState::Downloading,
            "state is untouched until recovery runs"
        );

        assert_eq!(q.recover_interrupted().await.unwrap(), 1);
        assert_eq!(q.get(ids.0).await.unwrap().state, DownloadState::Waiting);
        assert_eq!(
            q.get(ids.1).await.unwrap().state,
            DownloadState::Paused,
            "a paused item must stay paused across a restart"
        );
    }

    fn category(name: &str, kind: CategoryKind, pattern: &str, directory: &str) -> NewCategory {
        NewCategory {
            name: name.into(),
            kind,
            pattern: pattern.into(),
            directory: PathBuf::from(directory),
            priority: 0,
        }
    }

    #[tokio::test]
    async fn a_matching_category_overrides_the_destination() {
        let q = queue().await;
        q.add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();

        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        let item = q.get(id).await.unwrap();
        assert_eq!(item.directory, PathBuf::from("/dest/zip"));
        assert_eq!(item.category_name.as_deref(), Some("Archives"));
    }

    #[tokio::test]
    async fn a_non_matching_category_leaves_the_destination_alone() {
        let q = queue().await;
        q.add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();

        let id = q
            .enqueue(NewDownload::new("https://host.test/a.iso", "/tmp"))
            .await
            .unwrap();

        let item = q.get(id).await.unwrap();
        assert_eq!(item.directory, PathBuf::from("/tmp"));
        assert_eq!(item.category_name, None);
    }

    #[tokio::test]
    async fn categorize_false_bypasses_matching_even_with_a_match_available() {
        let q = queue().await;
        q.add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();

        let id = q
            .enqueue(NewDownload {
                categorize: false,
                ..NewDownload::new("https://host.test/a.zip", "/tmp/chosen")
            })
            .await
            .unwrap();

        let item = q.get(id).await.unwrap();
        assert_eq!(
            item.directory,
            PathBuf::from("/tmp/chosen"),
            "an explicit choice must not be overridden by a filing rule"
        );
    }

    #[tokio::test]
    async fn a_disabled_category_is_skipped() {
        let q = queue().await;
        let id = q
            .add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();
        q.set_category_enabled(id, false).await.unwrap();

        let download = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        assert_eq!(q.get(download).await.unwrap().directory, PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn higher_priority_category_wins_when_both_match() {
        let q = queue().await;
        q.add_category(category("Generic", CategoryKind::Extension, "zip", "/dest/generic"))
            .await
            .unwrap();
        q.add_category(NewCategory {
            priority: 10,
            ..category("Specific", CategoryKind::Domain, "host.test", "/dest/specific")
        })
        .await
        .unwrap();

        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        assert_eq!(q.get(id).await.unwrap().category_name.as_deref(), Some("Specific"));
    }

    #[tokio::test]
    async fn removing_a_category_does_not_touch_downloads_it_already_routed() {
        let q = queue().await;
        let cat_id = q
            .add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();

        let id = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();

        q.remove_category(cat_id).await.unwrap();

        let item = q.get(id).await.unwrap();
        assert_eq!(item.directory, PathBuf::from("/dest/zip"));
        assert_eq!(
            item.category_name.as_deref(),
            Some("Archives"),
            "the label should survive even though the rule is gone"
        );
        assert!(q.list_categories().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn adding_a_category_with_an_invalid_regex_is_rejected() {
        let q = queue().await;
        let err = q
            .add_category(category("Bad", CategoryKind::Regex, "(unclosed", "/dest"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)), "got {err:?}");
        assert!(q.list_categories().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn removing_or_toggling_an_unknown_category_is_reported() {
        let q = queue().await;
        assert!(q.remove_category(9999).await.is_err());
        assert!(q.set_category_enabled(9999, false).await.is_err());
    }

    #[tokio::test]
    async fn settings_default_when_nothing_has_been_saved() {
        let q = queue().await;
        assert_eq!(q.load_settings().await.unwrap(), EngineSettings::default());
    }

    #[tokio::test]
    async fn settings_round_trip_and_can_be_overwritten() {
        let q = queue().await;
        let mut settings = EngineSettings {
            bandwidth_limit_bytes_per_sec: Some(1_000_000),
            default_connections: Some(4),
            ..EngineSettings::default()
        };
        q.save_settings(&settings).await.unwrap();
        assert_eq!(q.load_settings().await.unwrap(), settings);

        settings.bandwidth_limit_bytes_per_sec = None;
        q.save_settings(&settings).await.unwrap();
        assert_eq!(q.load_settings().await.unwrap(), settings);
    }

    #[tokio::test]
    async fn a_default_directory_setting_only_applies_when_no_category_matches() {
        let q = queue().await;
        q.save_settings(&EngineSettings {
            default_directory: Some(PathBuf::from("/settings-default")),
            ..EngineSettings::default()
        })
        .await
        .unwrap();
        q.add_category(category("Archives", CategoryKind::Extension, "zip", "/dest/zip"))
            .await
            .unwrap();

        let zip = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        assert_eq!(
            q.get(zip).await.unwrap().directory,
            PathBuf::from("/dest/zip"),
            "a category match must still win over the settings default"
        );

        let iso = q
            .enqueue(NewDownload::new("https://host.test/a.iso", "/tmp"))
            .await
            .unwrap();
        assert_eq!(
            q.get(iso).await.unwrap().directory,
            PathBuf::from("/settings-default"),
            "no category matched, so the settings default should have applied"
        );
    }

    #[tokio::test]
    async fn a_default_directory_setting_is_skipped_when_categorize_is_false() {
        let q = queue().await;
        q.save_settings(&EngineSettings {
            default_directory: Some(PathBuf::from("/settings-default")),
            ..EngineSettings::default()
        })
        .await
        .unwrap();

        let id = q
            .enqueue(NewDownload {
                categorize: false,
                ..NewDownload::new("https://host.test/a.zip", "/tmp/chosen")
            })
            .await
            .unwrap();
        assert_eq!(q.get(id).await.unwrap().directory, PathBuf::from("/tmp/chosen"));
    }

    #[tokio::test]
    async fn a_default_connections_setting_applies_only_when_unspecified() {
        let q = queue().await;
        q.save_settings(&EngineSettings {
            default_connections: Some(6),
            ..EngineSettings::default()
        })
        .await
        .unwrap();

        let unset = q
            .enqueue(NewDownload::new("https://host.test/a.zip", "/tmp"))
            .await
            .unwrap();
        assert_eq!(q.get(unset).await.unwrap().connections, Some(6));

        let explicit = q
            .enqueue(NewDownload {
                connections: Some(2),
                ..NewDownload::new("https://host.test/b.zip", "/tmp")
            })
            .await
            .unwrap();
        assert_eq!(
            q.get(explicit).await.unwrap().connections,
            Some(2),
            "an explicit connection count must not be overridden"
        );
    }
}
