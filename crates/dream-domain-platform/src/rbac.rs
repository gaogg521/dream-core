//! Admin extractor for platform-config routes. Reuses the upstream auth
//! middleware's `CurrentUser`; resolves enterprise membership/role from
//! one-org's `one_user_org` table via the shared pool (same cross-crate
//! precedent as `dream_domain_devops::user_org_role`), so one-platform needs no
//! dependency on one-org.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use dream_core_auth::CurrentUser;

use crate::error::PlatformError;
use crate::service::PlatformActor;
use crate::state::OnePlatformRouterState;
use dream_core_common::governance_caller::{GovernanceCaller, classify_governance_caller};

/// Requires enterprise membership with an admin role. Personal edition (no
/// membership row) → `NotInEnterprise`; non-admin → `Forbidden`.
#[derive(Debug, Clone)]
pub struct RequirePlatformAdmin(pub PlatformActor);

impl FromRequestParts<OnePlatformRouterState> for RequirePlatformAdmin {
    type Rejection = PlatformError;

    async fn from_request_parts(parts: &mut Parts, state: &OnePlatformRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| PlatformError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_admin(&user.id).await?;
        Ok(Self(actor))
    }
}

/// Requires any enterprise membership (any role) — the member-facing
/// counterpart to [`RequirePlatformAdmin`], for self-service routes such as
/// the in-app notification inbox. Admins pass too (they are members).
#[derive(Debug, Clone)]
pub struct RequirePlatformMember(pub PlatformActor);

impl FromRequestParts<OnePlatformRouterState> for RequirePlatformMember {
    type Rejection = PlatformError;

    async fn from_request_parts(parts: &mut Parts, state: &OnePlatformRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| PlatformError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_member(&user.id).await?;
        // C1-2 fix: a blocked machine must stop receiving governance data
        // (security policy, memory, DLP, ...) it polls as "a member in good
        // standing". The client self-reports its machine id in
        // `x-dream-machine-id`; no header means the check is skipped, which
        // also keeps this extractor's other caller — the admin console,
        // reached over a browser session that never sends this header —
        // unaffected. Deliberately not on `RequirePlatformAdmin`: blocking a
        // runtime node must never be able to lock an admin out of the
        // console they would need to undo it from.
        let caller_machine = match classify_governance_caller(&parts.headers) {
            GovernanceCaller::Identified(id) => Some(Some(id)),
            GovernanceCaller::UnidentifiedRemoteClient => Some(None),
            // The console reaches this same extractor over a session cookie and
            // never sends a machine id — judging it on one would lock an
            // administrator whose own laptop is blocked out of the page that
            // unblocks it.
            GovernanceCaller::BrowserSession => None,
        };
        if let Some(machine_id) = caller_machine
            && state
                .service
                .machine_blocked(&actor.tenant_id, &user.id, machine_id)
                .await?
        {
            return Err(PlatformError::MachineBlocked(
                "this machine has been blocked by an administrator".into(),
            ));
        }
        Ok(Self(actor))
    }
}
