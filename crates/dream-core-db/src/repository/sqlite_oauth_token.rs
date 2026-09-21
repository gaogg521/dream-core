use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::OAuthTokenRow;
use crate::repository::oauth_token::{IOAuthTokenRepository, UpsertOAuthTokenParams};

/// SQLite-backed implementation of [`IOAuthTokenRepository`].
///
/// `access_token`/`refresh_token` are OAuth bearer credentials for MCP
/// servers, so they are encrypted at rest with
/// [`dream_core_common::encrypt_field`]/[`dream_core_common::decrypt_field`]
/// under `encryption_key` — same key material as every other encrypted
/// column (`derive_encryption_key(&data_secret)`). `upsert` encrypts before
/// binding; `get_by_url` decrypts right after fetching, so `McpOAuthService`
/// (the sole consumer) sees plaintext transparently.
#[derive(Clone, Debug)]
pub struct SqliteOAuthTokenRepository {
    pool: SqlitePool,
    encryption_key: [u8; 32],
}

impl SqliteOAuthTokenRepository {
    pub fn new(pool: SqlitePool, encryption_key: [u8; 32]) -> Self {
        Self { pool, encryption_key }
    }

    /// Decrypt `row.access_token` and `row.refresh_token` in place. Legacy
    /// plaintext rows (not yet covered by `dream_core_db::encrypt_legacy_plaintext`)
    /// pass through unchanged — see [`dream_core_common::decrypt_field`].
    fn decrypt_row(&self, mut row: OAuthTokenRow) -> Result<OAuthTokenRow, DbError> {
        row.access_token = dream_core_common::decrypt_field(&row.access_token, &self.encryption_key)?;
        row.refresh_token = row
            .refresh_token
            .map(|rt| dream_core_common::decrypt_field(&rt, &self.encryption_key))
            .transpose()?;
        Ok(row)
    }
}

#[async_trait::async_trait]
impl IOAuthTokenRepository for SqliteOAuthTokenRepository {
    async fn get_by_url(&self, user_id: &str, server_url: &str) -> Result<Option<OAuthTokenRow>, DbError> {
        let row = sqlx::query_as::<_, OAuthTokenRow>("SELECT * FROM oauth_tokens WHERE user_id = ? AND server_url = ?")
            .bind(user_id)
            .bind(server_url)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|row| self.decrypt_row(row)).transpose()
    }

    async fn upsert(&self, params: UpsertOAuthTokenParams<'_>) -> Result<OAuthTokenRow, DbError> {
        let now = dream_core_common::now_ms();
        let encrypted_access_token = dream_core_common::encrypt_field(params.access_token, &self.encryption_key)?;
        let encrypted_refresh_token = params
            .refresh_token
            .map(|rt| dream_core_common::encrypt_field(rt, &self.encryption_key))
            .transpose()?;

        sqlx::query(
            "INSERT INTO oauth_tokens \
                (user_id, server_url, access_token, refresh_token, token_type, \
                 expires_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, server_url) DO UPDATE SET \
                access_token = excluded.access_token, \
                refresh_token = excluded.refresh_token, \
                token_type = excluded.token_type, \
                expires_at = excluded.expires_at, \
                updated_at = excluded.updated_at",
        )
        .bind(params.user_id)
        .bind(params.server_url)
        .bind(&encrypted_access_token)
        .bind(&encrypted_refresh_token)
        .bind(params.token_type)
        .bind(params.expires_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        // Fetch the row to get the correct created_at (preserved on conflict).
        let row = self
            .get_by_url(params.user_id, params.server_url)
            .await?
            .ok_or_else(|| DbError::Init("Upsert succeeded but row not found".to_string()))?;

        Ok(row)
    }

    async fn delete(&self, user_id: &str, server_url: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM oauth_tokens WHERE user_id = ? AND server_url = ?")
            .bind(user_id)
            .bind(server_url)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("OAuth token for '{server_url}' not found")));
        }

        Ok(())
    }

    async fn list_authenticated_urls(&self, user_id: &str) -> Result<Vec<String>, DbError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT server_url FROM oauth_tokens WHERE user_id = ? ORDER BY created_at ASC")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;

        Ok(rows.into_iter().map(|(url,)| url).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    const USER_A: &str = "system_default_user";
    const USER_B: &str = "user_b";
    const TEST_KEY: [u8; 32] = [0x22; 32];

    async fn setup() -> (SqliteOAuthTokenRepository, crate::Database) {
        let db = init_database_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO users (id, user_type, username, password_hash, status, session_generation, created_at, updated_at) \
             VALUES (?, 'local', ?, 'hash', 'active', 0, 1, 1)",
        )
        .bind(USER_B)
        .bind(USER_B)
        .execute(db.pool())
        .await
        .unwrap();
        let repo = SqliteOAuthTokenRepository::new(db.pool().clone(), TEST_KEY);
        (repo, db)
    }

    fn sample_params() -> UpsertOAuthTokenParams<'static> {
        UpsertOAuthTokenParams {
            user_id: USER_A,
            server_url: "https://mcp.example.com",
            access_token: "enc_access_token_123",
            refresh_token: Some("enc_refresh_token_456"),
            token_type: "bearer",
            expires_at: Some(1700000000000),
        }
    }

    #[tokio::test]
    async fn get_by_url_nonexistent() {
        let (repo, _db) = setup().await;
        assert!(repo.get_by_url(USER_A, "https://nope.com").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_insert_new_token() {
        let (repo, _db) = setup().await;
        let token = repo.upsert(sample_params()).await.unwrap();

        assert_eq!(token.server_url, "https://mcp.example.com");
        assert_eq!(token.access_token, "enc_access_token_123");
        assert_eq!(token.refresh_token.as_deref(), Some("enc_refresh_token_456"));
        assert_eq!(token.token_type, "bearer");
        assert_eq!(token.expires_at, Some(1700000000000));
        assert!(token.created_at > 0);
        assert_eq!(token.created_at, token.updated_at);
    }

    #[tokio::test]
    async fn upsert_updates_existing_token() {
        let (repo, _db) = setup().await;
        let original = repo.upsert(sample_params()).await.unwrap();

        let updated = repo
            .upsert(UpsertOAuthTokenParams {
                user_id: USER_A,
                server_url: "https://mcp.example.com",
                access_token: "new_access_token",
                refresh_token: None,
                token_type: "bearer",
                expires_at: Some(1800000000000),
            })
            .await
            .unwrap();

        assert_eq!(updated.server_url, original.server_url);
        assert_eq!(updated.access_token, "new_access_token");
        assert!(updated.refresh_token.is_none());
        assert_eq!(updated.expires_at, Some(1800000000000));
        // created_at preserved from original insert
        assert_eq!(updated.created_at, original.created_at);
    }

    #[tokio::test]
    async fn get_by_url_returns_upserted_token() {
        let (repo, _db) = setup().await;
        repo.upsert(sample_params()).await.unwrap();

        let found = repo
            .get_by_url(USER_A, "https://mcp.example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.access_token, "enc_access_token_123");
    }

    #[tokio::test]
    async fn delete_existing_token() {
        let (repo, _db) = setup().await;
        repo.upsert(sample_params()).await.unwrap();

        repo.delete(USER_A, "https://mcp.example.com").await.unwrap();
        assert!(
            repo.get_by_url(USER_A, "https://mcp.example.com")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let (repo, _db) = setup().await;
        let err = repo.delete(USER_A, "https://nope.com").await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn list_authenticated_urls_empty() {
        let (repo, _db) = setup().await;
        let urls = repo.list_authenticated_urls(USER_A).await.unwrap();
        assert!(urls.is_empty());
    }

    #[tokio::test]
    async fn list_authenticated_urls_returns_all() {
        let (repo, _db) = setup().await;
        repo.upsert(sample_params()).await.unwrap();
        repo.upsert(UpsertOAuthTokenParams {
            user_id: USER_A,
            server_url: "https://other.example.com",
            access_token: "token2",
            refresh_token: None,
            token_type: "bearer",
            expires_at: None,
        })
        .await
        .unwrap();

        let urls = repo.list_authenticated_urls(USER_A).await.unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://mcp.example.com".to_string()));
        assert!(urls.contains(&"https://other.example.com".to_string()));
    }

    #[tokio::test]
    async fn oauth_tokens_are_scoped_by_user() {
        let (repo, _db) = setup().await;
        repo.upsert(sample_params()).await.unwrap();
        repo.upsert(UpsertOAuthTokenParams {
            user_id: USER_B,
            access_token: "user_b_token",
            refresh_token: None,
            ..sample_params()
        })
        .await
        .unwrap();

        assert_eq!(
            repo.get_by_url(USER_A, "https://mcp.example.com")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "enc_access_token_123"
        );
        assert_eq!(
            repo.get_by_url(USER_B, "https://mcp.example.com")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "user_b_token"
        );

        repo.delete(USER_B, "https://mcp.example.com").await.unwrap();
        assert!(
            repo.get_by_url(USER_B, "https://mcp.example.com")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repo.get_by_url(USER_A, "https://mcp.example.com")
                .await
                .unwrap()
                .is_some()
        );
    }

    // -- token encryption at rest ----------------------------------------------

    /// Read the raw columns directly from SQLite, bypassing the repository so
    /// the test observes exactly what's on disk.
    async fn raw_tokens(db: &crate::Database, user_id: &str, server_url: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT access_token, refresh_token FROM oauth_tokens WHERE user_id = ? AND server_url = ?")
            .bind(user_id)
            .bind(server_url)
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn access_and_refresh_tokens_are_encrypted_at_rest() {
        let (repo, db) = setup().await;
        repo.upsert(UpsertOAuthTokenParams {
            user_id: USER_A,
            server_url: "https://mcp.example.com",
            access_token: "sk-live-access-secret",
            refresh_token: Some("sk-live-refresh-secret"),
            token_type: "bearer",
            expires_at: None,
        })
        .await
        .unwrap();

        let (raw_access, raw_refresh) = raw_tokens(&db, USER_A, "https://mcp.example.com").await;
        assert!(dream_core_common::is_encrypted_field(&raw_access));
        assert!(!raw_access.contains("sk-live-access-secret"));
        let raw_refresh = raw_refresh.expect("refresh token stored");
        assert!(dream_core_common::is_encrypted_field(&raw_refresh));
        assert!(!raw_refresh.contains("sk-live-refresh-secret"));

        // The repository API still hands back plaintext transparently.
        let row = repo
            .get_by_url(USER_A, "https://mcp.example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.access_token, "sk-live-access-secret");
        assert_eq!(row.refresh_token.as_deref(), Some("sk-live-refresh-secret"));
    }

    #[tokio::test]
    async fn refresh_with_no_refresh_token_stores_null_not_encrypted_empty() {
        let (repo, db) = setup().await;
        repo.upsert(UpsertOAuthTokenParams {
            user_id: USER_A,
            server_url: "https://no-refresh.example.com",
            access_token: "access-only",
            refresh_token: None,
            token_type: "bearer",
            expires_at: None,
        })
        .await
        .unwrap();

        let (_, raw_refresh) = raw_tokens(&db, USER_A, "https://no-refresh.example.com").await;
        assert!(raw_refresh.is_none());
    }

    #[tokio::test]
    async fn wrong_encryption_key_fails_to_decrypt() {
        let (repo, db) = setup().await;
        repo.upsert(sample_params()).await.unwrap();

        let wrong_key_repo = SqliteOAuthTokenRepository::new(db.pool().clone(), [0xBB; 32]);
        let err = wrong_key_repo
            .get_by_url(USER_A, "https://mcp.example.com")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Crypto(_)));
    }
}
