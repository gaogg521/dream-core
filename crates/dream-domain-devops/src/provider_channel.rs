//! Company-provisioned model channels (T2).
//!
//! An admin configures a chat / image / video channel once, in the enterprise
//! console, and every member can use it **without ever holding the API key**.
//! Until this existed the only way to give an employee a model was to hand them
//! a key, which is what enterprise procurement objects to hardest: keys handed
//! out are keys lost, and a leaver keeps theirs.
//!
//! # The credential never leaves the server
//!
//! This is the one rule the whole design exists to enforce, so it is worth
//! stating plainly: `api_key_encrypted` is decrypted **only** inside the model
//! proxy (`crate::model_proxy`), on the machine that stores it. Nothing a member
//! can call returns it, and [`ProviderChannelDto`] has no field to put it in.
//!
//! Members instead receive a **channel token**: long-lived, revocable, and
//! scoped to one (member, channel) pair. Deliberately not the session JWT —
//! one-org rotates that whenever membership changes (to kill a removed member's
//! sessions), which would break every provisioned channel until the next sync.
//! A dedicated token is decoupled from that, revoked in one statement when
//! someone leaves, and identifies the member at the proxy, which is what a
//! content audit will need.
//!
//! ⚠️ This deliberately gives up the offline-first property that
//! `migrations/008_mcp_secrets.sql` chose for MCP connectors. That comment says
//! offline-first "rules out a server-side proxy" — true for MCP, where the
//! secret has to reach a local subprocess. A model channel has no such
//! constraint, and here the tradeoff runs the other way: a company channel that
//! needs the server reachable is worth far more than one whose key is sitting
//! on every laptop.
//!
//! # Access control
//!
//! Scope / team_id / visibility mirror the other three registries exactly, so
//! `member_visibility_where()` and `validate_resource_scope()` apply unchanged:
//! enterprise-wide by default, overridable per project group.

use base64::Engine;
use sha2::{Digest, Sha256};

use dream_core_common::now_ms;
use dream_core_common::{decrypt_string, encrypt_string};

use dream_core_db::db_params;
use crate::error::DevopsError;
use crate::models::ProviderChannelDto;
use crate::service::DevopsService;

const COLS: &str = "id, name, platform, upstream_base_url, \
                    (api_key_encrypted != '') AS has_key, models, model_settings, model_protocols, enabled, \
                    scope, team_id, visibility, created_by, created_at, updated_at";

/// Prefix on issued tokens. Purely so a leaked string is recognisable in a log
/// or a bug report as "a One Work channel token" and can be revoked.
const TOKEN_PREFIX: &str = "onech-";

/// What a member is given, once. Only the hash is persisted, so the plaintext
/// exists exactly here and in the response that carries it away.
pub struct IssuedChannelToken {
    pub token: String,
    pub channel_id: String,
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn generate_token() -> Result<String, DevopsError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| DevopsError::Internal(format!("failed to generate channel token: {e}")))?;
    Ok(format!(
        "{TOKEN_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

/// A channel resolved for an authenticated proxy request: everything the proxy
/// needs, including the decrypted key it is about to use and immediately drop.
pub struct ResolvedChannel {
    pub id: String,
    pub upstream_base_url: String,
    pub api_key: String,
    pub user_id: String,
}

/// One-shot LLM call settings for the memory turn extractor (P2-2 followups
/// §A.6): the channel's OpenAI-compatible upstream + credential (decrypted
/// server-side, never leaves the process) + the model to call.
#[derive(Debug, Clone)]
pub struct ExtractionLlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl DevopsService {
    /// Resolves a company model channel into one-shot LLM call settings for
    /// the memory turn extractor (P2-2 followups §A.6). OpenAI-compatible
    /// channels only — extraction is a plain chat completion against
    /// `upstream_base_url`, the same protocol the model proxy speaks. `model`
    /// overrides the channel's first configured model; `None` picks it.
    /// Returns `Ok(None)` when the channel does not exist (extraction is then
    /// skipped, not failed) and errors when it exists but is unusable
    /// (disabled, not OpenAI-compatible, no credential, no model).
    pub async fn extraction_llm_config(
        &self,
        channel_id: &str,
        model: Option<&str>,
    ) -> Result<Option<ExtractionLlmConfig>, DevopsError> {
        let row: Option<(String, String, String, String)> = self.db.fetch_optional_as::<(String, String, String, String)>(
            "SELECT platform, upstream_base_url, api_key_encrypted, models              FROM one_provider_registry WHERE id = ? AND enabled = 1",
        &db_params![channel_id])
        .await?;

        let Some((platform, upstream_base_url, api_key_encrypted, models_json)) = row else {
            return Ok(None);
        };
        if platform != "openai" {
            return Err(DevopsError::BadRequest(format!(
                "extraction channel '{channel_id}' has platform '{platform}'; extraction supports OpenAI-compatible channels only"
            )));
        }
        if api_key_encrypted.is_empty() {
            return Err(DevopsError::BadRequest(format!(
                "extraction channel '{channel_id}' has no credential configured"
            )));
        }
        let api_key = decrypt_string(&api_key_encrypted, self.encryption_key()?)
            .map_err(|e| DevopsError::Internal(format!("failed to decrypt channel credential: {e}")))?;
        let models: Vec<String> = serde_json::from_str(&models_json).unwrap_or_default();
        let Some(model) = model
            .map(str::to_owned)
            .or_else(|| models.first().cloned())
        else {
            return Err(DevopsError::BadRequest(format!(
                "extraction channel '{channel_id}' has no model configured"
            )));
        };
        Ok(Some(ExtractionLlmConfig {
            base_url: upstream_base_url,
            api_key,
            model,
        }))
    }

    fn encryption_key(&self) -> Result<&[u8; 32], DevopsError> {
        self.encryption_key.as_ref().ok_or_else(|| {
            DevopsError::Internal(
                "model channels need the deployment data key, which this server did not provide".into(),
            )
        })
    }

    // -- registry ---------------------------------------------------------

    /// Channels this viewer may see. Admins see every row; members see org-wide
    /// channels plus those bound to a project group they belong to, and only
    /// `visibility='all'` — the same predicate the other registries use.
    pub async fn list_provider_channels(&self, viewer_user_id: &str) -> Result<Vec<ProviderChannelDto>, DevopsError> {
        let privileged = self.viewer_is_privileged(viewer_user_id).await?;
        if privileged {
            let sql = format!("SELECT {COLS} FROM one_provider_registry ORDER BY updated_at DESC");
            return Ok(self.db.fetch_all_as::<ProviderChannelDto>(&sql, &[])
                .await?);
        }
        // Widened with `model_channel` grants, same as the other registries.
        // Without this the matrix accepts, stores and reports a
        // `model_channel` grant as effective while the member's channel list
        // stays unchanged — a silently no-op grant the admin console renders
        // as active.
        let grants = self
            .extra_grants(viewer_user_id, crate::grants::resource_type::MODEL_CHANNEL)
            .await;
        let (predicate, grant_binds) =
            Self::widen_with_grants(&Self::member_visibility_where(""), &grants, "", viewer_user_id);
        let sql = format!("SELECT {COLS} FROM one_provider_registry WHERE {predicate} ORDER BY updated_at DESC");
        let mut params = db_params![viewer_user_id];
        for bind in &grant_binds {
            params.push(bind.as_str().into());
        }
        Ok(self.db.fetch_all_as::<ProviderChannelDto>(&sql, &params).await?)
    }

    /// Create or update a channel.
    ///
    /// `api_key` is write-only in both directions: `None` leaves whatever is
    /// stored alone, so an admin can rename a channel or change its scope
    /// without re-entering the credential — and no read path can hand it back
    /// to them either.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_provider_channel(
        &self,
        id: Option<&str>,
        name: &str,
        platform: &str,
        upstream_base_url: &str,
        api_key: Option<&str>,
        models: &str,
        model_settings: Option<&str>,
        model_protocols: Option<&str>,
        enabled: bool,
        scope: &str,
        team_id: Option<&str>,
        visibility: &str,
        created_by: &str,
    ) -> Result<ProviderChannelDto, DevopsError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DevopsError::BadRequest("name is required".into()));
        }
        let upstream_base_url = upstream_base_url.trim().trim_end_matches('/');
        if upstream_base_url.is_empty() {
            return Err(DevopsError::BadRequest("upstream base URL is required".into()));
        }
        if !upstream_base_url.starts_with("http://") && !upstream_base_url.starts_with("https://") {
            return Err(DevopsError::BadRequest(
                "upstream base URL must start with http:// or https://".into(),
            ));
        }
        let team_id = self
            .validate_resource_scope(created_by, scope, team_id, visibility)
            .await?;
        // The INCOMING team_id, checked on every write (create and update
        // alike): without this, an actor who legitimately owns some channel
        // could re-scope it INTO a team they don't administer in the same
        // call the current-row check below guards against re-scoping OUT of
        // one they don't own.
        if !self.actor_can_touch_team(created_by, team_id).await? {
            return Err(DevopsError::Forbidden(
                "cannot assign this model channel to a different project group".into(),
            ));
        }

        // Encrypt before touching the database, so a missing deployment key
        // fails the write instead of quietly storing an empty credential that
        // only shows up as a confusing 401 at call time.
        let encrypted = match api_key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(key) => Some(
                encrypt_string(key, self.encryption_key()?)
                    .map_err(|e| DevopsError::Internal(format!("failed to encrypt channel credential: {e}")))?,
            ),
            None => None,
        };

        let now = now_ms();
        let id = match id {
            Some(existing) => {
                // The row's CURRENT team_id, not the incoming one: an actor
                // editing must already own what the row belongs to today,
                // otherwise they could both rewrite another team's channel
                // (base_url + rotate the key) and re-scope it away from that
                // team in the same call. Same pattern as skill/MCP registries.
                let current_team_id: Option<String> =
                    self.db.fetch_optional_scalar("SELECT team_id FROM one_provider_registry WHERE id = ?", &db_params![existing])
                        .await?
                        .ok_or_else(|| DevopsError::NotFound(format!("model channel {existing}")))?;
                if !self
                    .actor_can_touch_team(created_by, current_team_id.as_deref())
                    .await?
                {
                    return Err(DevopsError::Forbidden(
                        "this model channel belongs to a different project group".into(),
                    ));
                }
                let updated = match &encrypted {
                    Some(secret) => {
                        self.db.execute(
                            "UPDATE one_provider_registry SET name = ?, platform = ?, upstream_base_url = ?, \
                             api_key_encrypted = ?, models = ?, model_settings = ?, model_protocols = ?, enabled = ?, scope = ?, \
                             team_id = ?, visibility = ?, updated_at = ? WHERE id = ?",
                        &db_params![name, platform, upstream_base_url, secret, models, model_settings, model_protocols, enabled, scope, team_id, visibility, now, existing])
                        .await?
                    }
                    None => {
                        self.db.execute(
                            "UPDATE one_provider_registry SET name = ?, platform = ?, upstream_base_url = ?, \
                             models = ?, model_settings = ?, model_protocols = ?, enabled = ?, scope = ?, team_id = ?, \
                             visibility = ?, updated_at = ? WHERE id = ?",
                        &db_params![name, platform, upstream_base_url, models, model_settings, model_protocols, enabled, scope, team_id, visibility, now, existing])
                        .await?
                    }
                };
                if updated == 0 {
                    return Err(DevopsError::NotFound(format!("model channel {existing}")));
                }
                existing.to_owned()
            }
            None => {
                let id = format!("ochan_{}", uuid::Uuid::now_v7().simple());
                self.db.execute(
                    "INSERT INTO one_provider_registry \
                        (id, name, platform, upstream_base_url, api_key_encrypted, models, model_settings, model_protocols, \
                         enabled, scope, team_id, visibility, created_by, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![&id, name, platform, upstream_base_url, encrypted.unwrap_or_default(), models, model_settings, model_protocols, enabled, scope, team_id, visibility, created_by, now, now])
                .await?;
                id
            }
        };

        self.db.fetch_one_as::<ProviderChannelDto>(&format!("SELECT {COLS} FROM one_provider_registry WHERE id = ?"), &db_params![&id])
            .await
            .map_err(Into::into)
    }

    /// Delete a channel and every token issued against it, in one transaction.
    ///
    /// Leaving tokens behind would let a member keep reaching a channel an
    /// admin believes they deleted — and, worse, a recycled id would silently
    /// re-authorize them.
    pub async fn delete_provider_channel(&self, actor_user_id: &str, id: &str) -> Result<(), DevopsError> {
        let team_id: Option<String> = self.db.fetch_optional_scalar("SELECT team_id FROM one_provider_registry WHERE id = ?", &db_params![id])
            .await?
            .ok_or_else(|| DevopsError::NotFound(format!("model channel {id}")))?;
        if !self.actor_can_touch_team(actor_user_id, team_id.as_deref()).await? {
            return Err(DevopsError::Forbidden(
                "this model channel belongs to a different project group".into(),
            ));
        }
        let mut tx = self.db.begin().await?;
        tx.execute("DELETE FROM one_provider_channel_tokens WHERE channel_id = ?", &db_params![id])
            .await?;
        let deleted = tx.execute("DELETE FROM one_provider_registry WHERE id = ?", &db_params![id])
            .await?;
        if deleted == 0 {
            return Err(DevopsError::NotFound(format!("model channel {id}")));
        }
        tx.commit().await?;
        Ok(())
    }

    // -- tokens -----------------------------------------------------------

    /// Issue this member's token for a channel they can see.
    ///
    /// Rotating on every call is intentional: only the hash is stored, so there
    /// is nothing to hand back, and the alternative — keeping a plaintext copy
    /// to re-serve — is exactly the storage this design exists to avoid. The
    /// client persists what it gets and only re-asks when it has none.
    pub async fn issue_channel_token(
        &self,
        user_id: &str,
        channel_id: &str,
    ) -> Result<IssuedChannelToken, DevopsError> {
        // Visibility is the authorization: a member may only mint a token for a
        // channel they are allowed to see in the first place.
        let visible = self.list_provider_channels(user_id).await?;
        let channel = visible
            .into_iter()
            .find(|c| c.id == channel_id)
            .ok_or_else(|| DevopsError::NotFound(format!("model channel {channel_id}")))?;
        if !channel.enabled {
            return Err(DevopsError::BadRequest("this model channel is disabled".into()));
        }

        let token = generate_token()?;
        let now = now_ms();
        self.upsert(
            "INSERT INTO one_provider_channel_tokens (token_hash, user_id, channel_id, created_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(user_id, channel_id) DO UPDATE SET \
                token_hash = excluded.token_hash, created_at = excluded.created_at, \
                last_used = NULL, revoked_at = NULL",
            "INSERT INTO one_provider_channel_tokens (token_hash, user_id, channel_id, created_at) \
             VALUES (?, ?, ?, ?) AS new \
             ON DUPLICATE KEY UPDATE \
                token_hash = new.token_hash, created_at = new.created_at, \
                last_used = NULL, revoked_at = NULL",
        &db_params![hash_token(&token), user_id, channel_id, now])
        .await?;

        Ok(IssuedChannelToken {
            token,
            channel_id: channel_id.to_owned(),
        })
    }

    /// Revoke every channel token a user holds. Called when they are removed
    /// from the company: their session JWT is already invalidated there, and
    /// this closes the one credential that deliberately outlives it.
    pub async fn revoke_channel_tokens_for_user(&self, user_id: &str) -> Result<u64, DevopsError> {
        let result = self.db.execute(
            "UPDATE one_provider_channel_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        &db_params![now_ms(), user_id])
        .await?;
        Ok(result)
    }

    /// Authenticate a proxy request and produce everything it needs to forward.
    ///
    /// Returns `None` for any failure a caller must not be able to tell apart —
    /// unknown token, revoked token, token for a different channel, disabled or
    /// deleted channel. All of them are "not authorized here", and
    /// distinguishing them would let someone probe which channels exist.
    pub async fn resolve_channel_for_token(
        &self,
        channel_id: &str,
        token: &str,
    ) -> Result<Option<ResolvedChannel>, DevopsError> {
        let row: Option<(String, String, String)> = self.db.fetch_optional_as::<(String, String, String)>(
            "SELECT t.user_id, r.upstream_base_url, r.api_key_encrypted \
             FROM one_provider_channel_tokens t \
             JOIN one_provider_registry r ON r.id = t.channel_id \
             WHERE t.token_hash = ? AND t.channel_id = ? AND t.revoked_at IS NULL AND r.enabled = 1",
        &db_params![hash_token(token), channel_id])
        .await?;

        let Some((user_id, upstream_base_url, api_key_encrypted)) = row else {
            return Ok(None);
        };
        if api_key_encrypted.is_empty() {
            return Err(DevopsError::BadRequest(
                "this model channel has no credential configured".into(),
            ));
        }
        let api_key = decrypt_string(&api_key_encrypted, self.encryption_key()?)
            .map_err(|e| DevopsError::Internal(format!("failed to decrypt channel credential: {e}")))?;

        // Best-effort: a failed bookkeeping write must not fail the call the
        // user is actually making.
        let _ = self.db.execute("UPDATE one_provider_channel_tokens SET last_used = ? WHERE token_hash = ?", &db_params![now_ms(), hash_token(token)])
            .await;

        Ok(Some(ResolvedChannel {
            id: channel_id.to_owned(),
            upstream_base_url,
            api_key,
            user_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::run_one_devops_migrations;

    const KEY: [u8; 32] = [7u8; 32];
    const SECRET: &str = "sk-real-company-credential";

    async fn service() -> DevopsService {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone())).await.unwrap();
        // The visibility predicate reads one-org's tables through the shared
        // pool, same cross-crate precedent the other registries use. Column
        // list copied from the existing service tests so the shapes cannot
        // drift apart.
        sqlx::raw_sql(
            "CREATE TABLE one_tenants (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));
             CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0);
             INSERT INTO one_tenants (id, name) VALUES ('tA', 'Group A'), ('tB', 'Group B');
             INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin1', 'tA', 'org_admin'), ('memberA', 'tA', 'member'), ('memberB', 'tB', 'member');
             INSERT INTO one_active_tenant (user_id, tenant_id) VALUES ('admin1', 'tA'), ('memberA', 'tA'), ('memberB', 'tB');",
        )
        .execute(&pool)
        .await
        .unwrap();
        DevopsService::new(dream_core_db::DbPool::Sqlite(pool.clone())).with_encryption_key(KEY)
    }

    async fn make_channel(svc: &DevopsService, name: &str) -> ProviderChannelDto {
        svc.upsert_provider_channel(
            None,
            name,
            "openai",
            "https://gateway.corp.example",
            Some(SECRET),
            r#"["gpt-image-2"]"#,
            None,
            None,
            true,
            "org",
            None,
            "all",
            "admin1",
        )
        .await
        .unwrap()
    }

    /// The rule the whole feature exists for. If this ever fails, the product
    /// is back to handing every employee the company's key.
    #[tokio::test]
    async fn the_real_credential_is_never_readable_through_any_listing() {
        let svc = service().await;
        make_channel(&svc, "corp-gateway").await;

        let listed = svc.list_provider_channels("admin1").await.unwrap();
        let serialized = serde_json::to_string(&listed).unwrap();
        assert!(
            !serialized.contains(SECRET),
            "the credential leaked into the channel listing: {serialized}"
        );
        assert!(listed[0].has_key, "but the admin must still see that one is set");
    }

    #[tokio::test]
    async fn a_channel_can_be_edited_without_re_entering_the_credential() {
        let svc = service().await;
        let created = make_channel(&svc, "corp-gateway").await;

        svc.upsert_provider_channel(
            Some(&created.id),
            "corp-gateway-renamed",
            "openai",
            "https://gateway.corp.example",
            None, // no key supplied
            r#"["gpt-image-2"]"#,
            None,
            None,
            true,
            "org",
            None,
            "all",
            "admin1",
        )
        .await
        .unwrap();

        let token = svc.issue_channel_token("admin1", &created.id).await.unwrap();
        let resolved = svc
            .resolve_channel_for_token(&created.id, &token.token)
            .await
            .unwrap()
            .expect("channel should still authenticate");
        assert_eq!(resolved.api_key, SECRET, "the stored credential must survive an edit");
    }

    /// `model_protocols` round-trips through create and update, and a listing
    /// carries it back for the client sync to materialise.
    #[tokio::test]
    async fn model_protocols_round_trip_through_upsert_and_listing() {
        let svc = service().await;
        let created = svc
            .upsert_provider_channel(
                None,
                "corp-newapi",
                "new-api",
                "https://gateway.corp.example",
                Some(SECRET),
                r#"["claude-sonnet-4","gpt-4o"]"#,
                None,
                Some(r#"{"claude-sonnet-4":"anthropic"}"#),
                true,
                "org",
                None,
                "all",
                "admin1",
            )
            .await
            .unwrap();
        assert_eq!(
            created.model_protocols.as_deref(),
            Some(r#"{"claude-sonnet-4":"anthropic"}"#)
        );

        let updated = svc
            .upsert_provider_channel(
                Some(&created.id),
                "corp-newapi",
                "new-api",
                "https://gateway.corp.example",
                None,
                r#"["claude-sonnet-4","gpt-4o"]"#,
                None,
                Some(r#"{"claude-sonnet-4":"anthropic","gpt-4o":"openai"}"#),
                true,
                "org",
                None,
                "all",
                "admin1",
            )
            .await
            .unwrap();
        assert_eq!(
            updated.model_protocols.as_deref(),
            Some(r#"{"claude-sonnet-4":"anthropic","gpt-4o":"openai"}"#)
        );

        let listed = svc.list_provider_channels("admin1").await.unwrap();
        assert_eq!(
            listed[0].model_protocols.as_deref(),
            Some(r#"{"claude-sonnet-4":"anthropic","gpt-4o":"openai"}"#)
        );
    }

    #[tokio::test]
    async fn a_token_authenticates_only_its_own_channel() {
        let svc = service().await;
        let a = make_channel(&svc, "channel-a").await;
        let b = make_channel(&svc, "channel-b").await;

        let token = svc.issue_channel_token("admin1", &a.id).await.unwrap();
        assert!(
            svc.resolve_channel_for_token(&a.id, &token.token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            svc.resolve_channel_for_token(&b.id, &token.token)
                .await
                .unwrap()
                .is_none(),
            "a token minted for one channel must not open another"
        );
    }

    #[tokio::test]
    async fn revoking_a_users_tokens_closes_the_channel_immediately() {
        let svc = service().await;
        let channel = make_channel(&svc, "corp-gateway").await;
        let token = svc.issue_channel_token("admin1", &channel.id).await.unwrap();

        assert_eq!(svc.revoke_channel_tokens_for_user("admin1").await.unwrap(), 1);
        assert!(
            svc.resolve_channel_for_token(&channel.id, &token.token)
                .await
                .unwrap()
                .is_none(),
            "a revoked token must stop working with no further sync"
        );
    }

    #[tokio::test]
    async fn a_disabled_channel_stops_serving() {
        let svc = service().await;
        let channel = make_channel(&svc, "corp-gateway").await;
        let token = svc.issue_channel_token("admin1", &channel.id).await.unwrap();

        svc.upsert_provider_channel(
            Some(&channel.id),
            "corp-gateway",
            "openai",
            "https://gateway.corp.example",
            None,
            r#"["gpt-image-2"]"#,
            None,
            None,
            false, // disabled
            "org",
            None,
            "all",
            "admin1",
        )
        .await
        .unwrap();

        assert!(
            svc.resolve_channel_for_token(&channel.id, &token.token)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Deleting must not leave a working key behind — including for a future
    /// row that happened to reuse the id.
    #[tokio::test]
    async fn deleting_a_channel_takes_its_tokens_with_it() {
        let svc = service().await;
        let channel = make_channel(&svc, "corp-gateway").await;
        let token = svc.issue_channel_token("admin1", &channel.id).await.unwrap();

        svc.delete_provider_channel("admin1", &channel.id).await.unwrap();

        let orphans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_provider_channel_tokens WHERE channel_id = ?")
            .bind(&channel.id)
            .fetch_one(svc.db.sqlite())
            .await
            .unwrap();
        assert_eq!(orphans, 0);
        assert!(
            svc.resolve_channel_for_token(&channel.id, &token.token)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Storing a credential in the clear because nobody wired the data key is
    /// worse than refusing the write.
    #[tokio::test]
    async fn a_channel_credential_is_never_stored_without_a_key() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone())).await.unwrap();
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT, tenant_id TEXT, role TEXT);")
            .execute(&pool)
            .await
            .unwrap();
        let svc = DevopsService::new(dream_core_db::DbPool::Sqlite(pool.clone())); // no encryption key

        let err = svc
            .upsert_provider_channel(
                None,
                "corp",
                "openai",
                "https://gateway.corp.example",
                Some(SECRET),
                "[]",
                None,
                None,
                true,
                "org",
                None,
                "all",
                "admin1",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DevopsError::Internal(_)), "got {err:?}");

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_provider_registry")
            .fetch_one(svc.db.sqlite())
            .await
            .unwrap();
        assert_eq!(
            rows, 0,
            "nothing may be written when the credential cannot be encrypted"
        );
    }

    #[tokio::test]
    async fn an_upstream_must_be_a_real_http_url() {
        let svc = service().await;
        for bad in ["", "  ", "gateway.corp.example", "ftp://gateway"] {
            assert!(
                svc.upsert_provider_channel(
                    None, "corp", "openai", bad, None, "[]", None, None, true, "org", None, "all", "admin1",
                )
                .await
                .is_err(),
                "should have rejected upstream {bad:?}"
            );
        }
    }

    /// Re-issuing replaces rather than accumulates: one row per member per
    /// channel, and the previous token stops working.
    #[tokio::test]
    async fn re_issuing_replaces_the_previous_token() {
        let svc = service().await;
        let channel = make_channel(&svc, "corp-gateway").await;
        let first = svc.issue_channel_token("admin1", &channel.id).await.unwrap();
        let second = svc.issue_channel_token("admin1", &channel.id).await.unwrap();
        assert_ne!(first.token, second.token);

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM one_provider_channel_tokens WHERE user_id = 'admin1'")
                .fetch_one(svc.db.sqlite())
                .await
                .unwrap();
        assert_eq!(count, 1);

        assert!(
            svc.resolve_channel_for_token(&channel.id, &first.token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            svc.resolve_channel_for_token(&channel.id, &second.token)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// The scope decision the user asked for: enterprise-wide by default,
    /// overridable per project group. Marketing gets the expensive image
    /// channel; engineering does not see it.
    #[tokio::test]
    async fn a_team_scoped_channel_is_only_visible_inside_that_project_group() {
        let svc = service().await;
        make_channel(&svc, "company-wide").await;
        svc.upsert_provider_channel(
            None,
            "group-a-only",
            "openai",
            "https://gateway.corp.example",
            Some(SECRET),
            "[]",
            None,
            None,
            true,
            "team",
            Some("tA"),
            "all",
            "admin1",
        )
        .await
        .unwrap();

        let seen = |rows: Vec<ProviderChannelDto>| -> Vec<String> { rows.into_iter().map(|c| c.name).collect() };

        let member_a = seen(svc.list_provider_channels("memberA").await.unwrap());
        assert!(member_a.contains(&"company-wide".to_owned()));
        assert!(member_a.contains(&"group-a-only".to_owned()));

        let member_b = seen(svc.list_provider_channels("memberB").await.unwrap());
        assert!(member_b.contains(&"company-wide".to_owned()));
        assert!(
            !member_b.contains(&"group-a-only".to_owned()),
            "another project group's channel must not be visible"
        );

        // …and visibility is the authorization, so B cannot mint a token for it
        // by guessing the id either.
        let channels = svc.list_provider_channels("admin1").await.unwrap();
        let group_a = channels.iter().find(|c| c.name == "group-a-only").unwrap();
        assert!(svc.issue_channel_token("memberB", &group_a.id).await.is_err());
        assert!(svc.issue_channel_token("memberA", &group_a.id).await.is_ok());
    }

    /// Only the hash is persisted, so a database read cannot yield a working
    /// token.
    #[tokio::test]
    async fn tokens_are_stored_hashed() {
        let svc = service().await;
        let channel = make_channel(&svc, "corp-gateway").await;
        let issued = svc.issue_channel_token("admin1", &channel.id).await.unwrap();

        let stored: String = sqlx::query_scalar("SELECT token_hash FROM one_provider_channel_tokens LIMIT 1")
            .fetch_one(svc.db.sqlite())
            .await
            .unwrap();
        assert_ne!(stored, issued.token);
        assert!(!stored.contains(&issued.token[TOKEN_PREFIX.len()..]));
    }
}
