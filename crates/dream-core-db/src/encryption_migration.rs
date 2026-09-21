//! One-time, idempotent encryption of legacy plaintext credentials.
//!
//! A security audit found that `mcp_servers.transport_config` (stdio `env`
//! vars, SSE/HTTP `headers` — both routinely carry bearer tokens) and
//! `oauth_tokens.{access_token,refresh_token}` were stored in plaintext,
//! unlike every other sensitive column in this database (provider API keys,
//! MFA secrets), which is encrypted with
//! [`dream_core_common::encrypt_string`]/[`dream_core_common::decrypt_string`]
//! under a key derived from `users.data_secret`
//! (`derive_encryption_key`, in `dream-core-app`).
//!
//! [`crate::SqliteMcpServerRepository`] and [`crate::SqliteOAuthTokenRepository`]
//! now encrypt those columns transparently for every new write. This module
//! is the other half: it walks the *existing* rows once and encrypts whatever
//! is still plaintext, in place.
//!
//! # Why this isn't a normal `NNN_*.sql` migration
//!
//! This crate's sqlx migrations run inside [`crate::database::init_database`],
//! which only opens a raw SQLite connection — no application key material
//! exists at that point. The AES key needed here is derived from
//! `users.data_secret`, a row that itself may need to be seeded on first run,
//! and that seeding happens in `AppServices::from_config_with_backend_binary_path`
//! (dream-core-app), strictly *after* `init_database` (and therefore after
//! every sqlx migration) has already completed. A `.sql` migration file
//! cannot call into Rust AES-GCM code with a key it has no way to obtain, so
//! encrypting historical rows has to happen as a distinct step, later, once
//! the key exists — this function is that step. It is still versioned and
//! idempotent in spirit, the same way `migrate_repair.rs` is: instead of
//! gating on the sqlx migration ledger (there is no migration version this
//! could hang off), it gates row-by-row on the data itself.
//!
//! # Idempotency
//!
//! Every row is checked with [`dream_core_common::is_encrypted_field`] before
//! being touched. A row that already carries the encryption envelope is left
//! alone. This makes the whole pass safe to run on every startup forever
//! (steady state costs one `SELECT` per table and no writes), and safe to
//! interrupt at any point — a crash or power loss mid-pass leaves some rows
//! encrypted and some still plaintext, and the next run picks up exactly
//! where it left off without ever re-encrypting an already-encrypted value
//! (which would turn it into unrecoverable garbage on the next decrypt).

use dream_core_common::{encrypt_field, is_encrypted_field};
use sqlx::SqlitePool;
use tracing::info;

use crate::error::DbError;

/// Outcome of one pass, for startup logging.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionMigrationReport {
    pub mcp_servers_encrypted: usize,
    pub oauth_access_tokens_encrypted: usize,
    pub oauth_refresh_tokens_encrypted: usize,
}

impl EncryptionMigrationReport {
    fn is_empty(&self) -> bool {
        self.mcp_servers_encrypted == 0 && self.oauth_access_tokens_encrypted == 0 && self.oauth_refresh_tokens_encrypted == 0
    }
}

/// Encrypt any still-plaintext `mcp_servers.transport_config` and
/// `oauth_tokens.{access_token,refresh_token}` rows in place, using
/// `encryption_key` (the same 32-byte AES-256 key the repositories use for
/// new writes). Safe to call on every startup — see the module docs for why
/// this is idempotent and interruption-safe.
pub async fn encrypt_legacy_plaintext(pool: &SqlitePool, encryption_key: &[u8]) -> Result<EncryptionMigrationReport, DbError> {
    let mut report = EncryptionMigrationReport {
        mcp_servers_encrypted: encrypt_mcp_server_configs(pool, encryption_key).await?,
        ..Default::default()
    };
    let (access, refresh) = encrypt_oauth_tokens(pool, encryption_key).await?;
    report.oauth_access_tokens_encrypted = access;
    report.oauth_refresh_tokens_encrypted = refresh;

    if !report.is_empty() {
        info!(
            mcp_servers_encrypted = report.mcp_servers_encrypted,
            oauth_access_tokens_encrypted = report.oauth_access_tokens_encrypted,
            oauth_refresh_tokens_encrypted = report.oauth_refresh_tokens_encrypted,
            "encrypted legacy plaintext MCP/OAuth credentials at rest"
        );
    }
    Ok(report)
}

async fn encrypt_mcp_server_configs(pool: &SqlitePool, key: &[u8]) -> Result<usize, DbError> {
    // The table may not exist yet on a schema older than migration 007
    // (mcp_soft_delete) reconciles, or in an isolated test fixture that only
    // creates a subset of tables.
    if !table_exists(pool, "mcp_servers").await? {
        return Ok(0);
    }

    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, transport_config FROM mcp_servers")
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;

    let mut count = 0usize;
    for (id, transport_config) in rows {
        if is_encrypted_field(&transport_config) {
            continue;
        }
        let encrypted = encrypt_field(&transport_config, key)?;
        sqlx::query("UPDATE mcp_servers SET transport_config = ? WHERE id = ?")
            .bind(encrypted)
            .bind(&id)
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
        count += 1;
    }
    Ok(count)
}

async fn encrypt_oauth_tokens(pool: &SqlitePool, key: &[u8]) -> Result<(usize, usize), DbError> {
    if !table_exists(pool, "oauth_tokens").await? {
        return Ok((0, 0));
    }

    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT user_id, server_url, access_token, refresh_token FROM oauth_tokens")
            .fetch_all(pool)
            .await
            .map_err(DbError::Query)?;

    let mut access_count = 0usize;
    let mut refresh_count = 0usize;
    for (user_id, server_url, access_token, refresh_token) in rows {
        let mut changed = false;

        let access_final = if is_encrypted_field(&access_token) {
            access_token
        } else {
            changed = true;
            access_count += 1;
            encrypt_field(&access_token, key)?
        };

        let refresh_final = match refresh_token {
            None => None,
            Some(rt) if is_encrypted_field(&rt) => Some(rt),
            Some(rt) => {
                changed = true;
                refresh_count += 1;
                Some(encrypt_field(&rt, key)?)
            }
        };

        if !changed {
            continue;
        }

        sqlx::query("UPDATE oauth_tokens SET access_token = ?, refresh_token = ? WHERE user_id = ? AND server_url = ?")
            .bind(access_final)
            .bind(refresh_final)
            .bind(&user_id)
            .bind(&server_url)
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
    }
    Ok((access_count, refresh_count))
}

async fn table_exists(pool: &SqlitePool, name: &str) -> Result<bool, DbError> {
    sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    const TEST_KEY: [u8; 32] = [0x33; 32];

    async fn seed_mcp_server(pool: &SqlitePool, id: &str, transport_config: &str) {
        let now = dream_core_common::now_ms();
        sqlx::query(
            "INSERT INTO mcp_servers \
                (id, user_id, name, description, enabled, transport_type, transport_config, \
                 tools, last_test_status, last_connected, original_json, builtin, \
                 deleted_at, created_at, updated_at) \
             VALUES (?, 'system_default_user', ?, NULL, 0, 'stdio', ?, NULL, 'disconnected', NULL, NULL, 0, NULL, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(transport_config)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_oauth_token(pool: &SqlitePool, server_url: &str, access_token: &str, refresh_token: Option<&str>) {
        let now = dream_core_common::now_ms();
        sqlx::query(
            "INSERT INTO oauth_tokens \
                (user_id, server_url, access_token, refresh_token, token_type, expires_at, created_at, updated_at) \
             VALUES ('system_default_user', ?, ?, ?, 'bearer', NULL, ?, ?)",
        )
        .bind(server_url)
        .bind(access_token)
        .bind(refresh_token)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn raw_mcp_config(pool: &SqlitePool, id: &str) -> String {
        sqlx::query_scalar("SELECT transport_config FROM mcp_servers WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn raw_oauth_tokens(pool: &SqlitePool, server_url: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT access_token, refresh_token FROM oauth_tokens WHERE server_url = ?")
            .bind(server_url)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn encrypts_a_batch_of_plaintext_mcp_servers_and_oauth_tokens() {
        let db = init_database_memory().await.unwrap();
        let pool = db.pool();

        seed_mcp_server(pool, "mcp_1", r#"{"command":"npx","args":[],"env":{"TOKEN":"secret-1"}}"#).await;
        seed_mcp_server(
            pool,
            "mcp_2",
            r#"{"url":"https://x","headers":{"Authorization":"Bearer secret-2"}}"#,
        )
        .await;
        seed_oauth_token(pool, "https://a.example.com", "access-secret-a", Some("refresh-secret-a")).await;
        seed_oauth_token(pool, "https://b.example.com", "access-secret-b", None).await;

        let report = encrypt_legacy_plaintext(pool, &TEST_KEY).await.unwrap();
        assert_eq!(report.mcp_servers_encrypted, 2);
        assert_eq!(report.oauth_access_tokens_encrypted, 2);
        assert_eq!(report.oauth_refresh_tokens_encrypted, 1);

        for id in ["mcp_1", "mcp_2"] {
            let raw = raw_mcp_config(pool, id).await;
            assert!(is_encrypted_field(&raw), "{id} should be encrypted, got {raw}");
        }
        let (access_a, refresh_a) = raw_oauth_tokens(pool, "https://a.example.com").await;
        assert!(is_encrypted_field(&access_a));
        assert!(is_encrypted_field(&refresh_a.unwrap()));
        let (access_b, refresh_b) = raw_oauth_tokens(pool, "https://b.example.com").await;
        assert!(is_encrypted_field(&access_b));
        assert!(refresh_b.is_none());

        // Decrypting with the same key recovers the original plaintext.
        assert_eq!(
            dream_core_common::decrypt_field(&raw_mcp_config(pool, "mcp_1").await, &TEST_KEY).unwrap(),
            r#"{"command":"npx","args":[],"env":{"TOKEN":"secret-1"}}"#
        );
        assert_eq!(
            dream_core_common::decrypt_field(&raw_oauth_tokens(pool, "https://a.example.com").await.0, &TEST_KEY)
                .unwrap(),
            "access-secret-a"
        );
    }

    #[tokio::test]
    async fn is_idempotent_a_second_pass_touches_nothing() {
        let db = init_database_memory().await.unwrap();
        let pool = db.pool();
        seed_mcp_server(pool, "mcp_1", r#"{"command":"npx","args":[],"env":{}}"#).await;
        seed_oauth_token(pool, "https://a.example.com", "access-secret-a", Some("refresh-secret-a")).await;

        let first = encrypt_legacy_plaintext(pool, &TEST_KEY).await.unwrap();
        assert!(!first.is_empty());
        let encrypted_config_after_first = raw_mcp_config(pool, "mcp_1").await;
        let (encrypted_access_after_first, encrypted_refresh_after_first) =
            raw_oauth_tokens(pool, "https://a.example.com").await;

        // Re-run: nothing should change, and specifically nothing should be
        // re-encrypted (double-encryption would make decrypt_field, which
        // strips exactly one envelope layer, return garbage instead of the
        // original plaintext).
        let second = encrypt_legacy_plaintext(pool, &TEST_KEY).await.unwrap();
        assert_eq!(second, EncryptionMigrationReport::default());

        assert_eq!(raw_mcp_config(pool, "mcp_1").await, encrypted_config_after_first);
        let (encrypted_access_after_second, encrypted_refresh_after_second) =
            raw_oauth_tokens(pool, "https://a.example.com").await;
        assert_eq!(encrypted_access_after_second, encrypted_access_after_first);
        assert_eq!(encrypted_refresh_after_second, encrypted_refresh_after_first);

        // And the value still decrypts to the original plaintext.
        assert_eq!(
            dream_core_common::decrypt_field(&raw_mcp_config(pool, "mcp_1").await, &TEST_KEY).unwrap(),
            r#"{"command":"npx","args":[],"env":{}}"#
        );
        assert_eq!(
            dream_core_common::decrypt_field(&encrypted_access_after_second, &TEST_KEY).unwrap(),
            "access-secret-a"
        );
    }

    #[tokio::test]
    async fn mixed_batch_only_encrypts_rows_still_in_plaintext() {
        // Simulates an interrupted prior pass: one row already encrypted
        // (e.g. from a partial run, or written by a newer binary), one still
        // plaintext. Confirms the migration distinguishes them correctly
        // rather than blanket-encrypting or blanket-skipping.
        let db = init_database_memory().await.unwrap();
        let pool = db.pool();

        let already_encrypted = encrypt_field("already-done", &TEST_KEY).unwrap();
        seed_mcp_server(pool, "mcp_done", &already_encrypted).await;
        seed_mcp_server(pool, "mcp_pending", "still-plaintext").await;

        let report = encrypt_legacy_plaintext(pool, &TEST_KEY).await.unwrap();
        assert_eq!(report.mcp_servers_encrypted, 1, "only the plaintext row should be touched");

        assert_eq!(
            raw_mcp_config(pool, "mcp_done").await, already_encrypted,
            "already-encrypted row must be left byte-for-byte untouched"
        );
        assert!(is_encrypted_field(&raw_mcp_config(pool, "mcp_pending").await));
    }

    #[tokio::test]
    async fn no_op_on_empty_tables() {
        let db = init_database_memory().await.unwrap();
        let report = encrypt_legacy_plaintext(db.pool(), &TEST_KEY).await.unwrap();
        assert_eq!(report, EncryptionMigrationReport::default());
    }
}
