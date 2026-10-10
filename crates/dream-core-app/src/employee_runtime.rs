//! Resolve enterprise employee bindings at execution time, never into persisted extra.
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use dream_core_ai_agent::{AgentError, AgentSessionKind, task_manager::AgentFactory, types::BuildTaskOptions};
use dream_core_api_types::{SessionMcpServer, SessionMcpTransport};
use dream_domain_devops::DevopsService;
use dream_domain_employee::EmployeeService;
use serde_json::Value;
#[cfg(feature = "enterprise")]
use sha2::{Digest, Sha256};

#[derive(Default)]
pub(crate) struct EmployeeRuntime {
    services: OnceLock<(Arc<EmployeeService>, Arc<DevopsService>)>,
    #[cfg(feature = "enterprise")]
    revisions: tokio::sync::Mutex<HashMap<String, [u8; 32]>>,
}

impl EmployeeRuntime {
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

    #[cfg(feature = "enterprise")]
    pub(crate) async fn check_conversation(&self, actor: &str, conversation: &str) -> Result<(), AgentError> {
        let Some((employee, devops)) = self.services.get() else {
            return Ok(());
        };
        let Some(agent) = employee
            .runtime_agent_for_conversation(actor, conversation)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?
        else {
            return Ok(());
        };
        let config: Value = serde_json::from_str(&agent.automation_config)
            .map_err(|_| AgentError::internal("Invalid employee automation configuration"))?;
        let mut revision = Sha256::new();
        revision.update(agent.automation_config.as_bytes());
        revision.update(agent.updated_at.to_le_bytes());
        let skills = binding_ids(&config, "boundSkillIds")?;
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
        let actor = &options.context.conversation.user_id;
        let agent = employee
            .runtime_agent_for_conversation(actor, &options.context.conversation.conversation_id)
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?;
        let Some(agent) = agent else {
            return Ok(());
        };
        let config: Value = serde_json::from_str(&agent.automation_config)
            .map_err(|_| AgentError::internal("Invalid employee automation configuration"))?;
        let skill_ids = binding_ids(&config, "boundSkillIds")?;
        let mcp_ids = binding_ids(&config, "boundMcpIds")?;
        let persona = config.get("persona").and_then(Value::as_str).unwrap_or_default();
        let AgentSessionKind::DreamEngine(context) = &mut options.context.kind else {
            if !skill_ids.is_empty() || !mcp_ids.is_empty() || !persona.trim().is_empty() {
                return Err(AgentError::internal(
                    "Employee resource bindings require the Dream engine",
                ));
            }
            return Ok(());
        };
        let mut instructions = vec![persona.to_owned()];
        if !skill_ids.is_empty() {
            let skills = devops
                .runtime_skills(actor)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            for id in skill_ids {
                let skill = skills.iter().find(|s| s.id == id).ok_or_else(|| {
                    AgentError::internal("Bound skill is unavailable or not authorized for this member")
                })?;
                instructions.push(format!("[Bound skill: {}]\n{}", skill.name, skill.content));
            }
        }
        if !mcp_ids.is_empty() {
            let servers = devops
                .runtime_mcp_registry(actor)
                .await
                .map_err(|e| AgentError::internal(e.to_string()))?;
            for id in mcp_ids {
                let server = servers.iter().find(|s| s.id == id).ok_or_else(|| {
                    AgentError::internal("Bound MCP server is unavailable or not authorized for this member")
                })?;
                let secrets: HashMap<String, String> = server
                    .secrets_json
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|_| AgentError::internal("Invalid bound MCP credential configuration"))?
                    .unwrap_or_default();
                let transport = match server.r#type.as_str() {
                    "sse" => SessionMcpTransport::Sse {
                        url: server.endpoint.clone(),
                        headers: secrets,
                    },
                    "stdio" => {
                        let tokens = dream_core_mcp::service::shell_split(&server.endpoint)
                            .map_err(|_| AgentError::internal("Invalid bound MCP command line"))?;
                        let Some((command, args)) = tokens.split_first() else {
                            return Err(AgentError::internal("Bound MCP command is empty"));
                        };
                        SessionMcpTransport::Stdio {
                            command: command.clone(),
                            args: args.to_vec(),
                            env: secrets,
                        }
                    }
                    _ => return Err(AgentError::internal("Unsupported bound MCP transport")),
                };
                context.config.session_mcp_servers.push(SessionMcpServer {
                    id: server.id.clone(),
                    name: format!("enterprise_{}", server.id),
                    transport,
                });
            }
        }
        let instructions = instructions
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !instructions.is_empty() {
            context.config.preset_rules = Some(match context.config.preset_rules.take() {
                Some(existing) => format!("{existing}\n\n{instructions}"),
                None => instructions,
            });
        }
        Ok(())
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
