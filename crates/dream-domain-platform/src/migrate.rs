//! Self-managed migration runner for one-platform tables. Independent ledger
//! (`_one_platform_migrations`), name-keyed and append-only, mirroring the
//! other `one-*` crates. Since P3-3 the runner logic lives in
//! `dream_core_db::run_ledgered_migrations`; this file only carries the two
//! migration trees (SQLite `migrations/`, MySQL `migrations_mysql/`) and hands
//! them to the shared runner keyed by the pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::PlatformError;

/// Embedded migrations, applied in array order. Append-only: never edit or
/// reorder shipped entries — add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    ("002_security", include_str!("../migrations/002_security.sql")),
    (
        "003_resource_grants",
        include_str!("../migrations/003_resource_grants.sql"),
    ),
    ("004_scenes", include_str!("../migrations/004_scenes.sql")),
    (
        "005_security_policy",
        include_str!("../migrations/005_security_policy.sql"),
    ),
    ("006_api_keys", include_str!("../migrations/006_api_keys.sql")),
    ("007_notifications", include_str!("../migrations/007_notifications.sql")),
    ("008_file_vault", include_str!("../migrations/008_file_vault.sql")),
    (
        "009_security_policy_templates",
        include_str!("../migrations/009_security_policy_templates.sql"),
    ),
    ("010_config_vault", include_str!("../migrations/010_config_vault.sql")),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "001_init",
        include_str!("../migrations_mysql/001_init.sql"),
    ),
    (
        "002_security",
        include_str!("../migrations_mysql/002_security.sql"),
    ),
    (
        "003_resource_grants",
        include_str!("../migrations_mysql/003_resource_grants.sql"),
    ),
    (
        "004_scenes",
        include_str!("../migrations_mysql/004_scenes.sql"),
    ),
    (
        "005_security_policy",
        include_str!("../migrations_mysql/005_security_policy.sql"),
    ),
    (
        "006_api_keys",
        include_str!("../migrations_mysql/006_api_keys.sql"),
    ),
    (
        "007_notifications",
        include_str!("../migrations_mysql/007_notifications.sql"),
    ),
    (
        "008_file_vault",
        include_str!("../migrations_mysql/008_file_vault.sql"),
    ),
    (
        "009_security_policy_templates",
        include_str!("../migrations_mysql/009_security_policy_templates.sql"),
    ),
    (
        "010_config_vault",
        include_str!("../migrations_mysql/010_config_vault.sql"),
    ),
];

/// Run all pending one-platform migrations on the pool's backend. Idempotent;
/// call once at startup after the upstream database has been initialized.
pub async fn run_one_platform_migrations(pool: &DbPool) -> Result<(), PlatformError> {
    run_ledgered_migrations(
        pool,
        "_one_platform_migrations",
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
        run_one_platform_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_platform_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_platform_migrations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset. Exercises the config-vault reserved-word handling
    /// (`key`) and the 5-column unique key width (`one_resource_grants`).
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_platform_migrations(&db.pool).await.unwrap();
        run_one_platform_migrations(&db.pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_platform_migrations")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS_MYSQL.len() as i64);

        db.cleanup().await.unwrap();
    }
}
