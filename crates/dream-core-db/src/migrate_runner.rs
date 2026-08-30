//! Shared self-managed migration runner for the enterprise domain crates.
//!
//! Every `dream-domain-*` crate owns its migration trees (`migrations/` for
//! SQLite, `migrations_mysql/` for MySQL) and a name-keyed ledger table
//! (`_one_migrations` etc.) — deliberately independent from the upstream
//! `sqlx::migrate!()` pipeline so upstream rebases never turn into
//! out-of-order failures. The runner logic was duplicated per crate until
//! P3-3; this module is the single implementation. The SQLite arm is the
//! logic that used to live in each crate's `migrate.rs`, verbatim.
//!
//! Shipped migration files are immutable once applied (checksum guard in
//! `scripts/migration/`); new schema changes are new files, on both trees.

use sqlx::{MySqlPool, SqlitePool};

use crate::pool::DbPool;

/// The two migration trees a crate ships. Ledger keys (`001_init`, …) line up
/// between the trees so an install that moves backends re-checks the same
/// names against the same ledger.
pub struct MigrationSet {
    /// Existing SQLite tree (`migrations/`), applied verbatim on personal and
    /// enterprise-SQLite deployments.
    pub sqlite: &'static [(&'static str, &'static str)],
    /// MySQL tree (`migrations_mysql/`), applied on enterprise-MySQL
    /// deployments. Hand-written final-state ports, not replays of the SQLite
    /// history (see the P3-3 implementation plan §1).
    pub mysql: &'static [(&'static str, &'static str)],
}

/// Runs all pending migrations from the tree matching the pool's backend.
/// Idempotent; call once at startup, serially (the routes layer runs the
/// crates in FK order, which also guarantees the first runner creates any
/// shared ledger table before the others use it).
pub async fn run_ledgered_migrations(
    pool: &DbPool,
    ledger: &str,
    set: MigrationSet,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Sqlite(pool) => run_sqlite(pool, ledger, set.sqlite).await,
        DbPool::MySql(pool) => run_mysql(pool, ledger, set.mysql).await,
    }
}

async fn run_sqlite(pool: &SqlitePool, ledger: &str, migrations: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {ledger} (\
             name TEXT PRIMARY KEY,\
             applied_at INTEGER NOT NULL\
         )"
    ))
    .execute(pool)
    .await?;

    for (name, sql) in migrations {
        let applied: bool =
            sqlx::query_scalar(&format!("SELECT COUNT(*) > 0 FROM {ledger} WHERE name = ?"))
                .bind(name)
                .fetch_one(pool)
                .await?;
        if applied {
            continue;
        }

        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query(&format!("INSERT INTO {ledger} (name, applied_at) VALUES (?, ?)"))
            .bind(name)
            .bind(dream_core_common::now_ms())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(migration = name, "migration applied (sqlite)");
    }

    Ok(())
}

async fn run_mysql(pool: &MySqlPool, ledger: &str, migrations: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {ledger} (\
             name VARCHAR(255) PRIMARY KEY,\
             applied_at BIGINT NOT NULL\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs"
    ))
    .execute(pool)
    .await?;

    for (name, sql) in migrations {
        let applied: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {ledger} WHERE name = ?"))
            .bind(name)
            .fetch_one(pool)
            .await?;
        if applied > 0 {
            continue;
        }

        let mut tx = pool.begin().await?;
        // Multi-statement bodies (the norm for our migration files) ride on
        // the MySQL text protocol's multi-statement capability — fine for
        // parameterless raw SQL, which is all migration bodies ever contain.
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query(&format!("INSERT INTO {ledger} (name, applied_at) VALUES (?, ?)"))
            .bind(name)
            .bind(dream_core_common::now_ms())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(migration = name, "migration applied (mysql)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MIGRATIONS: &[(&str, &str)] = &[
        ("001_first", "CREATE TABLE IF NOT EXISTS runner_test (id TEXT PRIMARY KEY, n INTEGER NOT NULL);"),
        ("002_second", "ALTER TABLE runner_test ADD COLUMN extra TEXT;"),
    ];

    #[tokio::test]
    async fn sqlite_ledger_is_idempotent() {
        let db = crate::init_database_memory().await.unwrap();
        let pool = DbPool::Sqlite(db.pool().clone());

        run_ledgered_migrations(&pool, "_test_runner_ledger", MigrationSet {
            sqlite: TEST_MIGRATIONS,
            mysql: &[],
        })
        .await
        .unwrap();
        // Second run is a no-op, not an error (ALTER TABLE would fail if re-applied).
        run_ledgered_migrations(&pool, "_test_runner_ledger", MigrationSet {
            sqlite: TEST_MIGRATIONS,
            mysql: &[],
        })
        .await
        .unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _test_runner_ledger")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(applied, TEST_MIGRATIONS.len() as i64);

        let has_column: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('runner_test') WHERE name = 'extra'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(has_column);
    }
}
