use std::collections::HashSet;
use std::path::Path;

use dream_core_extension::SkillSource;
use dream_engine_agent::skills::frontmatter::{parse_frontmatter, parse_skill_fields};
use dream_engine_agent::skills::types::{LoadedFrom, SkillMetadata, SkillSource as EngineSkillSource};

use super::AcpSkillManager;
use crate::error::AgentError;

impl AcpSkillManager {
    /// Resolve a session-local list without mutating the shared ACP discovery
    /// cache. Keep each authorized source root for scripts and other resources.
    pub(crate) async fn engine_skills_for_user(
        &self,
        user_id: &str,
        selected: &[String],
    ) -> Result<Vec<SkillMetadata>, AgentError> {
        let names: HashSet<&str> = selected.iter().map(String::as_str).collect();
        let available = self
            .list_available_skills_for_user(user_id)
            .await
            .map_err(|e| AgentError::internal(format!("Failed to resolve session skills: {e}")))?;
        let mut skills = Vec::new();
        for item in available {
            if !(names.contains(item.name.as_str()) || item.source == SkillSource::Team && item.auto_active) {
                continue;
            }
            let location = Path::new(&item.location);
            let manifest = if location.is_dir() {
                location.join("SKILL.md")
            } else {
                location.to_path_buf()
            };
            let content = match tokio::fs::read_to_string(&manifest).await {
                Ok(content) => content,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && matches!(item.source, SkillSource::Custom | SkillSource::Extension) =>
                {
                    // Imported skills are automatically selected for ordinary chats.
                    // A stale catalog row must not make every new chat unusable.
                    tracing::warn!(skill = %item.name, source = ?item.source,
                        "Skipping missing optional session skill; re-import or remove its catalog entry");
                    continue;
                }
                Err(error) => {
                    return Err(AgentError::internal(format!(
                        "Failed to read selected skill manifest ({}): {error}",
                        item.name
                    )));
                }
            };
            let parsed = parse_frontmatter(&content);
            let root = manifest.parent().map(|path| path.to_string_lossy().into_owned());
            skills.push(parse_skill_fields(
                &parsed.frontmatter,
                &parsed.content,
                &item.name,
                EngineSkillSource::Managed,
                LoadedFrom::Managed,
                root.as_deref(),
            ));
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }
}

#[cfg(test)]
#[path = "engine_test.rs"]
mod engine_test;
