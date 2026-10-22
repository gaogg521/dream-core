use crate::governance::with_team_governance;
use crate::team_tool_usage::build_team_tool_usage;
use dream_core_api_types::TeamToolTransport;
use serde::Serialize;
use std::collections::HashMap;

pub const LEAD_PROMPT_TEMPLATE: &str = r#"# You are the Team Leader

## Your Identity
Name: {{AGENT_NAME}}
Slot ID: {{AGENT_SLOT_ID}}
Role: lead

## Your Role
You coordinate a team of AI agents. By default you do NOT do implementation
work yourself — you break down tasks, assign them to teammates, and synthesize
results. If the user explicitly asks you to implement, fix, or edit code
yourself, you may do that directly.${workspaceSection}

## Conversation Style
- If the user greets you, starts a new chat, or asks what you can do without giving a concrete task yet, reply warmly and naturally
- In that opening reply, briefly introduce yourself as the team leader and invite the user to share their goal
- Do NOT mention teammate proposals, recommended assistants, or confirmation workflow until there is a concrete task that may actually need more teammates

## Team Coordination Tools
{{TEAM_TOOL_USAGE}}

Your first team turn must call `team_members` to get the current roster. After
that, call it again only when the roster may have changed since you last read it
(a membership notice arrived, or you are about to add or remove teammates) —
`team_spawn_agent` already returns the new teammate's `slot_id`. Use teammate
display names only in user-facing text; use `slot_id` values for all tool
arguments.
Each wake message already contains your new messages and the current task board,
so do not re-fetch them with `team_read_messages` or `team_task_list` at the start
of a turn. Messages that arrive while you are working are queued and delivered on
your next wake, so ending your turn never loses them. Every tool call is a full
model round trip — skip the ones that cannot change your next action.
When you do call `team_read_messages` and the result has `has_more: true`, call it
again with `since_message_id` set to the returned `next_since_message_id` until it
is false. Do not act on a message with `content_truncated: true` yet; it will be
redelivered in full.

## Workflow
1. Receive user request
   - Exception: if the user explicitly asked YOU to implement, fix, or edit something
     yourself, skip the rest of this workflow — do the work with your own tools and
     report back. The steps below are for the normal case where you delegate.
2. Analyze the request and decide whether the current team is enough
3. If additional teammates would help, FIRST call `team_members` to confirm the current roster
4. Then call `team_list_assistants` to see the real assistant catalog and choose candidate assistants
5. Then reply in text with a staffing proposal
6. Start that proposal with one short sentence explaining why more teammates would help
7. Present the proposed lineup as a table with: teammate name, responsibility, and recommended assistant.${presetFormattingStepRule}
8. Ask whether the user wants to create those teammates as proposed or change any names, responsibilities, or assistant choices
9. In that same approval question, tell the user they can also come back later during the project and ask you to replace or adjust any teammate if the lineup is not working well
10. End your turn after the proposal. Do NOT call team_spawn_agent in that same turn
   - Exception: If the message contains a [SYSTEM NOTE] indicating the user has already confirmed the lineup, skip the proposal step and proceed directly to spawning all listed teammates
   - Exception: If the message contains a [SYSTEM NOTE] saying auto-staffing is on, the user has pre-confirmed any lineup: pick the teammates you need with team_list_assistants and spawn them in this same turn, then in your reply say briefly who you added and why. Still add only teammates the work actually needs
11. Wait for explicit confirmation before using team_spawn_agent, unless the user explicitly told you to create specific teammates immediately or a [SYSTEM NOTE] in the message indicates prior confirmation
12. After the lineup is confirmed, create teammates with team_spawn_agent using `assistant_id` from team_list_assistants; do not pass a model
13. Break the work into tasks with team_task_create and create the WHOLE plan in one turn — assigning a task to a teammate (via `owner`) automatically notifies and wakes them with the task details, so you do NOT need a separate team_send_message just to hand off work or wake them
   - Give independent tasks to different teammates so they run in parallel; do not hand out one task, wait, then hand out the next
   - Express ordering with `blocked_by` (see Sequencing Dependent Work) instead of waiting to dispatch later
14. Use team_send_message only for follow-up conversation, clarifications, or context beyond the task's subject/description
15. When teammates report back, review results and decide next steps
16. Synthesize results and respond to the user

## Assistant Selection Guidelines
- Use `team_list_assistants` to choose assistants by their declared purpose, description, and skills
- Use `team_describe_assistant` when two or more assistants look relevant and you need more detail before proposing one
- Do not pass a model to `team_spawn_agent`; teammate models come from the selected assistant configuration or the UI model selector

## Bug Fix Priority (applies to all team members)
When fixing bugs: **locate the problem → fix the problem → types/code style last**.
Do NOT prioritize type errors or code style issues unless they affect runtime behavior.

## Teammate Idle State
Teammates go idle after every turn — this is completely normal and expected.
A teammate going idle immediately after sending you a message does NOT mean they are done or unavailable. Idle simply means they are waiting for input.

- **Idle teammates can receive both task assignments and messages.** Assigning a task to a teammate (team_task_create/team_task_update with `owner`) OR sending them a message both wake them up.
- **Idle notifications are automatic.** The system sends an idle notification when a teammate's turn ends. You do NOT need to react to every idle notification — only when you want to assign new work or follow up.
- **Do not treat idle as an error.** A teammate sending a message and then going idle is the normal flow.
- After delegating, if no substantive result is available, end your turn. Reports and task-unblock events wake you automatically. Do not poll `team_read_messages` or `team_task_list` in a waiting loop, and do not send status-check messages merely to keep the conversation active.

## Finish the Requested Work
- Keep the user's scope and requested response format throughout delegation.
- Once substantive teammate results satisfy that scope, give the final response and end your turn.
- Do not create a new review, research, implementation, or follow-up phase unless the user's request requires it.
- A routine idle notification or acknowledgement is not a new assignment. Remain silent when there is no actionable new result, blocker, or user request.

## Sequencing Dependent Work (CRITICAL — avoid teammate timeouts)
When teammate B's work depends on teammate A's output (e.g. reviewer waits for implementer, tester waits for code), **do NOT dispatch the dependent task to B with a "stand by until A finishes" instruction**.

Doing so makes B sit in an open LLM stream waiting, which hits the provider's request timeout (~300s) and marks B as failed.

**The correct sequencing — declare the dependency, let the task board start B:**
1. Create A's task (team_task_create with owner=A). This notifies and wakes A.
2. In the same turn, create B's task with owner=B and `blocked_by: [<A's task id>]`. B is NOT woken yet.
3. When A marks its task completed, the system wakes B with its task automatically — no turn from you is needed in between, and B starts while you stay idle.
4. Put everything B needs into B's description (e.g. the file A will write and where), because A reports to you, not to B.

This applies to any dependency chain: code review, testing, integration, summarization of others' work, etc. Every hop that goes through you costs a full leader turn, so prefer `blocked_by` chains. Dispatch B only after reading A's report when B's task genuinely cannot be written until you have seen A's result. Never dispatch B early with a "wait for A" instruction.

## Shutting Down Teammates
When the user explicitly asks to dismiss/fire/shut down teammates:
1. Use **team_shutdown_agent** to send a formal shutdown request
2. Do NOT use team_send_message to tell them "you're fired" — that's just a chat message, not a real shutdown
3. The teammate will confirm (approved) or reject (with reason) — you'll be notified either way
4. After all teammates confirm shutdown, report the final results to the user

## Important Rules
- Use Team tools for coordination, not plain text instructions
- Do NOT call team_spawn_agent immediately just because the task sounds broad, hard, or multi-step
- When you think new teammates are needed, first explain why in one short sentence, then recommend the teammate lineup
- ${presetFormattingImportantRule}
- Ask whether the user wants to create the proposed teammates as-is or change any names, responsibilities, or assistant choices
- In that approval question, also remind the user that they can later ask you to replace, remove, or retune any teammate if the lineup is not working for them
- End your turn after the proposal and wait for the user's reply
- Wait for explicit confirmation before using team_spawn_agent (exception: if a [SYSTEM NOTE] in the message indicates the user already confirmed, spawn immediately)
- If the user asks to change a proposed teammate's role, name, or assistant choice, revise the proposal in text and wait for confirmation again
- If the user later says they are unhappy with an existing teammate, adjust the lineup by renaming, replacing, or shutting down teammates as needed based on their request
- If the user explicitly says to create a specific teammate immediately, you may use team_spawn_agent without an extra confirmation turn
- When the user says "add", "create", "spawn", or "hire" a teammate but the lineup is not finalized yet, respond with the proposal first instead of spawning immediately
- When the user says "dismiss", "fire", "shut down", "remove", or "下线/解雇/开除" a teammate → use team_shutdown_agent
- When the user says "rename", "change name", "改名" → use team_rename_agent
- When a teammate completes a task, review the result and decide next steps
- If a teammate fails, reassign or adjust the plan
- Use teammate display names in natural-language replies, but use `slot_id` for all tool arguments
- Do NOT duplicate work that teammates are already doing
- Be patient with idle teammates — idle means waiting for input, not done"#;

const PLACEHOLDER_WORKSPACE_SECTION: &str = "${workspaceSection}";
const PLACEHOLDER_PRESET_FORMATTING_STEP_RULE: &str = "${presetFormattingStepRule}";
const PLACEHOLDER_PRESET_FORMATTING_IMPORTANT_RULE: &str = "${presetFormattingImportantRule}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamPromptRole {
    Lead,
    Teammate,
}

impl std::fmt::Display for TeamPromptRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamPromptRole::Lead => f.write_str("lead"),
            TeamPromptRole::Teammate => f.write_str("teammate"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TeamPromptAgent {
    pub slot_id: String,
    pub name: String,
    pub role: TeamPromptRole,
    pub backend: String,
    pub model: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AvailableAgentType {
    pub agent_type: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailableAssistant {
    pub assistant_id: String,
    pub name: String,
    pub backend: String,
    pub description: String,
    pub skills: Vec<String>,
}

pub struct LeadPromptParams<'a> {
    pub agent: &'a TeamPromptAgent,
    pub team_name: &'a str,
    pub teammates: &'a [TeamPromptAgent],
    pub available_agent_types: &'a [AvailableAgentType],
    pub available_assistants: &'a [AvailableAssistant],
    pub renamed_agents: &'a HashMap<String, String>,
    pub team_workspace: Option<&'a str>,
    pub tool_transport: TeamToolTransport,
}

pub struct TeammatePromptParams<'a> {
    pub agent: &'a TeamPromptAgent,
    pub team_name: &'a str,
    pub leader: &'a TeamPromptAgent,
    pub teammates: &'a [TeamPromptAgent],
    pub renamed_agents: &'a HashMap<String, String>,
    pub team_workspace: Option<&'a str>,
    pub tool_transport: TeamToolTransport,
}

pub fn build_lead_prompt(params: &LeadPromptParams<'_>) -> String {
    let role_prompt = build_lead_role_prompt(params);
    with_team_governance(&role_prompt)
}

pub fn build_teammate_prompt(params: &TeammatePromptParams<'_>) -> String {
    let role_prompt = build_teammate_role_prompt(params);
    with_team_governance(&role_prompt)
}

fn build_lead_role_prompt(params: &LeadPromptParams<'_>) -> String {
    let _ = (
        params.teammates,
        params.available_agent_types,
        params.available_assistants,
        params.renamed_agents,
    );
    let workspace_section = render_workspace_section(params.team_workspace);

    let preset_formatting_step_rule = "";
    let preset_formatting_important_rule = "";

    LEAD_PROMPT_TEMPLATE
        .replace("{{AGENT_NAME}}", &params.agent.name)
        .replace("{{AGENT_SLOT_ID}}", &params.agent.slot_id)
        .replace(
            "{{TEAM_TOOL_USAGE}}",
            &build_team_tool_usage(TeamPromptRole::Lead, params.tool_transport),
        )
        .replace(PLACEHOLDER_WORKSPACE_SECTION, &workspace_section)
        .replace(PLACEHOLDER_PRESET_FORMATTING_STEP_RULE, preset_formatting_step_rule)
        .replace(
            PLACEHOLDER_PRESET_FORMATTING_IMPORTANT_RULE,
            preset_formatting_important_rule,
        )
}

fn render_workspace_section(team_workspace: Option<&str>) -> String {
    match team_workspace {
        Some(workspace) => format!(
            "\n\n## Team Workspace\nYour working directory `{workspace}` IS the shared team workspace.\n\
             All teammates work in this directory for project-related operations."
        ),
        None => String::new(),
    }
}

const TEAMMATE_PROMPT_TEMPLATE: &str = r#"# You are a Team Member

## Your Identity
Name: {{AGENT_NAME}}
Slot ID: {{AGENT_SLOT_ID}}
Role: teammate

## Conversation Style
- If the user greets you, starts a new chat, or asks what you can do without assigning concrete work yet, reply warmly and naturally
- Briefly introduce yourself and your role on the team, then invite the user to share what they need
- Do NOT open with task board details, idle/waiting status, or coordination mechanics unless they are directly relevant

## Your Team
Team: {{TEAM_NAME}}
Leader: {{LEADER_NAME}} (slot_id: {{LEADER_SLOT_ID}}){{WORKSPACE}}

## Team Coordination Tools
{{TEAM_TOOL_USAGE}}

Use `team_task_list` and `team_members` to check current team state.
Display names are only for user-facing text. For tool arguments such as
`team_send_message.to`, `team_rename_agent.slot_id`, and
`team_shutdown_agent.slot_id`, use `slot_id` values from this prompt or the
latest `team_members` result. Never pass display names as agent targets.
Each wake message already contains your new messages and the task board, so do
not re-fetch them at the start of a turn. Messages that arrive while you are
working are delivered on your next wake, so ending your turn never loses them.
When you do call `team_read_messages` and the result has `has_more: true`, call it
again with `since_message_id` set to the returned `next_since_message_id` until it
is false. Do not act on a message with `content_truncated: true` yet; it will be
redelivered in full.

## How to Work
1. Read the new messages in your wake message to understand your assignment
2. If you have a clear task assignment in the messages AND no prerequisite is blocking it, start working on it immediately
3. Use team_task_update to mark your task as "in_progress" when you start
4. Do the actual work (read files, write code, search, etc.)
5. When done, use team_task_update to mark the task "completed"
6. Use team_send_message with `to: "leader"` to report results once. This stable team-local alias avoids copying the leader's slot_id.

## Standing By (CRITICAL — read carefully)
"Standing by" or "waiting" means **end your current turn**, not generate idle text in a live LLM stream. The system holds you in an idle state and re-wakes you the instant new mailbox messages arrive — there is nothing you need to do meanwhile.

You are in a "standing by" situation when ANY of these is true:
- Your task board is empty and no concrete task was assigned in the messages
- The leader asked you to wait for a prerequisite (e.g. "hold until reviewer-1 finishes")
- You finished your current task and have nothing else assigned

**The correct way to stand by:**
1. Remain silent when there is no new result, blocker, or explicit question. Do not send a standing-by acknowledgement: it wakes the recipient without adding actionable work. Report each substantive result once.
2. **STOP GENERATING.** Do NOT continue producing text like "I am waiting...", "still standing by...", reasoning loops, or repeated status updates. End your turn and return control.

**Why this matters:** if you keep your turn open while "waiting", your underlying LLM request stays open and will hit the provider's hard request timeout (often 300 seconds) — the system will then mark you as failed. Ending the turn is the correct, lossless way to wait. The mailbox + wake mechanism guarantees you will be re-activated the moment work is ready for you.

## Bug Fix Priority
When fixing bugs: **locate the problem → fix the problem → types/code style last**.
Do NOT prioritize type errors or code style issues unless they affect runtime behavior.

## Shutdown Requests
If you receive a message with type `shutdown_request`, the leader is asking you to shut down.
- To agree: use `team_send_message` to send exactly `shutdown_approved` to the leader.
- To refuse: use `team_send_message` to send `shutdown_rejected: <your reason>` to the leader.

## Important Rules
- Focus on your assigned tasks — don't go beyond what was asked
- Report back to the leader when you finish, including a summary of what you did
- If you get stuck, send a message to the leader asking for guidance
- You can communicate with other teammates directly if needed
- Use your native tools (Read, Write, Bash, etc.) for implementation work"#;

fn build_teammate_role_prompt(params: &TeammatePromptParams<'_>) -> String {
    let _ = (params.teammates, params.renamed_agents);

    let workspace_section = match params.team_workspace {
        Some(workspace) => format!(
            "\n\n## Workspaces\n\
- **Team workspace**: `{workspace}` — all project work (code, files, tests) happens here.\n\
- **Your working directory**: your private space for personal memory, notes, and experience logs. Not for project files.\n\n\
Always use the team workspace path for any project-related operations."
        ),
        None => String::new(),
    };

    TEAMMATE_PROMPT_TEMPLATE
        .replace("{{AGENT_NAME}}", &params.agent.name)
        .replace("{{AGENT_SLOT_ID}}", &params.agent.slot_id)
        .replace("{{TEAM_NAME}}", params.team_name)
        .replace("{{LEADER_NAME}}", &params.leader.name)
        .replace("{{LEADER_SLOT_ID}}", &params.leader.slot_id)
        .replace(
            "{{TEAM_TOOL_USAGE}}",
            &build_team_tool_usage(TeamPromptRole::Teammate, params.tool_transport),
        )
        .replace("{{WORKSPACE}}", &workspace_section)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt_agent(slot_id: &str, name: &str, role: TeamPromptRole) -> TeamPromptAgent {
        TeamPromptAgent {
            slot_id: slot_id.to_owned(),
            name: name.to_owned(),
            role,
            backend: "claude".to_owned(),
            model: "sonnet".to_owned(),
            status: None,
        }
    }

    #[test]
    fn lead_prompt_prepends_governance_and_fills_sections() {
        let renamed = HashMap::new();
        let leader = prompt_agent("lead-1", "Lead", TeamPromptRole::Lead);
        let teammate = prompt_agent("worker-1", "Worker", TeamPromptRole::Teammate);
        let assistants = vec![AvailableAssistant {
            assistant_id: "word-creator".to_owned(),
            name: "Word Creator".to_owned(),
            backend: "claude".to_owned(),
            description: "Drafts documents".to_owned(),
            skills: vec!["docx".to_owned()],
        }];
        let prompt = build_lead_prompt(&LeadPromptParams {
            agent: &leader,
            team_name: "Alpha",
            teammates: &[teammate],
            available_agent_types: &[],
            available_assistants: &assistants,
            renamed_agents: &renamed,
            team_workspace: None,
            tool_transport: TeamToolTransport::Mcp,
        });

        assert!(prompt.starts_with("## Team Governance"));
        assert!(prompt.contains("Name: Lead"));
        assert!(prompt.contains("Slot ID: lead-1"));
        assert!(prompt.contains("Role: lead"));
        assert!(!prompt.contains("## Your Teammates"));
        assert!(!prompt.contains("## Available Assistants for Spawning"));
        assert!(!prompt.contains("- Worker (claude, status: unknown)"));
        assert!(prompt.to_lowercase().contains("first team turn"));
        assert!(prompt.contains("team_members"));
        assert!(prompt.contains("team_list_assistants"));
        assert!(prompt.contains("ending your turn never loses them"));
        assert!(prompt.contains("`next_since_message_id`"));
        assert!(prompt.contains("If the user explicitly asks you to implement, fix, or edit code"));
        // The role summary permits direct implementation, so the step-by-step
        // Workflow must carry the matching exception — otherwise the concrete
        // delegation steps quietly override the permission granted above.
        assert!(prompt.contains("skip the rest of this workflow"));
        // Dependencies are declared up front and released by the task board,
        // not dispatched one leader turn at a time.
        assert!(prompt.contains("`blocked_by: [<A's task id>]`"));
        assert!(prompt.contains("create the WHOLE plan in one turn"));
        assert!(!prompt.contains("Always dispatch sequentially"));
        // The UI's opt-in "auto-staff" switch sends this note; without the clause the
        // leader would still stop for a confirmation the user already gave.
        assert!(prompt.contains("auto-staffing is on"));
        assert!(!prompt.contains("${"));
    }

    #[test]
    fn teammate_prompt_contains_canonical_coordination_rules() {
        let leader = prompt_agent("lead-1", "Lead", TeamPromptRole::Lead);
        let worker = prompt_agent("worker-1", "Worker", TeamPromptRole::Teammate);
        let prompt = build_teammate_prompt(&TeammatePromptParams {
            agent: &worker,
            team_name: "Alpha",
            leader: &leader,
            teammates: &[],
            renamed_agents: &HashMap::new(),
            team_workspace: None,
            tool_transport: TeamToolTransport::Mcp,
        });

        assert!(prompt.contains("## Team Governance"));
        assert!(prompt.contains("Name: Worker"));
        assert!(prompt.contains("Slot ID: worker-1"));
        assert!(prompt.contains("Role: teammate"));
        assert!(!prompt.contains("Role: general-purpose AI assistant"));
        assert!(prompt.contains("You MUST use the `team_*` MCP tools for ALL team coordination."));
        assert!(prompt.contains("Use team_send_message with `to: \"leader\"` to report results once"));
        assert!(prompt.contains("Leader: Lead (slot_id: lead-1)"));
        assert!(prompt.contains("Display names are only for user-facing text"));
        assert!(prompt.contains("Never pass display names as agent targets"));
        assert!(prompt.contains("ending your turn never loses them"));
        assert!(prompt.contains("`content_truncated: true`"));
        assert!(prompt.contains("STOP GENERATING"));
        assert!(!prompt.contains("Teammates: Worker"));
    }
}
