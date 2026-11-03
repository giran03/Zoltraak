//! Schema migrations, tolerant of line-ending churn.
//!
//! sqlx records a SHA-384 of each migration's SQL and refuses to start when
//! the embedded file no longer matches. Git normalizes these files to LF, but
//! a Windows working copy can hold them as CRLF, so builds made from different
//! checkouts embed different bytes for the *same* migration. A database
//! migrated by one such build would then refuse every other one with
//! "migration N was previously applied but has been modified" — the app dies
//! at startup with nothing the user can do about it.
//!
//! Before running, any recorded checksum that matches the embedded SQL once
//! line endings are ignored is rewritten to the embedded checksum. A real
//! content change still fails exactly as before.

use sha2::{Digest, Sha384};
use sqlx::migrate::Migrator;
use sqlx::SqlitePool;

use crate::error::{Error, Result};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub(crate) async fn run(pool: &SqlitePool) -> Result<()> {
    reconcile_line_endings(pool).await?;
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| Error::Corrupt(format!("migration failed: {e}")))
}

async fn reconcile_line_endings(pool: &SqlitePool) -> Result<()> {
    let tracked: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'")
            .fetch_optional(pool)
            .await?;
    if tracked.is_none() {
        return Ok(());
    }
    for migration in MIGRATOR.iter() {
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_optional(pool)
                .await?;
        let Some(stored) = stored else { continue };
        if stored.as_slice() == &*migration.checksum {
            continue;
        }
        if line_ending_variants(&migration.sql).iter().any(|variant| variant.as_slice() == stored) {
            tracing::info!(version = migration.version, "accepting migration that differs only in line endings");
            sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                .bind(&*migration.checksum)
                .bind(migration.version)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Checksums of `sql` with LF and with CRLF line endings.
fn line_ending_variants(sql: &str) -> [Vec<u8>; 2] {
    let lf = sql.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    [Sha384::digest(lf.as_bytes()).to_vec(), Sha384::digest(crlf.as_bytes()).to_vec()]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn file_pool(path: &std::path::Path) -> SqlitePool {
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(path).create_if_missing(true);
        SqlitePool::connect_with(options).await.unwrap()
    }

    async fn set_checksum(pool: &SqlitePool, version: i64, checksum: &[u8]) {
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
            .bind(checksum)
            .bind(version)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_database_migrated_from_a_crlf_checkout_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let pool = file_pool(&dir.path().join("q.db")).await;
        run(&pool).await.unwrap();
        for migration in MIGRATOR.iter() {
            let [_, crlf] = line_ending_variants(&migration.sql);
            set_checksum(&pool, migration.version, &crlf).await;
        }
        run(&pool).await.expect("line endings alone must not block startup");
        let latest = MIGRATOR.iter().last().unwrap();
        let stored: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
            .bind(latest.version)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored.as_slice(), &*latest.checksum, "the checksum is repaired, not just skipped");
    }

    #[tokio::test]
    async fn a_genuinely_modified_migration_is_still_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let pool = file_pool(&dir.path().join("q.db")).await;
        run(&pool).await.unwrap();
        let first = MIGRATOR.iter().next().unwrap();
        set_checksum(&pool, first.version, &Sha384::digest(b"different schema").to_vec()).await;
        let error = run(&pool).await.unwrap_err().to_string();
        assert!(error.contains("previously applied but has been modified"), "{error}");
    }
}
