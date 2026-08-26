//! Company security policy as seen by the ACP permission router.
//!
//! Same shape as [`crate::model_policy::ModelAllowlistGate`]: the trait
//! lives here, `dream-app` implements it over `PlatformService`. The
//! platform crate owns `one_security_policy`, but `dream-core-ai-agent`
//! cannot depend on it directly (Capability layer must not depend on
//! Domain layer), so the check arrives through this port.

use async_trait::async_trait;

/// Whether an ACP tool-call permission request should be blocked outright
/// by the caller's company security policy, before it ever reaches the user
/// for approval or is auto-approved. Covers two of `one_security_policy`'s
/// fields that share the same interception point:
///
/// - `destructive_commands_blocked` + `blocked_command_patterns`: matched
///   against `command_text`.
/// - `external_network_denied_by_default`: matched against
///   `is_network_fetch`.
///
/// Both are checked in one call (rather than one trait method each) so a
/// tool call only costs one tenant/policy lookup regardless of how many
/// policy dimensions apply to it.
///
/// `command_text` is best-effort text extracted from the tool call (title
/// plus the raw input serialized to JSON) — this crate has no reliable,
/// verified way to know which specific field of a given agent CLI's tool
/// call carries the actual shell command (that would require agent-CLI
/// wire evidence per-backend, which does not exist for this), so the check
/// searches whatever text ACP surfaced for the call rather than assuming a
/// specific schema. `is_network_fetch` is `true` when ACP tagged the tool
/// call `ToolKind::Fetch` ("retrieving external data" — verified against
/// the vendored `agent-client-protocol-schema` crate's `ToolKind` enum).
///
/// Returns `Ok(Some(reason))` to block. `Ok(None)` lets the call proceed
/// through the normal flow. `Err` means the check itself failed (a DB
/// error, say) — callers decide the fail-open/fail-closed policy for that,
/// same convention as [`crate::model_policy::ModelAllowlistGate`].
#[async_trait]
pub trait ToolCallSecurityGate: Send + Sync {
    async fn check(&self, user_id: &str, command_text: &str, is_network_fetch: bool) -> Result<Option<String>, String>;
}
