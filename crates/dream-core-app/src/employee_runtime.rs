//! Resolve enterprise employee bindings at execution time, never into persisted extra.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use dream_core_ai_agent::{AgentError, AgentSessionKind, task_manager::AgentFactory, types::BuildTaskOptions};
use dream_core_api_types::{SessionMcpServer, SessionMcpTransport};
use dream_core_mcp::types::McpServerTransport;
use dream_domain_devops::DevopsService;
use dream_domain_employee::{EmployeeService, TEAM_ORIGIN, TRIGGER_CHAT};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Default)]
pub(crate) struct EmployeeRuntime {
    services: OnceLock<(Arc<EmployeeService>, Arc<DevopsService>)>,
    /// Local team copies — consulted only for employees the company
    /// distributed (`origin = 'team'`).
    team: Option<TeamResourceSource>,
    revisions: tokio::sync::Mutex<HashMap<String, [u8; 32]>>,
}

impl EmployeeRuntime {
    pub(crate) fn new(team: TeamResourceSource) -> Self {
        Self {
            team: Some(team),
            ..Self::default()
        }
    }

    pub(crate) fn bind(&self, employee: Arc<EmployeeService>, devops: Arc<DevopsService>) {
        let _ = self.services.set((employee, devops));
    }

    pub(crate) fn wrap(self: &Arc<Self>, inner: AgentFactory) -> AgentFactory {
        let resolver = self.clone();
        Arc::new(move |mut options| {
            let resolver = resolver.clone();
            let inner = inner.clone();
            Box::pin(async move {
                resolver.enrich(&mut options).await?;
                inner(options).await
            })
        })
    }

    pub(crate) async fn check_conversation(&self, actor: &str, conversation: &str) -> Result<(), AgentError> {
        let Some((employee, devops)) = self.services.get() else {
            return Ok(());
        };
        let Some(agent) = employee
            .runtime_agent_for_conversation(actor, conversation)
            .await
            .map_err(|e| AgentError::internal(format!("Employee is unavailable or no longer authorized: {e}")))?
        else {
            return Ok(());
        };
        let config: Value = serde_json::from_str(&agent.automation_config)
            .map_err(|_| AgentError::internal("Invalid employee automation configuration"))?;
        let mut revision = Sha256::new();
        revision.update(agent.automation_config.as_bytes());
        revision.update(agent.updated_at.to_le_bytes());
        let skills = binding_ids(&config, "boundSkillIds")?;
        if agent.origin == TEAM_ORIGIN {
            let team = self
                .team
                .as_ref()
                .ok_or_else(|| AgentError::internal("Team resources are unavailable"))?;
            for id in &skills {
                let (_, content) = dream_core_extension::team_sync::read_team_skill(&team.skills_dir, id)
                    .await
                    .ok_or_else(unavailable_skill)?;
                revision.update(id.as_bytes());
                revision.update(content.as_bytes());
            }
            for id in binding_ids(&config, "boundMcpIds")? {
                let (_, transport) = team
                    .mcp
                    .team_server_transport(actor, &id)
                    .await
                    .map_err(|e| AgentError::internal(e.to_string()))?
                    .ok_or_else(unavailable_mcp)?;
                revision.update(id.as_bytes());
                let mut transport = serde_json::to_value(session_transport(transport))
                    .map_err(|e| AgentError::internal(e.to_string()))?;
                transport.sort_all_objects();
                revision.update(serde_json::to_vec(&transport).map_err(|e| AgentError::internal(e.to_string()))?);
            }
            return self.check_revision(conversation, revision.finalize().into()).await;
        }
        if !skills.is_empty() {
            let available = devops
                .runtime_skills(actor)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            if skills.iter().any(|id| !available.iter().any(|row| &row.id == id)) {
                return Err(AgentError::internal(
                    "Bound skill is unavailable or not authorized for this member",
                ));
            }
            for id in &skills {
                let row = available
                    .iter()
                    .find(|row| &row.id == id)
                    .expect("binding checked above");
                revision.update(id.as_bytes());
                revision.update(row.updated_at.to_le_bytes());
            }
        }
        let servers = binding_ids(&config, "boundMcpIds")?;
        if !servers.is_empty() {
            let available = devops
                .runtime_mcp_registry(actor)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            if servers.iter().any(|id| !available.iter().any(|row| &row.id == id)) {
                return Err(AgentError::internal(
                    "Bound MCP server is unavailable or not authorized for this member",
                ));
            }
            for id in &servers {
                let row = available
                    .iter()
                    .find(|row| &row.id == id)
                    .expect("binding checked above");
                revision.update(id.as_bytes());
                revision.update(row.updated_at.to_le_bytes());
            }
        }
        let revision: [u8; 32] = revision.finalize().into();
        self.check_revision(conversation, revision).await
    }

    async fn check_revision(&self, conversation: &str, revision: [u8; 32]) -> Result<(), AgentError> {
        let mut revisions = self.revisions.lock().await;
        let initial = revisions.entry(conversation.to_owned()).or_insert(revision);
        if *initial != revision {
            return Err(AgentError::internal(
                "Employee or bound resource configuration changed; start a new run",
            ));
        }
        Ok(())
    }

    async fn enrich(&self, options: &mut BuildTaskOptions) -> Result<(), AgentError> {
        let Some((employee, devops)) = self.services.get() else {
            return Ok(());
        };
        let actor = options.context.conversation.user_id.clone();
        let binding = employee
            .runtime_binding_for_conversation(&actor, &options.context.conversation.conversation_id)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?;
        let Some((agent, trigger)) = binding else {
            return Ok(());
        };
        let config: Value = serde_json::from_str(&agent.automation_config)
            .map_err(|_| AgentError::internal("Invalid employee automation configuration"))?;
        let skill_ids = binding_ids(&config, "boundSkillIds")?;
        let mcp_ids = binding_ids(&config, "boundMcpIds")?;
        // An employee carries an explicit resource scope. Inheriting `None`
        // here would connect every enabled enterprise connector, even when
        // this employee binds only one (or none).
        let selected_local = match &mut options.context.kind {
            AgentSessionKind::DreamEngine(context) => employee_mcp_selection(&mut context.config.mcp_server_ids),
            AgentSessionKind::Acp(context) => employee_mcp_selection(&mut context.config.mcp_server_ids),
            AgentSessionKind::Antigravity(_) => Vec::new(),
        };
        tracing::info!(employee_id = %agent.id, bound_mcp_count = mcp_ids.len(), selected_mcp_count = selected_local.len(), "Resolved employee MCP scope");
        let persona = employee_persona(&config, &trigger, &agent.origin);
        if skill_ids.is_empty() && mcp_ids.is_empty() && persona.trim().is_empty() {
            return Ok(());
        }
        // A company-distributed employee resolves its bindings from the local
        // team copies, and only from them: on a member's machine those copies
        // are the only place the bindings exist, and the registry is not even
        // queryable there (a personal build has no org tables, so asking it
        // fails outright). An employee defined on this machine binds rows of
        // this machine's own registry and nothing else.
        let team = if agent.origin == TEAM_ORIGIN {
            self.team.as_ref()
        } else {
            None
        };

        let mut instructions = vec![persona.to_owned()];
        if !skill_ids.is_empty() {
            match team {
                Some(team) => {
                    for id in &skill_ids {
                        let (name, content) = dream_core_extension::team_sync::read_team_skill(&team.skills_dir, id)
                            .await
                            .ok_or_else(unavailable_skill)?;
                        instructions.push(format!("[Bound skill: {name}]\n{content}"));
                    }
                }
                None => {
                    let registry = devops
                        .runtime_skills(&actor)
                        .await
                        .map_err(|e| AgentError::internal(e.to_string()))?;
                    for id in &skill_ids {
                        let skill = registry.iter().find(|s| &s.id == id).ok_or_else(unavailable_skill)?;
                        instructions.push(format!("[Bound skill: {}]\n{}", skill.name, skill.content));
                    }
                }
            }
        }

        let mut servers = Vec::with_capacity(mcp_ids.len());
        if !mcp_ids.is_empty() {
            let registry = match team {
                Some(_) => Vec::new(),
                None => devops
                    .runtime_mcp_registry(&actor)
                    .await
                    .map_err(|e| AgentError::internal(e.to_string()))?,
            };
            for id in &mcp_ids {
                let transport = match team {
                    Some(team) => {
                        let (local_id, transport) = team
                            .mcp
                            .team_server_transport(&actor, id)
                            .await
                            .map_err(|e| AgentError::internal(e.to_string()))?
                            .ok_or_else(unavailable_mcp)?;
                        // The team sync materialized this connector as an
                        // enabled local server, so the session usually loads
                        // it already. Mounting it a second time starts a second
                        // process and makes every one of its tools collide with
                        // itself; the engine then renames them after the
                        // server — a company name the provider API rejects.
                        if already_loaded(Some(&selected_local), &local_id) {
                            continue;
                        }
                        session_transport(transport)
                    }
                    None => registry_transport(registry.iter().find(|s| &s.id == id).ok_or_else(unavailable_mcp)?)?,
                };
                servers.push(SessionMcpServer {
                    id: id.clone(),
                    name: format!("enterprise_{id}"),
                    transport,
                });
            }
        }

        let instructions = instructions
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        // The persona and bound skills reach a Dream session as standing rules
        // and an ACP CLI through its first-message preset context — the same
        // two channels an expert's own rules use on each backend.
        let (rules, session_servers) = match &mut options.context.kind {
            AgentSessionKind::DreamEngine(context) => (
                &mut context.config.preset_rules,
                &mut context.config.session_mcp_servers,
            ),
            AgentSessionKind::Acp(context) => (
                &mut context.config.preset_context,
                &mut context.config.session_mcp_servers,
            ),
            AgentSessionKind::Antigravity(_) => {
                return Err(AgentError::internal(
                    "This backend cannot carry a digital employee's persona or resources",
                ));
            }
        };
        if !instructions.is_empty() {
            *rules = Some(match rules.take() {
                Some(existing) if !existing.trim().is_empty() => format!("{existing}\n\n{instructions}"),
                _ => instructions,
            });
        }
        session_servers.extend(servers);
        Ok(())
    }
}

/// Where a member's machine finds the company resources a distributed
/// employee binds: the copies the team sync wrote for this member.
pub(crate) struct TeamResourceSource {
    pub(crate) mcp: dream_core_mcp::McpConfigService,
    pub(crate) skills_dir: PathBuf,
}

/// The persona a run of this employee carries.
///
/// A member's own employee keeps its standing prompt in `instructions` (the
/// create form calls it the system prompt). Scheduled and manual runs already
/// send it as the run prompt; a chat sends nothing, so there it stands in for
/// the persona. A company employee's persona is always explicit — its
/// `instructions`, when set, is a scheduled task, never an identity.
fn employee_persona(config: &Value, trigger: &str, origin: &str) -> String {
    let text = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let standing_prompt = || {
        (trigger == TRIGGER_CHAT && origin != TEAM_ORIGIN)
            .then(|| text("instructions"))
            .flatten()
    };
    text("persona").or_else(standing_prompt).unwrap_or_default().to_owned()
}

/// Whether an enabled local server is part of this session already: listed in
/// its snapshot, or — without one — loaded with every other enabled server.
fn already_loaded(selected_local: Option<&[String]>, local_id: &str) -> bool {
    selected_local.is_none_or(|ids| ids.iter().any(|id| id == local_id))
}

fn employee_mcp_selection(selected: &mut Option<Vec<String>>) -> Vec<String> {
    selected.get_or_insert_default().clone()
}

fn unavailable_skill() -> AgentError {
    AgentError::internal("Bound skill is unavailable or not authorized for this member")
}

fn unavailable_mcp() -> AgentError {
    AgentError::internal("Bound MCP server is unavailable or not authorized for this member")
}

fn registry_transport(server: &dream_domain_devops::models::McpRegistryDto) -> Result<SessionMcpTransport, AgentError> {
    let secrets: HashMap<String, String> = server
        .secrets_json
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| AgentError::internal("Invalid bound MCP credential configuration"))?
        .unwrap_or_default();
    match server.r#type.as_str() {
        "sse" => Ok(SessionMcpTransport::Sse {
            url: server.endpoint.clone(),
            headers: secrets,
        }),
        "stdio" => {
            let tokens = dream_core_mcp::service::shell_split(&server.endpoint)
                .map_err(|_| AgentError::internal("Invalid bound MCP command line"))?;
            let Some((command, args)) = tokens.split_first() else {
                return Err(AgentError::internal("Bound MCP command is empty"));
            };
            Ok(SessionMcpTransport::Stdio {
                command: command.clone(),
                args: args.to_vec(),
                env: secrets,
            })
        }
        _ => Err(AgentError::internal("Unsupported bound MCP transport")),
    }
}

fn session_transport(transport: McpServerTransport) -> SessionMcpTransport {
    match transport {
        McpServerTransport::Stdio { command, args, env } => SessionMcpTransport::Stdio { command, args, env },
        McpServerTransport::Sse { url, headers } => SessionMcpTransport::Sse { url, headers },
        McpServerTransport::Http { url, headers } => SessionMcpTransport::Http { url, headers },
    }
}

fn binding_ids(config: &Value, key: &str) -> Result<Vec<String>, AgentError> {
    config
        .get(key)
        .cloned()
        .filter(|v| !v.is_null())
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| AgentError::internal(format!("Invalid employee {key}")))
        .map(Option::unwrap_or_default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn an_existing_employee_session_rejects_changed_dependencies() {
        let runtime = EmployeeRuntime::default();
        assert!(runtime.check_revision("conversation", [1; 32]).await.is_ok());
        assert!(runtime.check_revision("conversation", [1; 32]).await.is_ok());
        assert!(runtime.check_revision("conversation", [2; 32]).await.is_err());
    }

    #[test]
    fn employee_without_a_snapshot_never_inherits_global_connectors() {
        let mut selected = None;
        assert!(employee_mcp_selection(&mut selected).is_empty());
        assert_eq!(selected, Some(Vec::new()));
        assert!(!already_loaded(selected.as_deref(), "unrelated_enabled_server"));
        let mut explicit = Some(vec!["selected_personal_server".to_owned()]);
        assert_eq!(employee_mcp_selection(&mut explicit), vec!["selected_personal_server"]);
        assert_eq!(explicit, Some(vec!["selected_personal_server".to_owned()]));
    }

    #[test]
    fn explicit_persona_wins_for_every_trigger() {
        let config = json!({ "persona": "You review contracts.", "instructions": "Check the inbox." });
        for trigger in ["chat", "manual", "cron"] {
            assert_eq!(employee_persona(&config, trigger, "team"), "You review contracts.");
            assert_eq!(
                employee_persona(&config, trigger, "self_built"),
                "You review contracts."
            );
        }
    }

    #[test]
    fn own_employee_instructions_stand_in_only_for_a_chat() {
        let config = json!({ "instructions": "You are a research coordinator." });
        assert_eq!(
            employee_persona(&config, "chat", "self_built"),
            "You are a research coordinator."
        );
        // Scheduled/manual runs already send it as the run prompt.
        assert_eq!(employee_persona(&config, "cron", "self_built"), "");
        assert_eq!(employee_persona(&config, "manual", "self_built"), "");
    }

    #[test]
    fn company_employee_instructions_are_never_a_persona() {
        let config = json!({ "instructions": "Post the weekly digest." });
        assert_eq!(employee_persona(&config, "chat", "team"), "");
    }

    #[test]
    fn an_enabled_team_server_counts_as_loaded_unless_a_snapshot_leaves_it_out() {
        assert!(
            already_loaded(None, "mcp_local"),
            "no snapshot loads every enabled server"
        );
        assert!(already_loaded(Some(&["mcp_local".to_owned()]), "mcp_local"));
        assert!(!already_loaded(Some(&["mcp_other".to_owned()]), "mcp_local"));
        assert!(!already_loaded(Some(&[]), "mcp_local"));
    }
}
