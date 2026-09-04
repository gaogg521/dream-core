//! Project-group (one-org) auto-join hook for SSO login (P2-4 onboarding).
//!
//! Kept as a trait so one-sso does not take a hard dependency on one-org
//! (same-layer domain crates interact through traits only). The app layer
//! implements it over `dream_domain_org::OrgService::auto_join_by_email`. When no hook
//! is wired (personal edition, unit tests, or a login with no usable email)
//! SSO login behaves exactly as before — authenticate only.
//!
//! Deliberately SEPARATE from `EnterpriseSync`: this joins a `one_tenants`
//! project group by email-domain policy, never the SSO "real company".

use async_trait::async_trait;

#[async_trait]
pub trait OrgAutoJoin: Send + Sync {
    /// Called after a successful SSO login when the IdP profile yields a
    /// usable email address. Implementations look up any project group whose
    /// `allowed_email_domains` matches the email's domain and join the user to
    /// it (idempotent — already-a-member is a no-op).
    ///
    /// Best-effort and **must never fail the login**: implementations swallow
    /// their own errors.
    async fn auto_join_by_email(&self, user_id: &str, email: &str);

    /// Called after EVERY successful SSO login (all five providers, OAuth and
    /// LDAP alike), to put the user into a project group so the enterprise's
    /// capabilities and policies actually reach them.
    ///
    /// This exists because `auto_join_by_email` above never fired for the
    /// providers enterprises actually deploy: it keys off an '@' in the
    /// display name, and Feishu / DingTalk / WeCom / LDAP all surface a human
    /// name there. The result was that an SSO user authenticated against an
    /// enterprise server and landed with no `one_user_org` row at all — which
    /// `PlatformService::resolve_actor` reads as "not in an enterprise", so
    /// every tenant-scoped policy silently did not apply to them and no admin
    /// could see them in the roster.
    ///
    /// `personal_external_id` is the individual's own IdP id (Feishu
    /// `open_id`/`union_id`), matching `one_directory_people.external_id` —
    /// NOT the company's shared `tenant_key`. Implementations use it to find
    /// the person's department in the directory mirror and place them in the
    /// project group that mapped that branch, falling back to the deployment's
    /// root group when there is no match (LDAP, unmapped branch, no directory).
    ///
    /// Best-effort and **must never fail the login**: implementations swallow
    /// their own errors. Idempotent — a returning member is left untouched.
    async fn auto_join_after_sso(&self, user_id: &str, personal_external_id: &str, org_unit_path: Option<&str>);
}
