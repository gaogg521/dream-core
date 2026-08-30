//! Raising an access-review task when a runtime node first checks in under
//! an approval-required policy (P1-7 智能体节点控制面).
//!
//! one-org owns the node roster (`one_runtime_nodes`), but access-review
//! tasks live in one-workflow — a different domain crate with its own
//! lifecycle, and neither depends on the other. So, like every cross-crate
//! side effect in this codebase, the event crosses through a trait the app
//! layer wires up (`dream_core_app::router` implements it over
//! `WorkflowService`), mirroring `CredentialRevoker` / `EnterpriseSync`.
//!
//! Best-effort by contract, same as `CredentialRevoker`: a failure to raise
//! the review task must not fail the heartbeat itself — the machine is
//! healthy and its row is already registered as `pending`; losing the task
//! means the admin must notice the pending row in the registry, which is
//! degraded visibility, not a broken machine. Errors are logged for
//! follow-up.

use async_trait::async_trait;

#[async_trait]
pub trait NodeReviewSink: Send + Sync {
    /// A first-seen machine registered as `pending` under an
    /// approval-required policy. Implementations raise the review task.
    async fn on_node_awaiting_approval(
        &self,
        tenant_id: &str,
        node_id: &str,
        machine_id: &str,
        display_name: &str,
        user_id: &str,
    );
}
