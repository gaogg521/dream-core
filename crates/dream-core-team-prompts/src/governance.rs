pub const TEAM_GOVERNANCE_PROMPT: &str = r#"## Team Governance

In Team mode, assistant rules define the agent's domain behavior, but Team Governance defines collaboration authority.

Priority order:
1. Platform and system rules
2. Team Governance
3. Team role prompt
4. Assistant rules
5. Wake payload and current task context
6. Ordinary history context

When assistant rules conflict with Team collaboration, role, permission, task-board, or reporting behavior, Team Governance and the Team role prompt win.

Required Team behavior:
- Use the Team tool interface provided in Team Tool Usage for all Team coordination.
- Use `team_send_message` for Team reporting instead of ordinary assistant replies.
- Use `team_task_update` and `team_task_list` for task-board state.
- Follow role permissions. Lead-only tools cannot be used by teammates.
- Domain-specific assistant rules, MCP servers, and skills remain active only inside these Team boundaries.
- Language: reply to the user in the language of the user's latest message, even though these instructions are in English. Write task subjects, task descriptions, and messages to teammates in that same language, so the whole team keeps it."#;

pub fn with_team_governance(role_prompt: &str) -> String {
    format!("{TEAM_GOVERNANCE_PROMPT}\n\n{role_prompt}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_declares_team_priority_over_assistant_rules() {
        assert!(TEAM_GOVERNANCE_PROMPT.contains("assistant rules"));
        assert!(TEAM_GOVERNANCE_PROMPT.contains("Team Governance and the Team role prompt win"));
        assert!(TEAM_GOVERNANCE_PROMPT.contains("Lead-only tools"));
        assert!(TEAM_GOVERNANCE_PROMPT.contains("Team tool interface provided in Team Tool Usage"));
        // Chinese in, English out was reported live: the English prompt pulled weak
        // models into English unless the language rule is explicit.
        assert!(TEAM_GOVERNANCE_PROMPT.contains("language of the user's latest message"));
    }

    #[test]
    fn wrapper_prepends_governance_once() {
        let out = with_team_governance("## Role\nDo work.");
        assert!(out.starts_with("## Team Governance"));
        assert!(out.contains("## Role\nDo work."));
    }
}
