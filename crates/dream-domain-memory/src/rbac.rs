//! Admin/member extractors for one-memory routes. Same cross-crate
//! precedent as one-platform's `rbac.rs`: resolve enterprise membership from
//! one-org's `one_user_org` table via the shared pool, so one-memory needs
//! no dependency on one-org.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use dream_core_auth::CurrentUser;

use crate::error::MemoryError;
use crate::service::MemoryActor;
use crate::state::OneMemoryRouterState;
use dream_core_common::governance_caller::{GovernanceCaller, classify_governance_caller};

/// Requires enterprise membership with an admin role — collection inventory,
/// refinement, and grant administration belong to them.
#[derive(Debug, Clone)]
pub struct RequireMemoryAdmin(pub MemoryActor);

impl FromRequestParts<OneMemoryRouterState> for RequireMemoryAdmin {
    type Rejection = MemoryError;

    async fn from_request_parts(parts: &mut Parts, state: &OneMemoryRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| MemoryError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_admin(&user.id).await?;
        Ok(Self(actor))
    }
}

/// Requires any enterprise membership (any role) — members read and write
/// the collections their tier and grants allow.
#[derive(Debug, Clone)]
pub struct RequireMemoryMember(pub MemoryActor);

impl FromRequestParts<OneMemoryRouterState> for RequireMemoryMember {
    type Rejection = MemoryError;

    async fn from_request_parts(parts: &mut Parts, state: &OneMemoryRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| MemoryError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_member(&user.id).await?;
        // C1-2 fix: a blocked machine must stop reading company memory. See
        // `dream_domain_platform::rbac::RequirePlatformMember` for the full
        // reasoning (deliberately not on `RequireMemoryAdmin`, same as there).
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
            return Err(MemoryError::MachineBlocked(
                "this machine has been blocked by an administrator".into(),
            ));
        }
        Ok(Self(actor))
    }
}
