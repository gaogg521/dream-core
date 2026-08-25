//! Top-level router assembly: middleware stack + module route merges.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
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
use dream_core_auth::{
    AuthIdentityMode, AuthRouterState, AuthState, IRuntimeTokenVerifier, SystemDefaultFilesystemAdopter,
    auth_middleware, auth_routes, csrf_middleware, security_headers_middleware,
};
use dream_core_channel::channel_routes;
#[cfg(feature = "weixin")]
use dream_core_channel::weixin_login_route;
use dream_core_claude_bridge::claude_bridge_config_routes;
use dream_core_codex_bridge::{codex_bridge_config_routes, codex_bridge_public_routes};
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
            // here on every request.
            Ok(None) => return Ok(true),
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
            PolicyVerdict::NotGoverned => Ok(()),
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
            // No company on this server governs nobody's model choice.
            Err(dream_domain_billing::BillingError::EnterpriseNotFound) => Ok(true),
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
    pub sso_public: Router,
    pub sso_admin: Router,
    /// Handed back rather than kept: one-employee and one-devops both need it,
    /// and it can only be built from the org service constructed in here.
    pub tenant_resolver: std::sync::Arc<dyn dream_domain_employee::TenantResolver>,
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
    // one-org enterprise routes (/api/one/*) — RBAC extractors depend on the
    // upstream auth middleware injecting CurrentUser.
    let one_org_service = std::sync::Arc::new(
        dream_domain_org::OrgService::new(
            services.database.pool().clone(),
            services.user_repo.clone(),
            services.data_dir.clone(),
            crate::config::derive_encryption_key(&services.data_secret_raw),
        )
        .with_credential_revoker(std::sync::Arc::new(ModelChannelRevoker(one_devops_service.clone()))),
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
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-enterprise routes (/api/one/enterprise/*) — the SSO-company
    // "enterprise org" dimension + the company tier (Direction B). The service
    // was constructed above so the company-admin bridges could be wired.
    let one_enterprise_state = dream_domain_enterprise::OneEnterpriseRouterState::new(one_enterprise_service.clone());
    let one_enterprise_authenticated = dream_domain_enterprise::one_enterprise_routes(one_enterprise_state)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-billing routes (/api/one/billing/*) — subscription tier, seats, and
    // usage dashboard. No payment provider wired: manual provisioning via
    // `PUT /tier`. The service was built above (before the conversation routes)
    // so its usage recorder could be injected there.
    let one_billing_state = dream_domain_billing::OneBillingRouterState::new(one_billing_service.clone());
    let one_billing_authenticated = dream_domain_billing::one_billing_routes(one_billing_state)
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // one-platform routes (/api/one/admin/platform/*) — deployment infra config
    // (P1-3 container runtime + P2-2 realtime collaboration). Reserved adapters:
    // the Noop defaults report "not configured" until a real runtime/provider
    // is wired here via `with_container_runtime` / `with_collaboration_provider`.
    // Service itself was built above, alongside `auth_mw_state`, so its IP
    // allowlist could be wired into the auth middleware.
    let one_platform_state = dream_domain_platform::OnePlatformRouterState::new(one_platform_service.clone());
    let one_platform_authenticated = dream_domain_platform::one_platform_routes(one_platform_state)
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
        .route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));
    GovernancePlane {
        org: one_org_authenticated,
        enterprise: one_enterprise_authenticated,
        billing: one_billing_authenticated,
        platform: one_platform_authenticated,
        sso_public: one_sso_public,
        sso_admin: one_sso_admin,
        tenant_resolver,
    }
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
    let one_platform_service = std::sync::Arc::new(dream_domain_platform::PlatformService::new(
        services.database.pool().clone(),
        crate::config::derive_encryption_key(&services.data_secret_raw),
    ));

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

    let auth_mw_state = AuthState {
        jwt_service: services.jwt_service.clone(),
        user_repo: services.user_repo.clone(),
        identity_mode: auth_identity_mode(services.identity_mode),
        runtime_token_verifier: Some(Arc::new(ConversationHelperTokenVerifier {
            runtime_token_service: services.runtime_token_service.clone(),
        })),
        ip_allowlist,
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
    states
        .conversation
        .service
        .with_usage_recorder(std::sync::Arc::new(BillingUsageRecorder(one_billing_service.clone())));

    // Conversation routes protected by auth middleware.
    let conversation_state =
        states
            .conversation
            .clone()
            .with_content_inspector(std::sync::Arc::new(LocalContentInspector(
                services.content_inspection.clone(),
            )));
    // No send gate in the personal edition: seat caps, spend caps and the model
    // allowlist are all billing-plane policy. `None` skips the check entirely
    // (see `dream_core_conversation::routes`), which is the pre-billing path.
    #[cfg(feature = "enterprise")]
    let conversation_state =
        conversation_state.with_send_gate(std::sync::Arc::new(BillingSendGate { billing: one_billing_service.clone(), grace: policy_grace.clone() }));
    let conversation_authenticated =
        conversation_routes(conversation_state).route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware));

    // The ops router hosts set-config-option (model switch) — gate it too so
    // the P1-2 model allowlist is enforced at model selection.
    let conversation_ops_state = states.conversation;
    #[cfg(feature = "enterprise")]
    let conversation_ops_state =
        conversation_ops_state.with_send_gate(std::sync::Arc::new(BillingSendGate { billing: one_billing_service.clone(), grace: policy_grace.clone() }));
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
        .merge(governance.sso_public)
        .merge(governance.sso_admin);

    // Personal edition: the desktop shell still asks "am I in an org?" on every
    // launch, and a 404 there is not the same answer as "no" — without these the
    // identity entry renders an error state instead of the personal one. Behind
    // the same auth layer the real routes use, so `CurrentUser` is injected:
    // the context handler reads it to answer with the caller's role, exactly as
    // `OrgService::effective_role` would. See `personal_identity_routes`.
    #[cfg(not(feature = "enterprise"))]
    let router = router.merge(
        personal_identity_routes().route_layer(from_fn_with_state(auth_mw_state.clone(), auth_middleware)),
    );

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

    use super::{
        boundary_error_for_status, create_router_with_runtime, forward_event_bus_to_websocket, is_global_websocket_event,
    };
    #[cfg(feature = "enterprise")]
    use super::{ENTERPRISE_POLICY_GRACE_MS, PolicyGrace, PolicyVerdict, billing_denial};
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
}
