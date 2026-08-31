//! MySQL test harness for the enterprise crates' migration and dialect tests.
//!
//! Opt-in per environment, mirroring the `DREAM_TEST_POSTGRES_URL` precedent
//! in [`crate::postgres`]: tests call [`mysql_test_pool`], which returns
//! `None` when `DREAM_TEST_MYSQL_URL` is unset so `cargo nextest` stays
//! hermetic by default (CI skips; `just test-mysql` sets the URL against a
//! throwaway `mysql:8` container). The server must be MySQL 8.0.16+ and the
//! credentials must be allowed to CREATE/DROP databases.
//!
//! Each call creates a fresh, uniquely-named database with the case-sensitive
//! collation the P3-3 schema requires (`utf8mb4_0900_as_cs` — the same
//! collation every `migrations_mysql` file sets per table), hands out a
//! [`DbPool`] bound to it, and drops the database on [`MySqlTestDb::cleanup`].
//! A test that fails before reaching cleanup leaves its database behind on
//! the dev box; CI containers are ephemeral so that is acceptable there.

use crate::pool::DbPool;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::MySqlPool;

/// A throwaway MySQL database for one test, plus its pool.
pub struct MySqlTestDb {
    pub pool: DbPool,
    server_url: String,
    pub name: String,
}

impl MySqlTestDb {
    /// URL addressing this test's database — for code under test that takes
    /// a connection URL (e.g. [`crate::init_database_mysql`]) instead of a pool.
    pub fn mysql_url(&self) -> String {
        format!("{}/{}", self.server_url, self.name)
    }

    /// `DROP DATABASE` for this test's database and close the pool. Call at
    /// the end of the test; on error paths after cleanup, the database stays
    /// behind (harmless on an ephemeral server).
    pub async fn cleanup(self) -> Result<(), sqlx::Error> {
        let admin = MySqlPool::connect(&self.server_url).await?;
        sqlx::query(&format!("DROP DATABASE IF EXISTS `{}`", self.name))
            .execute(&admin)
            .await?;
        admin.close().await;
        match self.pool {
            DbPool::Sqlite(pool) => pool.close().await,
            DbPool::MySql(pool) => pool.close().await,
        }
        Ok(())
    }
}

/// Creates a fresh per-test database, or `None` when `DREAM_TEST_MYSQL_URL`
/// is not set (test skips). Errors after the env var is present panic — a
/// configured-but-broken server should fail loudly, not silently skip.
pub async fn mysql_test_pool() -> Option<MySqlTestDb> {
    let server_url = strip_database_path(&std::env::var("DREAM_TEST_MYSQL_URL").ok()?)?;

    let name = format!(
        "dream_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos()
    );

    let admin = MySqlPool::connect(&server_url)
        .await
        .expect("connect to DREAM_TEST_MYSQL_URL server");
    sqlx::query(&format!(
        "CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs"
    ))
    .execute(&admin)
    .await
    .expect("create per-test database");
    admin.close().await;

    let pool = MySqlPoolOptions::new()
        .max_connections(5)
        .connect(&format!("{server_url}/{name}"))
        .await
        .expect("connect to per-test database");

    Some(MySqlTestDb {
        pool: DbPool::MySql(pool),
        server_url,
        name,
    })
}

/// Strims the path (and query) off `mysql://user:pass@host:port/db?x=y` so the
/// remainder addresses the server itself; `None` for unparseable URLs.
fn strip_database_path(url: &str) -> Option<String> {
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let base = match rest.find('/') {
        Some(slash) => &rest[..slash],
        None => rest,
    };
    let base = base.split('?').next().unwrap_or(base);
    if base.is_empty() {
        return None;
    }
    Some(format!("{}{}", &url[..scheme_end], base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_database_and_query_from_urls() {
        assert_eq!(
            strip_database_path("mysql://root:root@127.0.0.1:3306/dream_test").as_deref(),
            Some("mysql://root:root@127.0.0.1:3306")
        );
        assert_eq!(
            strip_database_path("mysql://root:root@127.0.0.1:3306/dream?x=y").as_deref(),
            Some("mysql://root:root@127.0.0.1:3306")
        );
        assert_eq!(
            strip_database_path("mysql://root:root@127.0.0.1:3306").as_deref(),
            Some("mysql://root:root@127.0.0.1:3306")
        );
        assert_eq!(strip_database_path("not-a-url"), None);
    }
}
