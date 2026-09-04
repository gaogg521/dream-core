//! 登录二次认证（MFA · TOTP）的判定与挑战生命周期。
//!
//! 判定矩阵（顺序即实现顺序，见 [`MfaService::decide`]）：
//!   全局档 off → 直接放行；豁免 → 放行；单用户强制 / 全局强制 / 已绑定 → 进第二步。
//!
//! 挑战 = 第一步通过后签发的一次性临时凭证：原文（32 字节随机）只在响应里
//! 出现一次，库里存 SHA-256 哈希；≤5 分钟有效；失败计数上限
//! [`MAX_ATTEMPTS`]，超限作废。绑定（enroll）挑战额外暂存待确认密钥的
//! AES-GCM 密文——验证通过一次后才落到 users 表。

use crate::totp;
use dream_core_common::{decrypt_string, encrypt_string};
use dream_core_db::models::User;
use dream_core_db::{
    AttemptBump, DbError, MfaAuditEntry, MfaAuditRow, MfaChallengePurpose, MfaChallengeRow, MfaMode,
    MfaStore, IUserRepository, MFA_MAX_ATTEMPTS,
};
use std::sync::Arc;

/// 挑战有效期：5 分钟。
pub const CHALLENGE_TTL_MS: i64 = 5 * 60 * 1000;
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
        Self { user_repo, store, encryption_key }
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
            let secret = totp::generate_secret();
            let cipher = encrypt_string(&secret, &self.encryption_key).map_err(|e| MfaError::Internal(e.to_string()))?;
            Some((secret, cipher))
        } else {
            None
        };
        let expires_at = self
            .store
            .challenge_create(
                &token_hash,
                &user.id,
                purpose,
                secret_cipher.as_ref().map(|(_, c)| c.as_str()),
                redirect_target,
                desktop,
                scheme,
                ip,
                CHALLENGE_TTL_MS,
            )
            .await?;
        self.audit(
            Some(&user.id),
            user.username.as_deref(),
            if purpose == MfaChallengePurpose::Enroll { "mfa_enroll_started" } else { "mfa_challenge_issued" },
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
    ) -> Result<Result<(String, String, bool), (String, i64)>, MfaError> {
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
                // 一次性消费：并发第二次 verify 抢不到 → 拒绝。
                if !self.store.challenge_consume(&token_hash).await? {
                    return Err(MfaError::NotFound);
                }
                let enrolled = challenge.purpose == MfaChallengePurpose::Enroll;
                if enrolled {
                    self.user_repo
                        .set_mfa_binding(&user.id, &cipher, dream_core_common::now_ms())
                        .await?;
                }
                self.user_repo.set_mfa_last_step(&user.id, step).await?;
                self.audit(
                    Some(&user.id),
                    user.username.as_deref(),
                    if enrolled { "mfa_bound" } else { "mfa_verify_success" },
                    None,
                    ip,
                )
                .await;
                Ok(Ok((user.id, user.username.unwrap_or_else(|| "external_user".into()), enrolled)))
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
                    self.audit(Some(&user.id), user.username.as_deref(), "mfa_challenge_invalidated", Some("attempts exhausted".to_string()), ip).await;
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
        let MfaChallengeRow { purpose, user_id, pending_secret_cipher, .. } = challenge;
        if purpose != MfaChallengePurpose::Enroll {
            return Err(MfaError::BadRequest("不是绑定挑战".into()));
        }
        let Some(cipher) = pending_secret_cipher else {
            return Err(MfaError::Internal("enroll challenge has no pending secret".into()));
        };
        let secret = decrypt_string(&cipher, &self.encryption_key).map_err(|e| MfaError::Internal(e.to_string()))?;
        let username = self
            .user_repo
            .find_by_id(&user_id)
            .await?
            .and_then(|u| u.username)
            .unwrap_or_else(|| "external_user".into());
        Ok((totp::otpauth_uri(OPTRAUTH_ISSUER_PLACEHOLDER, &username, &secret), secret))
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
        self.create_challenge(&user, purpose, ip, redirect_target, desktop, scheme).await
    }

    pub async fn admin_audit_list(&self, limit: i64) -> Result<Vec<MfaAuditRow>, MfaError> {
        Ok(self.store.audit_list(limit).await?)
    }

    // --- 管理端 ---

    pub async fn admin_policy_get(&self) -> Result<MfaMode, MfaError> {
        Ok(self.store.policy_mode().await?)
    }

    pub async fn admin_policy_set(&self, mode: MfaMode, operator: &str) -> Result<(), MfaError> {
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

    pub async fn admin_set_flags(&self, user_id: &str, exempt: bool, force: bool, operator: &str) -> Result<(), MfaError> {
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
        let _ = self
            .store
            .audit_insert(&MfaAuditEntry {
                user_id: user_id.map(str::to_owned),
                username: username.map(str::to_owned),
                action,
                detail,
                ip: ip.map(str::to_owned),
            })
            .await;
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

