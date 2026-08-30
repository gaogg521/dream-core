//! Top-level router assembly: middleware stack + module route merges.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
#[cfg(feature = "enterprise")]
use axum::extract::State;
use axum::http::{HeaderName, Method, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, middleware};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use dream_core_ai_agent::{
    RuntimeTokenScope, RuntimeTokenService, TEAM_RUNTIME_TOKEN_SESSION_GENERATION, agent_routes, remote_agent_routes,
};
use dream_core_api_types::{ErrorResponse, WebSocketMessage};
use dream_core_assets::{AssetRouterState, asset_routes};
use dream_core_assistant::assistant_routes;
#[cfg(feature = "enterprise")]
use dream_core_auth::CurrentUser;
use dream_core_auth::{
    AuthIdentityMode, AuthRouterState, AuthState, IRuntimeTokenVerifier, SystemDefaultFilesystemAdopter,
    auth_middleware, auth_routes, csrf_middleware, security_headers_middleware,
};
use dream_core_channel::channel_routes;
#[cfg(feature = "weixin")]
use dream_core_channel::weixin_login_route;
use dream_core_claude_bridge::claude_bridge_config_routes;
use dream_core_codex_bridge::{codex_bridge_config_routes, codex_bridge_public_routes};
#[cfg(feature = "enterprise")]
use dream_core_common::ApiError;
use dream_core_common::ApiErrorLogContext;
use dream_core_conversation::{conversation_ops_routes, conversation_routes};
use dream_core_cron::cron_routes;
use dream_core_extension::{extension_routes, hub_routes, skill_routes};
use dream_core_file::file_routes;
use dream_core_mcp::mcp_routes;
use dream_core_office::{office_proxy_routes, office_routes};
use dream_core_project::project_routes;
use dream_core_realtime::{NoopMessageRouter, WebSocketManager, WsHandlerState, ws_upgrade_handler};
use dream_core_shell::shell_routes;
use dream_core_system::{ClientPrefService, connection_test_routes, system_routes};
use dream_core_team::{TeamSessionService, team_routes};

use crate::services::AppServices;

/// Adapts one-org's `OrgService::tenant_of` to the `dream_domain_employee::TenantResolver`
/// trait, so one-employee / one-devops can resolve a caller's tenant (for
/// team-shared employees, A1 L3) without depending on one-org. Resolution
/// errors fall back to the personal `default` tenant.
#[cfg(feature = "enterprise")]
struct OrgTenantResolver(std::sync::Arc<dream_domain_org::OrgService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_employee::TenantResolver for OrgTenantResolver {
    async fn tenant_of(&self, user_id: &str) -> String {
        self.0
            .tenant_of(user_id)
            .await
            .unwrap_or_else(|_| dream_domain_employee::DEFAULT_TENANT.to_owned())
    }
}

/// Adapts one-org's `OrgService::auto_join_by_email` to the
/// `dream_domain_sso::OrgAutoJoin` trait (P2-4 onboarding: domain-based project-group
/// auto-join). Errors are logged and swallowed — never blocks a valid login.
#[cfg(feature = "enterprise")]
struct OrgAutoJoinAdapter(std::sync::Arc<dream_domain_org::OrgService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_sso::OrgAutoJoin for OrgAutoJoinAdapter {
    async fn auto_join_by_email(&self, user_id: &str, email: &str) {
        match self.0.auto_join_by_email(user_id, email).await {
            Ok(Some(tenant_id)) => {
                tracing::info!(
                    user_id,
                    tenant_id,
                    "SSO login: auto-joined project group by email domain"
                );
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, user_id, "onboarding domain auto-join failed; login continues");
            }
        }
    }
}

/// Adapts one-enterprise's `EnterpriseService::sync_member` to the
/// `dream_domain_sso::EnterpriseSync` trait, so an SSO login can sync the caller's
/// company + membership into the enterprise-org domain without one-sso
/// depending on one-enterprise. Best-effort by construction (see the service
/// method); errors are logged and swallowed so a failed sync can never block a
/// valid login.
/// Lets one-org revoke a departing member's company model channel tokens
/// without depending on one-devops (same layer). Those tokens deliberately
/// outlive JWT rotation, so removing a member has to close them explicitly —
/// otherwise the leaver keeps a working key to the company's models, which is
/// the whole thing channel provisioning exists to prevent.
#[cfg(feature = "enterprise")]
struct ModelChannelRevoker(std::sync::Arc<dream_domain_devops::DevopsService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::CredentialRevoker for ModelChannelRevoker {
    async fn revoke_for_user(&self, user_id: &str) {
        match self.0.revoke_channel_tokens_for_user(user_id).await {
            Ok(0) => {}
            Ok(revoked) => tracing::info!(user_id, revoked, "revoked model channel tokens"),
            // Never block the removal: a member who could not be fully
            // de-provisioned must still be removed. Logged loudly because it
            // leaves a live credential behind and needs following up.
            Err(error) => tracing::error!(%error, user_id, "failed to revoke model channel tokens on removal"),
        }
    }
}

/// Revokes the open-integration API keys a leaver minted (E5).
///
/// An API key has no session generation to rotate, so
/// `invalidate_user_tokens` does not touch it: without this a removed member
/// keeps a working key to the company's governance API indefinitely, because
/// `authenticate_api_key` only checks `status = 'active'` and their user row
/// survives removal. Exactly the hazard `ModelChannelRevoker` exists for.
#[cfg(feature = "enterprise")]
struct ApiKeyRevoker(std::sync::Arc<dream_domain_platform::PlatformService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::CredentialRevoker for ApiKeyRevoker {
    async fn revoke_for_user(&self, user_id: &str) {
        match self.0.revoke_api_keys_for_user(user_id).await {
            Ok(0) => {}
            Ok(revoked) => tracing::info!(user_id, revoked, "revoked open-integration API keys"),
            // Never block the removal, same posture as `ModelChannelRevoker`:
            // logged loudly because it leaves a live credential behind.
            Err(error) => tracing::error!(%error, user_id, "failed to revoke API keys on removal"),
        }
    }
}

/// Runs several `CredentialRevoker`s for one removal. `OrgService` holds a
/// single revoker slot, and a leaver now has two kinds of bearer credential
/// that outlive JWT rotation (model channel tokens, API keys), so they compose
/// here rather than either one displacing the other.
#[cfg(feature = "enterprise")]
struct CompositeCredentialRevoker(Vec<std::sync::Arc<dyn dream_domain_org::CredentialRevoker>>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::CredentialRevoker for CompositeCredentialRevoker {
    async fn revoke_for_user(&self, user_id: &str) {
        // Sequential and unconditional: each revoker already swallows its own
        // errors, so one failing must not skip the others.
        for revoker in &self.0 {
            revoker.revoke_for_user(user_id).await;
        }
    }
}

/// Lets a company removal cut off the leaver's access without one-enterprise
/// depending on one-org (same layer). one-org owns the per-user JWT secret and,
/// via its own `CredentialRevoker`, the company model channel tokens — so both
/// tiers of removal end up rotating exactly the same set of credentials.
#[cfg(feature = "enterprise")]
struct OrgSessionRevoker(std::sync::Arc<dream_domain_org::OrgService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_enterprise::SessionRevoker for OrgSessionRevoker {
    async fn revoke_sessions(&self, user_id: &str) {
        // Never block the removal: a member who could not be fully
        // de-provisioned must still lose their seat. Logged at error because it
        // leaves a live session behind and needs following up.
        if let Err(error) = self.0.invalidate_user_tokens(user_id).await {
            tracing::error!(%error, user_id, "failed to revoke sessions on company removal");
        }
    }
}

/// Lets disbanding a company delete what it owns in one-org (every project
/// group) and one-billing (every usage/license record) without
/// one-enterprise depending on either (same layer). Best-effort per side, by
/// the trait's own contract — a company the operator asked to disband must
/// actually go away even if one side's cleanup hits an error.
#[cfg(feature = "enterprise")]
struct CompanyDisbandCascadeImpl {
    org: std::sync::Arc<dream_domain_org::OrgService>,
    billing: std::sync::Arc<dream_domain_billing::BillingService>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_enterprise::CompanyDisbandCascade for CompanyDisbandCascadeImpl {
    async fn disband(&self, enterprise_id: &str) -> Vec<String> {
        if let Err(error) = self.billing.delete_enterprise_billing_data(enterprise_id).await {
            tracing::error!(%error, enterprise_id, "failed to delete enterprise billing data on disband");
        }
        match self.org.disband_tenants_for_enterprise(enterprise_id).await {
            Ok(deleted) => deleted,
            Err(error) => {
                tracing::error!(%error, enterprise_id, "failed to disband project groups on company disband");
                Vec::new()
            }
        }
    }
}

/// Lets one-org read the company directory mirror to map a subtree into a
/// project group's department tree (T6 stage 3), without depending on
/// one-enterprise (same layer). No company, or no directory sync ever run →
/// empty, which the caller treats as "nothing to map" rather than an error.
#[cfg(feature = "enterprise")]
struct DirectoryTreeSourceAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::DirectoryTreeSource for DirectoryTreeSourceAdapter {
    async fn directory_departments(&self) -> Vec<dream_domain_org::DirectoryDepartmentRef> {
        let Some(enterprise_id) = self.0.deployment_company_id().await.ok().flatten() else {
            return Vec::new();
        };
        self.0
            .list_directory_departments(&enterprise_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| dream_domain_org::DirectoryDepartmentRef {
                external_id: d.external_id,
                parent_external_id: d.parent_external_id,
                name: d.name,
            })
            .collect()
    }
}

/// Stores a completed directory pull (T6). one-sso knows how to talk to Feishu,
/// one-enterprise owns the company's tables, and they are the same layer — so
/// they meet here.
#[cfg(feature = "enterprise")]
struct DirectorySinkAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_sso::DirectorySink for DirectorySinkAdapter {
    async fn enterprise_id(&self) -> Option<String> {
        // No company set up → nothing to attribute a directory to, and the
        // caller treats that as "skip", not "fail".
        self.0.deployment_company_id().await.ok().flatten()
    }

    async fn apply_snapshot(&self, enterprise_id: &str, snapshot: dream_domain_sso::DirectorySnapshotPayload) {
        let input = dream_domain_enterprise::directory::DirectorySyncInput {
            provider: snapshot.provider,
            external_id_field: snapshot.external_id_field,
            departments: snapshot
                .departments
                .into_iter()
                .map(|d| dream_domain_enterprise::directory::DirectoryDepartmentInput {
                    external_id: d.external_id,
                    parent_external_id: d.parent_external_id,
                    name: d.name,
                })
                .collect(),
            people: snapshot
                .people
                .into_iter()
                .map(|p| dream_domain_enterprise::directory::DirectoryPersonInput {
                    external_id: p.external_id,
                    name: p.name,
                    job_title: p.job_title,
                    department_external_id: p.department_external_id,
                    active: p.active,
                })
                .collect(),
            // Carried through verbatim. This is the flag that decides whether
            // absence means "left the company" — quietly defaulting it either
            // way would be the worst bug this feature could have.
            complete: snapshot.complete,
            error: snapshot.error,
        };

        match self.0.apply_directory_snapshot(enterprise_id, &input).await {
            Ok(report) => tracing::info!(
                enterprise_id,
                departments = report.departments,
                people = report.people,
                newly_missing = report.newly_missing,
                returned = report.returned,
                complete = report.complete,
                "directory sync applied"
            ),
            // Swallowed so a storage failure cannot unwind the background loop.
            // Logged at error because a directory that silently stops updating
            // is exactly how offboarding suggestions go stale.
            Err(error) => tracing::error!(%error, enterprise_id, "directory sync could not be stored"),
        }
    }
}

#[cfg(feature = "enterprise")]
struct EnterpriseSyncAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_sso::EnterpriseSync for EnterpriseSyncAdapter {
    async fn sync_member(
        &self,
        user_id: &str,
        provider: &str,
        external_id: &str,
        personal_external_id: &str,
        display_name: Option<&str>,
        department: Option<&str>,
        job_title: Option<&str>,
    ) {
        if let Err(error) = self
            .0
            .sync_member(
                user_id,
                provider,
                external_id,
                personal_external_id,
                display_name,
                department,
                job_title,
            )
            .await
        {
            tracing::warn!(%error, user_id, provider, "enterprise-org sync failed; login continues");
        }
    }
}

/// Adapts one-enterprise's `EnterpriseService::is_company_admin_of` to the
/// `dream_domain_org::CompanyAdminResolver` trait (Direction B), so a company admin can
/// create/list the project groups their company owns without one-org depending
/// on one-enterprise. Resolution errors deny (fail closed).
#[cfg(feature = "enterprise")]
struct CompanyAdminResolverAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::CompanyAdminResolver for CompanyAdminResolverAdapter {
    async fn is_company_admin(&self, user_id: &str, enterprise_id: &str) -> bool {
        self.0
            .is_company_admin_of(user_id, enterprise_id)
            .await
            .unwrap_or(false)
    }
}

/// Adapts one-enterprise's `EnterpriseService::ensure_member` to the
/// `dream_domain_org::CompanySeatSync` trait, so joining a project group that belongs
/// to a company also registers the joiner as a company member (seat-capped,
/// same rule as SSO auto-provisioning) — see `dream_domain_org::enterprise_hooks`
/// module docs for why this exists.
#[cfg(feature = "enterprise")]
struct CompanySeatSyncAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_org::CompanySeatSync for CompanySeatSyncAdapter {
    async fn ensure_company_member(&self, user_id: &str, enterprise_id: &str, display_name: Option<&str>) {
        if let Err(error) = self.0.ensure_member(user_id, enterprise_id, display_name).await {
            tracing::warn!(%error, user_id, enterprise_id, "company seat sync failed; project-group join continues");
        }
    }

    async fn release_company_member(&self, user_id: &str, enterprise_id: &str) {
        if let Err(error) = self.0.leave_company(enterprise_id, user_id).await {
            // MemberNotFound is expected and not worth a warning: not every
            // project-group joiner ever synced into the company (the hook
            // this mirrors is best-effort too), and LastCompanyAdmin is a
            // real, intentional refusal — the seat is deliberately kept
            // occupied rather than leaving the company with no admin.
            if !matches!(
                error,
                dream_domain_enterprise::EnterpriseError::MemberNotFound
                    | dream_domain_enterprise::EnterpriseError::LastCompanyAdmin
            ) {
                tracing::warn!(%error, user_id, enterprise_id, "company seat release failed; project-group leave continues");
            }
        }
    }
}

/// Adapts one-enterprise's `EnterpriseService::is_company_admin` to the
/// `dream_domain_sso::CompanyAdminCheck` trait, so a company admin may manage the
/// company-level SSO config (企业认证). Errors deny (fail closed).
#[cfg(feature = "enterprise")]
struct CompanyAdminCheckAdapter(std::sync::Arc<dream_domain_enterprise::EnterpriseService>);

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_sso::CompanyAdminCheck for CompanyAdminCheckAdapter {
    async fn is_company_admin(&self, user_id: &str) -> bool {
        self.0.is_company_admin(user_id).await.unwrap_or(false)
    }

    async fn company_exists(&self) -> bool {
        self.0.company_exists().await.unwrap_or(false)
    }
}

/// Adapts one-platform's per-project-group IP allowlist to the auth
/// middleware's `IpAllowlistGate` trait — same-layer bridge, same
/// arrangement as every other adapter in this file.
///
/// A caller with nothing to check (no resolvable project group — personal
/// edition, or an enterprise account with no membership — or a group whose
/// allowlist is disabled, the reserved default) is always allowed regardless
/// of whether `ip` resolved. Only once a group's allowlist is actually
/// enabled does an unresolvable `ip` become a denial: see the trait's own
/// doc comment for why getting this ordering backwards would 403 every test
/// (and every personal-edition install) that never wired a real
/// `ConnectInfo`.
#[cfg(feature = "enterprise")]
struct PlatformIpAllowlistGate {
    platform: std::sync::Arc<dream_domain_platform::PlatformService>,
    grace: std::sync::Arc<PolicyGrace>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_auth::IpAllowlistGate for PlatformIpAllowlistGate {
    /// Returning `Err` here is the most expensive failure in the system: the
    /// auth middleware turns it into a 500, so *every* authenticated request
    /// fails — conversations, agents, files, the whole personal workbench, none
    /// of which the IP allowlist has any business governing. A platform-table
    /// read that times out must therefore allow, not 500. An actual `false`
    /// from the allowlist still blocks, because that is the policy answering.
    async fn is_allowed(&self, user_id: &str, ip: Option<std::net::IpAddr>) -> Result<bool, String> {
        let actor = match self.platform.resolve_actor(user_id).await {
            // No actor: nothing governs this caller. A standalone install lands
            // here on every request. Still `answered()` — the plane replied,
            // it just replied "nobody governs this caller"; see
            // `SecurityPolicySendRateGate::check` for why treating a reachable
            // plane as silent would expire the shared grace window.
            Ok(None) => {
                self.grace.answered();
                return Ok(true);
            }
            Ok(Some(actor)) => {
                self.grace.answered();
                actor
            }
            Err(e) => return self.unanswerable(user_id, &e.to_string()),
        };
        let outcome = match ip {
            Some(ip) => self.platform.is_ip_allowed(&actor.tenant_id, &ip.to_string()).await,
            // No caller IP to check. The allowlist can then only pass everyone
            // or nobody, and blocking everyone because a header was missing is
            // not a decision an administrator made.
            None => self
                .platform
                .get_ip_allowlist(&actor.tenant_id)
                .await
                .map(|cfg| !cfg.enabled),
        };
        match outcome {
            Ok(allowed) => {
                self.grace.answered();
                Ok(allowed)
            }
            Err(e) => self.unanswerable(user_id, &e.to_string()),
        }
    }
}

#[cfg(feature = "enterprise")]
impl PlatformIpAllowlistGate {
    /// Returning `Err` here is the most expensive failure in the system: the
    /// auth middleware turns it into a 500, so *every* authenticated request
    /// fails — conversations, agents, files, the whole personal workbench, none
    /// of which the IP allowlist has any business governing. So a platform read
    /// that cannot answer allows, until it has been failing long enough to be
    /// an outage rather than a hiccup.
    fn unanswerable(&self, user_id: &str, error: &str) -> Result<bool, String> {
        if self.grace.within_window() {
            tracing::warn!(user_id, error, "ip allowlist unanswerable; inside grace window");
            return Ok(true);
        }
        tracing::error!(user_id, error, "ip allowlist unreachable beyond grace window");
        Err(error.to_owned())
    }
}

/// Adapts `PlatformService::authenticate_api_key` (E5 open-integration API
/// keys) to `dream-core-auth`'s `ApiKeyGate` port.
///
/// Unlike the IP allowlist, a lookup failure here has no "unanswerable, fail
/// open with a grace window" story: this gate is only ever consulted for a
/// request that is *already* presenting an API-key-shaped credential, never
/// for the personal workbench or an ordinary JWT session, so there is no
/// broad blast radius to protect against — a genuine DB error should simply
/// 500 that one request, same as any other backend failure.
#[cfg(feature = "enterprise")]
struct PlatformApiKeyGate {
    platform: std::sync::Arc<dream_domain_platform::PlatformService>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_auth::ApiKeyGate for PlatformApiKeyGate {
    async fn authenticate(&self, secret: &str, request_path: &str) -> Result<dream_core_auth::ApiKeyVerdict, String> {
        use dream_domain_platform::ApiKeyAuthOutcome;
        self.platform
            .authenticate_api_key(secret, request_path)
            .await
            .map(|outcome| match outcome {
                ApiKeyAuthOutcome::Invalid => dream_core_auth::ApiKeyVerdict::Invalid,
                ApiKeyAuthOutcome::PathNotAllowed => dream_core_auth::ApiKeyVerdict::PathNotAllowed,
                ApiKeyAuthOutcome::Authenticated { user_id } => {
                    dream_core_auth::ApiKeyVerdict::Authenticated { user_id }
                }
            })
            .map_err(|e| e.to_string())
    }
}

/// Adapts one-billing's `BillingService::record_turn` to the conversation
/// crate's `UsageRecorder` trait (P0-3). Fire-and-forget: spawns the async
/// insert so metering never blocks or fails the send path.
///
/// `model`/tokens flow through from `ConversationTurnOrchestrator`, which
/// only calls this once a turn has actually completed — real cost is known
/// only then, never at accept time (see the trait's own doc comment for why
/// that used to make every chat turn cost $0 regardless of the real bill).
#[cfg(feature = "enterprise")]
struct BillingUsageRecorder(std::sync::Arc<dream_domain_billing::BillingService>);

#[cfg(feature = "enterprise")]
impl dream_core_conversation::UsageRecorder for BillingUsageRecorder {
    fn record_turn(
        &self,
        user_id: String,
        conversation_id: String,
        model: Option<String>,
        channel_id: Option<String>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
    ) {
        let service = self.0.clone();
        tokio::spawn(async move {
            if let Err(e) = service
                .record_turn(
                    &user_id,
                    Some(&conversation_id),
                    model.as_deref(),
                    channel_id.as_deref(),
                    input_tokens,
                    output_tokens,
                )
                .await
            {
                tracing::debug!(error = %e, "usage record_turn failed (non-fatal)");
            }
        });
    }
}

/// Adapts one-billing's `check_send_allowed` to the conversation crate's
/// `SendGate` trait (P1-2 model control). Blocks a send when the team is over
/// its spend budget / off-allowlist; personal users always pass.
#[cfg(feature = "enterprise")]
struct BillingSendGate {
    billing: std::sync::Arc<dream_domain_billing::BillingService>,
    grace: std::sync::Arc<PolicyGrace>,
}

/// Map a billing refusal to something the member can act on.
///
/// ⚠️ Only the two *policy* variants become visible text. Everything else is an
/// internal failure (a DB error, say) — the gate still fails closed, but its
/// `to_string()` must not reach the user, and there is nothing actionable in it
/// for them anyway. This mattered the moment these messages stopped being
/// redacted: before, `ApiError::Forbidden` hid every one of them equally.
#[cfg(feature = "enterprise")]
/// How long the conversation path keeps working while the enterprise policy
/// plane cannot be reached.
///
/// Two failure modes need opposite answers and they arrive as the same `Err`.
/// A member whose admin tightened a policy must feel it immediately; a member
/// whose company database is briefly unreachable must not be stopped from
/// working. Refusing on every error made a transient enterprise-side fault take
/// the personal workbench down with it; allowing on every error let an outage
/// suspend policy indefinitely.
///
/// A window resolves it: keep working while the plane is only *briefly* silent,
/// and stop extending enterprise scope once the silence is long enough to be a
/// real outage rather than a hiccup. Thirty minutes is long enough to cover a
/// restart, a lock storm or a failover, short enough that a genuinely dead
/// plane does not govern nobody all day.
#[cfg(feature = "enterprise")]
const ENTERPRISE_POLICY_GRACE_MS: i64 = 30 * 60 * 1000;

/// Tracks when the enterprise policy plane last managed to answer, so an
/// unanswerable check can tell "briefly unreachable" from "gone".
///
/// Starts at process start: a deployment that never once reaches its policy
/// plane gets one grace window and then stops extending enterprise scope,
/// rather than running ungoverned forever.
#[cfg(feature = "enterprise")]
#[derive(Debug)]
pub(crate) struct PolicyGrace {
    last_answered_ms: std::sync::atomic::AtomicI64,
}

#[cfg(feature = "enterprise")]
impl PolicyGrace {
    pub(crate) fn new() -> Self {
        Self {
            last_answered_ms: std::sync::atomic::AtomicI64::new(dream_core_common::now_ms()),
        }
    }

    /// The plane answered — whether it allowed or refused is irrelevant here,
    /// what matters is that it was reachable.
    fn answered(&self) {
        self.last_answered_ms
            .store(dream_core_common::now_ms(), std::sync::atomic::Ordering::Relaxed);
    }

    fn within_window(&self) -> bool {
        let last = self.last_answered_ms.load(std::sync::atomic::Ordering::Relaxed);
        dream_core_common::now_ms().saturating_sub(last) < ENTERPRISE_POLICY_GRACE_MS
    }
}

/// What a billing error means for this send.
///
/// Three outcomes, because there are three situations and only one of them is
/// the policy refusing:
///
/// * `Governs(denial)` — the policy answered no. Always enforced.
/// * `NotGoverned` — no company exists on this server, so nothing governs this
///   member. A standalone install must never be affected by an enterprise plane
///   it does not participate in.
/// * `Unanswerable` — the check itself failed. Deferred to the grace window.
#[cfg(feature = "enterprise")]
enum PolicyVerdict {
    Governs(dream_core_conversation::PolicyDenial),
    NotGoverned,
    Unanswerable,
}

#[cfg(feature = "enterprise")]
fn billing_denial(error: dream_domain_billing::BillingError) -> PolicyVerdict {
    use dream_domain_billing::BillingError;
    PolicyVerdict::Governs(match error {
        BillingError::ModelNotAllowed(model) => dream_core_conversation::PolicyDenial::new(
            "MODEL_NOT_ALLOWED",
            format!("Model '{model}' is not allowed by the team's policy"),
        )
        .with_details(serde_json::json!({ "model": model })),
        BillingError::BudgetExceeded => dream_core_conversation::PolicyDenial::new(
            "BUDGET_EXCEEDED",
            "The team's usage budget for this period has been reached",
        ),
        // T6-4: arrived after the plan's seat cap filled. Distinct from every
        // other denial here — it isn't a rule the admin configured, it's the
        // absence of a seat, and the fix is "wait or ask for more seats" rather
        // than "wait for the budget window" or "ask for a different model".
        BillingError::SeatLimitExceeded => dream_core_conversation::PolicyDenial::new(
            "SEAT_LIMIT_EXCEEDED",
            "Your organization's seat limit has been reached; an administrator needs to free a seat or upgrade the plan before you can send messages",
        ),
        // T7: a tighter cap layered under the company-wide budget, scoped to
        // this member's department. Distinct code so the UI can point at the
        // department's own budget panel rather than the company one.
        BillingError::DepartmentBudgetExceeded => dream_core_conversation::PolicyDenial::new(
            "DEPARTMENT_BUDGET_EXCEEDED",
            "This department's usage budget for this period has been reached",
        ),
        // Not a refusal: this server has no company, so no enterprise policy
        // applies to anyone on it.
        BillingError::EnterpriseNotFound => return PolicyVerdict::NotGoverned,
        // The check failed rather than the policy refusing. Whether that stops
        // the member depends on how long the plane has been silent.
        other => {
            tracing::warn!(error = %other, "billing policy check could not answer");
            return PolicyVerdict::Unanswerable;
        }
    })
}

#[cfg(feature = "enterprise")]
impl BillingSendGate {
    /// Turn a policy-plane result into an allow/deny for this send.
    fn settle(
        &self,
        result: Result<(), dream_domain_billing::BillingError>,
    ) -> Result<(), dream_core_conversation::PolicyDenial> {
        let error = match result {
            Ok(()) => {
                self.grace.answered();
                return Ok(());
            }
            Err(e) => e,
        };
        match billing_denial(error) {
            PolicyVerdict::Governs(denial) => {
                self.grace.answered();
                Err(denial)
            }
            // The plane answered "no company on this server" — reachable, so it
            // counts as answered for the shared grace window.
            PolicyVerdict::NotGoverned => {
                self.grace.answered();
                Ok(())
            }
            PolicyVerdict::Unanswerable if self.grace.within_window() => Ok(()),
            PolicyVerdict::Unanswerable => {
                tracing::error!(
                    "enterprise policy plane unreachable beyond the grace window; refusing enterprise-scoped sends"
                );
                Err(dream_core_conversation::PolicyDenial::new(
                    "ENTERPRISE_POLICY_UNAVAILABLE",
                    "Company policy has been unreachable for too long; switch to a personal workspace or contact your administrator",
                ))
            }
        }
    }
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_conversation::SendGate for BillingSendGate {
    async fn check_send(
        &self,
        user_id: &str,
        model: Option<&str>,
    ) -> Result<(), dream_core_conversation::PolicyDenial> {
        self.settle(self.billing.check_send_allowed(user_id, model).await)
    }

    async fn check_model(&self, user_id: &str, model: &str) -> Result<(), dream_core_conversation::PolicyDenial> {
        self.settle(self.billing.check_model_allowed(user_id, model).await)
    }
}

/// Per-member fixed-window send counter for `one_security_policy`'s
/// `send_rate_limit_per_minute` (E5 security policy baseline). Unlike
/// `dream_core_auth::RateLimiter`, the limit is not baked in at construction
/// — it is read from the tenant's live policy on every check, since an admin
/// can change it at any time via the admin console.
#[cfg(feature = "enterprise")]
struct SendRateLimiter {
    counts: dashmap::DashMap<String, (u32, i64)>,
    window_ms: i64,
}

#[cfg(feature = "enterprise")]
impl SendRateLimiter {
    fn new() -> Self {
        Self::with_window_ms(60_000)
    }

    /// Test-only hook: a 60-second window can't be exercised end-to-end in a
    /// unit test, so tests construct with a millisecond-scale window instead
    /// (same technique as `dream_core_auth::RateLimiter`'s own tests).
    fn with_window_ms(window_ms: i64) -> Self {
        Self {
            counts: dashmap::DashMap::new(),
            window_ms,
        }
    }

    /// Records this send and returns whether it stays within `limit_per_minute`
    /// (a fixed window per `user_id`, matching the field's own "per minute"
    /// semantics).
    fn check_and_increment(&self, user_id: &str, limit_per_minute: u32) -> bool {
        let now = dream_core_common::now_ms();
        let mut entry = self
            .counts
            .entry(user_id.to_owned())
            .or_insert((0, now + self.window_ms));
        if now >= entry.1 {
            entry.0 = 0;
            entry.1 = now + self.window_ms;
        }
        if entry.0 >= limit_per_minute {
            return false;
        }
        entry.0 += 1;
        true
    }
}

/// Enforces `one_security_policy.send_rate_limit_per_minute` on the
/// pre-send path. `None` (the default for every tenant that has never
/// touched their policy, or is on the `relaxed` tier) means unlimited —
/// this gate never governs a caller that hasn't opted in, same posture as
/// every other enterprise gate in this file.
#[cfg(feature = "enterprise")]
struct SecurityPolicySendRateGate {
    platform: std::sync::Arc<dream_domain_platform::PlatformService>,
    limiter: SendRateLimiter,
    grace: std::sync::Arc<PolicyGrace>,
}

#[cfg(feature = "enterprise")]
impl SecurityPolicySendRateGate {
    fn new(
        platform: std::sync::Arc<dream_domain_platform::PlatformService>,
        grace: std::sync::Arc<PolicyGrace>,
    ) -> Self {
        Self {
            platform,
            limiter: SendRateLimiter::new(),
            grace,
        }
    }

    async fn check(&self, user_id: &str) -> Result<(), dream_core_conversation::PolicyDenial> {
        let actor = match self.platform.resolve_actor(user_id).await {
            // No enterprise membership: nothing governs this caller. Every
            // standalone install and every personal-edition test lands here.
            // Still `answered()`: the plane WAS reachable — it answered "no
            // membership". Skipping it here would freeze the shared grace
            // window at process start on any deployment whose traffic is all
            // ungoverned, so the first transient DB error 30 minutes in would
            // be treated as a dead policy plane.
            Ok(None) => {
                self.grace.answered();
                return Ok(());
            }
            Ok(Some(actor)) => {
                self.grace.answered();
                actor
            }
            Err(e) => return self.unanswerable(&e.to_string()),
        };
        let policy = match self.platform.get_security_policy(&actor.tenant_id).await {
            Ok(policy) => {
                self.grace.answered();
                policy
            }
            Err(e) => return self.unanswerable(&e.to_string()),
        };
        let Some(limit) = policy.send_rate_limit_per_minute.filter(|l| *l > 0) else {
            return Ok(());
        };
        // `send_rate_limit_per_minute` is an unvalidated `i64` straight from the
        // admin console, so saturate rather than `as u32`: a deliberately-huge
        // "effectively unlimited" value like 2^32 would otherwise WRAP TO ZERO
        // and refuse every send for the whole tenant — the exact opposite of
        // what the administrator asked for.
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        if self.limiter.check_and_increment(user_id, limit) {
            Ok(())
        } else {
            Err(dream_core_conversation::PolicyDenial::new(
                "SEND_RATE_LIMITED",
                "You're sending messages faster than your organization's policy allows; please slow down",
            ))
        }
    }

    /// Same posture as `PlatformIpAllowlistGate::unanswerable`: a transient
    /// platform-read failure must not stop every send in the process, so it
    /// is tolerated within the shared grace window and only enforced once
    /// the plane has been genuinely unreachable long enough to be an outage.
    fn unanswerable(&self, error: &str) -> Result<(), dream_core_conversation::PolicyDenial> {
        if self.grace.within_window() {
            tracing::warn!(error, "send rate limit unanswerable; inside grace window");
            return Ok(());
        }
        tracing::error!(
            error,
            "security policy plane unreachable beyond grace window; refusing enterprise-scoped sends"
        );
        Err(dream_core_conversation::PolicyDenial::new(
            "ENTERPRISE_POLICY_UNAVAILABLE",
            "Company policy has been unreachable for too long; switch to a personal workspace or contact your administrator",
        ))
    }
}

/// Combines the billing budget/model gate with the security-policy send-rate
/// limit: both are pre-send checks, and `ConversationRouterState` has exactly
/// one `SendGate` slot, so they compose here rather than each claiming it.
#[cfg(feature = "enterprise")]
struct EnterpriseSendGate {
    billing: BillingSendGate,
    rate: SecurityPolicySendRateGate,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_conversation::SendGate for EnterpriseSendGate {
    async fn check_send(
        &self,
        user_id: &str,
        model: Option<&str>,
    ) -> Result<(), dream_core_conversation::PolicyDenial> {
        self.billing.check_send(user_id, model).await?;
        self.rate.check(user_id).await
    }

    async fn check_model(&self, user_id: &str, model: &str) -> Result<(), dream_core_conversation::PolicyDenial> {
        // Model-switch allowlisting only — the send-rate limit governs
        // sends, not config changes, same as billing's own budget check.
        self.billing.check_model(user_id, model).await
    }
}

/// Adapts one-billing's `check_model_allowed` to the agent factory's
/// `ModelAllowlistGate` (P0, vision delegate).
///
/// Allowlist-only on purpose. The delegate is resolved once when the agent is
/// built and then reused for the whole session, so a budget check here would
/// answer a question about one instant and cache it for hours — and the send
/// path already checks the budget per turn. The delegate's *cost* is handled
/// separately, by metering the call so the cap can see it at all.
///
/// `pub(crate)` because `services.rs` builds the factory long before any router
/// exists; this is the same adapter, wired earlier.
#[cfg(feature = "enterprise")]
pub(crate) struct BillingModelAllowlistGate {
    pub(crate) billing: std::sync::Arc<dream_domain_billing::BillingService>,
    pub(crate) grace: std::sync::Arc<PolicyGrace>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_ai_agent::ModelAllowlistGate for BillingModelAllowlistGate {
    async fn is_model_allowed(&self, user_id: &str, model: &str) -> Result<bool, String> {
        match self.billing.check_model_allowed(user_id, model).await {
            Ok(()) => {
                self.grace.answered();
                Ok(true)
            }
            // The two policy refusals: the admin's allowlist, and a member who
            // arrived after the seat cap filled (T6-4 — governed by nothing, so
            // denied outright rather than falling through an empty allowlist).
            Err(
                dream_domain_billing::BillingError::ModelNotAllowed(_)
                | dream_domain_billing::BillingError::SeatLimitExceeded,
            ) => {
                self.grace.answered();
                Ok(false)
            }
            // No company on this server governs nobody's model choice. Still
            // `answered()`: the plane was reachable, it just has no company.
            Err(dream_domain_billing::BillingError::EnterpriseNotFound) => {
                self.grace.answered();
                Ok(true)
            }
            // The check failed rather than the policy refusing. Same posture
            // as `BillingSendGate`: tolerated while the plane is only briefly
            // silent, enforced once the silence outlasts the grace window.
            Err(other) if self.grace.within_window() => {
                tracing::warn!(error = %other, user_id, model, "model allowlist unanswerable; inside grace window");
                Ok(true)
            }
            Err(other) => {
                tracing::error!(error = %other, user_id, model, "model allowlist unreachable beyond grace window");
                Err(other.to_string())
            }
        }
    }
}

/// The billing-plane implementation of conversation's per-call trace seam
/// (P2-5): every model call the turn orchestrator relays lands in
/// `one_llm_calls`. Fire-and-forget, same as `BillingUsageRecorder` above —
/// a trace failure is logged, never propagated; the turn must not fail
/// because observability could not write a row.
#[cfg(feature = "enterprise")]
struct BillingLlmCallTrace(std::sync::Arc<dream_domain_billing::BillingService>);

#[cfg(feature = "enterprise")]
impl dream_core_conversation::LlmCallTraceRecorder for BillingLlmCallTrace {
    fn record_call(&self, user_id: String, conversation_id: String, trace: dream_core_conversation::LlmCallTrace) {
        let service = self.0.clone();
        tokio::spawn(async move {
            let call = dream_domain_billing::NewLlmCall {
                user_id,
                conversation_id: Some(conversation_id),
                model: trace.model,
                // The relay path is backend-agnostic; attributing a provider
                // shape would be a guess, so the column stays unset here.
                provider: None,
                tool_name: None,
                input_tokens: trace.input_tokens.unwrap_or(0),
                output_tokens: trace.output_tokens.unwrap_or(0),
                // P1-3 latency collection: the orchestrator times the whole
                // attempt; delegates arrive as None (no honest timer) and stay
                // NULL in the table so percentiles only see measured calls.
                duration_ms: trace.duration_ms,
                error: trace.error,
            };
            if let Err(e) = service.record_llm_call(call).await {
                tracing::warn!(error = %e, "failed to record llm call trace row");
            }
        });
    }
}

/// Adapts `one_security_policy`'s `destructive_commands_blocked` +
/// `blocked_command_patterns`, `external_network_denied_by_default`, and
/// `terminal_tools_require_approval` to `dream-core-ai-agent`'s
/// `ToolCallSecurityGate` port, consulted by the ACP permission router
/// before a tool call reaches the user for approval (or is auto-approved).
///
/// The approval dimension (P2-1's terminal-tool flow, T8's fifth field) is
/// the one that *blocks inside this call*: when the policy demands approval
/// for terminal tools, a `tool` workflow task is created and the call waits
/// — polling the shared ledger, so a decision made on the admin binary's
/// queue is seen here — until an administrator approves, rejects, or the
/// deadline passes. A timeout is a denial (`wait_for_decision`'s contract),
/// the conservative default the plan fixes because the reference product
/// does not expose its own.
///
/// No shared `PolicyGrace` here — unlike the IP allowlist or the send-rate
/// gate, a failure in this check only fails closed on the ONE tool call
/// that triggered it (the caller can just retry), not on every other
/// request in the process, so there is no broad blast radius to protect
/// against with a grace window.
#[cfg(feature = "enterprise")]
pub(crate) struct PlatformToolCallSecurityGate {
    pub(crate) platform: std::sync::Arc<dream_domain_platform::PlatformService>,
    /// The approval backend. `None` (tests, or a plane built without the
    /// workflow crate) keeps `terminal_tools_require_approval` at its T8
    /// posture: stored, surfaced in the UI, but not enforced.
    pub(crate) workflow: Option<std::sync::Arc<dream_domain_workflow::WorkflowService>>,
}

/// The task title carries a bounded slice of the command text: enough for an
/// administrator skimming the queue to tell two terminal calls apart, short
/// enough that a machine-generated blob cannot flood the list.
const APPROVAL_TITLE_MAX_CHARS: usize = 120;

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_core_ai_agent::ToolCallSecurityGate for PlatformToolCallSecurityGate {
    async fn check(
        &self,
        user_id: &str,
        command_text: &str,
        is_network_fetch: bool,
        is_terminal_tool: bool,
    ) -> Result<Option<String>, String> {
        let actor = match self.platform.resolve_actor(user_id).await {
            // No enterprise membership: nothing governs this caller.
            Ok(None) => return Ok(None),
            Ok(Some(actor)) => actor,
            Err(e) => return Err(e.to_string()),
        };
        let policy = self
            .platform
            .get_security_policy(&actor.tenant_id)
            .await
            .map_err(|e| e.to_string())?;

        if is_network_fetch && policy.external_network_denied_by_default {
            return Ok(Some(
                "blocked by company security policy (external network access denied by default)".to_owned(),
            ));
        }

        if policy.destructive_commands_blocked {
            let haystack = command_text.to_lowercase();
            if let Some(pattern) = policy
                .blocked_command_patterns
                .iter()
                .find(|pattern| !pattern.is_empty() && haystack.contains(&pattern.to_lowercase()))
            {
                return Ok(Some(format!(
                    "blocked by company security policy (matches '{pattern}')"
                )));
            }
        }

        if is_terminal_tool && policy.terminal_tools_require_approval {
            let Some(workflow) = self.workflow.as_ref() else {
                // Policy demands approval but no approval backend is wired:
                // fail closed on this call (same convention as a check
                // error) rather than silently executing what the tenant
                // asked to gate.
                return Err("terminal tools require approval but no approval backend is wired".to_owned());
            };
            match self
                .run_terminal_approval(workflow, &actor.tenant_id, user_id, command_text)
                .await
            {
                Ok(None) => {}
                other => return other,
            }
        }

        Ok(None)
    }
}

#[cfg(feature = "enterprise")]
impl PlatformToolCallSecurityGate {
    /// Create the `tool` approval task and block until it is decided or the
    /// deadline passes. `Ok(None)` = approved (the call proceeds);
    /// `Ok(Some(reason))` / `Err` = the call is denied, with the reason the
    /// transcript will show.
    async fn run_terminal_approval(
        &self,
        workflow: &dream_domain_workflow::WorkflowService,
        tenant_id: &str,
        user_id: &str,
        command_text: &str,
    ) -> Result<Option<String>, String> {
        let title: String = command_text.chars().take(APPROVAL_TITLE_MAX_CHARS).collect();
        let task = workflow
            .create_task(
                tenant_id,
                "tool",
                user_id,
                &title,
                "terminal tool call awaiting administrator approval (security policy)",
                &serde_json::json!({ "commandText": command_text }),
                Some(dream_core_common::now_ms() + dream_domain_workflow::TERMINAL_APPROVAL_TIMEOUT_MS),
            )
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            task_id = %task.id,
            tenant_id,
            user_id,
            "terminal tool call held for administrator approval"
        );
        match workflow
            .wait_for_decision(tenant_id, &task.id, dream_domain_workflow::TERMINAL_APPROVAL_TIMEOUT_MS)
            .await
        {
            Ok(dream_domain_workflow::ApprovalOutcome::Approved) => {
                tracing::info!(task_id = %task.id, "terminal tool call approved");
                Ok(None)
            }
            Ok(dream_domain_workflow::ApprovalOutcome::Denied { reason }) => {
                Ok(Some(format!("blocked by company security policy ({reason})")))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Adapts dream-system's local content inspector to the conversation crate's
/// `ContentInspector` trait (T4). Personal builds have no rules distributed, so
/// this costs a read lock and a length check per send.
struct LocalContentInspector(std::sync::Arc<dream_core_system::ContentInspectionService>);

impl dream_core_conversation::ContentInspector for LocalContentInspector {
    fn inspect(&self, conversation_id: &str, text: &str) -> Option<dream_core_conversation::PolicyDenial> {
        // The model is not known at this point in the send path (the billing
        // gate has the same limitation), so findings are attributed by
        // conversation, which is what a reviewer follows back anyway.
        self.0.inspect(Some(conversation_id), None, text).blocked.map(|block| {
            // The rule name travels as a parameter, not baked into the
            // sentence: it is admin-authored data, while the sentence around
            // it is product copy the client translates.
            dream_core_conversation::PolicyDenial::new("CONTENT_BLOCKED", block.reason)
                .with_details(serde_json::json!({ "ruleName": block.rule_name }))
        })
    }
}
use super::fs_monitor::spawn_fs_monitor;
use super::health::health_check;
use super::runtime_team_tools::{RuntimeTeamToolsState, runtime_team_tools_routes};
use super::scm_monitor::{CompositeMessageRouter, spawn_scm_monitor};
use super::state::{ModuleStates, RouterBuildError, build_module_states, build_ws_state};
use super::trace::with_access_log;

/// Personal-edition answers for the handful of governance endpoints the
/// desktop shell calls unconditionally.
///
/// The shell asks "which org am I in?" on every launch (`useOrgContext`,
/// `useMyTenants`, `useEnterpriseIdentity`), reports the machine into the
/// runtime-node roster from its layout effect, and superAssistant's resource
/// ACL picker asks for the tenant list. With `dream-domain-org` compiled out
/// those become 404s — and a 404 is not the same answer as "you are not in an
/// org": the identity entry renders an error state instead of the personal one.
///
/// So the personal build answers them itself, with the same shapes the real
/// handlers return for a user who has no org. The mutating half
/// (join/create/exit/switch/reset-local) returns an explicit refusal rather
/// than a 404, because "this build cannot do that" is the accurate answer:
/// in client mode these requests are routed to the remote enterprise server by
/// `GOVERNANCE_PATH_PREFIXES` and never reach a local personal backend, and a
/// standalone user has no server to join in the first place.
#[cfg(not(feature = "enterprise"))]
fn personal_identity_routes() -> Router {
    use axum::routing::post;

    /// Duplicated from `dream_domain_org::models` — that crate is compiled out
    /// here. Both are the id the bootstrap assigns to a desktop install's
    /// single local user; they must not drift.
    const SYSTEM_DEFAULT_USER_ID: &str = "system_default_user";
    /// Duplicated from `dream_domain_org::models::DEFAULT_TENANT_ID`, same reason.
    const DEFAULT_TENANT_ID: &str = "default";

    async fn org_context(
        axum::Extension(user): axum::Extension<dream_core_auth::CurrentUser>,
    ) -> Json<serde_json::Value> {
        // Mirrors `OrgService::effective_role`: with no membership table to
        // read, the local owner is system_admin and everyone else is a member.
        let role = if user.id == SYSTEM_DEFAULT_USER_ID {
            "system_admin"
        } else {
            "member"
        };
        Json(serde_json::json!({
            "success": true,
            "data": {
                "tenantId": DEFAULT_TENANT_ID,
                "tenantName": null,
                "role": role,
                "isEnterprise": false,
                "memberCount": 0,
            }
        }))
    }

    async fn empty_list() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "success": true, "data": [] }))
    }

    async fn null_identity() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "success": true, "data": null }))
    }

    async fn heartbeat_accepted() -> StatusCode {
        StatusCode::NO_CONTENT
    }

    async fn not_in_this_edition() -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(ErrorResponse::new(
                "ENTERPRISE_NOT_AVAILABLE",
                "this build has no enterprise plane; connect to an enterprise server instead",
            )),
        )
    }

    Router::new()
        .route("/api/one/org/context", get(org_context))
        .route("/api/one/org/my-tenants", get(empty_list))
        .route("/api/one/org/tenants", get(empty_list))
        .route("/api/one/enterprise/me", get(null_identity))
        .route("/api/one/enterprise/company", get(null_identity))
        .route("/api/one/admin/runtime/heartbeat", post(heartbeat_accepted))
        .route("/api/one/org/switch", post(not_in_this_edition))
        .route("/api/one/org/create", post(not_in_this_edition))
        .route("/api/one/org/join", post(not_in_this_edition))
        .route("/api/one/org/exit", post(not_in_this_edition))
        .route("/api/one/org/reset-local", post(not_in_this_edition))
}

pub struct RouterRuntime {
    pub client_pref_service: ClientPrefService,
    pub team_service: Arc<TeamSessionService>,
    /// The two halves of T6 directory sync, handed back so `cmd_server` can
    /// drive them on a timer. Built here because this is where every service is
    /// already assembled; whether they ever do anything is decided at run time
    /// by whether this machine holds the company's SSO config.
    #[cfg(feature = "enterprise")]
    pub sso_service: Arc<dream_domain_sso::SsoService>,
    #[cfg(feature = "enterprise")]
    pub directory_sink: Arc<dyn dream_domain_sso::DirectorySink>,
}

async fn forward_event_bus_to_websocket(
    mut event_rx: tokio::sync::broadcast::Receiver<WebSocketMessage<serde_json::Value>>,
    ws_manager: Arc<WebSocketManager>,
) {
    loop {
        let event = match event_rx.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    skipped,
                    "websocket event bus bridge lagged; skipped stale events and will continue"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        if let Some(user_id) = event
            .data
            .get("user_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
        {
            ws_manager.broadcast_to_user(&user_id, event);
        } else if is_global_websocket_event(&event.name) {
            ws_manager.broadcast_all(event);
        } else {
            tracing::warn!(
                event_name = %event.name,
                "dropping websocket event without user_id; add user_id to payload or whitelist explicit global event"
            );
        }
    }
}

/// Create the application router with all routes and global middleware.
///
/// Middleware stack (outermost → innermost):
/// 1. Security response headers (X-Frame-Options, etc.)
/// 2. CSRF protection (Double Submit Cookie)
/// 3. Route handlers (auth routes + system routes + conversation routes + file routes + health check)
pub async fn create_router(services: &AppServices) -> Result<Router, RouterBuildError> {
    let (router, _runtime) = create_router_with_runtime(services).await?;
    Ok(router)
}

/// Create the application router and return runtime handles needed by
/// background services started outside the router tree.
pub async fn create_router_with_runtime(services: &AppServices) -> Result<(Router, RouterRuntime), RouterBuildError> {
    let boot = Instant::now();
    tracing::info!("startup: router assembly started");

    // Bridge event bus → WebSocket manager: forward all broadcast events
    // to connected WebSocket clients.
    let event_rx = services.event_bus.subscribe();
    let ws_manager = services.ws_manager.clone();
    tokio::spawn(forward_event_bus_to_websocket(event_rx, ws_manager));

    let (states, channel_components) = build_module_states(services).await?;
    let client_pref_service = states.system.client_pref_service.clone();
    let team_service = states.team.service.clone();
    tracing::info!(elapsed_ms = boot.elapsed().as_millis(), "startup: module states built");

    // one-org keeps its own migration ledger (`_one_migrations`), fully
    // decoupled from the upstream sqlx migrator — see crates/one-org.
    #[cfg(feature = "enterprise")]
    {
        dream_domain_org::run_one_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new("router.dream_domain_org.migrate", "failed to run one-org migrations")
                    .with_source(e)
            })?;
    }
    dream_domain_employee::run_one_employee_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_employee.migrate",
                "failed to run one-employee migrations",
            )
            .with_source(e)
        })?;
    #[cfg(feature = "enterprise")]
    {
        dream_domain_sso::run_one_sso_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new("router.dream_domain_sso.migrate", "failed to run one-sso migrations")
                    .with_source(e)
            })?;
    }
    dream_domain_devops::run_one_devops_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_devops.migrate",
                "failed to run one-devops migrations",
            )
            .with_source(e)
        })?;
    #[cfg(feature = "enterprise")]
    {
        dream_domain_enterprise::run_one_enterprise_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new(
                    "router.dream_domain_enterprise.migrate",
                    "failed to run one-enterprise migrations",
                )
                .with_source(e)
            })?;
    }
    // MUST run after one-enterprise: billing_001_init grandfathers existing
    // one_enterprises rows to the top tier.
    #[cfg(feature = "enterprise")]
    {
        dream_domain_billing::run_one_billing_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new(
                    "router.dream_domain_billing.migrate",
                    "failed to run one-billing migrations",
                )
                .with_source(e)
            })?;
    }
    // one-platform: deployment infra config (P1-3 container + P2-2 collab).
    #[cfg(feature = "enterprise")]
    {
        dream_domain_platform::run_one_platform_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new(
                    "router.dream_domain_platform.migrate",
                    "failed to run one-platform migrations",
                )
                .with_source(e)
            })?;
    }
    // one-workflow: approval tasks (P2-1).
    #[cfg(feature = "enterprise")]
    {
        dream_domain_workflow::run_one_workflow_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new(
                    "router.dream_domain_workflow.migrate",
                    "failed to run one-workflow migrations",
                )
                .with_source(e)
            })?;
    }
    // one-memory: collections / items / refine jobs / grants (P2-2).
    #[cfg(feature = "enterprise")]
    {
        dream_domain_memory::run_one_memory_migrations(services.database.pool())
            .await
            .map_err(|e| {
                RouterBuildError::new(
                    "router.dream_domain_memory.migrate",
                    "failed to run one-memory migrations",
                )
                .with_source(e)
            })?;
    }

    // Start channel orchestrator (message loop)
    tokio::spawn(
        channel_components
            .orchestrator
            .run(channel_components.message_rx, channel_components.confirm_rx),
    );
    tracing::info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: channel orchestrator spawned"
    );

    // Restore enabled channel plugins (starts receiving IM messages)
    let chan_mgr = channel_components.manager;
    let chan_factory = channel_components.plugin_factory;
    let chan_owner_user_id = channel_components.owner_user_id;
    tokio::spawn(async move {
        if let Some(chan_owner_user_id) = chan_owner_user_id {
            if let Err(e) = chan_mgr.restore_plugins(&chan_owner_user_id, &chan_factory).await {
                tracing::warn!(
                    code = "BOOTSTRAP_DEGRADED_CHANNEL_RESTORE",
                    stage = "channel.restore",
                    owner_user_id = %chan_owner_user_id,
                    error = %e,
                    "failed to restore channel plugins"
                );
            }
        } else {
            tracing::info!(
                stage = "channel.restore",
                "skipping channel plugin restore until an owner user is available"
            );
        }
    });
    tracing::info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: channel plugin restore scheduled"
    );

    tracing::info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: route tree build started"
    );
    // Spawn the Project Explorer filesystem monitor and install its inbound
    // router (fs/* frames). Built here — inside the runtime — because the actor
    // runs as a background task. The sync test-only assembly path keeps a no-op.
    let fs_router = spawn_fs_monitor(Arc::new(services.project_service.clone()), services.ws_manager.clone());
    // Source control shares the connection but owns its own envelope name, so the
    // two inbound routers are composed behind the realtime layer's single slot.
    let scm_router = spawn_scm_monitor(Arc::new(services.project_service.clone()), services.ws_manager.clone());
    let inbound_router: Arc<dyn dream_core_realtime::MessageRouter> = match scm_router {
        Some(scm) => Arc::new(CompositeMessageRouter::new(vec![fs_router, scm])),
        None => fs_router,
    };
    let ws_state = build_ws_state(services, inbound_router);
    let router = create_router_with_all_state(services, states, ws_state);
    tracing::info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: router assembly completed"
    );

    // T6 directory sync's two halves, for cmd_server's scheduler. Built here
    // rather than threaded out of the route builder because that function is
    // also a public test entry point returning only a Router. A second
    // `SsoService` over the same pool is harmless: the sync path touches only
    // the provider config table, never the in-memory OAuth state this instance
    // also carries.
    #[cfg(feature = "enterprise")]
    let directory_sso_service = Arc::new(dream_domain_sso::SsoService::new(
        services.database.pool().clone(),
        services.user_repo.clone(),
        services.jwt_service.clone(),
        services.cookie_config.clone(),
    ));
    #[cfg(feature = "enterprise")]
    let directory_sink: Arc<dyn dream_domain_sso::DirectorySink> = Arc::new(DirectorySinkAdapter(Arc::new(
        dream_domain_enterprise::EnterpriseService::new(services.database.pool().clone()),
    )));
    Ok((
        router,
        RouterRuntime {
            client_pref_service,
            team_service,
            #[cfg(feature = "enterprise")]
            sso_service: directory_sso_service,
            #[cfg(feature = "enterprise")]
            directory_sink,
        },
    ))
}

/// Create the application router with custom module states.
///
/// Used for testing when specific service overrides are needed
/// (e.g. injecting a mock HTTP server URL for version check).
pub fn create_router_with_states(services: &AppServices, states: ModuleStates) -> Router {
    // No-op inbound router: this sync assembly path is for HTTP-focused tests and
    // does not spawn the fs monitor (which requires a runtime task).
    let ws_state = build_ws_state(services, Arc::new(NoopMessageRouter));
    create_router_with_all_state(services, states, ws_state)
}

/// Create the application router with custom module states and WebSocket state.
///
/// Full-control variant used by tests that need to override
/// module services and WebSocket behaviour.
/// Bridges the enterprise resource-authorization matrix
/// (`dream-domain-platform`) to the four registries that read it
/// (`dream-domain-devops`). Same shape as every other adapter here: the
/// registries own a trait, the enterprise plane owns the data, and this layer
/// is the only place that knows both exist.
///
/// A matrix that cannot be read yields no grants rather than an error. The
/// registries then fall back to their own `scope`/`visibility` predicates,
/// which is the same posture the policy gates take: an unreachable enterprise
/// plane must not make a member's skills vanish from their own workbench.
#[cfg(feature = "enterprise")]
struct MatrixGrantSource {
    platform: std::sync::Arc<dream_domain_platform::PlatformService>,
    org: std::sync::Arc<dream_domain_org::OrgService>,
}

#[async_trait::async_trait]
#[cfg(feature = "enterprise")]
impl dream_domain_devops::grants::ResourceGrantSource for MatrixGrantSource {
    async fn extra_grants(
        &self,
        viewer_user_id: &str,
        resource_type: &str,
    ) -> dream_domain_devops::grants::ExtraGrants {
        let tenant_id = match self.org.tenant_of(viewer_user_id).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(user_id = viewer_user_id, error = %e, "resource matrix: no tenant for viewer; no extra grants");
                return Default::default();
            }
        };
        match self
            .platform
            .effective_resource_ids(&tenant_id, viewer_user_id, resource_type)
            .await
        {
            Ok(dto) => dream_domain_devops::grants::ExtraGrants {
                all: dto.all,
                ids: dto.resource_ids,
            },
            Err(e) => {
                tracing::warn!(user_id = viewer_user_id, resource_type, error = %e, "resource matrix unreadable; no extra grants");
                Default::default()
            }
        }
    }
}

/// Every route group that exists only when `enterprise` is compiled in.
///
/// Extracted so the full server and the standalone admin binary mount exactly
/// the same governance surface. Assembling it twice would let the two drift —
/// and a governance route that exists in one process but not the other is the
/// kind of difference nothing fails on until an operator hits it.
#[cfg(feature = "enterprise")]
pub(crate) struct GovernancePlane {
    pub org: Router,
    pub enterprise: Router,
    pub billing: Router,
    pub platform: Router,
    pub workflow: Router,
    pub memory: Router,
    pub sso_public: Router,
    pub sso_admin: Router,
    /// Handed back rather than kept: one-employee and one-devops both need it,
    /// and it can only be built from the org service constructed in here.
    pub tenant_resolver: std::sync::Arc<dyn dream_domain_employee::TenantResolver>,
    /// Same reason: the matrix needs both the platform service and the org
    /// service, and this is the only place that holds both.
    pub grant_source: std::sync::Arc<dyn dream_domain_devops::grants::ResourceGrantSource>,
    /// Handed back so the terminal-tool approval gate (`PlatformToolCallSecurityGate`)
    /// can create tasks and block on decisions without a second service over
    /// the same pool.
    pub workflow_service: std::sync::Arc<dream_domain_workflow::WorkflowService>,
    /// Handed back so `create_admin_router` can apply the same E4 module gate
    /// to `admin_devops_routes` (built outside this function, since it isn't
    /// part of the personal-plane-shared devops surface) without constructing
    /// a second `Arc<BillingService>` clone of its own.
    pub license_gate: LicenseModuleGateState,
    /// Handed back for the same reason as `license_gate`: `create_admin_router`
    /// applies this to `admin_devops_routes` too, and constructing a second
    /// one would just be a second `Arc` clone of the same `user_repo`.
    pub password_gate: PasswordChangeGateState,
}

/// Route-path-independent module identifier for the whole governance plane.
/// The license's `modules` field lets a vendor sell a module on a different
/// clock than the base subscription — [`LicensePayload::modules`]'s doc
/// comment gives "sell `/admin/*` on a different clock" as the worked
/// example — and this plane (one-org/enterprise/billing/platform/sso-admin
/// plus, in `create_admin_router`, the admin-only devops routes) IS that
/// module: there is no finer-grained sub-route licensing here, so one
/// constant covers every route [`license_module_gate_middleware`] guards.
#[cfg(feature = "enterprise")]
const ADMIN_MODULE: &str = "/admin/*";

#[cfg(feature = "enterprise")]
#[derive(Clone)]
pub(crate) struct LicenseModuleGateState {
    billing: std::sync::Arc<dream_domain_billing::BillingService>,
}

/// Enforces E4 per-module license authorization. Wired via `.route_layer`
/// placed BEFORE (earlier in the chain than) the `.route_layer(auth_middleware)`
/// call on the same router, so — per this codebase's own layering convention,
/// see `authenticated_action_rate_limit_middleware` in `dream-core-auth` for
/// the same pattern — it runs AFTER `auth_middleware` has already populated
/// `CurrentUser` (a later `.route_layer` call becomes the outer, first-run
/// layer; an earlier one runs closer to the handler).
///
/// Fails OPEN, not closed, at every step before an actual license is read: no
/// `CurrentUser` (shouldn't happen behind auth_middleware, but this gate has
/// no business 500ing if it does), no enterprise for this caller (a personal
/// / standalone user — `resolve_enterprise_id`'s own contract is "`None`
/// means skip every check", and it collapses a lookup error to the same
/// `None`), or no license activated at all. Only an ACTUAL activated license
/// whose non-empty `modules` list does not authorize [`ADMIN_MODULE`] can
/// produce a 403 — matching the "additive only, never a new restriction where
/// none existed" rule the rest of E4/E5 follows in this codebase.
#[cfg(feature = "enterprise")]
async fn license_module_gate_middleware(
    State(state): State<LicenseModuleGateState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(user) = request.extensions().get::<CurrentUser>().cloned() else {
        return Ok(next.run(request).await);
    };

    let enterprise_id = match state.billing.resolve_enterprise_id(&user.id).await {
        Ok(Some(id)) => id,
        Ok(None) | Err(_) => return Ok(next.run(request).await),
    };

    let license = match state.billing.active_license(&enterprise_id).await {
        Ok(Some(license)) => license,
        Ok(None) | Err(_) => return Ok(next.run(request).await),
    };

    match license.classify_path_access(request.uri().path(), dream_core_common::now_ms()) {
        dream_domain_billing::ModuleAccess::Authorized => Ok(next.run(request).await),
        dream_domain_billing::ModuleAccess::NotAuthorized => Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "LICENSE_MODULE_NOT_AUTHORIZED",
            "this license does not authorize the admin module",
            None,
        )),
        dream_domain_billing::ModuleAccess::Expired => Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "LICENSE_MODULE_EXPIRED",
            "the admin module authorization on this license has expired",
            None,
        )),
    }
}

#[cfg(feature = "enterprise")]
#[derive(Clone)]
pub(crate) struct PasswordChangeGateState {
    user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository>,
}

/// Blocks the enterprise governance surface for an account whose password
/// still needs to be changed — in practice, only the seeded
/// `system_default_user` on a deployment that has never logged in and set
/// its own password (see `AppServices::from_config`'s first-boot bootstrap,
/// which generates and flags this account, and the same doc comment on
/// `dream_core_db::models::User::must_change_password`).
///
/// Wired the same way as [`license_module_gate_middleware`] — `.route_layer`
/// placed BEFORE `.route_layer(auth_middleware)` so it runs AFTER
/// `auth_middleware` has populated `CurrentUser` — and for the same reason
/// does its own `user_repo` lookup rather than trusting a flag on
/// `CurrentUser`: that struct is shared by every authenticated route in the
/// app and deliberately does not carry this field (see
/// `dream_core_auth::routes::user_handler`'s doc comment for why extending
/// it was rejected as too broad a blast radius for this one gate).
///
/// Fails OPEN when there is no `CurrentUser` (shouldn't happen behind
/// `auth_middleware`, but this gate has no business 500ing if it does) or
/// when the lookup errors — the same "never manufacture a new failure mode
/// out of a gate that wasn't there before" posture `license_module_gate_middleware`
/// documents for itself.
#[cfg(feature = "enterprise")]
async fn require_password_changed_gate(
    State(state): State<PasswordChangeGateState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(user) = request.extensions().get::<CurrentUser>().cloned() else {
        return Ok(next.run(request).await);
    };

    let must_change_password = match state.user_repo.find_by_id(&user.id).await {
        Ok(Some(user)) => user.must_change_password,
        Ok(None) | Err(_) => false,
    };

    if must_change_password {
        return Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "PASSWORD_CHANGE_REQUIRED",
            "this account must change its password before continuing",
            None,
        ));
    }

    Ok(next.run(request).await)
}

/// The three services are built by the caller because they are needed earlier:
/// platform wires the IP allowlist into the auth middleware, billing is owned
/// by `AppServices` (the agent factory takes its model allowlist), and devops
/// is shared with the personal plane.
#[cfg(feature = "enterprise")]
pub(crate) fn build_governance_plane(
    services: &AppServices,
    auth_mw_state: &AuthState,
    one_devops_service: std::sync::Arc<dream_domain_devops::DevopsService>,
    one_platform_service: std::sync::Arc<dream_domain_platform::PlatformService>,
    one_billing_service: std::sync::Arc<dream_domain_billing::BillingService>,
) -> GovernancePlane {
    // Built first: every `_authenticated` router below layers this in ahead
    // of `auth_middleware` (see `license_module_gate_middleware`'s doc
    // comment for why that ordering, not the reverse, is what makes
    // `CurrentUser` visible to it).
    let license_gate = LicenseModuleGateState {
        billing: one_billing_service.clone(),
    };
    let password_gate = PasswordChangeGateState {
        user_repo: services.user_repo.clone(),
    };

    // one-org enterprise routes (/api/one/*) — RBAC extractors depend on the
    // upstream auth middleware injecting CurrentUser.
    let one_org_service = std::sync::Arc::new(
        dream_domain_org::OrgService::new(
            services.database.pool().clone(),
            services.user_repo.clone(),
            services.data_dir.clone(),
            crate::config::derive_encryption_key(&services.data_secret_raw),
        )
        .with_credential_revoker(std::sync::Arc::new(CompositeCredentialRevoker(vec![
            std::sync::Arc::new(ModelChannelRevoker(one_devops_service.clone())),
            std::sync::Arc::new(ApiKeyRevoker(one_platform_service.clone())),
        ]))),
    );
    // one-enterprise service (真实企业 / company tier) — constructed here so its
    // company-admin bridges can be wired into one-org and one-sso below, and so
    // it can borrow one-org's credential revocation (built just above).
    let one_enterprise_service = std::sync::Arc::new(
        dream_domain_enterprise::EnterpriseService::new(services.database.pool().clone())
            .with_session_revoker(std::sync::Arc::new(OrgSessionRevoker(one_org_service.clone())))
            .with_disband_cascade(std::sync::Arc::new(CompanyDisbandCascadeImpl {
                org: one_org_service.clone(),
                billing: one_billing_service.clone(),
            })),
    );
    // Tenant resolver shared by one-employee + one-devops for team-shared
    // employees (A1 L3).
    // Personal edition has no tenants, so no resolver: `tenant_of` then returns
    // `DEFAULT_TENANT` for everyone, which is exactly the personal semantics
    // (see `dream_domain_employee::state`).
    let tenant_resolver: std::sync::Arc<dyn dream_domain_employee::TenantResolver> =
        std::sync::Arc::new(OrgTenantResolver(one_org_service.clone()));
    // Direction B: let a company admin create/list the project groups their
    // company owns (system_admin still governs everything as before). T6
    // stage 3: also let a project-group admin map a company directory
    // subtree into their own department tree.
    let one_org_state = dream_domain_org::OneOrgRouterState::new(one_org_service.clone())
        .with_company_admin_resolver(std::sync::Arc::new(CompanyAdminResolverAdapter(
            one_enterprise_service.clone(),
        )))
        .with_directory_source(std::sync::Arc::new(DirectoryTreeSourceAdapter(
            one_enterprise_service.clone(),
        )))
        // Direction B: a project-group join whose tenant belongs to a company
        // also registers the joiner as a company member (see
        // `CompanySeatSyncAdapter` / `dream_domain_org::enterprise_hooks` docs).
        .with_company_seat_sync(std::sync::Arc::new(CompanySeatSyncAdapter(
            one_enterprise_service.clone(),
        )));
    let one_org_authenticated = dream_domain_org::one_org_routes(one_org_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-enterprise routes (/api/one/enterprise/*) — the SSO-company
    // "enterprise org" dimension + the company tier (Direction B). The service
    // was constructed above so the company-admin bridges could be wired.
    let one_enterprise_state = dream_domain_enterprise::OneEnterpriseRouterState::new(one_enterprise_service.clone());
    let one_enterprise_authenticated = dream_domain_enterprise::one_enterprise_routes(one_enterprise_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-billing routes (/api/one/billing/*) — subscription tier, seats, and
    // usage dashboard. No payment provider wired: manual provisioning via
    // `PUT /tier`. The service was built above (before the conversation routes)
    // so its usage recorder could be injected there.
    let one_billing_state = dream_domain_billing::OneBillingRouterState::new(one_billing_service.clone());
    // Deliberately NOT wrapped in `license_module_gate_middleware`: this is
    // where `GET/POST /api/one/billing/license` live (view + activate a
    // license). Gating license management behind the very license it manages
    // would be a lockout with no escape hatch — a company whose license
    // doesn't cover the admin module could never activate a corrective one.
    let one_billing_authenticated = dream_domain_billing::one_billing_routes(one_billing_state)
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-platform routes (/api/one/admin/platform/*) — deployment infra config
    // (P1-3 container runtime + P2-2 realtime collaboration). Reserved adapters:
    // the Noop defaults report "not configured" until a real runtime/provider
    // is wired here via `with_container_runtime` / `with_collaboration_provider`.
    // Service itself was built above, alongside `auth_mw_state`, so its IP
    // allowlist could be wired into the auth middleware.
    let one_platform_state = dream_domain_platform::OnePlatformRouterState::new(one_platform_service.clone());
    let one_platform_authenticated = dream_domain_platform::one_platform_routes(one_platform_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-sso routes. Public half (providers/authorize/callback) is
    // unauthenticated so OAuth can run before the user has a session;
    // admin half (upsert provider) sits behind the auth middleware.
    let one_sso_state =
        dream_domain_sso::OneSsoRouterState::new(std::sync::Arc::new(dream_domain_sso::SsoService::new(
            services.database.pool().clone(),
            services.user_repo.clone(),
            services.jwt_service.clone(),
            services.cookie_config.clone(),
        )))
        // Enterprise-org sync: a successful SSO login upserts the caller's company
        // + membership into one-enterprise. No-op (aside from the upsert) for
        // personal edition / WebUI-only builds since it never touches
        // `one_tenants` / project-group membership.
        .with_enterprise_sync(std::sync::Arc::new(EnterpriseSyncAdapter(
            one_enterprise_service.clone(),
        )))
        // Direction B: SSO config (企业认证) is a company-level policy, so a company
        // admin may manage it. Falls back to the project-group admin when unset.
        .with_company_admin_check(std::sync::Arc::new(CompanyAdminCheckAdapter(
            one_enterprise_service.clone(),
        )))
        // P2-4 onboarding: auto-join a project group by email-domain policy. No-op
        // for logins whose IdP profile isn't email-shaped, or when no tenant has
        // `allowed_email_domains` set (the default).
        .with_org_auto_join(std::sync::Arc::new(OrgAutoJoinAdapter(one_org_service.clone())))
        // T6 directory sync: where a completed Feishu directory pull is stored.
        // Wiring it does not start anything — a pull only happens when an admin
        // asks or the scheduler fires, and both find nothing to do unless this
        // machine actually holds the company's SSO config.
        .with_directory_sink(std::sync::Arc::new(DirectorySinkAdapter(
            one_enterprise_service.clone(),
        )));
    let one_sso_public = dream_domain_sso::one_sso_public_routes(one_sso_state.clone());
    let one_sso_admin = dream_domain_sso::one_sso_admin_routes(one_sso_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
    let grant_source: std::sync::Arc<dyn dream_domain_devops::grants::ResourceGrantSource> =
        std::sync::Arc::new(MatrixGrantSource {
            platform: one_platform_service.clone(),
            org: one_org_service.clone(),
        });

    // one-workflow routes (/api/workflow/*) — the approval subsystem (P2-1).
    // Members submit and watch their own tasks; the pending queue and every
    // decision belong to the admin group. The service handle is handed back
    // on the plane so the terminal-tool approval gate can block on it (see
    // `PlatformToolCallSecurityGate`).
    let one_workflow_service = std::sync::Arc::new(dream_domain_workflow::WorkflowService::new(
        services.database.pool().clone(),
    ));
    let one_workflow_state = dream_domain_workflow::OneWorkflowRouterState::new(one_workflow_service.clone());
    let one_workflow_authenticated = dream_domain_workflow::one_workflow_routes(one_workflow_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-memory routes (/api/one/{admin/,}memory/*) — the memory subsystem
    // (P2-2): three collection tiers, refinement jobs, and read/write
    // grants. Same governance-plane assembly so a later admin-svc split can
    // lift it out whole.
    let one_memory_service = std::sync::Arc::new(dream_domain_memory::MemoryService::new(
        services.database.pool().clone(),
    ));
    let one_memory_state = dream_domain_memory::OneMemoryRouterState::new(one_memory_service);
    let one_memory_authenticated = dream_domain_memory::one_memory_routes(one_memory_state)
        .route_layer(from_fn_with_state(license_gate.clone(), license_module_gate_middleware))
        .route_layer(from_fn_with_state(password_gate.clone(), require_password_changed_gate))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    GovernancePlane {
        org: one_org_authenticated,
        enterprise: one_enterprise_authenticated,
        billing: one_billing_authenticated,
        platform: one_platform_authenticated,
        workflow: one_workflow_authenticated,
        memory: one_memory_authenticated,
        sso_public: one_sso_public,
        sso_admin: one_sso_admin,
        tenant_resolver,
        grant_source,
        workflow_service: one_workflow_service,
        license_gate,
        password_gate,
    }
}

/// Route tree for the standalone `dreamcore-admin` binary.
///
/// Only the governance plane (`/api/one/{org,enterprise,billing,admin,sso}/*`
/// — built by [`build_governance_plane`], the exact same assembly the main
/// server mounts, so the two processes can never drift) plus the three
/// admin-facing devops route groups ([`dream_domain_devops::admin_devops_routes`]:
/// DLP rule authoring, model-channel deletion, offboarding ownership
/// transfer). No conversation / agent / mcp / file / channel / cron /
/// WebSocket surface, and no `/api/auth/*` — login happens against the main
/// `dreamcore` process (proxied at the gateway's catch-all) and the resulting
/// session cookie is presented to this process directly, since both trust the
/// same JWT secret out of the same database.
#[cfg(feature = "enterprise")]
pub async fn create_admin_router(services: &AppServices) -> Result<Router, RouterBuildError> {
    // Same five migrations `create_router_with_runtime` runs (see there for
    // why billing must come after enterprise). `dream_domain_employee` is
    // deliberately not run here: admin never touches its tables — see
    // dream-en's docs/roadmap.zh-CN.md, E1.5's per-route survey.
    dream_domain_org::run_one_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new("router.dream_domain_org.migrate", "failed to run one-org migrations").with_source(e)
        })?;
    dream_domain_sso::run_one_sso_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new("router.dream_domain_sso.migrate", "failed to run one-sso migrations").with_source(e)
        })?;
    dream_domain_devops::run_one_devops_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_devops.migrate",
                "failed to run one-devops migrations",
            )
            .with_source(e)
        })?;
    dream_domain_enterprise::run_one_enterprise_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_enterprise.migrate",
                "failed to run one-enterprise migrations",
            )
            .with_source(e)
        })?;
    // MUST run after one-enterprise: billing_001_init grandfathers existing
    // one_enterprises rows to the top tier.
    dream_domain_billing::run_one_billing_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_billing.migrate",
                "failed to run one-billing migrations",
            )
            .with_source(e)
        })?;
    dream_domain_platform::run_one_platform_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_platform.migrate",
                "failed to run one-platform migrations",
            )
            .with_source(e)
        })?;
    dream_domain_workflow::run_one_workflow_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_workflow.migrate",
                "failed to run one-workflow migrations",
            )
            .with_source(e)
        })?;
    dream_domain_memory::run_one_memory_migrations(services.database.pool())
        .await
        .map_err(|e| {
            RouterBuildError::new(
                "router.dream_domain_memory.migrate",
                "failed to run one-memory migrations",
            )
            .with_source(e)
        })?;

    // Built ahead of `AuthState` so the IP allowlist can be wired into the
    // auth middleware — mirrors `create_router_with_all_state`.
    let one_platform_service = std::sync::Arc::new(
        dream_domain_platform::PlatformService::new(
            services.database.pool().clone(),
            crate::config::derive_encryption_key(&services.data_secret_raw),
        )
        // P2-4 personal file vault: same shared data dir / storage root as
        // the main binary (see create_router_with_runtime's wiring).
        .with_storage_root(services.data_dir.join("file-vault")),
    );
    let policy_grace = services.policy_grace.clone();
    let ip_allowlist: Option<std::sync::Arc<dyn dream_core_auth::IpAllowlistGate>> =
        Some(std::sync::Arc::new(PlatformIpAllowlistGate {
            platform: one_platform_service.clone(),
            grace: policy_grace.clone(),
        }));
    let api_key_gate: Option<std::sync::Arc<dyn dream_core_auth::ApiKeyGate>> =
        Some(std::sync::Arc::new(PlatformApiKeyGate {
            platform: one_platform_service.clone(),
        }));
    let auth_mw_state = AuthState {
        jwt_service: services.jwt_service.clone(),
        user_repo: services.user_repo.clone(),
        identity_mode: auth_identity_mode(services.identity_mode),
        // No conversation-helper CLI ever calls this process.
        runtime_token_verifier: None,
        ip_allowlist,
        api_key_gate,
    };

    let one_devops_service = std::sync::Arc::new(
        dream_domain_devops::DevopsService::new(services.database.pool().clone())
            .with_encryption_key(crate::config::derive_encryption_key(&services.data_secret_raw)),
    );
    let one_billing_service = services.billing.clone();

    let governance = build_governance_plane(
        services,
        &auth_mw_state,
        one_devops_service.clone(),
        one_platform_service.clone(),
        one_billing_service.clone(),
    );
    // Same wiring as the full server: the console's own registry reads must see
    // the matrix too, or an admin would be shown a different set of skills here
    // than the members they granted them to actually get.
    one_devops_service.set_grants(governance.grant_source.clone());

    let one_devops_state = dream_domain_devops::OneDevopsRouterState::new(one_devops_service)
        .with_tenant_resolver(governance.tenant_resolver.clone());
    // Admin-only devops routes (DLP rule authoring, model-channel deletion,
    // offboarding ownership transfer) — gated same as the rest of the
    // governance plane. Unlike `build_governance_plane`'s five routers, this
    // one isn't shared with the personal-plane's own `one_devops_routes`
    // (registry reads any member can make), so it's wired here rather than
    // there.
    let admin_devops_authenticated = dream_domain_devops::admin_devops_routes(one_devops_state)
        .route_layer(from_fn_with_state(
            governance.license_gate.clone(),
            license_module_gate_middleware,
        ))
        .route_layer(from_fn_with_state(
            governance.password_gate.clone(),
            require_password_changed_gate,
        ))
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    let router = Router::new()
        .route("/health", get(health_check))
        .merge(governance.org)
        .merge(governance.enterprise)
        .merge(governance.billing)
        .merge(governance.platform)
        .merge(governance.workflow)
        .merge(governance.memory)
        .merge(governance.sso_public)
        .merge(governance.sso_admin)
        .merge(admin_devops_authenticated)
        .layer(middleware::from_fn_with_state(
            services.cookie_config.clone(),
            csrf_middleware,
        ))
        .layer(middleware::from_fn(security_headers_middleware));

    let router = router.layer(DefaultBodyLimit::max(dream_core_common::constants::BODY_LIMIT));
    let router = router.layer(middleware::from_fn(normalize_boundary_error_response));
    let router = with_access_log(router);

    // Governance calls only ever arrive same-origin through the gateway
    // (admin console, admin API), so credentialed CORS with the request
    // origin reflected back is all that's needed — no bespoke local-mode
    // wildcard carve-out like the personal workbench has.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            HeaderName::from_static("x-csrf-token"),
        ]);

    Ok(router.layer(cors))
}

pub fn create_router_with_all_state(services: &AppServices, states: ModuleStates, ws_state: WsHandlerState) -> Router {
    let boot = Instant::now();
    tracing::info!("startup: route tree build with states started");

    let auth_state = AuthRouterState {
        jwt_service: services.jwt_service.clone(),
        user_repo: services.user_repo.clone(),
        fs_adopter: Some(Arc::new(SkillFilesystemAdopter {
            skill_paths: services.skill_paths.clone(),
            skill_repo: services.skill_repo.clone(),
        })),
        cookie_config: services.cookie_config.clone(),
        qr_token_store: services.qr_token_store.clone(),
        identity_mode: auth_identity_mode(services.identity_mode),
        bootstrap_secret: services.bootstrap_secret.clone(),
        session_revoked_hook: {
            let ws_manager = services.ws_manager.clone();
            let conversation_service = states.conversation.service.clone();
            let team_service = states.team.service.clone();
            let channel_manager = states.channel.manager.clone();
            let channel_session_manager = states.channel.session_manager.clone();
            let file_watch_service = states.file.watch_service.clone();
            let office_watch_manager = states.office.watch_manager.clone();
            Some(Arc::new(move |user_id: &str| {
                ws_manager.disconnect_user(user_id, "session revoked");
                let stopped_team_sessions = team_service.stop_sessions_for_user(user_id);
                if stopped_team_sessions > 0 {
                    tracing::info!(
                        user_id = %user_id,
                        stopped_team_sessions,
                        "stopped team sessions after session revocation"
                    );
                }
                let user_id = user_id.to_owned();
                let conversation_service = conversation_service.clone();
                let channel_manager = channel_manager.clone();
                let channel_session_manager = channel_session_manager.clone();
                let file_watch_service = file_watch_service.clone();
                let office_watch_manager = office_watch_manager.clone();
                tokio::spawn(async move {
                    channel_manager.shutdown_for_user(&user_id).await;
                    office_watch_manager.stop_all_for_user(&user_id);
                    if let Err(err) = channel_session_manager.clear_all_sessions(&user_id).await {
                        tracing::warn!(
                            user_id = %user_id,
                            error = %err,
                            "failed to clear channel sessions after session revocation"
                        );
                    }
                    if let Err(err) = conversation_service.terminate_runtime_for_user(&user_id).await {
                        tracing::warn!(
                            user_id = %user_id,
                            error = %err,
                            "failed to terminate runtimes after session revocation"
                        );
                    }
                    if let Err(err) = file_watch_service.stop_all_watches_for_user(&user_id).await {
                        tracing::warn!(
                            user_id = %user_id,
                            error = %err,
                            "failed to stop file watches after session revocation"
                        );
                    }
                    if let Err(err) = file_watch_service.stop_all_office_watches_for_user(&user_id).await {
                        tracing::warn!(
                            user_id = %user_id,
                            error = %err,
                            "failed to stop office file watches after session revocation"
                        );
                    }
                });
            }))
        },
        local: services.local,
        aionpro_mode: services.identity_mode == crate::config::IdentityMode::AionPro,
    };

    // one-platform service (IP allowlist among other deployment-infra config)
    // — built here, ahead of its own route mount further down, purely so the
    // auth middleware can wire the allowlist check into every route group
    // through a single shared `AuthState`. Cheap to construct this early:
    // only needs the pool + encryption key, both already on `services`.
    #[cfg(feature = "enterprise")]
    let one_platform_service = std::sync::Arc::new(
        dream_domain_platform::PlatformService::new(
            services.database.pool().clone(),
            crate::config::derive_encryption_key(&services.data_secret_raw),
        )
        // P2-4 personal file vault: objects live under the shared data dir,
        // same volume both binaries see (T2 verified concurrent access).
        .with_storage_root(services.data_dir.join("file-vault")),
    );

    // One tracker per process, shared by every enterprise gate: they all read
    // the same plane, so one of them reaching it proves the plane is alive for
    // the others too.
    #[cfg(feature = "enterprise")]
    let policy_grace = services.policy_grace.clone();

    // Personal edition has no deployment-infra config to enforce, so no
    // allowlist: `None` is the pre-platform behaviour (every source allowed).
    #[cfg(feature = "enterprise")]
    let ip_allowlist: Option<std::sync::Arc<dyn dream_core_auth::IpAllowlistGate>> =
        Some(std::sync::Arc::new(PlatformIpAllowlistGate {
            platform: one_platform_service.clone(),
            grace: policy_grace.clone(),
        }));
    #[cfg(not(feature = "enterprise"))]
    let ip_allowlist: Option<std::sync::Arc<dyn dream_core_auth::IpAllowlistGate>> = None;

    // Same posture as the allowlist: only a real enterprise deployment has
    // any `one_api_keys` rows to authenticate against.
    #[cfg(feature = "enterprise")]
    let api_key_gate: Option<std::sync::Arc<dyn dream_core_auth::ApiKeyGate>> =
        Some(std::sync::Arc::new(PlatformApiKeyGate {
            platform: one_platform_service.clone(),
        }));
    #[cfg(not(feature = "enterprise"))]
    let api_key_gate: Option<std::sync::Arc<dyn dream_core_auth::ApiKeyGate>> = None;

    let auth_mw_state = AuthState {
        jwt_service: services.jwt_service.clone(),
        user_repo: services.user_repo.clone(),
        identity_mode: auth_identity_mode(services.identity_mode),
        runtime_token_verifier: Some(Arc::new(ConversationHelperTokenVerifier {
            runtime_token_service: services.runtime_token_service.clone(),
        })),
        ip_allowlist,
        api_key_gate,
    };

    // System routes protected by auth middleware
    let system_authenticated =
        system_routes(states.system).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-billing service (subscription tier / seats / usage). Built in
    // `AppServices` rather than here: the agent factory is assembled before any
    // router exists and needs the same instance for the vision-delegate model
    // allowlist (`BillingModelAllowlistGate`). It stays dependency-free (pool +
    // manual provider), so nothing about the construction order changes.
    #[cfg(feature = "enterprise")]
    let one_billing_service = services.billing.clone();

    // P0-3 usage metering: wired onto the shared `ConversationService`
    // (interior-mutability setter, like `with_project_service` below) rather
    // than the per-router-mount `ConversationRouterState` builder chain —
    // `ConversationTurnOrchestrator` fires this from inside the service
    // itself once a turn actually completes, so every clone of the service
    // (HTTP routes, cron, team) needs to see the same wiring, not just
    // whichever local variable this chain happens to run through.
    #[cfg(feature = "enterprise")]
    {
        states
            .conversation
            .service
            .with_usage_recorder(std::sync::Arc::new(BillingUsageRecorder(one_billing_service.clone())));
        // P2-5 per-call trace, same slot-and-adapter shape as the recorder
        // above — a separate setter because `with_usage_recorder` (like every
        // interior-mutability setter here) returns `()`, not the service.
        states
            .conversation
            .service
            .with_llm_trace_recorder(std::sync::Arc::new(BillingLlmCallTrace(one_billing_service.clone())));
    }

    // P1-2 send policy gate (budget/rate; T3), same reasoning and the same
    // interior-mutability setter as `with_usage_recorder` right above: this
    // must reach every clone of the service, including the ones cron and the
    // IM-channel message service hold, not just whichever router mount this
    // local variable chains through. No gate in the personal edition — `None`
    // skips the check entirely (see `ConversationService::send_message`),
    // which is the pre-billing path.
    #[cfg(feature = "enterprise")]
    states
        .conversation
        .service
        .with_send_gate(std::sync::Arc::new(EnterpriseSendGate {
            billing: BillingSendGate {
                billing: one_billing_service.clone(),
                grace: policy_grace.clone(),
            },
            rate: SecurityPolicySendRateGate::new(one_platform_service.clone(), policy_grace.clone()),
        }));

    // Conversation routes protected by auth middleware.
    let conversation_state =
        states
            .conversation
            .clone()
            .with_content_inspector(std::sync::Arc::new(LocalContentInspector(
                services.content_inspection.clone(),
            )));
    let conversation_authenticated =
        conversation_routes(conversation_state).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // The ops router hosts set-config-option (model switch) — gate it too so
    // the P1-2 model allowlist is enforced at model selection.
    let conversation_ops_state = states.conversation;
    #[cfg(feature = "enterprise")]
    let conversation_ops_state = conversation_ops_state.with_send_gate(std::sync::Arc::new(BillingSendGate {
        billing: one_billing_service.clone(),
        grace: policy_grace.clone(),
    }));
    let conversation_ops_authenticated = conversation_ops_routes(conversation_ops_state)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Remote agent routes protected by auth middleware
    let remote_agent_authenticated = remote_agent_routes(states.remote_agent)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Unified agent listing/refresh/test routes protected by auth middleware
    let agent_authenticated =
        agent_routes(states.agent).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Connection test routes (Bedrock, Gemini) protected by auth middleware
    let connection_test_authenticated = connection_test_routes(states.connection_test)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // File routes protected by auth middleware
    let file_authenticated =
        file_routes(states.file).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Project control-plane routes protected by auth middleware
    let project_authenticated =
        project_routes(states.project).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // MCP routes protected by auth middleware
    let mcp_authenticated =
        mcp_routes(states.mcp).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Extension routes protected by auth middleware
    let extension_authenticated =
        extension_routes(states.extension).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Hub routes protected by auth middleware
    let hub_authenticated =
        hub_routes(states.hub).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Skill routes protected by auth middleware
    let skill_authenticated =
        skill_routes(states.skill).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Channel routes protected by auth middleware
    #[cfg(feature = "weixin")]
    let weixin_login_authenticated = weixin_login_route(states.channel.clone())
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
    let channel_authenticated =
        channel_routes(states.channel).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Team routes protected by auth middleware. Clone the team session
    // service out before moving the state into team_routes — one-employee
    // needs it for /run-team.
    let team_session_service = states.team.service.clone();
    let team_authenticated =
        team_routes(states.team.clone()).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Cron routes protected by auth middleware
    let cron_authenticated =
        cron_routes(states.cron).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Office routes protected by auth middleware
    let office_authenticated =
        office_routes(states.office.clone()).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Shell + STT routes protected by auth middleware
    let shell_authenticated =
        shell_routes(states.shell).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Assistant routes protected by auth middleware (T1a skeleton: all
    // handlers return 500 "not implemented"; T1b wires real service)
    let assistant_authenticated =
        assistant_routes(states.assistant).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Codex-bridge *settings* routes (which saved provider/model it forwards
    // to) are app-facing config, protected like any other authenticated
    // route. The bridge's own `/v1/responses` surface is registered
    // separately below, unauthenticated at the session-cookie layer — Codex
    // is an external process with no browser session, and gates itself with
    // its own bearer token instead (see `dream-codex-bridge::routes`).
    let codex_bridge_config_authenticated = codex_bridge_config_routes(states.codex_bridge.clone())
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Claude bridge settings: app-facing config only, no public/unauthenticated
    // surface — unlike Codex, the resolved provider is injected directly as
    // launch-time env vars (no local HTTP proxy for Claude Code to call).
    let claude_bridge_config_authenticated = claude_bridge_config_routes(states.claude_bridge.clone())
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // Built here, ahead of one-org, only because removing a member has to be
    // able to revoke that member's model channel tokens (see
    // `ModelChannelRevoker`). Its routes are still assembled further down with
    // the rest of one-devops.
    //
    // The data key is for company model channels: their credential is stored
    // encrypted and decrypted only inside the model proxy, so it never reaches
    // a member's machine.
    let one_devops_service = std::sync::Arc::new(
        dream_domain_devops::DevopsService::new(services.database.pool().clone())
            .with_encryption_key(crate::config::derive_encryption_key(&services.data_secret_raw)),
    );

    #[cfg(feature = "enterprise")]
    let governance = build_governance_plane(
        services,
        &auth_mw_state,
        one_devops_service.clone(),
        one_platform_service.clone(),
        one_billing_service.clone(),
    );
    #[cfg(feature = "enterprise")]
    let tenant_resolver = governance.tenant_resolver.clone();
    // Widens the four registries' read predicates with the matrix. Must run on
    // the same service instance the routes were built from — hence `set` on a
    // shared handle rather than a builder.
    #[cfg(feature = "enterprise")]
    one_devops_service.set_grants(governance.grant_source.clone());

    // one-employee digital employee routes (/api/one/employee/*).
    // Wire the team session service so /run-team can drive existing team
    // slots; spawn the 30s schedule scanner for cron-driven runs.
    let one_employee_service = std::sync::Arc::new(
        dream_domain_employee::EmployeeService::new(
            services.database.pool().clone(),
            std::sync::Arc::new(services.conversation_service.clone()),
            std::sync::Arc::new(dream_core_db::SqliteConversationRepository::new(
                services.database.pool().clone(),
            )),
            services.agent_registry.clone(),
            services.work_dir.clone(),
        )
        .with_team_session(team_session_service)
        // Lets an employee's dream model be validated against enabled
        // providers at save time instead of failing the run.
        .with_provider_repo(std::sync::Arc::new(dream_core_db::SqliteProviderRepository::new(
            services.database.pool().clone(),
        ))),
    );
    one_employee_service.spawn_scheduler();
    let one_employee_state = dream_domain_employee::OneEmployeeRouterState::new(one_employee_service.clone());
    #[cfg(feature = "enterprise")]
    let one_employee_state = one_employee_state.with_tenant_resolver(tenant_resolver.clone());
    let one_employee_authenticated = dream_domain_employee::one_employee_routes(one_employee_state)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-devops routes (/api/one/devops/*) — requirements board +
    // collaboration registries, member-writable behind auth.
    // `one_devops_service` was built earlier (see the comment there).
    // Installs whose knowledge base predates hybrid retrieval have chunks in
    // SQLite but no lexical index yet. The rebuild reads only text already
    // stored, so it costs no embedding-API calls, and it self-skips once
    // populated — a no-op on every subsequent boot and on personal installs
    // with no knowledge base at all.
    {
        let service = one_devops_service.clone();
        tokio::spawn(async move {
            match service.rebuild_lexical_index().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(chunks = n, "team knowledge lexical index built"),
                Err(e) => {
                    tracing::warn!(error = %e, "lexical index build failed; retrieval stays vector-only")
                }
            }
        });
    }
    let one_devops_state =
        dream_domain_devops::OneDevopsRouterState::new(one_devops_service).with_employee(one_employee_service.clone());
    #[cfg(feature = "enterprise")]
    let one_devops_state = one_devops_state.with_tenant_resolver(tenant_resolver.clone());
    let one_devops_authenticated = dream_domain_devops::one_devops_routes(one_devops_state.clone())
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
    // Not session-authenticated, for the same reason the Codex bridge is not:
    // the caller is an agent process presenting a channel token, not a browser
    // with a session. See `dream_domain_devops::model_proxy`.
    let model_proxy_public = dream_domain_devops::model_proxy_routes(one_devops_state);
    // Office proxy routes serve iframe content but still require auth so
    // preview ports remain scoped to the active Core user.
    let office_proxy =
        office_proxy_routes(states.office).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
    let public_assets = asset_routes(AssetRouterState::default());
    // Not session-authenticated: Codex CLI is an external process with no
    // browser session. Gated by its own per-installation bearer token
    // instead (checked inside the handler; see `dream-codex-bridge`).
    let codex_bridge_public = codex_bridge_public_routes(states.codex_bridge);

    // WebSocket upgrade route — exempt from CSRF (no cookie-based
    // double-submit) but still gets security response headers.
    let ws_routes = Router::new().route("/ws", get(ws_upgrade_handler)).with_state(ws_state);
    let runtime_team_tools = runtime_team_tools_routes(RuntimeTeamToolsState {
        team_service: states.team.service.clone(),
        runtime_token_service: services.runtime_token_service.clone(),
    });
    tracing::info!(elapsed_ms = boot.elapsed().as_millis(), "startup: route groups built");

    // Antigravity permission hook callback. Deliberately NOT behind
    // auth_middleware: the hook is a local process with no user session and
    // presents a per-conversation token instead (checked in the handler).
    let antigravity_hook = crate::router::antigravity_hook::antigravity_hook_routes(
        crate::router::antigravity_hook::AntigravityHookRouterState {
            task_manager: services.worker_task_manager.clone(),
            tokens: services.antigravity_hook_tokens.clone(),
        },
    );

    let router = Router::new()
        .merge(antigravity_hook)
        .route("/health", get(health_check))
        .merge(auth_routes(auth_state))
        .merge(system_authenticated)
        .merge(conversation_authenticated)
        .merge(conversation_ops_authenticated)
        .merge(remote_agent_authenticated)
        .merge(agent_authenticated)
        .merge(connection_test_authenticated)
        .merge(file_authenticated)
        .merge(project_authenticated)
        .merge(mcp_authenticated)
        .merge(extension_authenticated)
        .merge(hub_authenticated)
        .merge(skill_authenticated)
        .merge(channel_authenticated)
        .merge(team_authenticated)
        .merge(cron_authenticated)
        .merge(office_authenticated)
        .merge(shell_authenticated)
        .merge(assistant_authenticated)
        .merge(codex_bridge_config_authenticated)
        .merge(claude_bridge_config_authenticated)
        .merge(one_employee_authenticated)
        .merge(one_devops_authenticated);

    // Enterprise governance plane. Absent from the personal edition entirely —
    // not merely hidden behind a role check, which is all that guarded the
    // admin console before the split.
    #[cfg(feature = "enterprise")]
    let router = router
        .merge(governance.org)
        .merge(governance.enterprise)
        .merge(governance.billing)
        .merge(governance.platform)
        .merge(governance.workflow)
        .merge(governance.memory)
        .merge(governance.sso_public)
        .merge(governance.sso_admin);

    // Personal edition: the desktop shell still asks "am I in an org?" on every
    // launch, and a 404 there is not the same answer as "no" — without these the
    // identity entry renders an error state instead of the personal one. Behind
    // the same auth layer the real routes use, so `CurrentUser` is injected:
    // the context handler reads it to answer with the caller's role, exactly as
    // `OrgService::effective_role` would. See `personal_identity_routes`.
    #[cfg(not(feature = "enterprise"))]
    let router = router
        .merge(personal_identity_routes().route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware)));

    // Conditionally merge WeChat login SSE route (feature-gated)
    #[cfg(feature = "weixin")]
    let router = router.merge(weixin_login_authenticated);

    let router = if services.identity_mode.is_local() {
        router
    } else {
        router.layer(middleware::from_fn_with_state(
            services.cookie_config.clone(),
            csrf_middleware,
        ))
    }
    .merge(ws_routes)
    .merge(runtime_team_tools)
    .merge(office_proxy)
    .merge(public_assets)
    .merge(codex_bridge_public)
    .merge(model_proxy_public)
    .layer(middleware::from_fn(security_headers_middleware));

    // Raise the default request body limit from axum's 2MB default to
    // `BODY_LIMIT` (10MB). Routes that need a larger cap (e.g. `/api/fs/upload`)
    // disable this default and install their own `RequestBodyLimitLayer`.
    let router = router.layer(DefaultBodyLimit::max(dream_core_common::constants::BODY_LIMIT));
    let router = router.layer(middleware::from_fn(normalize_boundary_error_response));

    let router = with_access_log(router);
    tracing::info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: route tree build with states completed"
    );

    // Local mode keeps the wildcard origin: without allow_credentials a
    // cross-origin request never carries the session cookie, so the cookie
    // path stays CSRF-protected and cross-origin callers must present a
    // Bearer token explicitly. That is what lets the desktop client (file://
    // or localhost origin) talk to a remote enterprise server.
    if services.identity_mode.is_local() {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(Any);
        router.layer(cors)
    } else {
        // Non-local (external identity) mode: the desktop renderer is a
        // cross-origin browser context (localhost:5173 in dev, file:// when
        // packaged) authenticating with the session cookie, so responses must
        // opt in to credentialed CORS. Credentialed mode forbids wildcards:
        // reflect the request origin and enumerate headers explicitly
        // (x-csrf-token is required by the CSRF double-submit middleware).
        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::mirror_request())
            .allow_credentials(true)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::CONTENT_TYPE,
                header::AUTHORIZATION,
                HeaderName::from_static("x-csrf-token"),
            ]);
        router.layer(cors)
    }
}

/// Adapter running the on-disk side of Dream UI → DreamPro adoption over the
/// skill filesystem (auth crate cannot depend on the extension/filesystem).
struct SkillFilesystemAdopter {
    skill_paths: Arc<dream_core_extension::SkillPaths>,
    skill_repo: Arc<dyn dream_core_db::ISkillRepository>,
}

#[async_trait::async_trait]
impl SystemDefaultFilesystemAdopter for SkillFilesystemAdopter {
    async fn adopt_filesystem(&self, adopter_user_id: &str) {
        dream_core_extension::fs_adopt::adopt_user_filesystem(
            self.skill_paths.as_ref(),
            self.skill_repo.as_ref(),
            adopter_user_id,
        )
        .await;
    }
}

/// Adapter exposing the agent runtime's token service to the auth middleware
/// as the conversation-helper credential channel (dream-auth cannot depend on
/// dream-ai-agent, so the binding happens here in the composition layer).
struct ConversationHelperTokenVerifier {
    runtime_token_service: Arc<RuntimeTokenService>,
}

impl IRuntimeTokenVerifier for ConversationHelperTokenVerifier {
    fn verify_conversation_helper(&self, token: &str, user_id: &str, conversation_id: &str) -> bool {
        self.runtime_token_service
            .validate(
                Some(token),
                user_id,
                conversation_id,
                RuntimeTokenScope::ConversationHelper,
                TEAM_RUNTIME_TOKEN_SESSION_GENERATION,
            )
            .is_ok()
    }
}

fn auth_identity_mode(identity_mode: crate::config::IdentityMode) -> AuthIdentityMode {
    match identity_mode {
        crate::config::IdentityMode::Local => AuthIdentityMode::Local,
        crate::config::IdentityMode::WebUi => AuthIdentityMode::UserSession,
        crate::config::IdentityMode::AionPro => AuthIdentityMode::AionPro,
    }
}

fn is_global_websocket_event(event_name: &str) -> bool {
    matches!(
        event_name,
        "runtime.statusChanged" | "extensions.lifecycle" | "hub.state-changed"
    )
}

async fn normalize_boundary_error_response(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status().is_success() || response_has_json_content_type(&response) {
        return response;
    }

    let status = response.status();
    let Some((error, code)) = boundary_error_for_status(status) else {
        return response;
    };

    let original_headers = response.headers().clone();
    let mut normalized = (status, Json(ErrorResponse::new(error, code))).into_response();
    normalized.extensions_mut().insert(ApiErrorLogContext {
        code,
        message: error.to_owned(),
    });
    for (name, value) in original_headers.iter() {
        if *name != header::CONTENT_TYPE && *name != header::CONTENT_LENGTH {
            normalized.headers_mut().insert(name, value.clone());
        }
    }
    normalized
}

fn response_has_json_content_type(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("application/json"))
}

fn boundary_error_for_status(status: StatusCode) -> Option<(&'static str, &'static str)> {
    match status {
        StatusCode::BAD_REQUEST => Some(("Bad request.", "BAD_REQUEST")),
        StatusCode::UNAUTHORIZED => Some(("Unauthorized.", "UNAUTHORIZED")),
        StatusCode::FORBIDDEN => Some(("Forbidden.", "FORBIDDEN")),
        StatusCode::NOT_FOUND => Some(("Route not found.", "NOT_FOUND")),
        StatusCode::METHOD_NOT_ALLOWED => Some(("Method not allowed.", "METHOD_NOT_ALLOWED")),
        StatusCode::CONFLICT => Some(("Conflict.", "CONFLICT")),
        StatusCode::GONE => Some(("Gone.", "GONE")),
        StatusCode::PAYLOAD_TOO_LARGE => Some(("Request body is too large.", "PAYLOAD_TOO_LARGE")),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => Some(("Unsupported media type.", "UNSUPPORTED_MEDIA_TYPE")),
        StatusCode::UNPROCESSABLE_ENTITY => Some(("Unprocessable entity.", "UNPROCESSABLE_ENTITY")),
        StatusCode::TOO_MANY_REQUESTS => Some(("Rate limited", "RATE_LIMITED")),
        StatusCode::INTERNAL_SERVER_ERROR => Some(("Internal server error.", "INTERNAL_ERROR")),
        StatusCode::BAD_GATEWAY => Some(("Upstream service unavailable.", "BAD_GATEWAY")),
        StatusCode::GATEWAY_TIMEOUT => Some(("Request timed out.", "GATEWAY_TIMEOUT")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use dream_core_api_types::WebSocketMessage;
    use dream_core_realtime::{BroadcastEventBus, EventBroadcaster, WebSocketManager, WsOutbound};
    use serde_json::json;

    #[cfg(feature = "enterprise")]
    use super::{
        ENTERPRISE_POLICY_GRACE_MS, PlatformToolCallSecurityGate, PolicyGrace, PolicyVerdict,
        SecurityPolicySendRateGate, SendRateLimiter, billing_denial,
    };
    use super::{
        boundary_error_for_status, create_router_with_runtime, forward_event_bus_to_websocket,
        is_global_websocket_event,
    };
    use crate::config::AppConfig;
    use crate::services::AppServices;

    /// The two policy refusals are written for the member and must survive with
    /// their parameters, so a client can say them in the reader's language.
    #[cfg(feature = "enterprise")]
    #[test]
    fn billing_policy_denials_keep_their_message_and_parameters() {
        let PolicyVerdict::Governs(model) =
            billing_denial(dream_domain_billing::BillingError::ModelNotAllowed("gpt-9".into()))
        else {
            panic!("an allowlist refusal is the policy answering");
        };
        assert_eq!(model.code, "MODEL_NOT_ALLOWED");
        assert!(model.message.contains("gpt-9"));
        assert_eq!(
            model.details.expect("model id travels as a parameter")["model"],
            "gpt-9"
        );

        let PolicyVerdict::Governs(budget) = billing_denial(dream_domain_billing::BillingError::BudgetExceeded) else {
            panic!("a spent budget is the policy answering");
        };
        assert_eq!(budget.code, "BUDGET_EXCEEDED");
        assert!(budget.message.contains("budget"));
    }

    /// A server with no company governs nobody. A standalone install must not
    /// be touched by an enterprise plane it does not participate in — this is
    /// the case that must never depend on the grace window.
    #[cfg(feature = "enterprise")]
    #[test]
    fn no_company_means_no_governance() {
        assert!(matches!(
            billing_denial(dream_domain_billing::BillingError::EnterpriseNotFound),
            PolicyVerdict::NotGoverned
        ));
    }

    /// An unanswerable check is neither a refusal nor a pass — the caller
    /// decides using the grace window.
    #[cfg(feature = "enterprise")]
    #[test]
    fn unreachable_policy_plane_is_not_a_refusal() {
        let leaky = "no such table: one_licenses; DB at C:/Users/someone/secret.db";
        assert!(matches!(
            billing_denial(dream_domain_billing::BillingError::Internal(leaky.into())),
            PolicyVerdict::Unanswerable
        ));
    }

    /// The window is what separates "briefly silent" from "gone". A tracker
    /// that just answered is inside it; one that last answered before the
    /// window opened is not.
    #[cfg(feature = "enterprise")]
    #[test]
    fn grace_window_expires() {
        let grace = PolicyGrace::new();
        assert!(grace.within_window(), "a plane that just answered is inside the window");

        grace.last_answered_ms.store(
            dream_core_common::now_ms() - ENTERPRISE_POLICY_GRACE_MS - 1,
            std::sync::atomic::Ordering::Relaxed,
        );
        assert!(!grace.within_window(), "silence beyond the window is an outage");

        grace.answered();
        assert!(grace.within_window(), "an answer reopens the window");
    }

    #[test]
    fn boundary_error_for_status_covers_common_fallback_statuses() {
        let cases = [
            (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            (StatusCode::FORBIDDEN, "FORBIDDEN"),
            (StatusCode::NOT_FOUND, "NOT_FOUND"),
            (StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED"),
            (StatusCode::CONFLICT, "CONFLICT"),
            (StatusCode::GONE, "GONE"),
            (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "UNSUPPORTED_MEDIA_TYPE"),
            (StatusCode::UNPROCESSABLE_ENTITY, "UNPROCESSABLE_ENTITY"),
            (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            (StatusCode::BAD_GATEWAY, "BAD_GATEWAY"),
            (StatusCode::GATEWAY_TIMEOUT, "GATEWAY_TIMEOUT"),
        ];

        for (status, code) in cases {
            let (_, actual_code) = boundary_error_for_status(status).expect("status should be normalized");
            assert_eq!(actual_code, code);
        }
    }

    #[test]
    fn extension_enablement_events_are_user_scoped() {
        assert!(!is_global_websocket_event("extensions.state-changed"));
        assert!(is_global_websocket_event("extensions.lifecycle"));
    }

    #[cfg(feature = "enterprise")]
    mod license_module_gate {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt;

        use super::super::{ADMIN_MODULE, LicenseModuleGateState, license_module_gate_middleware};
        use super::StatusCode;

        /// A migrated in-memory DB plus a `BillingService` over it — everything
        /// the gate itself touches (`resolve_enterprise_id`, `active_license`).
        /// `db` must stay alive for the pool to keep working; callers hold it
        /// in a `let` binding for the test's lifetime even though its fields
        /// are never read again.
        async fn billing_service_for_test() -> (
            dream_core_db::Database,
            std::sync::Arc<dream_domain_billing::BillingService>,
        ) {
            let db = dream_core_db::init_database_memory().await.unwrap();
            dream_domain_enterprise::run_one_enterprise_migrations(db.pool())
                .await
                .unwrap();
            dream_domain_billing::run_one_billing_migrations(db.pool())
                .await
                .unwrap();
            let billing = std::sync::Arc::new(dream_domain_billing::BillingService::new(
                db.pool().clone(),
                std::sync::Arc::new(dream_domain_billing::ManualBillingProvider),
            ));
            (db, billing)
        }

        fn current_user(id: &str) -> dream_core_auth::CurrentUser {
            dream_core_auth::CurrentUser {
                id: id.to_owned(),
                username: "tester".to_owned(),
                user_type: dream_core_db::UserType::Local,
                status: dream_core_db::UserStatus::Active,
            }
        }

        async fn seed_enterprise_member(pool: &sqlx::SqlitePool, user_id: &str, enterprise_id: &str) {
            sqlx::query(
                "INSERT INTO one_enterprise_members (user_id, enterprise_id, role, joined_at, updated_at) \
                 VALUES (?, ?, 'member', 0, 0)",
            )
            .bind(user_id)
            .bind(enterprise_id)
            .execute(pool)
            .await
            .unwrap();
        }

        async fn seed_license(pool: &sqlx::SqlitePool, enterprise_id: &str, modules_json: &str) {
            sqlx::query(
                "INSERT INTO one_license_activation \
                     (license_id, enterprise_id, customer, tier, issued_at, activated_at, activated_by, modules) \
                 VALUES ('lic1', ?, 'Acme', 'enterprise', 0, 0, 'admin1', ?)",
            )
            .bind(enterprise_id)
            .bind(modules_json)
            .execute(pool)
            .await
            .unwrap();
        }

        fn gate_app(billing: std::sync::Arc<dream_domain_billing::BillingService>) -> axum::Router {
            axum::Router::new()
                .route("/probe", get(|| async { StatusCode::OK }))
                .route("/api/one/admin/users", get(|| async { StatusCode::OK }))
                .route("/api/one/admin/users/role", get(|| async { StatusCode::OK }))
                .route("/api/one/admin/sso", get(|| async { StatusCode::OK }))
                .route_layer(axum::middleware::from_fn_with_state(
                    LicenseModuleGateState { billing },
                    license_module_gate_middleware,
                ))
        }

        /// Request one of the page-shaped routes; the middleware strips
        /// `/api/one` to get the module id, so the two routes above map to
        /// `/admin/users` and `/admin/sso`.
        async fn request_page(
            app: axum::Router,
            user: Option<dream_core_auth::CurrentUser>,
            path: &'static str,
        ) -> axum::response::Response {
            let mut request = Request::builder().uri(path);
            if let Some(user) = user {
                request = request.extension(user);
            }
            app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
        }

        async fn request_as(app: axum::Router, user: Option<dream_core_auth::CurrentUser>) -> axum::response::Response {
            let mut request = Request::builder().uri("/probe");
            if let Some(user) = user {
                request = request.extension(user);
            }
            app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
        }

        async fn error_code(response: axum::response::Response) -> String {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            body["code"].as_str().unwrap().to_owned()
        }

        #[tokio::test]
        async fn no_current_user_passes_through() {
            let (_db, billing) = billing_service_for_test().await;
            let response = request_as(gate_app(billing), None).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "this gate has no business 500ing on a missing extension"
            );
        }

        #[tokio::test]
        async fn personal_user_with_no_enterprise_passes_through() {
            let (_db, billing) = billing_service_for_test().await;
            let response = request_as(gate_app(billing), Some(current_user("standalone-user"))).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "resolve_enterprise_id: None means skip every check"
            );
        }

        #[tokio::test]
        async fn enterprise_with_no_activated_license_passes_through() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            let response = request_as(gate_app(billing), Some(current_user("u1"))).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "no license activated = nothing to restrict against yet"
            );
        }

        /// The one invariant that must never invert: an empty `modules` list is
        /// "this license never configured per-module restriction", not "nothing
        /// authorized". Getting this backwards locks every existing deployment
        /// out of its own admin plane on upgrade.
        #[tokio::test]
        async fn empty_modules_license_allows_everything() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(db.pool(), "ent1", "[]").await;
            let response = request_as(gate_app(billing), Some(current_user("u1"))).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "empty modules must mean unrestricted, not locked out"
            );
        }

        #[tokio::test]
        async fn license_naming_the_admin_module_is_authorized() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(
                db.pool(),
                "ent1",
                r#"[{"module":"/admin/*","startsAt":null,"expiresAt":null}]"#,
            )
            .await;
            let response = request_as(gate_app(billing), Some(current_user("u1"))).await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn license_naming_only_a_different_module_is_forbidden_as_not_authorized() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(
                db.pool(),
                "ent1",
                r#"[{"module":"/some-other-addon/*","startsAt":null,"expiresAt":null}]"#,
            )
            .await;
            let response = request_as(gate_app(billing), Some(current_user("u1"))).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(error_code(response).await, "LICENSE_MODULE_NOT_AUTHORIZED");
        }

        /// Denial reasons must be distinguishable: "never granted" and
        /// "granted, but lapsed" are different problems an operator would fix
        /// differently (buy the addon vs. renew it).
        /// P1-10 per-page granularity: an entry naming one admin page
        /// authorizes that page's subtree and nothing else on the plane.
        #[tokio::test]
        async fn per_page_entry_authorizes_its_subtree_and_blocks_the_rest() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(
                db.pool(),
                "ent1",
                r#"[{"module":"/admin/users","startsAt":null,"expiresAt":null}]"#,
            )
            .await;
            let app = gate_app(billing);

            let ok = request_page(app.clone(), Some(current_user("u1")), "/api/one/admin/users").await;
            assert_eq!(ok.status(), StatusCode::OK, "the named page itself must pass");

            let subtree = request_page(app.clone(), Some(current_user("u1")), "/api/one/admin/users/role").await;
            assert_eq!(subtree.status(), StatusCode::OK, "the page's subtree passes too");

            let other = request_page(app, Some(current_user("u1")), "/api/one/admin/sso").await;
            assert_eq!(
                other.status(),
                StatusCode::FORBIDDEN,
                "a page the license does not name is blocked"
            );
            assert_eq!(error_code(other).await, "LICENSE_MODULE_NOT_AUTHORIZED");
        }

        /// Back-compat: the T5 coarse `/admin/*` token still covers the whole
        /// plane — narrowing it would strip access from keys already sold
        /// under the coarse semantics.
        #[tokio::test]
        async fn the_coarse_admin_star_token_still_covers_the_whole_plane() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(
                db.pool(),
                "ent1",
                r#"[{"module":"/admin/*","startsAt":null,"expiresAt":null}]"#,
            )
            .await;
            let app = gate_app(billing);
            for path in ["/probe", "/api/one/admin/users", "/api/one/admin/sso"] {
                let response = request_page(app.clone(), Some(current_user("u1")), path).await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "path {path} must stay covered by /admin/*"
                );
            }
        }

        #[tokio::test]
        async fn expired_admin_module_grant_is_forbidden_as_expired_not_not_authorized() {
            let (db, billing) = billing_service_for_test().await;
            seed_enterprise_member(db.pool(), "u1", "ent1").await;
            seed_license(
                db.pool(),
                "ent1",
                &format!(r#"[{{"module":{ADMIN_MODULE:?},"startsAt":null,"expiresAt":1}}]"#),
            )
            .await;
            let response = request_as(gate_app(billing), Some(current_user("u1"))).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(error_code(response).await, "LICENSE_MODULE_EXPIRED");
        }
    }

    #[cfg(feature = "enterprise")]
    mod password_change_gate {
        use axum::body::Body;
        use axum::http::Request;
        use axum::routing::get;
        use tower::ServiceExt;

        use super::super::{PasswordChangeGateState, require_password_changed_gate};
        use super::StatusCode;

        fn current_user(id: &str) -> dream_core_auth::CurrentUser {
            dream_core_auth::CurrentUser {
                id: id.to_owned(),
                username: "tester".to_owned(),
                user_type: dream_core_db::UserType::Local,
                status: dream_core_db::UserStatus::Active,
            }
        }

        fn gate_app(user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository>) -> axum::Router {
            axum::Router::new()
                .route("/probe", get(|| async { StatusCode::OK }))
                .route_layer(axum::middleware::from_fn_with_state(
                    PasswordChangeGateState { user_repo },
                    require_password_changed_gate,
                ))
        }

        async fn request_as(app: axum::Router, user: Option<dream_core_auth::CurrentUser>) -> axum::response::Response {
            let mut request = Request::builder().uri("/probe");
            if let Some(user) = user {
                request = request.extension(user);
            }
            app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
        }

        async fn error_code(response: axum::response::Response) -> String {
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            body["code"].as_str().unwrap().to_owned()
        }

        #[tokio::test]
        async fn no_current_user_passes_through() {
            let db = dream_core_db::init_database_memory().await.unwrap();
            let user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository> =
                std::sync::Arc::new(dream_core_db::SqliteUserRepository::new(db.pool().clone()));
            let response = request_as(gate_app(user_repo), None).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "this gate has no business 500ing on a missing extension"
            );
        }

        #[tokio::test]
        async fn user_without_the_flag_passes_through() {
            let db = dream_core_db::init_database_memory().await.unwrap();
            let user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository> =
                std::sync::Arc::new(dream_core_db::SqliteUserRepository::new(db.pool().clone()));
            let user = user_repo.create_user("regular", "hash").await.unwrap();
            let response = request_as(gate_app(user_repo), Some(current_user(&user.id))).await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn flagged_user_is_forbidden_with_password_change_required() {
            let db = dream_core_db::init_database_memory().await.unwrap();
            let user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository> =
                std::sync::Arc::new(dream_core_db::SqliteUserRepository::new(db.pool().clone()));
            let user = user_repo.create_user("bootstrapped", "hash").await.unwrap();
            user_repo.set_must_change_password(&user.id, true).await.unwrap();
            let response = request_as(gate_app(user_repo), Some(current_user(&user.id))).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(error_code(response).await, "PASSWORD_CHANGE_REQUIRED");
        }

        #[tokio::test]
        async fn nonexistent_user_passes_through() {
            let db = dream_core_db::init_database_memory().await.unwrap();
            let user_repo: std::sync::Arc<dyn dream_core_db::IUserRepository> =
                std::sync::Arc::new(dream_core_db::SqliteUserRepository::new(db.pool().clone()));
            let response = request_as(gate_app(user_repo), Some(current_user("no-such-id"))).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "fail open on a lookup miss rather than manufacture a new failure mode"
            );
        }
    }

    #[tokio::test]
    async fn create_router_with_runtime_exposes_team_service_for_background_coordinators() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();

        let (_router, _runtime) = create_router_with_runtime(&services)
            .await
            .expect("router runtime should build");
    }

    #[tokio::test]
    async fn websocket_event_bridge_continues_after_receiver_lag() {
        let event_bus = BroadcastEventBus::new(2);
        let event_rx = event_bus.subscribe();
        let ws_manager = Arc::new(WebSocketManager::new());
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(8);
        ws_manager.add_client_for_user("user-a".into(), "token".into(), outbound_tx);

        for sequence in 1..=3 {
            event_bus.broadcast(WebSocketMessage::new(
                "test.beforeLag",
                json!({"user_id": "user-a", "sequence": sequence}),
            ));
        }

        let bridge = tokio::spawn(forward_event_bus_to_websocket(event_rx, ws_manager));
        event_bus.broadcast(WebSocketMessage::new(
            "test.afterLag",
            json!({"user_id": "user-a", "sequence": 4}),
        ));

        let delivered = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match outbound_rx.recv().await {
                    Some(WsOutbound::Text(text)) => {
                        let event: serde_json::Value = serde_json::from_str(&text).unwrap();
                        if event["name"] == "test.afterLag" {
                            break event;
                        }
                    }
                    other => panic!("expected a text websocket event, got {other:?}"),
                }
            }
        })
        .await
        .expect("the bridge should deliver events after recovering from lag");

        assert_eq!(delivered["data"]["sequence"], 4);
        bridge.abort();
    }

    // --- SendRateLimiter (E5 security policy send-rate limit) ---

    #[cfg(feature = "enterprise")]
    #[test]
    fn send_rate_limiter_enforces_the_limit_within_the_window() {
        let limiter = SendRateLimiter::new();
        assert!(limiter.check_and_increment("user-1", 2));
        assert!(limiter.check_and_increment("user-1", 2));
        assert!(!limiter.check_and_increment("user-1", 2));
    }

    #[cfg(feature = "enterprise")]
    #[test]
    fn send_rate_limiter_tracks_users_independently() {
        let limiter = SendRateLimiter::new();
        assert!(limiter.check_and_increment("user-a", 1));
        assert!(limiter.check_and_increment("user-b", 1));
        assert!(!limiter.check_and_increment("user-a", 1));
    }

    #[cfg(feature = "enterprise")]
    #[test]
    fn send_rate_limiter_resets_after_the_window_expires() {
        let limiter = SendRateLimiter::with_window_ms(50);
        assert!(limiter.check_and_increment("user-1", 1));
        assert!(!limiter.check_and_increment("user-1", 1));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(limiter.check_and_increment("user-1", 1));
    }

    // --- SecurityPolicySendRateGate ---

    #[cfg(feature = "enterprise")]
    async fn platform_service_for_test() -> (dream_core_db::Database, Arc<dream_domain_platform::PlatformService>) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        dream_domain_platform::run_one_platform_migrations(db.pool())
            .await
            .unwrap();
        let service = Arc::new(dream_domain_platform::PlatformService::new(
            db.pool().clone(),
            [9u8; 32],
        ));
        (db, service)
    }

    /// Same minimal seed as `dream-domain-platform`'s own tests use for
    /// `resolve_actor` — this crate doesn't depend on `dream-domain-org`, so
    /// the tables are created inline rather than via a real org migration.
    #[cfg(feature = "enterprise")]
    async fn seed_membership(pool: &sqlx::SqlitePool, user_id: &str, tenant_id: &str) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 role TEXT NOT NULL DEFAULT 'member', department_id TEXT, created_at INTEGER NOT NULL DEFAULT 0, \
                 updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));\
             CREATE TABLE IF NOT EXISTS one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
                 updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, role) VALUES (?, ?, 'member')")
            .bind(user_id)
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Personal edition / no enterprise membership: nothing governs the
    /// caller, no matter how many sends arrive. This is the case every
    /// standalone install and every unrelated test that drives the send
    /// route without a real membership row must land in.
    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn security_policy_send_rate_gate_passes_without_an_actor() {
        let (_db, platform) = platform_service_for_test().await;
        let gate = SecurityPolicySendRateGate::new(platform, Arc::new(PolicyGrace::new()));
        for _ in 0..5 {
            assert!(gate.check("no-membership-user").await.is_ok());
        }
    }

    /// A tenant that has never touched its security policy (or is on the
    /// `relaxed` tier) has `send_rate_limit_per_minute = None` — unlimited.
    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn security_policy_send_rate_gate_passes_when_no_rate_limit_is_configured() {
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        let gate = SecurityPolicySendRateGate::new(platform, Arc::new(PolicyGrace::new()));
        for _ in 0..5 {
            assert!(gate.check("user-1").await.is_ok());
        }
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn security_policy_send_rate_gate_blocks_once_the_configured_limit_is_reached() {
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        platform
            .set_security_policy("t1", false, false, &[], false, false, false, Some(2))
            .await
            .unwrap();

        let gate = SecurityPolicySendRateGate::new(platform, Arc::new(PolicyGrace::new()));
        assert!(gate.check("user-1").await.is_ok());
        assert!(gate.check("user-1").await.is_ok());
        let denial = gate.check("user-1").await.unwrap_err();
        assert_eq!(denial.code, "SEND_RATE_LIMITED");
    }

    /// Two tenants with different configured limits (or none at all) must
    /// not interfere with each other's counters.
    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn security_policy_send_rate_gate_isolates_tenants() {
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-strict", "t-strict").await;
        seed_membership(db.pool(), "user-relaxed", "t-relaxed").await;
        platform
            .set_security_policy("t-strict", false, false, &[], false, false, false, Some(1))
            .await
            .unwrap();

        let gate = SecurityPolicySendRateGate::new(platform, Arc::new(PolicyGrace::new()));
        assert!(gate.check("user-strict").await.is_ok());
        assert!(gate.check("user-strict").await.is_err());
        // The relaxed tenant's member is unaffected by the strict tenant's cap.
        for _ in 0..5 {
            assert!(gate.check("user-relaxed").await.is_ok());
        }
    }

    // --- PlatformToolCallSecurityGate ---

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_passes_without_an_actor() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (_db, platform) = platform_service_for_test().await;
        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        assert!(
            gate.check("no-membership-user", "rm -rf /", false, false)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_passes_on_the_relaxed_default() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        assert!(gate.check("user-1", "rm -rf /", true, false).await.unwrap().is_none());
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_blocks_a_matching_destructive_command() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        platform.apply_security_policy_tier("t1", "standard").await.unwrap();

        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        // Uses only "sudo" (not e.g. "shutdown") so the matched pattern is
        // unambiguous regardless of `blocked_command_patterns` iteration order.
        let reason = gate
            .check("user-1", "Run shell command: sudo apt-get remove foo", false, true)
            .await
            .unwrap();
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("sudo"));
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_allows_a_non_matching_command_under_the_standard_tier() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        platform.apply_security_policy_tier("t1", "standard").await.unwrap();

        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        assert!(
            gate.check("user-1", "Read /tmp/notes.txt", false, false)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_blocks_network_fetch_under_the_strict_tier() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        platform.apply_security_policy_tier("t1", "strict").await.unwrap();

        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        assert!(
            gate.check("user-1", "Fetch https://example.com", true, false)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_allows_network_fetch_under_the_standard_tier() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        // standard blocks destructive commands but not network access.
        platform.apply_security_policy_tier("t1", "standard").await.unwrap();

        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        assert!(
            gate.check("user-1", "Fetch https://example.com", true, false)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "enterprise")]
    #[tokio::test]
    async fn tool_call_security_gate_does_not_flag_a_non_fetch_call_even_under_the_strict_tier() {
        use dream_core_ai_agent::ToolCallSecurityGate;
        let (db, platform) = platform_service_for_test().await;
        seed_membership(db.pool(), "user-1", "t1").await;
        platform.apply_security_policy_tier("t1", "strict").await.unwrap();

        let gate = PlatformToolCallSecurityGate {
            platform,
            workflow: None,
        };
        // is_network_fetch = false: a non-network call must not be blocked by
        // the network-denial flag, even though the tenant is on strict.
        assert!(
            gate.check("user-1", "Read /tmp/notes.txt", false, false)
                .await
                .unwrap()
                .is_none()
        );
    }
}
