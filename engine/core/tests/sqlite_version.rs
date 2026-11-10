//! Guards the vendored SQLite (engine/vendor/libsqlite3-sys/VENDORED.md).
//!
//! The Rust-side version constant still reads 3.46.0, so only asking the
//! linked library proves the patched amalgamation is what actually ships.

#[tokio::test]
async fn linked_sqlite_includes_the_rustsec_2026_0459_fix() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&pool)
        .await
        .unwrap();

    let parts: Vec<u32> = version.split('.').map(|p| p.parse().unwrap()).collect();
    assert!(
        parts.as_slice() >= [3, 54, 0].as_slice(),
        "linked SQLite {version} predates the 3.54.0 fix; is the [patch.crates-io] entry missing?"
    );
}
