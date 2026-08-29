//! DTOs for the approval workflow UI. `payload` is the kind-specific JSON
//! blob submitted with the task — the workflow service never interprets it,
//! it only carries it for the console to render.

use serde::Serialize;

/// One approval task, as every view (pending queue / decided history /
/// requester's own list) shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTaskDto {
    pub id: String,
    /// One of [`crate::service::WORKFLOW_TASK_KINDS`].
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub payload: serde_json::Value,
    pub requester_id: String,
    /// `"pending" | "approved" | "rejected" | "expired"`.
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub note: Option<String>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}
