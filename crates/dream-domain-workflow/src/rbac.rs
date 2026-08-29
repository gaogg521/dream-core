//! Admin/member extractors for one-workflow routes. Same cross-crate
//! precedent as one-platform's `rbac.rs`: resolve enterprise membership from
//! one-org's `one_user_org` table via the shared pool, so one-workflow needs
//! no dependency on one-org.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use dream_core_auth::CurrentUser;

use crate::error::WorkflowError;
use crate::service::WorkflowActor;
use crate::state::OneWorkflowRouterState;

/// Requires enterprise membership with an admin role — the approval group
/// ("弹给管理组"): the pending queue and every decision belong to them.
#[derive(Debug, Clone)]
pub struct RequireWorkflowAdmin(pub WorkflowActor);

impl FromRequestParts<OneWorkflowRouterState> for RequireWorkflowAdmin {
    type Rejection = WorkflowError;

    async fn from_request_parts(parts: &mut Parts, state: &OneWorkflowRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| WorkflowError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_admin(&user.id).await?;
        Ok(Self(actor))
    }
}

/// Requires any enterprise membership (any role) — members submit tasks and
/// watch their own submissions.
#[derive(Debug, Clone)]
pub struct RequireWorkflowMember(pub WorkflowActor);

impl FromRequestParts<OneWorkflowRouterState> for RequireWorkflowMember {
    type Rejection = WorkflowError;

    async fn from_request_parts(parts: &mut Parts, state: &OneWorkflowRouterState) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or_else(|| WorkflowError::Forbidden("Authentication required".into()))?;
        let actor = state.service.require_member(&user.id).await?;
        Ok(Self(actor))
    }
}
