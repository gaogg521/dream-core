//! Persistent knowledge spaces and nested folders. Document ACLs remain authoritative.
use crate::models::RagLibraryDto;
use crate::service::new_id;
use crate::{DevopsError, DevopsService};
use dream_core_common::now_ms;
use dream_core_db::db_params;
use std::collections::HashSet;

const COLUMNS: &str = "id, name, description, created_by, created_at, updated_at, parent_id, kind, scope";

impl DevopsService {
    pub async fn list_rag_libraries(&self) -> Result<Vec<RagLibraryDto>, DevopsError> {
        Ok(self
            .db
            .fetch_all_as(&format!("SELECT {COLUMNS} FROM one_rag_libraries ORDER BY name"), &[])
            .await?)
    }

    pub async fn list_visible_rag_libraries(&self, viewer: &str) -> Result<Vec<RagLibraryDto>, DevopsError> {
        let libraries = self.list_rag_libraries().await?;
        let admin = self.is_registry_admin(viewer).await?;
        if admin {
            return Ok(libraries
                .into_iter()
                .filter(|item| item.scope != "personal" || item.created_by == viewer)
                .collect());
        }
        let documents = self.list_rag_documents(viewer).await?;
        let mut visible: HashSet<String> = documents
            .iter()
            .filter(|doc| doc.scope != "personal")
            .map(|doc| doc.library_id.clone())
            .collect();
        visible.extend(
            libraries
                .iter()
                .filter(|item| item.scope == "personal" && item.created_by == viewer)
                .map(|item| item.id.clone()),
        );
        // Include the path to an authorized document without disclosing other branches.
        loop {
            let parents: Vec<String> = libraries
                .iter()
                .filter(|item| visible.contains(&item.id))
                .filter_map(|item| item.parent_id.clone())
                .collect();
            let previous = visible.len();
            visible.extend(parents);
            if previous == visible.len() {
                break;
            }
        }
        Ok(libraries
            .into_iter()
            .filter(|item| visible.contains(&item.id) && (item.scope != "personal" || item.created_by == viewer))
            .collect())
    }

    async fn rag_library(&self, id: &str) -> Result<RagLibraryDto, DevopsError> {
        self.db
            .fetch_optional_as(
                &format!("SELECT {COLUMNS} FROM one_rag_libraries WHERE id = ?"),
                &db_params![id],
            )
            .await?
            .ok_or_else(|| DevopsError::NotFound(format!("library {id}")))
    }

    pub async fn assert_rag_library_writable(&self, actor: &str, id: &str) -> Result<(), DevopsError> {
        let item = self.rag_library(id).await?;
        if item.scope == "personal" {
            if item.created_by != actor {
                return Err(DevopsError::Forbidden("this is another member's private folder".into()));
            }
        } else if !self.is_registry_admin(actor).await? {
            return Err(DevopsError::Forbidden(
                "only administrators can manage company knowledge spaces".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn assert_rag_library_target(
        &self,
        actor: &str,
        id: &str,
        doc_scope: &str,
    ) -> Result<(), DevopsError> {
        let item = self.rag_library(id).await?;
        if item.scope == "personal" && (item.created_by != actor || doc_scope != "personal") {
            return Err(DevopsError::Forbidden(
                "documents cannot be placed in another member's private folder".into(),
            ));
        }
        Ok(())
    }

    pub async fn upsert_rag_library(
        &self,
        id: Option<&str>,
        name: &str,
        description: &str,
        actor: &str,
    ) -> Result<RagLibraryDto, DevopsError> {
        self.upsert_rag_container(id, name, description, None, "space", "org", actor)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_rag_container(
        &self,
        id: Option<&str>,
        name: &str,
        description: &str,
        parent_id: Option<&str>,
        kind: &str,
        scope: &str,
        actor: &str,
    ) -> Result<RagLibraryDto, DevopsError> {
        let name = name.trim();
        if name.is_empty() || name.len() > 255 {
            return Err(DevopsError::BadRequest(
                "library name must contain 1 to 255 bytes".into(),
            ));
        }
        if let Some(id) = id {
            self.assert_rag_library_writable(actor, id).await?;
            self.db
                .execute(
                    "UPDATE one_rag_libraries SET name = ?, description = ?, updated_at = ? WHERE id = ?",
                    &db_params![name, description, now_ms(), id],
                )
                .await?;
            return self.rag_library(id).await;
        }
        if !["space", "folder"].contains(&kind) || !["org", "personal"].contains(&scope) {
            return Err(DevopsError::BadRequest(
                "invalid knowledge container kind or scope".into(),
            ));
        }
        let mut inherited_scope = scope.to_owned();
        if let Some(parent) = parent_id {
            let item = self.rag_library(parent).await?;
            self.assert_rag_library_writable(actor, parent).await?;
            if kind != "folder" {
                return Err(DevopsError::BadRequest("only folders may be nested".into()));
            }
            inherited_scope = item.scope;
            let mut depth = 1;
            let mut ancestor = item.parent_id;
            while let Some(parent) = ancestor {
                depth += 1;
                if depth >= 16 {
                    return Err(DevopsError::BadRequest("folder nesting exceeds 16 levels".into()));
                }
                ancestor = self.rag_library(&parent).await?.parent_id;
            }
        }
        if inherited_scope == "org" && !self.is_registry_admin(actor).await? {
            return Err(DevopsError::Forbidden(
                "only administrators can create company knowledge spaces".into(),
            ));
        }
        if kind == "space" && inherited_scope == "personal" {
            return Err(DevopsError::BadRequest("personal containers must be folders".into()));
        }
        let id = new_id("oraglib");
        let now = now_ms();
        self.db.execute("INSERT INTO one_rag_libraries (id, name, description, created_by, created_at, updated_at, parent_id, kind, scope) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)", &db_params![&id, name, description, actor, now, now, parent_id, kind, &inherited_scope]).await?;
        self.rag_library(&id).await
    }

    /// Rename or move an authorized document without changing its sharing scope.
    pub async fn update_rag_document_location(
        &self,
        actor: &str,
        id: &str,
        title: &str,
        library_id: Option<&str>,
    ) -> Result<(), DevopsError> {
        self.assert_rag_writable(actor, id).await?;
        let title = title.trim();
        if title.is_empty() || title.len() > 1024 {
            return Err(DevopsError::BadRequest(
                "document title must contain 1 to 1024 bytes".into(),
            ));
        }
        if let Some(target) = library_id {
            let (mut scope, owner): (String, String) = self
                .db
                .fetch_one_as(
                    "SELECT scope, created_by FROM one_rag_documents WHERE id = ?",
                    &db_params![id],
                )
                .await?;
            if self.rag_library(target).await?.scope == "personal"
                && scope != "personal"
                && owner == actor
                && self.user_org_role(actor).await?.is_none()
            {
                scope = "personal".into();
            }
            self.assert_rag_library_target(actor, target, &scope).await?;
            self.db
                .execute(
                    "UPDATE one_rag_documents SET title = ?, library_id = ?, scope = ? WHERE id = ?",
                    &db_params![title, target, &scope, id],
                )
                .await?;
        } else {
            self.db
                .execute(
                    "UPDATE one_rag_documents SET title = ? WHERE id = ?",
                    &db_params![title, id],
                )
                .await?;
        }
        Ok(())
    }

    pub async fn delete_rag_library(&self, id: &str) -> Result<(), DevopsError> {
        if id == "oraglib_default" {
            return Err(DevopsError::BadRequest(
                "cannot delete the default knowledge library".into(),
            ));
        }
        let documents: i64 = self
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_rag_documents WHERE library_id = ?",
                &db_params![id],
            )
            .await?;
        let folders: i64 = self
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_rag_libraries WHERE parent_id = ?",
                &db_params![id],
            )
            .await?;
        if documents > 0 || folders > 0 {
            return Err(DevopsError::BadRequest(
                "move or delete documents and child folders in this library first".into(),
            ));
        }
        let deleted = self
            .db
            .execute("DELETE FROM one_rag_libraries WHERE id = ?", &db_params![id])
            .await?;
        if deleted == 0 {
            return Err(DevopsError::NotFound(format!("library {id}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn service() -> DevopsService {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let db = dream_core_db::DbPool::Sqlite(pool.clone());
        crate::run_one_devops_migrations(&db).await.unwrap();
        sqlx::raw_sql("CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(user_id, tenant_id)); CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL); INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin', 'company', 'org_admin'), ('member', 'company', 'member'), ('other', 'company', 'member');").execute(&pool).await.unwrap();
        DevopsService::new(db)
    }
    #[tokio::test]
    async fn nested_folders_keep_the_visible_document_path_without_disclosing_siblings() {
        let svc = service().await;
        let space = svc.upsert_rag_library(None, "Engineering", "", "admin").await.unwrap();
        let folder = svc
            .upsert_rag_container(None, "Runbooks", "", Some(&space.id), "folder", "org", "admin")
            .await
            .unwrap();
        let sibling = svc
            .upsert_rag_container(None, "Hidden", "", Some(&space.id), "folder", "org", "admin")
            .await
            .unwrap();
        let doc = svc
            .register_rag_document(
                "Deploy",
                None,
                None,
                None,
                "org",
                None,
                "all",
                "admin",
                Some(&folder.id),
            )
            .await
            .unwrap();
        assert_eq!(doc.library_id, folder.id);
        let visible = svc.list_visible_rag_libraries("member").await.unwrap();
        assert!(visible.iter().any(|item| item.id == space.id));
        assert!(visible.iter().any(|item| item.id == folder.id));
        assert!(!visible.iter().any(|item| item.id == sibling.id));
        assert!(
            matches!(svc.delete_rag_library(&space.id).await, Err(DevopsError::BadRequest(message)) if message.contains("child folders"))
        );
    }
    #[tokio::test]
    async fn private_folders_remain_private_to_their_author_including_from_admins() {
        let svc = service().await;
        let folder = svc
            .upsert_rag_container(None, "Notes", "", None, "folder", "personal", "member")
            .await
            .unwrap();
        assert!(
            svc.list_visible_rag_libraries("member")
                .await
                .unwrap()
                .iter()
                .any(|item| item.id == folder.id)
        );
        for viewer in ["other", "admin"] {
            assert!(
                !svc.list_visible_rag_libraries(viewer)
                    .await
                    .unwrap()
                    .iter()
                    .any(|item| item.id == folder.id)
            );
            assert!(
                matches!(svc.assert_rag_library_writable(viewer, &folder.id).await, Err(DevopsError::Forbidden(message)) if message.contains("private folder"))
            );
        }
        assert!(
            matches!(svc.register_rag_document("Intrusion", None, None, None, "personal", None, "all", "other", Some(&folder.id)).await, Err(DevopsError::Forbidden(message)) if message.contains("private folder"))
        );
        svc.register_rag_document(
            "Notes",
            None,
            None,
            None,
            "personal",
            None,
            "all",
            "member",
            Some(&folder.id),
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn moving_a_document_keeps_its_acl_and_rejects_unauthorized_targets() {
        let svc = service().await;
        let source = svc
            .upsert_rag_container(None, "Source", "", None, "folder", "personal", "member")
            .await
            .unwrap();
        let target = svc
            .upsert_rag_container(None, "Target", "", None, "folder", "personal", "member")
            .await
            .unwrap();
        let foreign = svc
            .upsert_rag_container(None, "Foreign", "", None, "folder", "personal", "other")
            .await
            .unwrap();
        let doc = svc
            .register_rag_document(
                "Notes",
                None,
                None,
                None,
                "personal",
                None,
                "all",
                "member",
                Some(&source.id),
            )
            .await
            .unwrap();
        svc.update_rag_document_location("member", &doc.id, "Renamed", Some(&target.id))
            .await
            .unwrap();
        let docs = svc.list_rag_documents("member").await.unwrap();
        let moved = docs.iter().find(|item| item.id == doc.id).unwrap();
        assert_eq!(moved.title, "Renamed");
        assert_eq!(moved.library_id, target.id);
        assert_eq!(moved.scope, "personal");
        assert!(
            matches!(svc.update_rag_document_location("other", &doc.id, "Stolen", Some(&foreign.id)).await, Err(DevopsError::Forbidden(message)) if message.contains("private document"))
        );
        assert!(
            matches!(svc.update_rag_document_location("member", &doc.id, "Moved", Some(&foreign.id)).await, Err(DevopsError::Forbidden(message)) if message.contains("private folder"))
        );
    }

    #[tokio::test]
    async fn members_cannot_create_company_folders_or_bypass_parent_permissions() {
        let svc = service().await;
        let space = svc.upsert_rag_library(None, "Company", "", "admin").await.unwrap();
        assert!(
            matches!(svc.upsert_rag_container(None, "Forbidden", "", Some(&space.id), "folder", "personal", "member").await, Err(DevopsError::Forbidden(message)) if message.contains("company knowledge"))
        );
        assert!(
            matches!(svc.upsert_rag_container(None, "Forbidden", "", None, "folder", "org", "member").await, Err(DevopsError::Forbidden(message)) if message.contains("company knowledge"))
        );
        assert!(
            matches!(svc.upsert_rag_container(None, "Missing", "", Some("unknown"), "folder", "org", "admin").await, Err(DevopsError::NotFound(message)) if message.contains("unknown"))
        );
    }
}
