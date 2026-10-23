//! Insert-only reference skills. Customer edits, publication and enabled state
//! survive upgrades; the stable id makes concurrent app startup idempotent.
use dream_core_db::{DbBackend, db_params};

use crate::{DevopsError, DevopsService};

impl DevopsService {
    pub async fn ensure_catalog_guide(
        &self,
        id: &str,
        name: &str,
        description: &str,
        content: &str,
    ) -> Result<u64, DevopsError> {
        if !id.starts_with("builtin-employee-") || !id.ends_with("-guide-v1") || content.trim().is_empty() {
            return Err(DevopsError::BadRequest("invalid builtin employee guide".into()));
        }
        let now = dream_core_common::now_ms();
        let sql = match self.db.backend() {
            DbBackend::Sqlite => {
                "INSERT INTO one_skill_registry \
                (id, name, description, content, enabled, auto_active, scope, visibility, origin, published, created_by, created_at, updated_at) \
                VALUES (?, ?, ?, ?, 1, 0, 'org', 'shared', 'catalog', 1, 'system_default_user', ?, ?) ON CONFLICT(id) DO NOTHING"
            }
            DbBackend::MySql => {
                "INSERT INTO one_skill_registry \
                (id, name, description, content, enabled, auto_active, scope, visibility, origin, published, created_by, created_at, updated_at) \
                VALUES (?, ?, ?, ?, 1, 0, 'org', 'shared', 'catalog', 1, 'system_default_user', ?, ?) ON DUPLICATE KEY UPDATE id = id"
            }
        };
        self.db
            .execute(sql, &db_params![id, name, description, content, now, now])
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guide_seed_is_published_bound_only_and_preserves_customer_edits() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let pool = dream_core_db::DbPool::Sqlite(db.pool().clone());
        crate::run_one_devops_migrations(&pool).await.unwrap();
        let service = DevopsService::new(pool.clone());
        let id = "builtin-employee-test-guide-v1";
        assert_eq!(
            service
                .ensure_catalog_guide(id, "test-guide", "Reference", "# Workflow")
                .await
                .unwrap(),
            1
        );
        let flags: (i64, i64, i64) =
            sqlx::query_as("SELECT published, enabled, auto_active FROM one_skill_registry WHERE id = ?")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            flags,
            (1, 1, 0),
            "available when bound, never injected into every conversation"
        );
        pool.execute(
            "UPDATE one_skill_registry SET content = 'Customer content', enabled = 0, published = 0 WHERE id = ?",
            &db_params![id],
        )
        .await
        .unwrap();
        service
            .ensure_catalog_guide(id, "test-guide", "Reference", "# New workflow")
            .await
            .unwrap();
        let row: (String, i64, i64) =
            sqlx::query_as("SELECT content, enabled, published FROM one_skill_registry WHERE id = ?")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(row, ("Customer content".into(), 0, 0));
        assert!(
            matches!(service.ensure_catalog_guide("customer-id", "test", "", "body").await, Err(DevopsError::BadRequest(message)) if message == "invalid builtin employee guide")
        );
    }
}
