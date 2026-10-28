//! Explicit per-member MCP activation. An allowed catalog is not an activation.
pub use dream_core_api_types::{McpCatalogItem, McpSelectionDto};
use dream_core_common::{mcp_policy::MAX_USER_MCP_SERVERS, now_ms};
use dream_core_db::db_params;

use crate::{DevopsService, error::DevopsError, models::McpRegistryDto};

impl DevopsService {
    pub async fn mcp_selection(&self, tenant: &str, user: &str) -> Result<McpSelectionDto, DevopsError> {
        let json: Option<String> = self
            .db
            .fetch_optional_scalar(
                "SELECT server_ids_json FROM one_mcp_user_selection WHERE tenant_id = ? AND user_id = ?",
                &db_params![tenant, user],
            )
            .await?;
        let selected_ids: Vec<String> = match json {
            Some(value) => serde_json::from_str(&value).map_err(|e| DevopsError::Internal(e.to_string()))?,
            None => Vec::new(),
        };
        if selected_ids.len() > MAX_USER_MCP_SERVERS {
            return Err(DevopsError::BadRequest(
                "MCP_LIMIT_EXCEEDED: replace the invalid selection".into(),
            ));
        }
        Ok(McpSelectionDto {
            selected_ids,
            max_count: MAX_USER_MCP_SERVERS,
        })
    }

    pub async fn replace_mcp_selection(
        &self,
        tenant: &str,
        user: &str,
        ids: &[String],
    ) -> Result<McpSelectionDto, DevopsError> {
        self.replace_mcp_selection_with_bindings(tenant, user, ids, &[]).await
    }

    pub async fn replace_mcp_selection_with_bindings(
        &self,
        tenant: &str,
        user: &str,
        ids: &[String],
        bound: &[String],
    ) -> Result<McpSelectionDto, DevopsError> {
        let _selection_guard = self.mcp_selection_mutation.lock().await;
        let mut ids = ids.to_vec();
        ids.sort();
        ids.dedup();
        if ids.len() > MAX_USER_MCP_SERVERS {
            return Err(DevopsError::BadRequest(
                "MCP_LIMIT_EXCEEDED: enterprise distribution maximum 50".into(),
            ));
        }
        let allowed = self.allowed_mcp_catalog(user).await?;
        let total: std::collections::HashSet<_> = ids
            .iter()
            .chain(bound.iter().filter(|id| allowed.iter().any(|row| &row.id == *id)))
            .collect();
        if total.len() > MAX_USER_MCP_SERVERS {
            return Err(DevopsError::BadRequest(
                "MCP_LIMIT_EXCEEDED: total enterprise distribution maximum 50, including employee dependencies".into(),
            ));
        }
        if ids.iter().any(|id| !allowed.iter().any(|row| &row.id == id)) {
            return Err(DevopsError::Forbidden(
                "MCP selection contains an unavailable or ungranted server".into(),
            ));
        }
        let previous_json: Option<String> = self
            .db
            .fetch_optional_scalar(
                "SELECT server_ids_json FROM one_mcp_user_selection WHERE tenant_id = ? AND user_id = ?",
                &db_params![tenant, user],
            )
            .await?;
        let previous: Vec<String> = previous_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|error| DevopsError::Internal(error.to_string()))?
            .unwrap_or_default();
        let additions = ids.iter().filter(|id| !previous.contains(id)).count();
        if additions > dream_core_common::mcp_policy::MAX_ENTERPRISE_MCP_ADDITIONS {
            return Err(DevopsError::BadRequest(
                "MCP_BATCH_LIMIT_EXCEEDED: add at most 10 enterprise connectors per save".into(),
            ));
        }
        let json = serde_json::to_string(&ids).map_err(|e| DevopsError::Internal(e.to_string()))?;
        self.upsert(
            "INSERT INTO one_mcp_user_selection (tenant_id,user_id,server_ids_json,updated_at) VALUES (?,?,?,?) ON CONFLICT(tenant_id,user_id) DO UPDATE SET server_ids_json=excluded.server_ids_json,updated_at=excluded.updated_at",
            "INSERT INTO one_mcp_user_selection (tenant_id,user_id,server_ids_json,updated_at) VALUES (?,?,?,?) AS new ON DUPLICATE KEY UPDATE server_ids_json=new.server_ids_json,updated_at=new.updated_at",
            &db_params![tenant,user,&json,now_ms()],
        ).await?;
        Ok(McpSelectionDto {
            selected_ids: ids,
            max_count: MAX_USER_MCP_SERVERS,
        })
    }

    pub(crate) async fn allowed_mcp_catalog(&self, user: &str) -> Result<Vec<McpRegistryDto>, DevopsError> {
        if self.grants.get().is_some() && self.user_org_role(user).await?.is_none() {
            return Err(DevopsError::Forbidden(
                "Execution requires active enterprise membership".into(),
            ));
        }
        let mut rows = self.list_mcp_registry_inner(user, true).await?;
        rows.retain(|row| row.enabled && row.published);
        Ok(rows)
    }

    /// Only explicitly selected connectors carry transport/credentials to clients.
    pub async fn selected_mcp_registry(&self, tenant: &str, user: &str) -> Result<Vec<McpRegistryDto>, DevopsError> {
        let mut rows = self.allowed_mcp_catalog(user).await?;
        let selection = self.mcp_selection(tenant, user).await?;
        rows.retain(|row| selection.selected_ids.contains(&row.id));
        Ok(rows)
    }

    /// Connectors a digital employee binds, resolved against the caller's
    /// authorized catalog (ACL, published, enabled) — NOT against the member's
    /// own selected connectors. Employee dependencies have their own resolution
    /// path; distribution still checks the combined enterprise limit. Ids the
    /// caller may not use are simply absent from the result.
    pub async fn employee_bound_mcp_registry(
        &self,
        user: &str,
        ids: &[String],
    ) -> Result<Vec<McpRegistryDto>, DevopsError> {
        if ids.len() > MAX_USER_MCP_SERVERS {
            return Err(DevopsError::BadRequest(format!(
                "MCP_LIMIT_EXCEEDED: a digital employee may bind at most {MAX_USER_MCP_SERVERS} connectors"
            )));
        }
        let mut rows = self.allowed_mcp_catalog(user).await?;
        rows.retain(|row| ids.contains(&row.id));
        Ok(rows)
    }

    /// Distribution additionally requires a binding from a visible employee.
    pub async fn visible_employee_bound_mcp_registry(
        &self,
        tenant: &str,
        user: &str,
        requested: &[String],
        bound: &[String],
    ) -> Result<Vec<McpRegistryDto>, DevopsError> {
        let _selection_guard = self.mcp_selection_mutation.lock().await;
        let mut rows = self.employee_bound_mcp_registry(user, requested).await?;
        rows.retain(|row| bound.contains(&row.id));
        let selection = self.mcp_selection(tenant, user).await?;
        let total: std::collections::HashSet<_> = selection
            .selected_ids
            .iter()
            .chain(rows.iter().map(|row| &row.id))
            .collect();
        if total.len() > MAX_USER_MCP_SERVERS {
            return Err(DevopsError::BadRequest(
                "MCP_LIMIT_EXCEEDED: total enterprise distribution maximum 50".into(),
            ));
        }
        Ok(rows)
    }

    /// Paged display-only catalog: unselected connector secrets never leave the server.
    pub async fn mcp_catalog_page(
        &self,
        user: &str,
        query: &str,
        offset: usize,
    ) -> Result<Vec<McpCatalogItem>, DevopsError> {
        let query = query.to_lowercase();
        Ok(self
            .allowed_mcp_catalog(user)
            .await?
            .into_iter()
            .filter(|row| row.name.to_lowercase().contains(&query))
            .skip(offset)
            .take(50)
            .map(|row| McpCatalogItem {
                id: row.id,
                name: row.name,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn selection_adds_ten_per_save_up_to_fifty_without_partial_writes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone()))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE one_user_org (user_id TEXT, tenant_id TEXT, role TEXT, department_id TEXT, created_at INTEGER)").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE one_active_tenant (user_id TEXT, tenant_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO one_user_org VALUES ('alice','default','member',NULL,0),('bob','default','member',NULL,0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let service = DevopsService::new(dream_core_db::DbPool::Sqlite(pool));
        assert!(
            service
                .selected_mcp_registry("default", "alice")
                .await
                .unwrap()
                .is_empty()
        );
        let mut ids = Vec::new();
        for i in 0..51 {
            let row = service
                .upsert_mcp_registry(
                    None,
                    &format!("connector-{i}"),
                    "stdio",
                    "node server.js",
                    true,
                    false,
                    None,
                    "org",
                    None,
                    "all",
                    None,
                    "alice",
                )
                .await
                .unwrap();
            ids.push(row.id);
        }
        assert!(
            matches!(service.replace_mcp_selection("default", "alice", &ids[..11]).await, Err(DevopsError::BadRequest(message)) if message.contains("MCP_BATCH_LIMIT_EXCEEDED"))
        );
        assert!(
            service
                .mcp_selection("default", "alice")
                .await
                .unwrap()
                .selected_ids
                .is_empty()
        );
        for count in [10, 20, 30, 40, 50] {
            service
                .replace_mcp_selection("default", "alice", &ids[..count])
                .await
                .unwrap();
        }
        assert!(
            matches!(service.replace_mcp_selection_with_bindings("default", "alice", &ids[..50], &ids[50..]).await, Err(DevopsError::BadRequest(message)) if message.contains("total enterprise distribution"))
        );
        assert!(matches!(
            service.replace_mcp_selection("default", "alice", &ids).await,
            Err(DevopsError::BadRequest(_))
        ));
        assert_eq!(
            service
                .mcp_selection("default", "alice")
                .await
                .unwrap()
                .selected_ids
                .len(),
            50
        );
        service
            .replace_mcp_selection("default", "alice", &ids[1..])
            .await
            .unwrap();
        let selected = service.selected_mcp_registry("default", "alice").await.unwrap();
        assert_eq!(selected.len(), 50);
        assert!(!selected.iter().any(|row| row.id == ids[0]));
        assert!(
            service
                .mcp_selection("default", "bob")
                .await
                .unwrap()
                .selected_ids
                .is_empty()
        );
        assert!(matches!(
            service
                .replace_mcp_selection("default", "alice", &["not-granted".into()])
                .await,
            Err(DevopsError::Forbidden(_))
        ));
    }
}
