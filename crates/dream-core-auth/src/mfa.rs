//! 登录二次认证（MFA · TOTP）的判定与挑战生命周期。
//!
//! 判定矩阵（顺序即实现顺序，见 [`MfaService::decide`]）：
//!   全局档 off → 直接放行；豁免 → 放行；单用户强制 / 全局强制 / 已绑定 → 进第二步。
//!
//! 挑战 = 第一步通过后签发的一次性临时凭证：原文（32 字节随机）只在响应里
//! 出现一次，库里存 SHA-256 哈希；输码挑战 5 分钟、绑定挑战 30 分钟有效；
//! 失败计数上限 [`MAX_ATTEMPTS`]，超限作废。绑定（enroll）挑战额外暂存待确认
//! 密钥的 AES-GCM 密文——验证通过一次后才落到 users 表。
//!
//! 绑定挑战重签时沿用 24 小时内最近一把待确认密钥：首次绑定要装验证器、扫码、
//! 输码，超时重登后若换新密钥，验证器里那把就永远对不上——表现和账号被锁死一样。

use crate::totp;
use dream_core_common::{decrypt_string, encrypt_string};
use dream_core_db::models::User;
use dream_core_db::{
    AttemptBump, DbError, IUserRepository, MFA_MAX_ATTEMPTS, MfaAuditEntry, MfaAuditRow, MfaChallengePurpose,
    MfaChallengeRow, MfaMode, MfaStore,
};
use std::sync::Arc;

/// 输码（login）挑战有效期：5 分钟。
pub const CHALLENGE_TTL_MS: i64 = 5 * 60 * 1000;
/// 绑定（enroll）挑战有效期：30 分钟——要装验证器、扫码、输码，5 分钟不够。
pub const ENROLL_CHALLENGE_TTL_MS: i64 = 30 * 60 * 1000;
/// 重签绑定挑战时可沿用的待确认密钥的最长年龄。
pub const ENROLL_SECRET_REUSE_MS: i64 = 24 * 60 * 60 * 1000;
/// 品牌名（otpauth URI 的 issuer）。
pub const OTPAUTH_ISSUER: &str = "One Work";

/// 第一步判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaDecision {
    /// 不需要第二步，按原流程签发登录态。
    Allow,
    /// 阻断，进入第二步。purpose = Login（已绑定输码）或 Enroll（强制绑定）。
    Challenge(MfaChallengePurpose),
}

/// 组装好的 MFA 服务：登录闸 + 挑战生命周期 + 管理端操作 + 审计。
/// `encryption_key` 与既有 API-key 加密同源（`derive_encryption_key(&data_secret_raw)`）。
pub struct MfaService {
    pub user_repo: Arc<dyn IUserRepository>,
    pub store: Arc<dyn MfaStore>,
    pub encryption_key: [u8; 32],
}

/// 管理端读到的单用户 MFA 状态。
#[derive(Debug, serde::Serialize)]
// camelCase: the admin console (the only consumer) reads `mfaExempt` /
// `userId`; snake_case left every per-user MFA action sending `undefined`.
#[serde(rename_all = "camelCase")]
pub struct MfaUserStatus {
    pub id: String,
    pub username: Option<String>,
    pub mfa_bound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfa_bound_at: Option<i64>,
    pub mfa_exempt: bool,
    pub mfa_force: bool,
}

#[derive(Debug)]
pub enum MfaError {
    BadRequest(String),
    /// 操作者本人尚未绑定，却要把全局策略设为强制——设了就把自己锁在门外。
    SelfNotEnrolled,
    /// 自助绑定：本人已绑定，不能再签发绑定挑战（要换设备请让管理员重置）。
    AlreadyEnrolled,
    NotFound,
    Unauthorized,
    Internal(String),
}

impl MfaError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }

    pub fn message(&self) -> String {
        match self {
            Self::BadRequest(m) => m.clone(),
            Self::SelfNotEnrolled => "请先为你自己的账号绑定 MFA，再开启强制模式——否则你下次登录也会被要求输码".into(),
            Self::AlreadyEnrolled => "你的账号已绑定 MFA".into(),
            Self::NotFound => "挑战不存在或已使用".into(),
            Self::Unauthorized => "动态码错误".into(),
            Self::Internal(m) => m.clone(),
        }
    }
}

impl From<DbError> for MfaError {
    fn from(e: DbError) -> Self {
        Self::Internal(format!("db: {e}"))
    }
}

impl MfaService {
    pub fn new(user_repo: Arc<dyn IUserRepository>, store: Arc<dyn MfaStore>, encryption_key: [u8; 32]) -> Self {
        Self {
            user_repo,
            store,
            encryption_key,
        }
    }

    /// 判定矩阵（严格按序）：全局关闭 → 放行；豁免 → 放行；单用户强制 → 阻断
    /// 绑定；已绑定 → 输码；全局强制 → 阻断绑定；全局可选未绑定 → 放行。
    pub async fn decide(&self, user: &User) -> Result<MfaDecision, MfaError> {
        let mode = self.store.policy_mode().await?;
        if mode == MfaMode::Off || user.mfa_exempt {
            return Ok(MfaDecision::Allow);
        }
        if user.mfa_enabled {
            return Ok(MfaDecision::Challenge(MfaChallengePurpose::Login));
        }
        if user.mfa_force || mode == MfaMode::Mandatory {
            return Ok(MfaDecision::Challenge(MfaChallengePurpose::Enroll));
        }
        Ok(MfaDecision::Allow)
    }

    /// 签发挑战：返回 (一次性原文 token, 过期时间戳, purpose)。
    /// enroll 挑战同时生成待确认密钥（密文入库，base32 原文经 enroll-info
    /// 端点一次性展示给本人——这是密钥唯一一次出现在响应里）。
    pub async fn create_challenge(
        &self,
        user: &User,
        purpose: MfaChallengePurpose,
        ip: Option<&str>,
        redirect_target: Option<&str>,
        desktop: bool,
        scheme: Option<&str>,
    ) -> Result<(String, i64, MfaChallengePurpose), MfaError> {
        let token = random_token();
        let token_hash = sha256_hex(token.as_bytes());
        let secret_cipher = if purpose == MfaChallengePurpose::Enroll {
            let since = dream_core_common::now_ms() - ENROLL_SECRET_REUSE_MS;
            match self.store.latest_pending_enroll_secret(&user.id, since).await? {
                Some(cipher) => Some(cipher),
                None => {
                    let secret = totp::generate_secret();
                    Some(encrypt_string(&secret, &self.encryption_key).map_err(|e| MfaError::Internal(e.to_string()))?)
                }
            }
        } else {
            None
        };
        let ttl_ms = if purpose == MfaChallengePurpose::Enroll {
            ENROLL_CHALLENGE_TTL_MS
        } else {
            CHALLENGE_TTL_MS
        };
        let expires_at = self
            .store
            .challenge_create(
                &token_hash,
                &user.id,
                user.session_generation,
                purpose,
                secret_cipher.as_deref(),
                redirect_target,
                desktop,
                scheme,
                ip,
                ttl_ms,
            )
            .await?;
        self.audit(
            Some(&user.id),
            user.username.as_deref(),
            if purpose == MfaChallengePurpose::Enroll {
                "mfa_enroll_started"
            } else {
                "mfa_challenge_issued"
            },
            None,
            ip,
        )
        .await;
        Ok((token, expires_at, purpose))
    }

    /// 第二步验证。成功返回 (user_id, username, 是否本次完成绑定)，
    /// 由路由层签发正式登录态；失败返回带剩余次数的错误。
    #[allow(clippy::too_many_arguments)]
    pub async fn verify(
        &self,
        mfa_token: &str,
        code: &str,
        ip: Option<&str>,
    ) -> Result<Result<(String, String, bool, i64), (String, i64)>, MfaError> {
        let token_hash = sha256_hex(mfa_token.as_bytes());
        let Some(challenge) = self.store.challenge_get(&token_hash).await? else {
            return Err(MfaError::NotFound);
        };
        if challenge.used || challenge.expires_at <= dream_core_common::now_ms() {
            return Err(MfaError::NotFound);
        }

        let Some(user) = self.user_repo.find_by_id(&challenge.user_id).await? else {
            return Err(MfaError::NotFound);
        };
        if user.status != dream_core_db::models::UserStatus::Active
            || user.session_generation != challenge.session_generation
            || (challenge.purpose == MfaChallengePurpose::Login && !user.mfa_enabled)
            || (challenge.purpose == MfaChallengePurpose::Enroll && user.mfa_enabled)
        {
            return Err(MfaError::NotFound);
        }

        // 解出待校验的密文：login 挑战用已绑定的密钥，enroll 挑战用暂存密钥。
        let secret_cipher = match challenge.purpose {
            MfaChallengePurpose::Login => user.mfa_secret_cipher.clone(),
            MfaChallengePurpose::Enroll => challenge.pending_secret_cipher.clone(),
        };
        let Some(cipher) = secret_cipher.filter(|c| !c.is_empty()) else {
            return Err(MfaError::Internal("challenge has no bound secret".into()));
        };
        let secret = decrypt_string(&cipher, &self.encryption_key).map_err(|e| MfaError::Internal(e.to_string()))?;

        match totp::verify_with_window(&secret, code, user.mfa_last_step, dream_core_common::now_ms()) {
            Some(step) => {
                // Commit consumption, binding and the user's replay counter
                // atomically; different challenges cannot reuse the same step.
                if !self.store.challenge_complete(&challenge, step, &cipher).await? {
                    return Err(MfaError::NotFound);
                }
                let enrolled = challenge.purpose == MfaChallengePurpose::Enroll;
                self.audit(
                    Some(&user.id),
                    user.username.as_deref(),
                    if enrolled { "mfa_bound" } else { "mfa_verify_success" },
                    None,
                    ip,
                )
                .await;
                Ok(Ok((
                    user.id,
                    user.username.unwrap_or_else(|| "external_user".into()),
                    enrolled,
                    challenge.session_generation,
                )))
            }
            None => {
                let AttemptBump { attempts, invalidated } = self.store.challenge_fail(&token_hash).await?;
                self.audit(
                    Some(&user.id),
                    user.username.as_deref(),
                    "mfa_verify_failed",
                    Some(format!("attempt {attempts}")),
                    ip,
                )
                .await;
                let attempts_left = (MFA_MAX_ATTEMPTS - attempts).max(0);
                if invalidated {
                    self.audit(
                        Some(&user.id),
                        user.username.as_deref(),
                        "mfa_challenge_invalidated",
                        Some("attempts exhausted".to_string()),
                        ip,
                    )
                    .await;
                    return Err(MfaError::NotFound);
                }
                Ok(Err(("动态码错误".into(), attempts_left)))
            }
        }
    }

    /// enroll 挑战的绑定信息（otpauth URI + 手工密钥）。只对 purpose=enroll、
    /// 未使用、未过期的挑战开放；这是密钥唯一一次出现在响应里，不落日志。
    pub async fn enroll_info(&self, mfa_token: &str) -> Result<(String, String), MfaError> {
        let token_hash = sha256_hex(mfa_token.as_bytes());
        let Some(challenge) = self.store.challenge_get(&token_hash).await? else {
            return Err(MfaError::NotFound);
        };
        if challenge.used || challenge.expires_at <= dream_core_common::now_ms() {
            return Err(MfaError::NotFound);
        }
        let user = self
            .user_repo
            .find_by_id(&challenge.user_id)
            .await?
            .ok_or(MfaError::NotFound)?;
        if user.status != dream_core_db::models::UserStatus::Active
            || user.session_generation != challenge.session_generation
            || user.mfa_enabled
        {
            return Err(MfaError::NotFound);
        }
        let MfaChallengeRow {
            purpose,
            pending_secret_cipher,
            ..
        } = challenge;
        if purpose != MfaChallengePurpose::Enroll {
            return Err(MfaError::BadRequest("不是绑定挑战".into()));
        }
        let Some(cipher) = pending_secret_cipher else {
            return Err(MfaError::Internal("enroll challenge has no pending secret".into()));
        };
        let secret = decrypt_string(&cipher, &self.encryption_key).map_err(|e| MfaError::Internal(e.to_string()))?;
        let username = user.username.unwrap_or_else(|| "external_user".into());
        Ok((
            totp::otpauth_uri(OPTRAUTH_ISSUER_PLACEHOLDER, &username, &secret),
            secret,
        ))
    }

    /// 供 SSO 回调 / LDAP 使用的闸入口：按 user_id 取用户后走同一判定矩阵。
    pub async fn decide_for_user(&self, user_id: &str, username: &str) -> Result<MfaDecision, MfaError> {
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| MfaError::Internal("mfa gate: user missing".into()))?;
        let _ = username;
        self.decide(&user).await
    }

    /// 供 SSO 回调 / LDAP 使用的挑战签发入口。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_challenge_for_user(
        &self,
        user_id: &str,
        username: &str,
        purpose: MfaChallengePurpose,
        ip: Option<&str>,
        redirect_target: Option<&str>,
        desktop: bool,
        scheme: Option<&str>,
    ) -> Result<(String, i64, MfaChallengePurpose), MfaError> {
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| MfaError::Internal("mfa gate: user missing".into()))?;
        let _ = username;
        self.create_challenge(&user, purpose, ip, redirect_target, desktop, scheme)
            .await
    }

    /// 本人是否已绑定（控制台据此决定开强制前要不要先走自助绑定）。
    pub async fn self_status(&self, user_id: &str) -> Result<bool, MfaError> {
        Ok(self.user_repo.find_by_id(user_id).await?.is_some_and(|u| u.mfa_enabled))
    }

    /// 已登录用户自助绑定：签发一张绑定挑战，之后与登录时的强制绑定走同一条
    /// enroll-info → verify 路径。已绑定则拒绝——换设备要走管理员重置。
    pub async fn self_enroll_start(&self, user_id: &str, ip: Option<&str>) -> Result<(String, i64), MfaError> {
        let user = self.user_repo.find_by_id(user_id).await?.ok_or(MfaError::NotFound)?;
        if user.mfa_enabled {
            return Err(MfaError::AlreadyEnrolled);
        }
        let (token, expires_at, _) = self
            .create_challenge(&user, MfaChallengePurpose::Enroll, ip, None, false, None)
            .await?;
        Ok((token, expires_at))
    }

    pub async fn admin_audit_list(&self, limit: i64) -> Result<Vec<MfaAuditRow>, MfaError> {
        Ok(self.store.audit_list(limit).await?)
    }

    // --- 管理端 ---

    pub async fn admin_policy_get(&self) -> Result<MfaMode, MfaError> {
        Ok(self.store.policy_mode().await?)
    }

    /// 设全局档。切到强制前校验操作者本人已绑定（或被豁免）：策略全局生效，
    /// 没有这道校验，开强制的那一刻就把自己变成了下一个登录被拦的人。
    pub async fn admin_policy_set(&self, mode: MfaMode, operator: &str) -> Result<(), MfaError> {
        if mode == MfaMode::Mandatory && self.store.policy_mode().await? != MfaMode::Mandatory {
            let operator_user = self.user_repo.find_by_id(operator).await?;
            let protected = operator_user.as_ref().is_some_and(|u| u.mfa_enabled || u.mfa_exempt);
            if !protected {
                return Err(MfaError::SelfNotEnrolled);
            }
        }
        self.store.policy_set(mode, operator).await?;
        self.audit(
            Some(operator),
            Some(operator),
            "mfa_policy_changed",
            Some(mode.as_str().to_string()),
            None,
        )
        .await;
        Ok(())
    }

    /// 用户管理的 MFA 状态清单（不翻页——企业成员量级 × 6 列，够用）。
    pub async fn admin_users_overview(&self) -> Result<Vec<MfaUserStatus>, MfaError> {
        let rows = self.store.list_users_mfa_status().await?;
        Ok(rows
            .into_iter()
            .map(|(id, username, enabled, bound_at, exempt, force)| MfaUserStatus {
                id,
                username,
                mfa_bound: enabled != 0,
                mfa_bound_at: bound_at,
                mfa_exempt: exempt != 0,
                mfa_force: force != 0,
            })
            .collect())
    }

    pub async fn admin_reset(&self, user_id: &str, operator: &str, reason: &str) -> Result<(), MfaError> {
        self.user_repo.clear_mfa_binding(user_id).await?;
        self.store.clear_pending_enroll_secrets(user_id).await?;
        self.audit(
            Some(user_id),
            None,
            "mfa_reset",
            Some(format!("by {operator}: {reason}")),
            None,
        )
        .await;
        Ok(())
    }

    pub async fn admin_set_flags(
        &self,
        user_id: &str,
        exempt: bool,
        force: bool,
        operator: &str,
    ) -> Result<(), MfaError> {
        self.user_repo.set_mfa_flags(user_id, exempt, force).await?;
        self.audit(
            Some(user_id),
            None,
            "mfa_flags_changed",
            Some(format!("by {operator}: exempt={exempt} force={force}")),
            None,
        )
        .await;
        Ok(())
    }

    #[allow(clippy::needless_pass_by_value)]
    async fn audit(
        &self,
        user_id: Option<&str>,
        username: Option<&str>,
        action: &'static str,
        detail: Option<String>,
        ip: Option<&str>,
    ) {
        // Best-effort (an audit hiccup must not fail a login), but never
        // silent: the insert SQL was malformed for months and every MFA audit
        // row was dropped without a single log line.
        if let Err(e) = self
            .store
            .audit_insert(&MfaAuditEntry {
                user_id: user_id.map(str::to_owned),
                username: username.map(str::to_owned),
                action,
                detail,
                ip: ip.map(str::to_owned),
            })
            .await
        {
            tracing::warn!(action, error = %e, "failed to record MFA audit event");
        }
    }
}

/// 32 字节随机 → 64 位 hex（挑战原文 token）。
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("OS RNG unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 挑战只存哈希——库泄露也拿不到可用于第二步的原文。
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// otpauth issuer 的占位（避免模块间命名抖动；实际值见 totp.rs 调用方常量）。
const OPTRAUTH_ISSUER_PLACEHOLDER: &str = "One Work";

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::{SqliteMfaStore, SqliteUserRepository};

    const KEY: [u8; 32] = [9u8; 32];

    async fn service() -> (MfaService, Arc<dyn IUserRepository>) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let users: Arc<dyn IUserRepository> = Arc::new(SqliteUserRepository::new(db.pool().clone()));
        let store: Arc<dyn MfaStore> = Arc::new(SqliteMfaStore::new(db.pool().clone()));
        (MfaService::new(users.clone(), store, KEY), users)
    }

    async fn user(users: &Arc<dyn IUserRepository>, name: &str) -> User {
        users.create_user(name, "hash").await.unwrap()
    }

    async fn enroll_secret(svc: &MfaService, token: &str) -> String {
        svc.enroll_info(token).await.unwrap().1
    }

    /// Finding 18 (D1): switching to mandatory while not enrolled is refused
    /// — it is how the reporting deployment's admin locked themselves out.
    #[tokio::test]
    async fn mandatory_policy_requires_the_operator_to_be_enrolled_first() {
        let (svc, users) = service().await;
        let admin = user(&users, "ops_admin").await;

        let err = svc.admin_policy_set(MfaMode::Mandatory, &admin.id).await.unwrap_err();
        assert!(matches!(err, MfaError::SelfNotEnrolled));
        assert_eq!(svc.admin_policy_get().await.unwrap(), MfaMode::Off);

        // Optional is always allowed, and an enrolled operator may go mandatory.
        svc.admin_policy_set(MfaMode::Optional, &admin.id).await.unwrap();
        users.set_mfa_binding(&admin.id, "cipher", 1).await.unwrap();
        svc.admin_policy_set(MfaMode::Mandatory, &admin.id).await.unwrap();
        assert_eq!(svc.admin_policy_get().await.unwrap(), MfaMode::Mandatory);
    }

    #[tokio::test]
    async fn mfa_events_land_in_the_audit_log() {
        let (svc, users) = service().await;
        let u = user(&users, "dave").await;
        svc.create_challenge(&u, MfaChallengePurpose::Enroll, Some("10.0.0.1"), None, false, None)
            .await
            .unwrap();
        svc.admin_reset(&u.id, "ops_admin", "lost phone").await.unwrap();
        let actions: Vec<String> = svc
            .admin_audit_list(10)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.action)
            .collect();
        assert!(actions.contains(&"mfa_enroll_started".to_string()), "{actions:?}");
        assert!(actions.contains(&"mfa_reset".to_string()), "{actions:?}");
    }

    #[tokio::test]
    async fn an_exempt_operator_may_enable_mandatory() {
        let (svc, users) = service().await;
        let admin = user(&users, "ops_admin").await;
        users.set_mfa_flags(&admin.id, true, false).await.unwrap();
        svc.admin_policy_set(MfaMode::Mandatory, &admin.id).await.unwrap();
    }

    /// Finding 19 (D2): a re-issued enroll challenge keeps the secret the
    /// user's authenticator already holds, and lives long enough to finish.
    #[tokio::test]
    async fn reissued_enroll_challenge_keeps_the_pending_secret() {
        let (svc, users) = service().await;
        let u = user(&users, "alice").await;

        let (t1, exp1, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Enroll, None, None, false, None)
            .await
            .unwrap();
        assert!(exp1 - dream_core_common::now_ms() > CHALLENGE_TTL_MS);
        let (t2, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Enroll, None, None, false, None)
            .await
            .unwrap();
        assert_ne!(t1, t2);
        let secret = enroll_secret(&svc, &t1).await;
        assert_eq!(secret, enroll_secret(&svc, &t2).await);

        // A code from that one secret completes enrollment via the newer challenge.
        let code = totp::totp_code(&secret, dream_core_common::now_ms()).unwrap();
        let (_, _, enrolled, _) = svc.verify(&t2, &code, None).await.unwrap().unwrap();
        assert!(enrolled);
    }

    /// After an admin reset the lost device's secret must not come back.
    #[tokio::test]
    async fn admin_reset_forgets_pending_secrets() {
        let (svc, users) = service().await;
        let u = user(&users, "bob").await;
        let (t1, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Enroll, None, None, false, None)
            .await
            .unwrap();
        let old = enroll_secret(&svc, &t1).await;

        svc.admin_reset(&u.id, "admin", "lost phone").await.unwrap();
        let u = users.find_by_id(&u.id).await.unwrap().unwrap();
        let (t2, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Enroll, None, None, false, None)
            .await
            .unwrap();
        assert_ne!(old, enroll_secret(&svc, &t2).await);
    }

    #[tokio::test]
    async fn self_enroll_issues_a_challenge_only_while_unbound() {
        let (svc, users) = service().await;
        let u = user(&users, "carol").await;
        assert!(!svc.self_status(&u.id).await.unwrap());

        let (token, _) = svc.self_enroll_start(&u.id, None).await.unwrap();
        let secret = enroll_secret(&svc, &token).await;
        let code = totp::totp_code(&secret, dream_core_common::now_ms()).unwrap();
        svc.verify(&token, &code, None).await.unwrap().unwrap();

        assert!(svc.self_status(&u.id).await.unwrap());
        assert!(matches!(
            svc.self_enroll_start(&u.id, None).await.unwrap_err(),
            MfaError::AlreadyEnrolled
        ));
    }

    #[tokio::test]
    async fn revoked_generation_invalidates_enrollment_and_login_challenges() {
        let (svc, users) = service().await;
        let u = user(&users, "revoked").await;
        let (token, _) = svc.self_enroll_start(&u.id, None).await.unwrap();
        let secret = enroll_secret(&svc, &token).await;
        users.increment_session_generation(&u.id).await.unwrap();
        let code = totp::totp_code(&secret, dream_core_common::now_ms()).unwrap();
        assert!(matches!(svc.enroll_info(&token).await, Err(MfaError::NotFound)));
        assert!(matches!(svc.verify(&token, &code, None).await, Err(MfaError::NotFound)));
        // A stale first-factor snapshot cannot issue a usable new challenge.
        let (stale, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Enroll, None, None, false, None)
            .await
            .unwrap();
        assert!(matches!(svc.enroll_info(&stale).await, Err(MfaError::NotFound)));
        let cipher = encrypt_string(&secret, &KEY).unwrap();
        users.set_mfa_binding(&u.id, &cipher, 1).await.unwrap();
        let current = users.find_by_id(&u.id).await.unwrap().unwrap();
        let (login, _, _) = svc
            .create_challenge(&current, MfaChallengePurpose::Login, None, None, false, None)
            .await
            .unwrap();
        users.increment_session_generation(&u.id).await.unwrap();
        assert!(matches!(svc.verify(&login, &code, None).await, Err(MfaError::NotFound)));
    }

    #[tokio::test]
    async fn disabled_user_and_reset_binding_cannot_complete_old_challenges() {
        let (svc, users) = service().await;
        let u = user(&users, "disabled").await;
        let (token, _) = svc.self_enroll_start(&u.id, None).await.unwrap();
        let secret = enroll_secret(&svc, &token).await;
        let code = totp::totp_code(&secret, dream_core_common::now_ms()).unwrap();
        users
            .set_status(&u.id, dream_core_db::models::UserStatus::Disabled)
            .await
            .unwrap();
        assert!(matches!(svc.enroll_info(&token).await, Err(MfaError::NotFound)));
        assert!(matches!(svc.verify(&token, &code, None).await, Err(MfaError::NotFound)));
        users
            .set_status(&u.id, dream_core_db::models::UserStatus::Active)
            .await
            .unwrap();
        svc.admin_reset(&u.id, "operator", "test reset").await.unwrap();
        assert!(matches!(svc.verify(&token, &code, None).await, Err(MfaError::NotFound)));
    }

    #[tokio::test]
    async fn different_challenges_cannot_replay_the_same_totp_step() {
        let (svc, users) = service().await;
        let u = user(&users, "parallel-login").await;
        let secret = totp::generate_secret();
        users
            .set_mfa_binding(&u.id, &encrypt_string(&secret, &KEY).unwrap(), 1)
            .await
            .unwrap();
        let u = users.find_by_id(&u.id).await.unwrap().unwrap();
        let (a, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Login, None, None, false, None)
            .await
            .unwrap();
        let (b, _, _) = svc
            .create_challenge(&u, MfaChallengePurpose::Login, None, None, false, None)
            .await
            .unwrap();
        let code = totp::totp_code(&secret, dream_core_common::now_ms()).unwrap();
        let (ra, rb) = tokio::join!(svc.verify(&a, &code, None), svc.verify(&b, &code, None));
        assert_eq!([&ra, &rb].iter().filter(|r| matches!(r, Ok(Ok(_)))).count(), 1);
        let recorded = users.find_by_id(&u.id).await.unwrap().unwrap().mfa_last_step.unwrap();
        assert!(totp::verify_with_window(&secret, &code, Some(recorded), dream_core_common::now_ms()).is_none());
    }

    #[tokio::test]
    async fn concurrent_failures_exhaust_exactly_five_attempts() {
        let (svc, users) = service().await;
        let u = user(&users, "parallel-failures").await;
        let (token, _) = svc.self_enroll_start(&u.id, None).await.unwrap();
        let hash = sha256_hex(token.as_bytes());
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let store = svc.store.clone();
            let hash = hash.clone();
            tasks.push(tokio::spawn(async move { store.challenge_fail(&hash).await.unwrap() }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let challenge = svc.store.challenge_get(&hash).await.unwrap().unwrap();
        assert_eq!(challenge.attempts, MFA_MAX_ATTEMPTS);
        assert!(challenge.used);
    }
}
