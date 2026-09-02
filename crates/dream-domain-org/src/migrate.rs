//! Self-managed migration runner for one-org tables.
//!
//! Deliberately independent from the upstream `sqlx::migrate!()` pipeline:
//! upstream tracks its files in `_sqlx_migrations` with sequential version
//! numbers, and mixing our files into that directory would make every
//! upstream rebase a potential out-of-order failure. We keep our own
//! `_one_migrations` ledger instead (name-keyed, append-only).
//!
//! Since P3-3 the runner logic lives in `dream_core_db::run_ledgered_migrations`:
//! this file only carries the two migration trees (SQLite `migrations/` for
//! personal + enterprise-SQLite deployments, `migrations_mysql/` for MySQL
//! deployments) and hands them to the shared runner keyed by the pool's
//! backend. Ledger keys line up between the trees file-for-file.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::OrgError;

/// Embedded SQLite migrations, applied in array order. Append-only: never
/// edit or reorder shipped entries — add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    (
        "002_membership_display",
        include_str!("../migrations/002_membership_display.sql"),
    ),
    (
        "003_membership_job_title",
        include_str!("../migrations/003_membership_job_title.sql"),
    ),
    (
        "004_tenant_sso_binding",
        include_str!("../migrations/004_tenant_sso_binding.sql"),
    ),
    (
        "005_drop_tenant_sso_binding",
        include_str!("../migrations/005_drop_tenant_sso_binding.sql"),
    ),
    (
        "006_tenant_enterprise_link",
        include_str!("../migrations/006_tenant_enterprise_link.sql"),
    ),
    (
        "007_multi_membership",
        include_str!("../migrations/007_multi_membership.sql"),
    ),
    ("008_onboarding", include_str!("../migrations/008_onboarding.sql")),
    ("009_departments", include_str!("../migrations/009_departments.sql")),
    ("010_integrations", include_str!("../migrations/010_integrations.sql")),
    (
        "011_department_directory_source",
        include_str!("../migrations/011_department_directory_source.sql"),
    ),
    (
        "012_department_directory_map_root",
        include_str!("../migrations/012_department_directory_map_root.sql"),
    ),
    (
        "013_runtime_control",
        include_str!("../migrations/013_runtime_control.sql"),
    ),
    (
        "014_audit_outcome",
        include_str!("../migrations/014_audit_outcome.sql"),
    ),
];

/// Embedded MySQL migrations (final-state ports, not a replay of the SQLite
/// history — see the P3-3 implementation plan §1). Append-only like the
/// SQLite tree; new schema changes land in both trees with the same ledger key.
const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "001_init",
        include_str!("../migrations_mysql/001_init.sql"),
    ),
    (
        "002_membership_display",
        include_str!("../migrations_mysql/002_membership_display.sql"),
    ),
    (
        "003_membership_job_title",
        include_str!("../migrations_mysql/003_membership_job_title.sql"),
    ),
    (
        "004_tenant_sso_binding",
        include_str!("../migrations_mysql/004_tenant_sso_binding.sql"),
    ),
    (
        "005_drop_tenant_sso_binding",
        include_str!("../migrations_mysql/005_drop_tenant_sso_binding.sql"),
    ),
    (
        "006_tenant_enterprise_link",
        include_str!("../migrations_mysql/006_tenant_enterprise_link.sql"),
    ),
    (
        "007_multi_membership",
        include_str!("../migrations_mysql/007_multi_membership.sql"),
    ),
    (
        "008_onboarding",
        include_str!("../migrations_mysql/008_onboarding.sql"),
    ),
    (
        "009_departments",
        include_str!("../migrations_mysql/009_departments.sql"),
    ),
    (
        "010_integrations",
        include_str!("../migrations_mysql/010_integrations.sql"),
    ),
    (
        "011_department_directory_source",
        include_str!("../migrations_mysql/011_department_directory_source.sql"),
    ),
    (
        "012_department_directory_map_root",
        include_str!("../migrations_mysql/012_department_directory_map_root.sql"),
    ),
    (
        "013_runtime_control",
        include_str!("../migrations_mysql/013_runtime_control.sql"),
    ),
    (
        "014_audit_outcome",
        include_str!("../migrations_mysql/014_audit_outcome.sql"),
    ),
];

/// Run all pending one-org migrations on the pool's backend. Idempotent; call
/// once at startup after the upstream database has been initialized.
pub async fn run_one_migrations(pool: &DbPool) -> Result<(), OrgError> {
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
        run_one_migrations(&DbPool::Sqlite(db.pool().clone())).await.unwrap();
        run_one_migrations(&DbPool::Sqlite(db.pool().clone())).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_migrations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);

        for table in [
            "one_tenants",
            "one_tenant_invites",
            "one_user_org",
            "one_active_tenant",
            "one_runtime_nodes",
            "one_audit_logs",
        ] {
            let exists: bool =
                sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(table)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert!(exists, "table {table} should exist");
        }
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`
    /// (e.g. a throwaway `mysql:8` container); skipped when unset.
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_migrations(&db.pool).await.unwrap();
        run_one_migrations(&db.pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_migrations")
            .fetch_one(db.pool.mysql())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS_MYSQL.len() as i64);

        for table in [
            "one_tenants",
            "one_tenant_invites",
            "one_user_org",
            "one_active_tenant",
            "one_runtime_nodes",
            "one_audit_logs",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            )
            .bind(table)
            .fetch_one(db.pool.mysql())
            .await
            .unwrap();
            assert_eq!(exists, 1, "table {table} should exist");
        }

        // The collation red line: case-sensitive lookups.
        let tenant_id = "collation-check-tenant";
        sqlx::query("INSERT INTO one_tenants (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(tenant_id)
            .bind("API")
            .bind(1_756_000_000_000_i64)
            .bind(1_756_000_000_000_i64)
            .execute(db.pool.mysql())
            .await
            .unwrap();
        let miss: Option<String> =
            sqlx::query_scalar("SELECT name FROM one_tenants WHERE name = ?")
                .bind("api")
                .fetch_optional(db.pool.mysql())
                .await
                .unwrap();
        assert_eq!(miss, None, "one_* tables must be case-sensitive");

        db.cleanup().await.unwrap();
    }
}
