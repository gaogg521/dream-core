//! SSO providers — Feishu / DingTalk / WeCom OAuth + LDAP bind.
//!
//! Each OAuth provider exposes the same shape:
//! - `build_authorize_url(config, redirect_uri, state) -> String`
//! - `exchange_code(config, code, redirect_uri) -> String` (access token)
//! - `fetch_user_info(token) -> ProviderUserInfo`
//! - `test_credentials(config) -> Result<()>`
//!
//! LDAP is password-based and lives in `ldap.rs`; it exposes
//! `authenticate(config, username, password) -> LdapAuthSuccess`.
//!
//! OIDC (`oidc.rs`) is the standard OpenID Connect authorization-code flow
//! (Okta / Azure AD / Google Workspace); it discovers its endpoints and adds a
//! `discover` step before `build_authorize_url` / `exchange_code`.

pub mod dingtalk;
pub mod feishu;
pub mod ldap;
pub mod oidc;
pub mod wecom;

pub use dingtalk::DingtalkProvider;
pub use feishu::FeishuProvider;
pub use ldap::LdapProvider;
pub use oidc::OidcProvider;
pub use wecom::WecomProvider;

/// First `max_chars` CHARACTERS of `id`, for the synthetic usernames the
/// providers fall back to when the IdP gives no display name
/// (`wecom_…` / `dingtalk_…` / `oidc_…`).
///
/// All three used to write `&id[..id.len().min(16)]`, which indexes a `&str`
/// by BYTES: an id longer than 16 bytes whose 16th byte lands mid-character
/// panics the login handler. Verified — a 12-character CJK id is 36 bytes and
/// byte 16 falls inside the 6th character. WeCom is the live case (an admin
/// can set a non-ASCII UserId), and the panic would be inside the OAuth
/// callback, so the member sees a dead browser tab with no explanation.
pub(crate) fn synthetic_username_suffix(id: &str, max_chars: usize) -> String {
    id.chars().take(max_chars).collect()
}

/// Normalized user info across OAuth providers.
#[derive(Debug, Clone)]
pub struct ProviderUserInfo {
    pub external_id: String,
    pub preferred_username: String,
    pub org_unit_path: Option<String>,
    /// Job title — only Feishu populates this today (via its Contact API,
    /// see `feishu::FeishuProvider::fetch_org_profile`); other providers
    /// leave it `None`.
    pub job_title: Option<String>,
    /// Company/organization identifier from the IdP (Feishu `tenant_key`,
    /// DingTalk `corp_id`, WeCom `corpid`). Used to bind/auto-join the SSO
    /// "real enterprise" tenant — same-company logins resolve to the same
    /// enterprise without an invite code. `None` for LDAP/local logins and
    /// providers that don't surface it yet.
    pub org_external_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_username_suffix_never_splits_a_character() {
        // The bug this replaces: `&id[..id.len().min(16)]` on a 12-character
        // CJK id (36 bytes) cuts inside the 6th character and panics — inside
        // the OAuth callback, so the member just gets a dead browser tab.
        let cjk = "张三李四王五赵六冯七陈八";
        assert_eq!(cjk.len(), 36, "precondition: longer than 16 BYTES");
        assert_eq!(synthetic_username_suffix(cjk, 16), cjk, "12 chars < 16, so all of it");

        let long_cjk = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未";
        assert_eq!(synthetic_username_suffix(&long_cjk, 16).chars().count(), 16);

        // ASCII behaviour is unchanged for the ids this actually sees today.
        assert_eq!(
            synthetic_username_suffix("on_a9837179f3d0061cada4", 16),
            "on_a9837179f3d00"
        );
        assert_eq!(synthetic_username_suffix("short", 16), "short");
        assert_eq!(synthetic_username_suffix("", 16), "");
    }

    #[test]
    fn dingtalk_does_not_put_the_mobile_number_in_the_department_path() {
        // `org_unit_path` renders as 部门路径 in the members roster, which
        // every member of the project group can read.
        let info = crate::providers::dingtalk::DingtalkUserInfo {
            union_id: Some("uid_1".into()),
            open_id: None,
            nick: Some("张三".into()),
            mobile: Some("13800138000".into()),
        };
        let profile = DingtalkProvider::to_provider_user_info(&info, "uid_1");
        assert_eq!(profile.org_unit_path, None, "the mobile number is not a department");
        assert_eq!(profile.preferred_username, "张三");
    }

    #[test]
    fn wecom_falls_back_to_a_synthetic_name_without_panicking_on_a_cjk_userid() {
        // WeCom UserId is admin-settable and legacy/imported tenants do carry
        // non-ASCII ones.
        let profile = WecomProvider::to_provider_user_info("甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未");
        assert!(profile.preferred_username.starts_with("wecom_"));
        assert_eq!(profile.org_unit_path, None);
    }
}
