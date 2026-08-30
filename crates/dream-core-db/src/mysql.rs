//! Additive, parallel MySQL support for the enterprise deployment.
//!
//! Structurally a twin of [`crate::postgres`] (kept dormant as the prep work
//! for a possible future PG switch): entirely independent of
//! [`crate::database::Database`] and its [`sqlx::SqlitePool`] — no shared
//! code, no shared migrations. `database.rs` exists to repair and evolve
//! years of historical SQLite installs; none of that applies to a fresh
//! MySQL deployment, which starts from a clean slate.
//!
//! Scope as of 2026-08-31 (P3-3): only the `users` table is ported (see
//! `migrations_mysql/001_users.sql`), because the enterprise domain crates
//! JOIN against it from the MySQL side. The main conversation schema
//! (`messages`, `conversations`, …) stays on SQLite in a MySQL enterprise
//! deployment — mixed storage by design, see the P3-3 implementation plan §4.
//!
//! Deployment requirements (set at CREATE DATABASE time or per table in the
//! migration files): `CHARACTER SET utf8mb4` and a case-sensitive collation
//! (`utf8mb4_0900_as_cs`). The server default `utf8mb4_0900_ai_ci` would make
//! `WHERE name = 'API'` match `'api'`, silently changing config-alias and
//! skill-name uniqueness semantics. Target server: MySQL 8.0.16+.

use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;

use crate::error::DbError;

static MYSQL_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations_mysql");

/// A MySQL-backed database handle, parallel to [`crate::database::Database`].
pub struct MySqlDatabase {
    pool: MySqlPool,
}

impl MySqlDatabase {
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Connects to `database_url` and runs the MySQL migrator
/// (`migrations_mysql/`, independent of the SQLite `migrations/` tree).
pub async fn init_database_mysql(database_url: &str) -> Result<MySqlDatabase, DbError> {
    let pool = MySqlPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    MYSQL_MIGRATOR.run(&pool).await?;

    Ok(MySqlDatabase { pool })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires a real MySQL 8.0.16+ instance — set `DREAM_TEST_MYSQL_URL` to
    /// run it (e.g. `mysql://root:test@localhost:13306/dream_test`). The
    /// database must be created case-sensitively:
    /// `CREATE DATABASE dream_test CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs`.
    /// Skipped by default so `cargo test -p dream-core-db` stays hermetic.
    #[tokio::test]
    async fn migrates_and_round_trips_a_user_row() {
        let Ok(url) = std::env::var("DREAM_TEST_MYSQL_URL") else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        let db = init_database_mysql(&url).await.expect("connect + migrate");

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE username = new.username",
        )
        .bind("test-user-1")
        .bind("alice")
        .bind("hash")
        .bind(1_756_000_000_000_i64)
        .bind(1_756_000_000_000_i64)
        .execute(db.pool())
        .await
        .expect("insert user");

        let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
            .bind("test-user-1")
            .fetch_one(db.pool())
            .await
            .expect("select user");

        assert_eq!(username, "alice");

        // Case-sensitive collation: 'ALICE' must not match 'alice'.
        let none: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE username = ?")
            .bind("ALICE")
            .fetch_optional(db.pool())
            .await
            .expect("case-sensitive lookup");
        assert_eq!(none, None, "collation must be case-sensitive");

        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind("test-user-1")
            .execute(db.pool())
            .await
            .expect("cleanup");

        db.close().await;
    }
}
