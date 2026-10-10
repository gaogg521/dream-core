//! MFA second-factor storage: global policy, one-time challenges and the
//! MFA audit trail (migration 055).
//!
//! Challenge tokens are stored **hashed** (SHA-256, hex) — the raw token only
//! ever lives in the login response for the ≤5 minute challenge window.

use crate::error::DbError;
use dream_core_common::now_ms;

/// Global MFA mode (admin-configured, instant effect).
///
/// Default when no row exists: `Off` — an upgraded deployment keeps its
/// current login behaviour until an admin explicitly turns MFA on. This is
/// the deliberate call: defaulting to `Mandatory` would force every existing
/// member through enrollment on their next login after upgrading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaMode {
    Off,
    Optional,
    Mandatory,
}

impl MfaMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Optional => "optional",
            Self::Mandatory => "mandatory",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "optional" => Self::Optional,
            "mandatory" => Self::Mandatory,
            _ => Self::Off,
        }
    }
}

/// What the second step asks of the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaChallengePurpose {
    /// Verify against the stored binding.
    Login,
    /// Verify against the pending secret, then persist the binding.
    Enroll,
}

impl MfaChallengePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Enroll => "enroll",
        }
    }
}

/// One issued challenge (fetched by token hash).
#[derive(Debug, Clone)]
pub struct MfaChallengeRow {
    pub token_hash: String,
    pub user_id: String,
    pub session_generation: i64,
    pub purpose: MfaChallengePurpose,
    pub attempts: i64,
    pub expires_at: i64,
    pub used: bool,
    /// AES-GCM ciphertext of the pending TOTP secret (enroll challenges only).
    pub pending_secret_cipher: Option<String>,
    /// Post-login redirect target the first step captured, if any.
    pub redirect_target: Option<String>,
    pub desktop: bool,
    pub scheme: Option<String>,
}

/// One audit-trail row.
#[derive(Debug, Clone)]
pub struct MfaAuditEntry {
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub action: &'static str,
    pub detail: Option<String>,
    pub ip: Option<String>,
}

/// Result of bumping the attempt counter on a failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptBump {
    pub attempts: i64,
    pub invalidated: bool,
}

pub const MFA_MAX_ATTEMPTS: i64 = 5;

/// Storage contract for MFA policy / challenges / audit. Implemented over the
/// primary SQLite pool (`users` lives there; see `repository/user.rs`).
#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)]
pub trait MfaStore: Send + Sync {
    async fn policy_mode(&self) -> Result<MfaMode, DbError>;
    async fn policy_set(&self, mode: MfaMode, updated_by: &str) -> Result<(), DbError>;

    async fn challenge_create(
        &self,
        token_hash: &str,
        user_id: &str,
        session_generation: i64,
        purpose: MfaChallengePurpose,
        pending_secret_cipher: Option<&str>,
        redirect_target: Option<&str>,
        desktop: bool,
        scheme: Option<&str>,
        ip: Option<&str>,
        ttl_ms: i64,
    ) -> Result<i64, DbError>;

    /// Returns the challenge if it exists and has not been consumed.
    async fn challenge_get(&self, token_hash: &str) -> Result<Option<MfaChallengeRow>, DbError>;

    /// Bumps the failure counter; flips `used` once the limit is reached.
    async fn challenge_fail(&self, token_hash: &str) -> Result<AttemptBump, DbError>;

    /// One-time consume: marks the challenge used, returns true when it was
    /// still unused (a concurrent verify loses the race).
    async fn challenge_complete(
        &self,
        challenge: &MfaChallengeRow,
        step: i64,
        secret_cipher: &str,
    ) -> Result<bool, DbError>;

    async fn challenge_save_pending_secret(&self, token_hash: &str, secret_cipher: &str) -> Result<(), DbError>;

    /// Most recent not-yet-bound enroll secret issued to this user at or after
    /// `since_ms`, expired challenges included. Lets a fresh enroll challenge
    /// keep the secret the user's authenticator already scanned instead of
    /// minting a new one that silently invalidates it.
    async fn latest_pending_enroll_secret(&self, user_id: &str, since_ms: i64) -> Result<Option<String>, DbError>;

    /// Forget every pending enroll secret of this user. Called once a secret
    /// is bound (it must not be handed out again as "pending") and on an admin
    /// reset (the lost device's secret must not come back on re-enrollment).
    async fn clear_pending_enroll_secrets(&self, user_id: &str) -> Result<(), DbError>;

    /// 用户管理的 MFA 状态清单：(id, username, enabled, bound_at, exempt, force)。
    async fn list_users_mfa_status(&self)
    -> Result<Vec<(String, Option<String>, i64, Option<i64>, i64, i64)>, DbError>;

    async fn audit_insert(&self, entry: &MfaAuditEntry) -> Result<(), DbError>;
    async fn audit_list(&self, limit: i64) -> Result<Vec<MfaAuditRow>, DbError>;
}

/// Audit row as returned by [`MfaStore::audit_list`].
#[derive(Debug, Clone, serde::Serialize)]
// camelCase: the admin console (the only consumer) reads `mfaExempt` /
// `userId`; snake_case left every per-user MFA action sending `undefined`.
#[serde(rename_all = "camelCase")]
pub struct MfaAuditRow {
    pub ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
}

/// SQLite implementation over the primary pool.
pub struct SqliteMfaStore {
    pool: sqlx::SqlitePool,
}

impl SqliteMfaStore {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MfaStore for SqliteMfaStore {
    async fn policy_mode(&self) -> Result<MfaMode, DbError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT mode FROM mfa_policy WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(m,)| MfaMode::parse(&m)).unwrap_or(MfaMode::Off))
    }

    async fn policy_set(&self, mode: MfaMode, updated_by: &str) -> Result<(), DbError> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO mfa_policy (id, mode, updated_by, updated_at) VALUES (1, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET mode = excluded.mode, updated_by = excluded.updated_by, \
             updated_at = excluded.updated_at",
        )
        .bind(mode.as_str())
        .bind(updated_by)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn challenge_create(
        &self,
        token_hash: &str,
        user_id: &str,
        session_generation: i64,
        purpose: MfaChallengePurpose,
        pending_secret_cipher: Option<&str>,
        redirect_target: Option<&str>,
        desktop: bool,
        scheme: Option<&str>,
        ip: Option<&str>,
        ttl_ms: i64,
    ) -> Result<i64, DbError> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO mfa_challenges (token_hash, user_id, purpose, attempts, expires_at, used, \
             pending_secret_cipher, redirect_target, desktop, scheme, created_ip, created_at, session_generation) \
             VALUES (?, ?, ?, 0, ?, 0, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(purpose.as_str())
        .bind(now + ttl_ms)
        .bind(pending_secret_cipher)
        .bind(redirect_target)
        .bind(desktop)
        .bind(scheme)
        .bind(ip)
        .bind(now)
        .bind(session_generation)
        .execute(&self.pool)
        .await?;
        Ok(now + ttl_ms)
    }

    async fn challenge_get(&self, token_hash: &str) -> Result<Option<MfaChallengeRow>, DbError> {
        let row: Option<(
            String,
            String,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
            i64,
            Option<String>,
            i64,
        )> = sqlx::query_as(
            "SELECT token_hash, user_id, purpose, attempts, expires_at, used, \
             pending_secret_cipher, redirect_target, desktop, scheme, session_generation \
             FROM mfa_challenges WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(
                token_hash,
                user_id,
                purpose,
                attempts,
                expires_at,
                used,
                pending_secret_cipher,
                redirect_target,
                desktop,
                scheme,
                session_generation,
            )| MfaChallengeRow {
                token_hash,
                user_id,
                session_generation,
                purpose: match purpose.as_str() {
                    "enroll" => MfaChallengePurpose::Enroll,
                    _ => MfaChallengePurpose::Login,
                },
                attempts,
                expires_at,
                used: used != 0,
                pending_secret_cipher,
                redirect_target,
                desktop: desktop != 0,
                scheme,
            },
        ))
    }

    async fn challenge_fail(&self, token_hash: &str) -> Result<AttemptBump, DbError> {
        // Increment and invalidate in one statement: concurrent failures must
        // not read the same counter and silently grant extra guesses.
        let row: Option<(i64, bool)> = sqlx::query_as(
            "UPDATE mfa_challenges SET attempts = attempts + 1, used = (attempts + 1 >= ?) \
             WHERE token_hash = ? AND used = 0 AND expires_at > ? RETURNING attempts, used",
        )
        .bind(MFA_MAX_ATTEMPTS)
        .bind(token_hash)
        .bind(now_ms())
        .fetch_optional(&self.pool)
        .await?;
        let Some((attempts, invalidated)) = row else {
            return Ok(AttemptBump {
                attempts: 0,
                invalidated: true,
            });
        };
        Ok(AttemptBump { attempts, invalidated })
    }

    async fn challenge_complete(
        &self,
        challenge: &MfaChallengeRow,
        step: i64,
        secret_cipher: &str,
    ) -> Result<bool, DbError> {
        let mut tx = self.pool.begin().await?;
        // Take the write lock before checking the user. The challenge and the
        // user's replay counter must commit together across all login attempts.
        let consumed = sqlx::query(
            "UPDATE mfa_challenges SET used = 1 WHERE token_hash = ? AND used = 0 AND expires_at > ? \
             AND session_generation = ?",
        )
        .bind(&challenge.token_hash)
        .bind(now_ms())
        .bind(challenge.session_generation)
        .execute(&mut *tx)
        .await?;
        if consumed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        let enrolled = challenge.purpose == MfaChallengePurpose::Enroll;
        let update = if enrolled {
            "UPDATE users SET mfa_secret_cipher = ?, mfa_enabled = 1, mfa_bound_at = ?, \
             mfa_last_step = ?, updated_at = ? WHERE id = ? AND session_generation = ? AND status = 'active' \
             AND mfa_enabled = 0 AND (mfa_last_step IS NULL OR mfa_last_step < ?)"
        } else {
            "UPDATE users SET mfa_last_step = ?, updated_at = ? WHERE id = ? AND session_generation = ? \
             AND status = 'active' AND mfa_enabled = 1 AND mfa_secret_cipher = ? \
             AND (mfa_last_step IS NULL OR mfa_last_step < ?)"
        };
        let mut query = sqlx::query(update);
        if enrolled {
            query = query.bind(secret_cipher).bind(now_ms());
        }
        query = query
            .bind(step)
            .bind(now_ms())
            .bind(&challenge.user_id)
            .bind(challenge.session_generation);
        if !enrolled {
            query = query.bind(secret_cipher);
        }
        let changed = query.bind(step).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        if enrolled {
            sqlx::query("UPDATE mfa_challenges SET used = 1, pending_secret_cipher = NULL WHERE user_id = ? AND purpose = 'enroll'")
                .bind(&challenge.user_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn challenge_save_pending_secret(&self, token_hash: &str, secret_cipher: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE mfa_challenges SET pending_secret_cipher = ? WHERE token_hash = ?")
            .bind(secret_cipher)
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn latest_pending_enroll_secret(&self, user_id: &str, since_ms: i64) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT pending_secret_cipher FROM mfa_challenges \
             WHERE user_id = ? AND purpose = ? AND pending_secret_cipher IS NOT NULL \
             AND pending_secret_cipher != '' AND created_at >= ? \
             AND session_generation = (SELECT session_generation FROM users WHERE id = user_id AND status = 'active') \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(MfaChallengePurpose::Enroll.as_str())
        .bind(since_ms)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(c,)| c))
    }

    async fn clear_pending_enroll_secrets(&self, user_id: &str) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE mfa_challenges SET used = 1, pending_secret_cipher = NULL WHERE user_id = ? AND purpose = ?",
        )
        .bind(user_id)
        .bind(MfaChallengePurpose::Enroll.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_users_mfa_status(
        &self,
    ) -> Result<Vec<(String, Option<String>, i64, Option<i64>, i64, i64)>, DbError> {
        let rows: Vec<(String, Option<String>, i64, Option<i64>, i64, i64)> = sqlx::query_as(
            "SELECT id, username, mfa_enabled, mfa_bound_at, mfa_exempt, mfa_force              FROM users WHERE user_type = 'local' ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn audit_insert(&self, entry: &MfaAuditEntry) -> Result<(), DbError> {
        sqlx::query("INSERT INTO mfa_audit (ts, user_id, username, action, detail, ip) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(now_ms())
            .bind(entry.user_id.as_deref())
            .bind(entry.username.as_deref())
            .bind(entry.action)
            .bind(entry.detail.as_deref())
            .bind(entry.ip.as_deref())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn audit_list(&self, limit: i64) -> Result<Vec<MfaAuditRow>, DbError> {
        let rows: Vec<(
            i64,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        )> = sqlx::query_as("SELECT ts, user_id, username, action, detail, ip FROM mfa_audit ORDER BY ts DESC LIMIT ?")
            .bind(limit.clamp(1, 500))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(ts, user_id, username, action, detail, ip)| MfaAuditRow {
                ts,
                user_id,
                username,
                action,
                detail,
                ip,
            })
            .collect())
    }
}
