//! Self-managed migration runner for one-billing tables.
//!
//! Shares the `_one_migrations` ledger with one-org / one-sso / one-enterprise
//! but keys entries under the `billing_` prefix so names never collide.
//! Append-only. MUST run after one-enterprise migrations — `billing_001_init`
//! grandfathers existing `one_enterprises` rows. Since P3-3 the runner logic
//! lives in `dream_core_db::run_ledgered_migrations`; this file only carries
//! the two migration trees (SQLite `migrations/`, MySQL `migrations_mysql/`)
//! and hands them to the shared runner keyed by the pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};
use sqlx::SqlitePool;

use crate::error::BillingError;

const MIGRATIONS: &[(&str, &str)] = &[
    ("billing_001_init", include_str!("../migrations/billing_001_init.sql")),
    (
        "billing_002_model_control",
        include_str!("../migrations/billing_002_model_control.sql"),
    ),
    (
        "billing_003_license_activation",
        include_str!("../migrations/billing_003_license_activation.sql"),
    ),
    (
        "billing_004_department_budgets",
        include_str!("../migrations/billing_004_department_budgets.sql"),
    ),
    (
        "billing_005_media_ledger",
        include_str!("../migrations/billing_005_media_ledger.sql"),
    ),
    (
        "billing_006_license_quotas",
        include_str!("../migrations/billing_006_license_quotas.sql"),
    ),
    (
        "billing_007_llm_calls",
        include_str!("../migrations/billing_007_llm_calls.sql"),
    ),
    (
        "billing_008_usage_channel",
        include_str!("../migrations/billing_008_usage_channel.sql"),
    ),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "billing_001_init",
        include_str!("../migrations_mysql/billing_001_init.sql"),
    ),
    (
        "billing_002_model_control",
        include_str!("../migrations_mysql/billing_002_model_control.sql"),
    ),
    (
        "billing_003_license_activation",
        include_str!("../migrations_mysql/billing_003_license_activation.sql"),
    ),
    (
        "billing_004_department_budgets",
        include_str!("../migrations_mysql/billing_004_department_budgets.sql"),
    ),
    (
        "billing_005_media_ledger",
        include_str!("../migrations_mysql/billing_005_media_ledger.sql"),
    ),
    (
        "billing_006_license_quotas",
        include_str!("../migrations_mysql/billing_006_license_quotas.sql"),
    ),
    (
        "billing_007_llm_calls",
        include_str!("../migrations_mysql/billing_007_llm_calls.sql"),
    ),
    (
        "billing_008_usage_channel",
        include_str!("../migrations_mysql/billing_008_usage_channel.sql"),
    ),
];

/// Run all pending one-billing migrations on the pool's backend. Idempotent;
/// call once at startup after the upstream database AND one-enterprise
/// migrations have run.
pub async fn run_one_billing_migrations(pool: &DbPool) -> Result<(), BillingError> {
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
pub(crate) mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        // one-enterprise tables must exist first (grandfather SELECT).
        one_enterprise_tables(db.pool()).await;
        run_one_billing_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_billing_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        for table in ["one_enterprise_license", "one_usage_events", "one_llm_calls"] {
            let exists: bool =
                sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(table)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert!(exists, "table {table} should exist");
        }
    }

    /// Minimal `one_enterprises` shape for tests that don't pull in the
    /// one-enterprise crate. `seat_status` mirrors `enterprise_004_seat_status`
    /// (T6-4) — must stay in sync with that migration, or a test member row
    /// inserted without the column would silently read back as NULL and get
    /// mistaken for a pending (unseated) member.
    pub(crate) async fn one_enterprise_tables(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_enterprises (id TEXT PRIMARY KEY, provider TEXT, external_id TEXT, display_name TEXT, created_at INTEGER, updated_at INTEGER);
             CREATE TABLE IF NOT EXISTS one_enterprise_members (user_id TEXT PRIMARY KEY, enterprise_id TEXT NOT NULL, display_name TEXT, department TEXT, job_title TEXT, role TEXT NOT NULL DEFAULT 'member', seat_status TEXT NOT NULL DEFAULT 'active', joined_at INTEGER, updated_at INTEGER);",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset. Seeds a minimal `one_enterprises` (same idiom as
    /// `one_enterprise_tables` above) so the grandfather SELECT has a source.
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_enterprises (id VARCHAR(255) PRIMARY KEY, provider VARCHAR(64) NULL, external_id VARCHAR(255) NULL, display_name VARCHAR(255) NULL, created_at BIGINT NULL, updated_at BIGINT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;",
        )
        .execute(db.pool.mysql())
        .await
        .unwrap();
        run_one_billing_migrations(&db.pool).await.unwrap();
        run_one_billing_migrations(&db.pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_migrations WHERE name LIKE 'billing_%'")
            .fetch_one(db.pool.mysql())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS_MYSQL.len() as i64);

        db.cleanup().await.unwrap();
    }
}
