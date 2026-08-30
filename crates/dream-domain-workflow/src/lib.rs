//! one-workflow: the approval workflow subsystem (P2-1, align-openocta).
//!
//! A deliberately generic task store driving the five OpenOcta-aligned
//! approval classes — 创作 / 资源 / 安全策略模板申请 / 工具 / Prompt — plus
//! the blocking terminal-tool approval the security policy's
//! `terminal_tools_require_approval` field has been waiting for since T8.
//! Everything lives in `one_workflow_tasks`; the service is the only writer.
//!
//! Routes mount under `/api/workflow/*` (OpenOcta's prefix, kept verbatim) on
//! the governance plane, so the same assembly both binaries serve. The crate
//! is enterprise-gated like the other five `dream-domain-*` crates: personal
//! builds have no approvals and no route surface, byte-for-byte as before.
//!
//! Kept out of `dream-domain-platform` on purpose: approvals are a subsystem
//! (own tables, own lifecycle, own route prefix), not platform config — and
//! the plan's next step (admin-svc split) wants it already separable.

pub mod decision_sink;
pub mod error;
pub mod migrate;
pub mod models;
pub mod rbac;
pub mod routes;
pub mod service;
pub mod state;

pub use decision_sink::DecisionSink;
pub use error::WorkflowError;
pub use migrate::run_one_workflow_migrations;
pub use models::WorkflowTaskDto;
pub use rbac::{RequireWorkflowAdmin, RequireWorkflowMember};
pub use routes::one_workflow_routes;
pub use service::{
    APPROVAL_POLL_INTERVAL_MS, ApprovalOutcome, TERMINAL_APPROVAL_TIMEOUT_MS, WORKFLOW_TASK_KINDS, WorkflowActor,
    WorkflowService,
};
pub use state::OneWorkflowRouterState;
