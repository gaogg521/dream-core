//! Migration runner for one-sso tables.
//!
//! Shares the `_one_migrations` ledger with one-org/one-employee; entry
//! names carry the `sso_` prefix so the key spaces stay disjoint. Since P3-3
//! the runner logic lives in `dream_core_db::run_ledgered_migrations`; this
//! file only carries the two migration trees (SQLite `migrations/`, MySQL
//! `migrations_mysql/`) and hands them to the shared runner keyed by the
//! pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::SsoError;

const MIGRATIONS: &[(&str, &str)] = &[
    ("sso_001_init", include_str!("../migrations/001_init.sql")),
    (
        "sso_002_identity_display",
        include_str!("../migrations/002_identity_display.sql"),
    ),
    (
        "sso_003_identity_job_title",
        include_str!("../migrations/003_identity_job_title.sql"),
    ),
    (
        "sso_004_identity_org_external_id",
        include_str!("../migrations/004_identity_org_external_id.sql"),
    ),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "sso_001_init",
        include_str!("../migrations_mysql/001_init.sql"),
    ),
    (
        "sso_002_identity_display",
        include_str!("../migrations_mysql/002_identity_display.sql"),
    ),
    (
        "sso_003_identity_job_title",
        include_str!("../migrations_mysql/003_identity_job_title.sql"),
    ),
    (
        "sso_004_identity_org_external_id",
        include_str!("../migrations_mysql/004_identity_org_external_id.sql"),
    ),
];

/// Run all pending one-sso migrations on the pool's backend. Idempotent.
pub async fn run_one_sso_migrations(pool: &DbPool) -> Result<(), SsoError> {
    run_ledgered_migrations(
        pool,
        "_one_migrations",
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
        run_one_sso_migrations(&DbPool::Sqlite(db.pool().clone())).await.unwrap();
        run_one_sso_migrations(&DbPool::Sqlite(db.pool().clone())).await.unwrap();

        for table in ["one_sso_providers", "one_sso_identities"] {
            let exists: bool =
                sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(table)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert!(exists, "table {table} should exist");
        }
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset.
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_sso_migrations(&db.pool).await.unwrap();
        run_one_sso_migrations(&db.pool).await.unwrap();

        for table in ["one_sso_providers", "one_sso_identities"] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            )
            .bind(table)
            .fetch_one(db.pool.mysql())
            .await
            .unwrap();
            assert_eq!(exists, 1, "table {table} should exist");
        }

        db.cleanup().await.unwrap();
    }
}
