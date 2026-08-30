//! Hooking a task's decision into the domain that raised it (P1-7).
//!
//! The workflow store is deliberately generic: it validates the task kind
//! vocabulary and records the decision, and it knows nothing about what a
//! node or a scene IS. But some kinds have a real effect to produce when the
//! decision lands — a node-access review that approves must actually flip
//! the node's status, or the approval is a number in a table while the
//! machine stays pending forever. So the effect crosses a trait the app
//! layer wires up (the same arrangement as one-org's `CredentialRevoker` /
//! `NodeReviewSink`), with the app adapter translating the generic
//! (kind, payload, decision) into the domain call.
//!
//! Best-effort by contract: a sink failure is logged and the decision still
//! stands — the human's ruling is the authoritative act; a failed side
//! effect is recoverable (the admin can flip the node in the registry) and
//! must not corrupt the decision record.

use async_trait::async_trait;

use crate::models::WorkflowTaskDto;

#[async_trait]
pub trait DecisionSink: Send + Sync {
    /// Called once per successful `decide`, AFTER the task row has been
    /// updated. `decision` is `"approved"` or `"rejected"`; expired tasks
    /// never reach a sink (they expire without a decision).
    async fn on_task_decided(&self, tenant_id: &str, task: &WorkflowTaskDto, decision: &str);
}
