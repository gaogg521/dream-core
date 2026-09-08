//! Company security policy on the 1ONE CLI path.
//!
//! # Why this exists alongside the ACP permission router's copy
//!
//! The router in [`crate::manager::acp::permission_router`] enforces the same
//! policy, but it can only act on tool calls that ask for permission — and the
//! dream engine, which runs the product's default conversation type, does not
//! ask when the session is in full-auto. Real-machine testing found exactly
//! that: with `AUDITBLOCKED` in the company's blocked-command list and the
//! member in 全自动, `ExecCommand` ran the command and printed it back. The
//! policy was configured, delivered, synced onto the machine, and enforced by
//! nobody, because the only enforcement point in the process was one the engine
//! never reached.
//!
//! `dream_engine_protocol::ToolCallGuard` is the engine's answer to that: a
//! veto asked before approval is even considered. This adapter puts the same
//! [`ToolCallSecurityGate`] behind it, so both conversation paths make the same
//! decision from the same policy.
//!
//! # What it can and cannot see
//!
//! The engine hands over a tool name and its raw JSON input. That is enough for
//! the command dimensions, and it is matched the same way the ACP path matches
//! them — over the tool name plus the serialized input, not a guessed "the
//! command lives in field X" schema. It carries the same known consequence: a
//! `Write` whose *content* contains a blocked pattern is refused too. That is
//! the existing behaviour on the other path, and quietly diverging here would
//! be worse than the false positive.
//!
//! `external_network_denied_by_default` is deliberately not asserted here. ACP
//! tags a call `ToolKind::Fetch`; the engine has no equivalent taxonomy, and
//! deciding "this tool reaches the network" from its name would be a guess
//! dressed up as a control. That dimension stays with the ACP path until the
//! engine surfaces a signal worth reading.

use std::sync::Arc;

use async_trait::async_trait;
use dream_engine_protocol::ToolCallGuard;
use serde_json::Value;
use tracing::warn;

use crate::security_policy::ToolCallSecurityGate;

/// The engine's built-in shell tool. The only one whose category is `Exec`
/// (verified: `dream-engine-tools`' `Tool::category` implementations), and so
/// the only built-in that feeds `terminal_tools_require_approval`.
const EXEC_TOOL: &str = "ExecCommand";

pub struct SecurityPolicyCallGuard {
    gate: Arc<dyn ToolCallSecurityGate>,
    user_id: String,
}

impl SecurityPolicyCallGuard {
    pub fn new(gate: Arc<dyn ToolCallSecurityGate>, user_id: impl Into<String>) -> Self {
        Self {
            gate,
            user_id: user_id.into(),
        }
    }
}

#[async_trait]
impl ToolCallGuard for SecurityPolicyCallGuard {
    async fn check(&self, tool_name: &str, input: &Value) -> Option<String> {
        let command_text = format!("{tool_name} {input}");
        match self
            .gate
            .check(&self.user_id, &command_text, false, tool_name == EXEC_TOOL)
            .await
        {
            Ok(reason) => reason,
            Err(error) => {
                // Refuse the one call rather than the turn. Same trade the ACP
                // router makes: a policy lookup that failed is not a policy
                // that allowed, and the model can retry.
                warn!(tool_name, error, "tool-call policy check failed; refusing this call");
                Some(format!(
                    "Tool call refused: the security policy check failed ({error})."
                ))
            }
        }
    }
}

#[cfg(test)]
#[path = "call_guard_test.rs"]
mod call_guard_test;
