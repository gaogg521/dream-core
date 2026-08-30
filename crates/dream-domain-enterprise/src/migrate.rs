//! Self-managed migration runner for one-enterprise tables.
//!
//! Shares the `_one_migrations` ledger with one-org / one-sso / one-employee
//! but keys its entries under the `enterprise_` prefix so names never collide
//! with another crate's `001_init`. Append-only: never edit or reorder shipped
//! entries — add a new file instead. Since P3-3 the runner logic lives in
//! `dream_core_db::run_ledgered_migrations`; this file only carries the two
//! migration trees (SQLite `migrations/`, MySQL `migrations_mysql/`) and hands
//! them to the shared runner keyed by the pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::EnterpriseError;

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "enterprise_001_init",
        include_str!("../migrations/enterprise_001_init.sql"),
    ),
    (
        "enterprise_002_company_origin",
        include_str!("../migrations/enterprise_002_company_origin.sql"),
    ),
    (
        "enterprise_003_directory",
        include_str!("../migrations/enterprise_003_directory.sql"),
    ),
    (
        "enterprise_004_seat_status",
        include_str!("../migrations/enterprise_004_seat_status.sql"),
    ),
    (
        "enterprise_005_invites",
        include_str!("../migrations/enterprise_005_invites.sql"),
    ),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "enterprise_001_init",
        include_str!("../migrations_mysql/enterprise_001_init.sql"),
    ),
    (
        "enterprise_002_company_origin",
        include_str!("../migrations_mysql/enterprise_002_company_origin.sql"),
    ),
    (
        "enterprise_003_directory",
        include_str!("../migrations_mysql/enterprise_003_directory.sql"),
    ),
    (
        "enterprise_004_seat_status",
        include_str!("../migrations_mysql/enterprise_004_seat_status.sql"),
    ),
    (
        "enterprise_005_invites",
        include_str!("../migrations_mysql/enterprise_005_invites.sql"),
    ),
];

/// Run all pending one-enterprise migrations on the pool's backend. Idempotent;
/// call once at startup after the upstream database has been initialized.
pub async fn run_one_enterprise_migrations(pool: &DbPool) -> Result<(), EnterpriseError> {
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
        run_one_enterprise_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_enterprise_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        for table in ["one_enterprises", "one_enterprise_members", "one_enterprise_invites"] {
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

        run_one_enterprise_migrations(&db.pool).await.unwrap();
        run_one_enterprise_migrations(&db.pool).await.unwrap();

        for table in ["one_enterprises", "one_enterprise_members", "one_enterprise_invites"] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            )
            .bind(table)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "table {table} should exist");
        }

        db.cleanup().await.unwrap();
    }
}
