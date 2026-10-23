//! Company security policy as seen by the ACP permission router.
//!
//! Same shape as [`crate::model_policy::ModelAllowlistGate`]: the trait
//! lives here, `dream-app` implements it over `PlatformService`. The
//! platform crate owns `one_security_policy`, but `dream-core-ai-agent`
//! cannot depend on it directly (Capability layer must not depend on
//! Domain layer), so the check arrives through this port.

use async_trait::async_trait;
use serde_json::Value;

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
/// `is_terminal_tool` (`ToolKind::Execute` on the ACP path) feeds the third
/// policy dimension, `terminal_tools_require_approval`: when the caller's
/// policy demands it, the implementation may create an approval task and
/// block inside `check` until an administrator decides (or the deadline
/// passes — a timeout denies, the conservative default). Blocking here is
/// the same trade the permission router already makes when it waits for the
/// human user's own confirmation.
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
    async fn check(
        &self,
        user_id: &str,
        command_text: &str,
        is_network_fetch: bool,
        is_terminal_tool: bool,
    ) -> Result<Option<String>, String>;

    /// Same verdict as [`Self::check`], with the conversation the call belongs
    /// to. Gates that hand a human an approval request override this so the
    /// request can say which conversation and which command it is about; the
    /// default ignores the context, so gates that decide silently need no change.
    async fn check_in_context(
        &self,
        user_id: &str,
        command_text: &str,
        is_network_fetch: bool,
        is_terminal_tool: bool,
        _context: &ToolCallContext,
    ) -> Result<Option<String>, String> {
        self.check(user_id, command_text, is_network_fetch, is_terminal_tool)
            .await
    }
}

/// What the engine knows about a tool call beyond its flattened text.
#[derive(Debug, Clone, Default)]
pub struct ToolCallContext {
    pub conversation_id: Option<String>,
    /// The member's own name for the conversation, looked up at call time (it
    /// is renamed after the agent is built, so it cannot be captured earlier).
    pub conversation_title: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
}

/// An approval request as an administrator reads it in the approval queue.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequestText {
    pub title: String,
    pub detail: String,
    pub payload: Value,
}

const TITLE_MAX_CHARS: usize = 120;
const COMMAND_MAX_CHARS: usize = 500;
/// Argument names the shell tools use for the command line, across the engine's
/// `ExecCommand` (`cmd`) and the ACP agents' variants.
const COMMAND_KEYS: [&str; 4] = ["cmd", "command", "script", "shell_command"];

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Split the flattened `"<tool or title> <raw json input>"` text back apart.
/// Best effort: anything that does not parse is left as plain text.
fn split_command_text(command_text: &str) -> (Option<String>, Option<Value>) {
    let Some(brace) = command_text.find('{') else {
        return (None, None);
    };
    let input = serde_json::from_str::<Value>(command_text[brace..].trim()).ok();
    let name = command_text[..brace].trim();
    ((!name.is_empty()).then(|| name.to_owned()), input)
}

fn command_from_input(input: &Value) -> Option<String> {
    match input {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => COMMAND_KEYS
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    }
}

/// Words for the approval queue: the member's conversation title as the
/// headline (it is what the member and the administrator both recognise),
/// the readable command in the detail, and the raw call kept in the payload.
pub fn approval_request_text(command_text: &str, context: &ToolCallContext) -> ApprovalRequestText {
    let (parsed_name, parsed_input) = split_command_text(command_text);
    let tool_name = context.tool_name.clone().or(parsed_name);
    let input = context.tool_input.clone().or(parsed_input);
    let command = input
        .as_ref()
        .and_then(command_from_input)
        .unwrap_or_else(|| command_text.trim().to_owned());
    let conversation_title = context
        .conversation_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());

    let title = match conversation_title {
        Some(name) => truncate_chars(name, TITLE_MAX_CHARS),
        None => format!("终端命令：{}", truncate_chars(&command, 80)),
    };
    let mut lines = vec![format!("请求执行命令：{}", truncate_chars(&command, COMMAND_MAX_CHARS))];
    if let Some(name) = conversation_title {
        lines.push(format!("所属会话：{name}"));
    }
    if let Some(tool) = tool_name.as_deref() {
        lines.push(format!("工具：{tool}"));
    }
    lines.push("原因：企业安全策略要求终端命令经管理员审批后才会执行".to_owned());

    ApprovalRequestText {
        title,
        detail: lines.join("\n"),
        payload: serde_json::json!({
            "commandText": command_text,
            "command": command,
            "toolName": tool_name,
            "conversationId": context.conversation_id,
            "conversationTitle": conversation_title,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_the_request_with_the_conversation_and_reads_the_command() {
        let context = ToolCallContext {
            conversation_id: Some("c1".into()),
            conversation_title: Some("我用这个测试".into()),
            ..Default::default()
        };
        let text = approval_request_text(r#"ExecCommand {"cmd":"Get-ChildItem -Force"}"#, &context);
        assert_eq!(text.title, "我用这个测试");
        assert!(text.detail.contains("请求执行命令：Get-ChildItem -Force"));
        assert!(text.detail.contains("所属会话：我用这个测试"));
        assert!(text.detail.contains("工具：ExecCommand"));
        assert_eq!(
            text.payload["commandText"],
            r#"ExecCommand {"cmd":"Get-ChildItem -Force"}"#
        );
        assert_eq!(text.payload["command"], "Get-ChildItem -Force");
        assert_eq!(text.payload["conversationId"], "c1");
    }

    #[test]
    fn without_a_conversation_the_title_is_the_command_not_json() {
        let text = approval_request_text(r#"ExecCommand {"cmd":"dir"}"#, &ToolCallContext::default());
        assert_eq!(text.title, "终端命令：dir");
        assert!(!text.detail.contains("所属会话"));
    }

    #[test]
    fn structured_context_wins_over_reparsing_the_text() {
        let context = ToolCallContext {
            tool_name: Some("ExecCommand".into()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            ..Default::default()
        };
        let text = approval_request_text("garbled", &context);
        assert_eq!(text.payload["command"], "ls");
    }

    #[test]
    fn unparseable_text_is_shown_as_is_and_long_values_are_cut() {
        let text = approval_request_text("rm -rf something", &ToolCallContext::default());
        assert_eq!(text.title, "终端命令：rm -rf something");
        let long = "x".repeat(900);
        let text = approval_request_text(&long, &ToolCallContext::default());
        assert!(text.detail.contains('…'));
        assert!(text.title.chars().count() <= "终端命令：".chars().count() + 81);
    }
}
