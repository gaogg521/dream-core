//! Self-managed migration runner for one-workflow tables. Independent ledger
//! (`_one_workflow_migrations`), name-keyed and append-only, mirroring the
//! other `one-*` crates. Since P3-3 the runner logic lives in
//! `dream_core_db::run_ledgered_migrations`; this file only carries the two
//! migration trees (SQLite `migrations/`, MySQL `migrations_mysql/`) and hands
//! them to the shared runner keyed by the pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::WorkflowError;

/// Embedded migrations, applied in array order. Append-only: never edit or
/// reorder shipped entries — add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[("001_init", include_str!("../migrations/001_init.sql"))];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[(
    "001_init",
    include_str!("../migrations_mysql/001_init.sql"),
)];

/// Run all pending one-workflow migrations on the pool's backend. Idempotent;
/// call once at startup after the upstream database has been initialized.
pub async fn run_one_workflow_migrations(pool: &DbPool) -> Result<(), WorkflowError> {
    run_ledgered_migrations(
        pool,
        "_one_workflow_migrations",
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
        run_one_workflow_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_workflow_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        let exists: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='one_workflow_tasks'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(exists);
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset.
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_workflow_migrations(&db.pool).await.unwrap();
        run_one_workflow_migrations(&db.pool).await.unwrap();

        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'one_workflow_tasks'",
        )
        .fetch_one(db.pool.mysql())
        .await
        .unwrap();
        assert_eq!(exists, 1);

        db.cleanup().await.unwrap();
    }
}
