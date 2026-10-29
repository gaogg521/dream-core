//! Small, context-matched scene defaults; explicit administrator choices win.
use crate::{PlatformService, error::PlatformError};
use dream_core_db::db_params;

type Candidate = (String, String, String, String, String);

fn matches_term(text: &str, term: &str) -> bool {
    if !term.is_ascii() {
        return text.contains(term);
    }
    text.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| {
        word == term
            || word.strip_suffix('s') == Some(term)
            || (term == "ops" && matches!(word, "devops" | "gitops"))
            || (["secur", "vulnerab", "copywrit", "monitor", "pentest"].contains(&term) && word.starts_with(term))
    })
}

fn recommend(name: &str, prompt: &str, candidates: &[Candidate]) -> Vec<String> {
    let terms = crate::scene_skill_defaults::mcp_terms(name, prompt);
    let mut ranked: Vec<_> = candidates
        .iter()
        .filter(|row| {
            let endpoint = row.4.to_lowercase();
            !endpoint.is_empty()
                && !["your-server", "/path/to/", "example.com", "your_api_key", "<your"]
                    .iter()
                    .any(|token| endpoint.contains(token))
        })
        .map(|row| {
            let title = row.1.to_lowercase();
            let desc = row.2.to_lowercase();
            let category = row.3.to_lowercase();
            let score: usize = terms
                .iter()
                .map(|word| {
                    usize::from(matches_term(&title, word)) * 8
                        + usize::from(matches_term(&category, word)) * 2
                        + usize::from(matches_term(&desc, word))
                })
                .sum();
            (score, row)
        })
        .filter(|(score, _)| *score >= 8)
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.1.cmp(&b.1.1))
            .then_with(|| a.1.0.cmp(&b.1.0))
    });
    ranked.into_iter().take(10).map(|(_, row)| row.0.clone()).collect()
}

impl PlatformService {
    pub(crate) async fn apply_scene_mcp_defaults(&self, tenant: &str) -> Result<(), PlatformError> {
        let scenes:Vec<(String,String,String,Option<String>)>=self.db.fetch_all_as(
            "SELECT s.id,s.name,COALESCE(s.prompt,''),d.context_key FROM one_scenes s LEFT JOIN one_scene_mcp_defaults d ON d.tenant_id=s.tenant_id AND d.scene_id=s.id WHERE s.tenant_id=?", &db_params![tenant]).await?;
        let scenes: Vec<_> = scenes
            .into_iter()
            .filter(|(_, name, prompt, previous)| {
                let context = serde_json::to_string(&(name, prompt)).expect("string tuple serializes");
                previous.as_deref() != Some(context.as_str())
            })
            .collect();
        if scenes.is_empty() {
            return Ok(());
        }
        // The MCP registry is optional in standalone platform tests and starts
        // empty before catalog import. Leave defaults pending in either case.
        let candidates:Vec<Candidate>=self.db.fetch_all_as(
            "SELECT m.id,m.name,COALESCE(SUBSTR(m.content,1,4000),''),COALESCE(c.name,''),m.endpoint FROM one_mcp_registry m LEFT JOIN one_content_categories c ON c.id=m.category_id WHERE m.enabled=1 AND m.published=1 AND m.visibility='all' AND m.scope='org' AND (m.has_keys=0 OR (m.secrets_json IS NOT NULL AND m.secrets_json<>'{}' AND m.secrets_json<>''))", &[]).await.unwrap_or_default();
        if candidates.is_empty() {
            return Ok(());
        }
        for (scene, name, prompt, previous) in scenes {
            let context =
                serde_json::to_string(&(&name, &prompt)).map_err(|e| PlatformError::Internal(e.to_string()))?;
            if previous.as_deref() == Some(context.as_str()) {
                continue;
            }
            self.db.execute("DELETE FROM one_resource_grants WHERE tenant_id=? AND subject_type='scene' AND subject_id=? AND resource_type='mcp' AND granted_by=?",&db_params![tenant,&scene,Self::BUILTIN_SCENE_GRANTOR]).await?;
            let existing:i64=self.db.fetch_one_scalar("SELECT COUNT(*) FROM one_resource_grants WHERE tenant_id=? AND subject_type='scene' AND subject_id=? AND resource_type='mcp'", &db_params![tenant,&scene]).await?;
            if existing == 0 {
                let recommended = recommend(&name, &prompt, &candidates);
                tracing::info!(
                    scene_id = &scene,
                    recommended_count = recommended.len(),
                    "Applied context-matched scene MCP defaults"
                );
                for id in recommended {
                    self.grant_resource(tenant, "scene", &scene, "mcp", &id, "use", Self::BUILTIN_SCENE_GRANTOR)
                        .await?;
                }
            }
            self.upsert(
                "INSERT INTO one_scene_mcp_defaults (tenant_id,scene_id,context_key,applied_at) VALUES (?,?,?,?) ON CONFLICT(tenant_id,scene_id) DO UPDATE SET context_key=excluded.context_key,applied_at=excluded.applied_at",
                "INSERT INTO one_scene_mcp_defaults (tenant_id,scene_id,context_key,applied_at) VALUES (?,?,?,?) AS new ON DUPLICATE KEY UPDATE context_key=new.context_key,applied_at=new.applied_at",
                &db_params![tenant,&scene,&context,dream_core_common::now_ms()]).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: &str, name: &str) -> Candidate {
        (
            id.into(),
            name.into(),
            String::new(),
            String::new(),
            "npx server".into(),
        )
    }
    #[tokio::test]
    async fn defaults_track_prompt_changes_preserve_admin_choices_and_do_not_refill_cleared_grants() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::run_one_platform_migrations(&dream_core_db::DbPool::Sqlite(pool.clone()))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE one_content_categories (id TEXT PRIMARY KEY,name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE one_mcp_registry (id TEXT,name TEXT,content TEXT,category_id TEXT,endpoint TEXT,enabled INTEGER,published INTEGER,visibility TEXT,scope TEXT,has_keys INTEGER,secrets_json TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO one_mcp_registry VALUES ('git','github',NULL,NULL,'npx github',1,1,'all','org',0,NULL),('calendar','calendar',NULL,NULL,'npx calendar',1,1,'all','org',0,NULL),('private','github private',NULL,NULL,'npx private',1,1,'admin','org',0,NULL)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO one_scenes (id,tenant_id,name,prompt,created_at,updated_at) VALUES ('scene','t1','代码','github',0,0)").execute(&pool).await.unwrap();
        let service = PlatformService::new(dream_core_db::DbPool::Sqlite(pool.clone()), [0x17; 32]);
        service.apply_scene_mcp_defaults("t1").await.unwrap();
        let ids = |pool: sqlx::SqlitePool| async move {
            sqlx::query_scalar::<_, String>(
                "SELECT resource_id FROM one_resource_grants WHERE subject_id='scene' ORDER BY resource_id",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        assert_eq!(ids(pool.clone()).await, vec!["git"]);
        sqlx::query("UPDATE one_scenes SET prompt='calendar' WHERE id='scene'")
            .execute(&pool)
            .await
            .unwrap();
        service.apply_scene_mcp_defaults("t1").await.unwrap();
        assert_eq!(ids(pool.clone()).await, vec!["calendar"]);
        service
            .grant_resource("t1", "scene", "scene", "mcp", "git", "use", "admin")
            .await
            .unwrap();
        sqlx::query("UPDATE one_scenes SET prompt='github changed' WHERE id='scene'")
            .execute(&pool)
            .await
            .unwrap();
        service.apply_scene_mcp_defaults("t1").await.unwrap();
        assert_eq!(ids(pool.clone()).await, vec!["git"]);
        sqlx::query("DELETE FROM one_resource_grants WHERE subject_id='scene'")
            .execute(&pool)
            .await
            .unwrap();
        service.apply_scene_mcp_defaults("t1").await.unwrap();
        assert!(ids(pool.clone()).await.is_empty());
        service.delete_scene("t1", "scene").await.unwrap();
        let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_scene_mcp_defaults WHERE scene_id='scene'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(markers, 0);
    }

    #[test]
    fn title_and_prompt_select_relevant_tools_without_filling_unrelated_slots() {
        let rows = vec![
            row("git", "github repository"),
            row("office", "calendar meeting"),
            row("other", "unrelated"),
        ];
        assert_eq!(
            recommend("代码审查", "Use GitHub to review repositories", &rows),
            vec!["git"]
        );
        assert_eq!(recommend("办公", "日程会议", &rows), vec!["office"]);
    }

    #[test]
    fn office_word_does_not_match_password_or_wordpress() {
        let rows = vec![
            row("vault", "password manager"),
            row("blog", "wordpress"),
            row("word", "word documents"),
        ];
        assert_eq!(recommend("办公", "", &rows), vec!["word"]);
    }
    #[test]
    fn defaults_are_small_deterministic_and_exclude_placeholders() {
        let mut rows: Vec<_> = (0..100)
            .map(|i| row(&format!("{i:03}"), &format!("github-{i:03}")))
            .collect();
        rows[0].4 = "https://your-server/mcp".into();
        let result = recommend("代码", "github", &rows);
        assert_eq!(result.len(), 10);
        assert_eq!(result[0], "001");
    }
}
