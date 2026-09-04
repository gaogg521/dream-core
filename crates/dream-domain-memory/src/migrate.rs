//! Self-managed migration runner for one-memory tables. Independent ledger
//! (`_one_memory_migrations`), name-keyed and append-only, mirroring the
//! other `one-*` crates. Since P3-3 the runner logic lives in
//! `dream_core_db::run_ledgered_migrations`; this file only carries the two
//! migration trees (SQLite `migrations/`, MySQL `migrations_mysql/`) and hands
//! them to the shared runner keyed by the pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::MemoryError;

/// Embedded migrations, applied in array order. Append-only: never edit or
/// reorder shipped entries — add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    (
        "002_memory_config",
        include_str!("../migrations/002_memory_config.sql"),
    ),
    (
        "003_member_memory",
        include_str!("../migrations/003_member_memory.sql"),
    ),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "001_init",
        include_str!("../migrations_mysql/001_init.sql"),
    ),
    (
        "002_memory_config",
        include_str!("../migrations_mysql/002_memory_config.sql"),
    ),
    (
        "003_member_memory",
        include_str!("../migrations_mysql/003_member_memory.sql"),
    ),
];

/// Run all pending one-memory migrations on the pool's backend. Idempotent;
/// call once at startup after the upstream database has been initialized.
pub async fn run_one_memory_migrations(pool: &DbPool) -> Result<(), MemoryError> {
    run_ledgered_migrations(
        pool,
        "_one_memory_migrations",
        MigrationSet {
            sqlite: MIGRATIONS,
            mysql: MIGRATIONS_MYSQL,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_memory_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_memory_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_memory_migrations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset.
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_memory_migrations(&db.pool).await.unwrap();
        run_one_memory_migrations(&db.pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_memory_migrations")
            .fetch_one(db.pool.mysql())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS_MYSQL.len() as i64);

        db.cleanup().await.unwrap();
    }
}
