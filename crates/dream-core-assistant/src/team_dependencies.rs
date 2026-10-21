//! User-scoped team expert copies; personal definitions never share their identity.
use super::*;
use dream_core_api_types::employee_sync::TeamAssistantSnapshot;
use sha2::{Digest, Sha256};

fn scope_prefix(scope: &str) -> String {
    format!("team:{}:", serde_json::to_string(scope).expect("string serialization"))
}

impl AssistantService {
    pub async fn export_team_assistant(
        &self,
        owner: &str,
        id: &str,
        agent_override: Option<&str>,
    ) -> Result<TeamAssistantSnapshot, AssistantError> {
        let mut snapshots = self
            .export_team_assistants(&[(owner.to_owned(), id.to_owned(), agent_override.map(str::to_owned))])
            .await?;
        Ok(snapshots.remove(0))
    }

    pub async fn export_team_assistants(
        &self,
        references: &[(String, String, Option<String>)],
    ) -> Result<Vec<TeamAssistantSnapshot>, AssistantError> {
        let mut projections = HashMap::new();
        // Reconcile the owner's catalog once per bundle, not once per expert.
        for (owner, _, _) in references {
            if let std::collections::hash_map::Entry::Vacant(entry) = projections.entry(owner.clone()) {
                entry.insert(self.reconcile_generated_assistants_for_user(owner).await?);
            }
        }
        let mut snapshots = Vec::with_capacity(references.len());
        for (owner, id, agent_override) in references {
            let detail = self
                .detail_with_projections(owner, id, None, &projections[owner])
                .await?;
            snapshots.push(
                self.team_snapshot_from_detail(owner, id, agent_override.as_deref(), detail)
                    .await?,
            );
        }
        Ok(snapshots)
    }

    async fn team_snapshot_from_detail(
        &self,
        owner: &str,
        id: &str,
        agent_override: Option<&str>,
        detail: AssistantDetailResponse,
    ) -> Result<TeamAssistantSnapshot, AssistantError> {
        let backend = self
            .resolve_runtime_backend_for_agent_id(owner, agent_override.unwrap_or(&detail.engine.agent_id))
            .await?;
        let mut profile = detail.profile;
        // Local avatar paths and the administrator's preferences/credentials
        // are not portable executable dependencies.
        profile.avatar = None;
        Ok(TeamAssistantSnapshot {
            key: serde_json::to_string(&(owner, id, agent_override))
                .map_err(|e| AssistantError::Internal(e.to_string()))?,
            profile,
            backend,
            rules: detail.rules.content,
            prompts: detail.prompts,
            permission: detail.defaults.permission,
            thought_level: detail.defaults.thought_level,
        })
    }

    pub async fn materialize_team_assistants(
        &self,
        user: &str,
        scope: &str,
        snapshots: &[TeamAssistantSnapshot],
    ) -> Result<HashMap<String, String>, AssistantError> {
        if scope.trim().is_empty() || scope.len() > 2048 {
            return Err(AssistantError::BadRequest(
                "a valid enterprise source scope is required".into(),
            ));
        }
        let prefix = scope_prefix(scope);
        let mut result = HashMap::new();
        let mut agent_ids = HashMap::new();
        for snapshot in snapshots {
            let source_ref = format!("{prefix}{}", snapshot.key);
            let local_id = format!("team_{:x}", Sha256::digest(source_ref.as_bytes()));
            let existing = self
                .definition_repo
                .get_by_assistant_id_including_deleted_for_user(user, &local_id)
                .await?;
            if existing
                .as_ref()
                .is_some_and(|row| row.source != "imported" || row.source_ref.as_deref() != Some(&source_ref))
            {
                return Err(AssistantError::Conflict(
                    "team expert conflicts with a personal definition".into(),
                ));
            }
            let agent_id = if let Some(id) = agent_ids.get(&snapshot.backend) {
                String::clone(id)
            } else {
                let id = self.resolve_agent_id_for_agent_ref(user, &snapshot.backend).await?;
                agent_ids.insert(snapshot.backend.clone(), id.clone());
                id
            };
            let definition_id = existing
                .as_ref()
                .map(|row| row.id.clone())
                .unwrap_or_else(|| generate_prefixed_id("asstdef"));
            let name_i18n = serde_json::to_string(&snapshot.profile.name_i18n)
                .map_err(|e| AssistantError::Internal(e.to_string()))?;
            let description_i18n = serde_json::to_string(&snapshot.profile.description_i18n)
                .map_err(|e| AssistantError::Internal(e.to_string()))?;
            let prompts = serde_json::to_string(&snapshot.prompts.recommended)
                .map_err(|e| AssistantError::Internal(e.to_string()))?;
            let prompts_i18n = serde_json::to_string(&snapshot.prompts.recommended_i18n)
                .map_err(|e| AssistantError::Internal(e.to_string()))?;
            let unchanged = existing.as_ref().is_some_and(|row| {
                row.deleted_at.is_none()
                    && row.name == snapshot.profile.name
                    && row.name_i18n == name_i18n
                    && row.description == snapshot.profile.description
                    && row.description_i18n == description_i18n
                    && row.agent_id == agent_id
                    && row.recommended_prompts == prompts
                    && row.recommended_prompts_i18n == prompts_i18n
                    && row.default_permission_mode == snapshot.permission.mode
                    && row.default_permission_value == snapshot.permission.value
                    && row.default_thought_level_mode == snapshot.thought_level.mode
                    && row.default_thought_level_value == snapshot.thought_level.value
                    && row.default_mcps_mode == "fixed"
                    && row.default_mcp_ids == "[]"
                    && row.default_skills_mode == "fixed"
                    && row.default_skill_ids == "[]"
            });
            if unchanged
                && tokio::fs::read_to_string(self.user_rule_path_for_user(user, &local_id, None))
                    .await
                    .ok()
                    .as_deref()
                    == Some(snapshot.rules.as_str())
            {
                result.insert(snapshot.key.clone(), local_id);
                continue;
            }
            self.definition_repo
                .upsert_for_user(
                    user,
                    &UpsertAssistantDefinitionParams {
                        id: &definition_id,
                        assistant_id: &local_id,
                        source: "imported",
                        owner_type: "user",
                        source_ref: Some(&source_ref),
                        name: &snapshot.profile.name,
                        name_i18n: &name_i18n,
                        description: snapshot.profile.description.as_deref(),
                        description_i18n: &description_i18n,
                        avatar_type: "none",
                        avatar_value: None,
                        agent_id: &agent_id,
                        rule_resource_type: "user_file",
                        rule_resource_ref: Some(&local_id),
                        recommended_prompts: &prompts,
                        recommended_prompts_i18n: &prompts_i18n,
                        default_model_mode: "auto",
                        default_model_value: None,
                        default_permission_mode: &snapshot.permission.mode,
                        default_permission_value: snapshot.permission.value.as_deref(),
                        default_thought_level_mode: &snapshot.thought_level.mode,
                        default_thought_level_value: snapshot.thought_level.value.as_deref(),
                        // Employee bindings are independently authorized and resolved by
                        // EmployeeRuntime. Never inherit the owner's private tool ids.
                        default_skills_mode: "fixed",
                        default_skill_ids: "[]",
                        custom_skill_names: "[]",
                        default_disabled_builtin_skill_ids: "[]",
                        default_mcps_mode: "fixed",
                        default_mcp_ids: "[]",
                    },
                )
                .await?;
            self.write_rule_for_user(user, &local_id, None, &snapshot.rules).await?;
            result.insert(snapshot.key.clone(), local_id);
        }
        Ok(result)
    }

    pub async fn prune_team_assistants(
        &self,
        user: &str,
        scope: &str,
        retained: &[String],
    ) -> Result<(), AssistantError> {
        // Empty scope is reserved for the explicit enterprise-disconnect purge.
        let prefix = if scope.is_empty() {
            "team:".to_owned()
        } else {
            scope_prefix(scope)
        };
        for row in self.definition_repo.list_for_user(user).await? {
            if row.source == "imported"
                && row
                    .source_ref
                    .as_deref()
                    .is_some_and(|source| source.starts_with(&prefix))
                && !retained.contains(&row.assistant_id)
            {
                self.state_repo.delete_for_user(user, &row.id).await?;
                self.preference_repo.delete_for_user(user, &row.id).await?;
                self.definition_repo
                    .soft_delete_for_user(user, &row.id, now_ms())
                    .await?;
                self.cleanup_user_assets_for_user(user, &row.assistant_id);
            }
        }
        Ok(())
    }
}
