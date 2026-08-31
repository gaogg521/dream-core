//! Enterprise memory service (P2-2). All logic lives here; routes only
//! transform request/response. Access follows the three-tier OpenOcta model:
//! `global` is tenant-wide readable, `department` is readable inside the
//! department (via `one_user_org.department_id`), `personal` is owner-only —
//! and grants can open any of them up, write access always requiring an
//! explicit `write` grant or an admin.

use std::collections::HashSet;

use dream_core_db::{DbPool, db_params};

use dream_core_common::{generate_prefixed_id, now_ms};
use sha2::{Digest, Sha256};

use crate::error::MemoryError;
use crate::models::{
    GrantCoverageDto, MemoryCollectionDto, MemoryConfigDto, MemoryGrantDto, MemoryItemDto,
    MemoryRefineJobDto,
};

/// The three OpenOcta-aligned memory tiers (§产品口径): global company
/// knowledge, fused department memory, and personal distillation. The
/// service treats them as a validated vocabulary whose invariants (which
/// binding column each tier requires) live in [`MemoryService::create_collection`].
pub const MEMORY_SCOPES: [&str; 3] = ["global", "department", "personal"];

/// Refinement trims only low-value items — anything below this importance.
pub const MEMORY_REFINE_MIN_IMPORTANCE: f64 = 0.3;

/// Refinement keeps a collection at this many active items at most; beyond
/// it, the oldest low-value items are trimmed first.
pub const MEMORY_REFINE_ACTIVE_FLOOR: i64 = 20;

/// The caller's resolved enterprise membership (active tenant + role).
#[derive(Debug, Clone)]
pub struct MemoryActor {
    pub tenant_id: String,
    pub role: String,
}

pub struct MemoryService {
    db: DbPool,
}

fn is_admin_role(role: &str) -> bool {
    matches!(role, "org_admin" | "system_admin" | "admin")
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

type CollectionRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    i64,
    i64,
);

fn collection_row_to_dto(row: CollectionRow) -> MemoryCollectionDto {
    let (id, tenant_id, scope, department_id, owner_user_id, name, description, created_at, updated_at) = row;
    MemoryCollectionDto {
        id,
        tenant_id,
        scope,
        department_id,
        owner_user_id,
        name,
        description,
        created_at,
        updated_at,
    }
}

type ItemRow = (
    String,
    String,
    String,
    String,
    f64,
    Option<String>,
    String,
    String,
    i64,
    i64,
);

fn item_row_to_dto(row: ItemRow) -> MemoryItemDto {
    let (
        id,
        collection_id,
        content,
        content_hash,
        importance,
        source_conversation_id,
        tags,
        status,
        created_at,
        updated_at,
    ) = row;
    MemoryItemDto {
        id,
        collection_id,
        content,
        content_hash,
        importance,
        source_conversation_id,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        status,
        created_at,
        updated_at,
    }
}

const ITEM_COLUMNS: &str = "id, collection_id, content, content_hash, importance, source_conversation_id, tags, status, created_at, updated_at";

/// A raw grant, kept as data so readability can be evaluated in bulk
/// (search and coverage both need the whole tenant's grant set at once).
#[derive(Debug, Clone)]
struct GrantRow {
    collection_id: String,
    subject_type: String,
    subject_id: String,
    access: String,
}

impl GrantRow {
    /// Whether this grant covers `user_id` (a `member` subject matches the
    /// user themselves, a `department` subject matches their department).
    fn covers(&self, user_id: &str, department_id: Option<&str>) -> bool {
        match self.subject_type.as_str() {
            "member" => self.subject_id == user_id,
            "department" => department_id == Some(self.subject_id.as_str()),
            _ => false,
        }
    }
}

/// One loaded collection plus the predicates evaluated against it.
struct Collection {
    id: String,
    scope: String,
    department_id: Option<String>,
    owner_user_id: Option<String>,
}

impl Collection {
    /// Readable: admins see everything; `personal` is owner-only;
    /// `department` is for the member's own department or explicit grant
    /// holders; `global` is readable by any tenant member.
    fn readable_by(&self, user_id: &str, admin: bool, department_id: Option<&str>, grants: &[GrantRow]) -> bool {
        if admin {
            return true;
        }
        match self.scope.as_str() {
            "personal" => self.owner_user_id.as_deref() == Some(user_id),
            "department" => {
                self.department_id.as_deref() == department_id
                    || grants
                        .iter()
                        .any(|g| g.collection_id == self.id && g.covers(user_id, department_id))
            }
            // Global is the tenant's shared knowledge: membership itself is
            // the read grant, so no lookup is needed beyond being a member.
            "global" => true,
            _ => false,
        }
    }

    /// Writable: the personal owner always; department/global only an admin
    /// or an explicit `write` grant covering the caller.
    fn writable_by(&self, user_id: &str, admin: bool, department_id: Option<&str>, grants: &[GrantRow]) -> bool {
        match self.scope.as_str() {
            "personal" => self.owner_user_id.as_deref() == Some(user_id),
            _ => {
                admin
                    || grants
                        .iter()
                        .any(|g| g.collection_id == self.id && g.access == "write" && g.covers(user_id, department_id))
            }
        }
    }
}

fn collection_row_to_model(row: &CollectionRow) -> Collection {
    Collection {
        id: row.0.clone(),
        scope: row.2.clone(),
        department_id: row.3.clone(),
        owner_user_id: row.4.clone(),
    }
}

impl MemoryService {
    /// Runs `sqlite_sql` or `mysql_sql` by backend — the dialects diverge on
    /// upsert syntax only; params are shared.
    async fn upsert(
        &self,
        sqlite_sql: &str,
        mysql_sql: &str,
        params: &[dream_core_db::DbValue],
    ) -> Result<u64, MemoryError> {
        let sql = match self.db.backend() {
            dream_core_db::DbBackend::Sqlite => sqlite_sql,
            dream_core_db::DbBackend::MySql => mysql_sql,
        };
        Ok(self.db.execute(sql, params).await?)
    }

    /// Per-tenant extraction settings, or `None` when the tenant never saved
    /// a config — the extractor's "disabled" signal (§A.6: no configured
    /// channel = no LLM extraction, explicit 「记住…」 requests still work).
    pub async fn memory_config(&self, tenant_id: &str) -> Result<Option<MemoryConfigDto>, MemoryError> {
        Ok(self
            .db
            .fetch_optional_as::<MemoryConfigDto>(
                "SELECT tenant_id, extraction_channel_id, extraction_model, updated_at                  FROM one_memory_config WHERE tenant_id = ?",
                &db_params![tenant_id],
            )
            .await?)
    }

    /// Save per-tenant extraction settings (admin action). Both fields are
    /// replaced wholesale; `extraction_channel_id = None` disables LLM
    /// extraction for the tenant.
    pub async fn set_memory_config(
        &self,
        tenant_id: &str,
        extraction_channel_id: Option<&str>,
        extraction_model: Option<&str>,
    ) -> Result<MemoryConfigDto, MemoryError> {
        let now = now_ms() as i64;
        self.upsert(
            "INSERT INTO one_memory_config (tenant_id, extraction_channel_id, extraction_model, updated_at)              VALUES (?, ?, ?, ?)              ON CONFLICT(tenant_id) DO UPDATE SET extraction_channel_id = excluded.extraction_channel_id,                  extraction_model = excluded.extraction_model, updated_at = excluded.updated_at",
            "INSERT INTO one_memory_config (tenant_id, extraction_channel_id, extraction_model, updated_at)              VALUES (?, ?, ?, ?)              ON DUPLICATE KEY UPDATE extraction_channel_id = new.extraction_channel_id,                  extraction_model = new.extraction_model, updated_at = new.updated_at",
            &db_params![tenant_id, extraction_channel_id, extraction_model, now],
        )
        .await?;
        Ok(self
            .memory_config(tenant_id)
            .await?
            .ok_or_else(|| MemoryError::Internal("memory config vanished immediately after write".into()))?)
    }

    pub fn new(db: DbPool) -> Self {
        Self { db }
    }

    /// Resolve the caller's active-tenant membership (same cross-crate query
    /// as one-platform's `resolve_actor`).
    pub async fn resolve_actor(&self, user_id: &str) -> Result<Option<MemoryActor>, MemoryError> {
        let result = self
            .db
            .fetch_optional_as::<(String, String)>(
                "SELECT uo.tenant_id, uo.role FROM one_user_org uo WHERE uo.user_id = ? \
                 ORDER BY (uo.tenant_id = (SELECT tenant_id FROM one_active_tenant WHERE user_id = uo.user_id)) DESC, \
                          uo.created_at DESC, uo.tenant_id ASC LIMIT 1",
                &db_params![user_id],
            )
            .await;
        match result {
            Ok(Some((tenant_id, role))) => Ok(Some(MemoryActor { tenant_id, role })),
            Ok(None) => Ok(None),
            Err(sqlx::Error::Database(e)) if e.message().contains("no such table") => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn require_admin(&self, user_id: &str) -> Result<MemoryActor, MemoryError> {
        match self.resolve_actor(user_id).await? {
            None => Err(MemoryError::NotInEnterprise),
            Some(actor) if !is_admin_role(&actor.role) => {
                Err(MemoryError::Forbidden("Administrator role required".into()))
            }
            Some(actor) => Ok(actor),
        }
    }

    pub async fn require_member(&self, user_id: &str) -> Result<MemoryActor, MemoryError> {
        match self.resolve_actor(user_id).await? {
            None => Err(MemoryError::NotInEnterprise),
            Some(actor) => Ok(actor),
        }
    }

    /// The caller's department within the tenant, if any — the department
    /// tier's readability hinge.
    async fn member_department(&self, tenant_id: &str, user_id: &str) -> Result<Option<String>, MemoryError> {
        let row: Option<(Option<String>,)> =
            self.db.fetch_optional_as::<(Option<String>,)>("SELECT department_id FROM one_user_org WHERE tenant_id = ? AND user_id = ?", &db_params![tenant_id, user_id])
                .await?;
        Ok(row.and_then(|(department_id,)| department_id))
    }

    async fn load_collections(&self, tenant_id: &str) -> Result<Vec<CollectionRow>, MemoryError> {
        let rows: Vec<CollectionRow> = self.db.fetch_all_as::<CollectionRow>(
            "SELECT id, tenant_id, scope, department_id, owner_user_id, name, description, created_at, updated_at \
             FROM one_memory_collections WHERE tenant_id = ? ORDER BY created_at ASC, id ASC",
        &db_params![tenant_id])
        .await?;
        Ok(rows)
    }

    async fn load_collection(&self, tenant_id: &str, id: &str) -> Result<Option<CollectionRow>, MemoryError> {
        let row: Option<CollectionRow> = self.db.fetch_optional_as::<CollectionRow>(
            "SELECT id, tenant_id, scope, department_id, owner_user_id, name, description, created_at, updated_at \
             FROM one_memory_collections WHERE tenant_id = ? AND id = ?",
        &db_params![tenant_id, id])
        .await?;
        Ok(row)
    }

    /// All grants in the tenant, evaluated in bulk by search and coverage.
    async fn load_grants(&self, tenant_id: &str) -> Result<Vec<GrantRow>, MemoryError> {
        let rows: Vec<(String, String, String, String)> = self.db.fetch_all_as::<(String, String, String, String)>(
            "SELECT collection_id, subject_type, subject_id, access FROM one_memory_grants WHERE tenant_id = ?",
        &db_params![tenant_id])
        .await?;
        Ok(rows
            .into_iter()
            .map(|(collection_id, subject_type, subject_id, access)| GrantRow {
                collection_id,
                subject_type,
                subject_id,
                access,
            })
            .collect())
    }

    /// Grants on one collection — the per-collection read/write check.
    async fn load_collection_grants(&self, tenant_id: &str, collection_id: &str) -> Result<Vec<GrantRow>, MemoryError> {
        let rows: Vec<(String, String, String, String)> = self.db.fetch_all_as::<(String, String, String, String)>(
            "SELECT collection_id, subject_type, subject_id, access FROM one_memory_grants \
             WHERE tenant_id = ? AND collection_id = ?",
        &db_params![tenant_id, collection_id])
        .await?;
        Ok(rows
            .into_iter()
            .map(|(collection_id, subject_type, subject_id, access)| GrantRow {
                collection_id,
                subject_type,
                subject_id,
                access,
            })
            .collect())
    }

    /// Load a collection and verify the caller may read it, distinguishing
    /// missing (404) from forbidden (403) so routes need no re-check.
    async fn authorize_read(
        &self,
        tenant_id: &str,
        user_id: &str,
        role: &str,
        collection_id: &str,
    ) -> Result<CollectionRow, MemoryError> {
        let row = self
            .load_collection(tenant_id, collection_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let admin = is_admin_role(role);
        let department_id = self.member_department(tenant_id, user_id).await?;
        let grants = self.load_collection_grants(tenant_id, collection_id).await?;
        if collection_row_to_model(&row).readable_by(user_id, admin, department_id.as_deref(), &grants) {
            Ok(row)
        } else {
            Err(MemoryError::Forbidden("you cannot read this memory collection".into()))
        }
    }

    /// The collection ids the caller can read, in bulk — the search filter.
    async fn readable_collection_ids(
        &self,
        tenant_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<Vec<String>, MemoryError> {
        let admin = is_admin_role(role);
        let department_id = self.member_department(tenant_id, user_id).await?;
        let grants = self.load_grants(tenant_id).await?;
        let rows = self.load_collections(tenant_id).await?;
        Ok(rows
            .into_iter()
            .filter(|row| collection_row_to_model(row).readable_by(user_id, admin, department_id.as_deref(), &grants))
            .map(|row| row.0)
            .collect())
    }

    /// Create a collection, enforcing the tier invariants: `global` carries
    /// neither department nor owner, `department` must carry a department,
    /// and `personal` is always bound to the caller — nobody can mint a
    /// personal collection for somebody else. Non-admins may only create
    /// their own personal collections; global/department memory is curated.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_collection(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        scope: &str,
        department_id: Option<&str>,
        owner_user_id: Option<&str>,
        name: &str,
        description: &str,
    ) -> Result<MemoryCollectionDto, MemoryError> {
        if !MEMORY_SCOPES.contains(&scope) {
            return Err(MemoryError::BadRequest(format!(
                "unknown memory scope '{scope}' (expected one of {MEMORY_SCOPES:?})"
            )));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(MemoryError::BadRequest("collection name must not be empty".into()));
        }
        let (department_id, owner_user_id) = match scope {
            "global" => {
                if !is_admin_role(caller_role) {
                    return Err(MemoryError::Forbidden("Administrator role required".into()));
                }
                if department_id.is_some() {
                    return Err(MemoryError::BadRequest(
                        "global collections must not be bound to a department".into(),
                    ));
                }
                if owner_user_id.is_some() {
                    return Err(MemoryError::BadRequest(
                        "global collections must not be bound to an owner".into(),
                    ));
                }
                (None, None)
            }
            "department" => {
                if !is_admin_role(caller_role) {
                    return Err(MemoryError::Forbidden("Administrator role required".into()));
                }
                let department_id = department_id.map(str::trim).filter(|d| !d.is_empty());
                let Some(department_id) = department_id else {
                    return Err(MemoryError::BadRequest(
                        "department collections require a department id".into(),
                    ));
                };
                (Some(department_id.to_owned()), None)
            }
            // Personal: the caller may omit their own id, but never name
            // somebody else as the owner.
            _ => {
                let owner = owner_user_id.unwrap_or(caller_id);
                if owner != caller_id {
                    return Err(MemoryError::BadRequest(
                        "personal collections must be owned by the caller".into(),
                    ));
                }
                (None, Some(owner.to_owned()))
            }
        };
        let id = generate_prefixed_id("memc");
        let now = now_ms();
        self.db.execute(
            "INSERT INTO one_memory_collections \
                 (id, tenant_id, scope, department_id, owner_user_id, name, description, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &db_params![&id, tenant_id, scope, &department_id, &owner_user_id, name, description.trim(), now, now])
        .await?;
        self.load_collection(tenant_id, &id)
            .await?
            .map(collection_row_to_dto)
            .ok_or_else(|| MemoryError::Internal("memory collection vanished immediately after insert".into()))
    }

    /// The collection inventory. Admins see the tenant's everything;
    /// members see the three tiers they can reach — global, their own
    /// department's, their own personal.
    pub async fn list_collections(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
    ) -> Result<Vec<MemoryCollectionDto>, MemoryError> {
        let admin = is_admin_role(caller_role);
        let department_id = self.member_department(tenant_id, caller_id).await?;
        let grants = self.load_grants(tenant_id).await?;
        let rows = self.load_collections(tenant_id).await?;
        Ok(rows
            .into_iter()
            .filter(|row| collection_row_to_model(row).readable_by(caller_id, admin, department_id.as_deref(), &grants))
            .map(collection_row_to_dto)
            .collect())
    }

    /// One collection, readable-only: `None` means the caller cannot see it.
    pub async fn get_collection(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        id: &str,
    ) -> Result<Option<MemoryCollectionDto>, MemoryError> {
        match self.authorize_read(tenant_id, caller_id, caller_role, id).await {
            Ok(row) => Ok(Some(collection_row_to_dto(row))),
            Err(MemoryError::Forbidden(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Rename / re-describe a collection. Personal only by the owner;
    /// department/global only by an admin — shared memory is curated.
    pub async fn update_collection(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<MemoryCollectionDto, MemoryError> {
        let row = self
            .load_collection(tenant_id, id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let model = collection_row_to_model(&row);
        let allowed = match model.scope.as_str() {
            "personal" => model.owner_user_id.as_deref() == Some(caller_id),
            _ => is_admin_role(caller_role),
        };
        if !allowed {
            return Err(MemoryError::Forbidden("you cannot edit this memory collection".into()));
        }
        if let Some(name) = name.map(str::trim) {
            if name.is_empty() {
                return Err(MemoryError::BadRequest("collection name must not be empty".into()));
            }
        }
        self.db.execute("UPDATE one_memory_collections SET name = COALESCE(?, name), description = COALESCE(?, description), updated_at = ? WHERE tenant_id = ? AND id = ?", &db_params![name.map(str::trim), description.map(str::trim), now_ms(), tenant_id, id])
            .await?;
        self.load_collection(tenant_id, id)
            .await?
            .map(collection_row_to_dto)
            .ok_or_else(|| MemoryError::Internal("memory collection vanished immediately after update".into()))
    }

    /// Delete a collection and its items/grants. Same ownership rule as
    /// update; refine jobs are kept as audit history.
    pub async fn delete_collection(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        id: &str,
    ) -> Result<(), MemoryError> {
        let row = self
            .load_collection(tenant_id, id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let model = collection_row_to_model(&row);
        let allowed = match model.scope.as_str() {
            "personal" => model.owner_user_id.as_deref() == Some(caller_id),
            _ => is_admin_role(caller_role),
        };
        if !allowed {
            return Err(MemoryError::Forbidden(
                "you cannot delete this memory collection".into(),
            ));
        }
        let mut tx = self.db.begin().await?;
        tx.execute(
            "DELETE FROM one_memory_items WHERE tenant_id = ? AND collection_id = ?",
            &db_params![tenant_id, id],
        )
        .await?;
        tx.execute(
            "DELETE FROM one_memory_grants WHERE tenant_id = ? AND collection_id = ?",
            &db_params![tenant_id, id],
        )
        .await?;
        tx.execute(
            "DELETE FROM one_memory_collections WHERE tenant_id = ? AND id = ?",
            &db_params![tenant_id, id],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Add one memory item. Writes are the guarded direction: personal is
    /// owner-only; department/global need an admin or a `write` grant —
    /// a `read` grant never confers it.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_item(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        collection_id: &str,
        content: &str,
        importance: f64,
        source_conversation_id: Option<&str>,
        tags: &[String],
    ) -> Result<MemoryItemDto, MemoryError> {
        let content = content.trim();
        if content.is_empty() {
            return Err(MemoryError::BadRequest("memory content must not be empty".into()));
        }
        let row = self
            .load_collection(tenant_id, collection_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let admin = is_admin_role(caller_role);
        let department_id = self.member_department(tenant_id, caller_id).await?;
        let grants = self.load_collection_grants(tenant_id, collection_id).await?;
        if !collection_row_to_model(&row).writable_by(caller_id, admin, department_id.as_deref(), &grants) {
            return Err(MemoryError::Forbidden(
                "you cannot write to this memory collection".into(),
            ));
        }
        let id = generate_prefixed_id("memi");
        let now = now_ms();
        self.db.execute(
            "INSERT INTO one_memory_items \
                 (id, tenant_id, collection_id, content, content_hash, importance, source_conversation_id, tags, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
        &db_params![&id, tenant_id, collection_id, content, sha256_hex(content), importance, source_conversation_id, serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()), now, now])
        .await?;
        self.get_item(tenant_id, collection_id, &id)
            .await?
            .ok_or_else(|| MemoryError::Internal("memory item vanished immediately after insert".into()))
    }

    async fn get_item(
        &self,
        tenant_id: &str,
        collection_id: &str,
        id: &str,
    ) -> Result<Option<MemoryItemDto>, MemoryError> {
        let sql =
            format!("SELECT {ITEM_COLUMNS} FROM one_memory_items WHERE tenant_id = ? AND collection_id = ? AND id = ?");
        let row: Option<ItemRow> = self.db.fetch_optional_as::<ItemRow>(&sql, &db_params![tenant_id, collection_id, id])
            .await?;
        Ok(row.map(item_row_to_dto))
    }

    /// A collection's items, newest first — trimmed ones included, with
    /// their status, because refinement is soft and auditable.
    pub async fn list_items(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        collection_id: &str,
        limit: i64,
    ) -> Result<Vec<MemoryItemDto>, MemoryError> {
        self.authorize_read(tenant_id, caller_id, caller_role, collection_id)
            .await?;
        let sql = format!(
            "SELECT {ITEM_COLUMNS} FROM one_memory_items \
             WHERE tenant_id = ? AND collection_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"
        );
        let rows: Vec<ItemRow> = self.db.fetch_all_as::<ItemRow>(&sql, &db_params![tenant_id, collection_id, limit])
            .await?;
        Ok(rows.into_iter().map(item_row_to_dto).collect())
    }

    /// Content search across every collection the caller can read, active
    /// items only — trimmed memories are history, not answers.
    pub async fn search_items(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        query: &str,
        collection_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<MemoryItemDto>, MemoryError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let readable = self.readable_collection_ids(tenant_id, caller_id, caller_role).await?;
        let collection_ids: Vec<String> = match collection_id {
            // An explicitly named collection must be readable — hide nothing
            // by accident, but never leak by omission either.
            Some(id) if readable.contains(&id.to_owned()) => vec![id.to_owned()],
            Some(id) => {
                if self.load_collection(tenant_id, id).await?.is_some() {
                    return Err(MemoryError::Forbidden("you cannot read this memory collection".into()));
                }
                return Err(MemoryError::NotFound("memory collection not found".into()));
            }
            None => readable,
        };
        if collection_ids.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = vec!["?"; collection_ids.len()].join(", ");
        let sql = format!(
            "SELECT {ITEM_COLUMNS} FROM one_memory_items \
             WHERE tenant_id = ? AND status = 'active' AND content LIKE '%' || ? || '%' \
             AND collection_id IN ({placeholders}) \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        );
        let mut params = db_params![tenant_id, query];
        params.extend(collection_ids.iter().map(|id| id.as_str().into()));
        params.push(limit.into());
        let rows = self
            .db
            .fetch_all_as::<ItemRow>(&sql, &params)
            .await?;
        Ok(rows.into_iter().map(item_row_to_dto).collect())
    }

    /// Relevance search for turn-start context injection (P2-2). Unlike
    /// [`Self::search_items`], which takes one substring, this tokenises
    /// `text` on whitespace and ORs the words, then ranks by how many
    /// distinct words a row matched — feeding a whole user message to a bare
    /// `LIKE '%…%'` would essentially never match. Active items only, capped
    /// at `limit`, across every collection the caller can read.
    pub async fn search_relevant(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        text: &str,
        limit: i64,
    ) -> Result<Vec<MemoryItemDto>, MemoryError> {
        // Words worth matching on: 2+ chars, deduped, capped so a long
        // paste can't build a monster query. CJK has no spaces, so also
        // keep the trimmed whole string as one term.
        let mut terms: Vec<String> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| w.chars().count() >= 2)
            .collect();
        let whole = text.trim().to_lowercase();
        if whole.chars().count() >= 2 && !terms.contains(&whole) {
            terms.push(whole);
        }
        terms.sort();
        terms.dedup();
        terms.truncate(16);
        if terms.is_empty() {
            return Ok(vec![]);
        }

        let collection_ids = self.readable_collection_ids(tenant_id, caller_id, caller_role).await?;
        if collection_ids.is_empty() {
            return Ok(vec![]);
        }
        let coll_ph = vec!["?"; collection_ids.len()].join(", ");
        let score = terms
            .iter()
            .map(|_| "(CASE WHEN LOWER(content) LIKE '%' || ? || '%' THEN 1 ELSE 0 END)")
            .collect::<Vec<_>>()
            .join(" + ");
        let where_any = terms
            .iter()
            .map(|_| "LOWER(content) LIKE '%' || ? || '%'")
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT {ITEM_COLUMNS} FROM one_memory_items \
             WHERE tenant_id = ? AND status = 'active' AND collection_id IN ({coll_ph}) AND ({where_any}) \
             ORDER BY ({score}) DESC, importance DESC, created_at DESC LIMIT ?"
        );
        let mut params = db_params![tenant_id];
        params.extend(collection_ids.iter().map(|id| id.as_str().into()));
        params.extend(terms.iter().map(|t| t.as_str().into())); // WHERE (…OR…)
        params.extend(terms.iter().map(|t| t.as_str().into())); // ORDER BY score
        params.push(limit.into());
        let rows = self.db.fetch_all_as::<ItemRow>(&sql, &params).await?;
        Ok(rows.into_iter().map(item_row_to_dto).collect())
    }

    /// The caller's own `personal` collection id, creating it on first use.
    /// The auto-write target for the P2-2 extraction pipeline — a member
    /// never has to create a collection by hand before memory starts
    /// accumulating.
    pub async fn ensure_personal_collection(
        &self,
        tenant_id: &str,
        caller_id: &str,
        caller_role: &str,
        default_name: &str,
    ) -> Result<String, MemoryError> {
        let existing: Option<String> = self.db.fetch_optional_scalar(
            "SELECT id FROM one_memory_collections \
             WHERE tenant_id = ? AND scope = 'personal' AND owner_user_id = ? \
             ORDER BY created_at ASC LIMIT 1",
        &db_params![tenant_id, caller_id])
        .await?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let dto = self
            .create_collection(
                tenant_id,
                caller_id,
                caller_role,
                "personal",
                None,
                Some(caller_id),
                default_name,
                "",
            )
            .await?;
        Ok(dto.id)
    }

    /// Merge duplicates and trim low-value items in one collection, then
    /// record the run. Synchronous by design in v1: the counts are the
    /// product of the run, not a promise of one.
    pub async fn run_refine_job(
        &self,
        tenant_id: &str,
        collection_id: &str,
    ) -> Result<MemoryRefineJobDto, MemoryError> {
        self.load_collection(tenant_id, collection_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let id = generate_prefixed_id("memr");
        let created_at = now_ms();
        match self.refine_collection(collection_id).await {
            Ok((merged_count, trimmed_count)) => {
                let finished_at = now_ms();
                self.db.execute(
                    "INSERT INTO one_memory_refine_jobs \
                         (id, tenant_id, collection_id, status, merged_count, trimmed_count, error, created_at, finished_at) \
                     VALUES (?, ?, ?, 'done', ?, ?, NULL, ?, ?)",
                &db_params![&id, tenant_id, collection_id, merged_count, trimmed_count, created_at, finished_at])
                .await?;
                Ok(MemoryRefineJobDto {
                    id,
                    collection_id: collection_id.to_owned(),
                    status: "done".into(),
                    merged_count,
                    trimmed_count,
                    error: None,
                    created_at,
                    finished_at: Some(finished_at),
                })
            }
            Err(e) => {
                // Best-effort audit: the run happened even though it failed.
                let _ = self.db.execute(
                    "INSERT INTO one_memory_refine_jobs \
                         (id, tenant_id, collection_id, status, merged_count, trimmed_count, error, created_at, finished_at) \
                     VALUES (?, ?, ?, 'failed', 0, 0, ?, ?, ?)",
                &db_params![&id, tenant_id, collection_id, e.to_string(), created_at, now_ms()])
                .await;
                Err(e)
            }
        }
    }

    /// The refine pass itself: (1) group active items by content hash and
    /// mark every duplicate after the earliest as trimmed; (2) if active
    /// items still exceed the floor, trim the oldest low-importance ones
    /// down to it. Soft deletion throughout — rows stay for audit.
    async fn refine_collection(&self, collection_id: &str) -> Result<(i64, i64), MemoryError> {
        let mut tx = self.db.begin().await?;

        let rows: Vec<(String, String)> = tx
            .fetch_all_as(
                "SELECT id, content_hash FROM one_memory_items \
                 WHERE collection_id = ? AND status = 'active' ORDER BY created_at ASC, id ASC",
                &db_params![collection_id],
            )
            .await?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut duplicates: Vec<String> = Vec::new();
        for (id, content_hash) in rows {
            // The first occurrence in creation order is the survivor; every
            // later identical hash is a duplicate to fold away.
            if !seen.insert(content_hash) {
                duplicates.push(id);
            }
        }
        let mut merged_count: i64 = 0;
        for id in duplicates {
            let rows = tx
                .execute(
                    "UPDATE one_memory_items SET status = 'trimmed', updated_at = ? WHERE id = ? AND status = 'active'",
                    &db_params![now_ms(), &id],
                )
                .await?;
            merged_count += rows as i64;
        }

        let active_count: i64 = tx
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_memory_items WHERE collection_id = ? AND status = 'active'",
                &db_params![collection_id],
            )
            .await?;
        let mut trimmed_count: i64 = 0;
        if active_count > MEMORY_REFINE_ACTIVE_FLOOR {
            let excess = active_count - MEMORY_REFINE_ACTIVE_FLOOR;
            let victims: Vec<(String,)> = tx
                .fetch_all_as(
                    "SELECT id FROM one_memory_items \
                     WHERE collection_id = ? AND status = 'active' AND importance < ? \
                     ORDER BY created_at ASC, id ASC LIMIT ?",
                    &db_params![collection_id, MEMORY_REFINE_MIN_IMPORTANCE, excess],
                )
                .await?;
            for (id,) in victims {
                let rows = tx
                    .execute(
                        "UPDATE one_memory_items SET status = 'trimmed', updated_at = ? WHERE id = ? AND status = 'active'",
                        &db_params![now_ms(), &id],
                    )
                    .await?;
                trimmed_count += rows as i64;
            }
        }

        tx.commit().await?;
        Ok((merged_count, trimmed_count))
    }

    /// Grant read/write on a collection to a member or a department.
    /// Re-granting the same subject overwrites the access in place — a
    /// collection has one row per subject, not a history of intentions.
    pub async fn grant_memory(
        &self,
        tenant_id: &str,
        collection_id: &str,
        subject_type: &str,
        subject_id: &str,
        access: &str,
        granted_by: &str,
    ) -> Result<MemoryGrantDto, MemoryError> {
        if !matches!(subject_type, "member" | "department") {
            return Err(MemoryError::BadRequest(format!(
                "unknown grant subject type '{subject_type}' (expected 'member' or 'department')"
            )));
        }
        if !matches!(access, "read" | "write") {
            return Err(MemoryError::BadRequest(format!(
                "unknown grant access '{access}' (expected 'read' or 'write')"
            )));
        }
        if subject_id.trim().is_empty() {
            return Err(MemoryError::BadRequest("grant subject id must not be empty".into()));
        }
        self.load_collection(tenant_id, collection_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let subject_id = subject_id.trim();
        let existing: Option<(String,)> = self.db.fetch_optional_as::<(String,)>(
            "SELECT id FROM one_memory_grants WHERE tenant_id = ? AND collection_id = ? AND subject_type = ? AND subject_id = ?",
        &db_params![tenant_id, collection_id, subject_type, subject_id])
        .await?;
        if let Some((id,)) = existing {
            self.db.execute("UPDATE one_memory_grants SET access = ?, granted_by = ? WHERE id = ?", &db_params![access, granted_by, &id])
                .await?;
            return self.get_grant(tenant_id, &id).await;
        }
        let id = generate_prefixed_id("memg");
        self.db.execute(
            "INSERT INTO one_memory_grants \
                 (id, tenant_id, collection_id, subject_type, subject_id, access, granted_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &db_params![&id, tenant_id, collection_id, subject_type, subject_id, access, granted_by, now_ms()])
        .await?;
        self.get_grant(tenant_id, &id).await
    }

    async fn get_grant(&self, tenant_id: &str, id: &str) -> Result<MemoryGrantDto, MemoryError> {
        let row: Option<(String, String, String, String, String, String, i64)> = self.db.fetch_optional_as::<(String, String, String, String, String, String, i64)>(
            "SELECT id, collection_id, subject_type, subject_id, access, granted_by, created_at \
             FROM one_memory_grants WHERE tenant_id = ? AND id = ?",
        &db_params![tenant_id, id])
        .await?;
        let Some((id, collection_id, subject_type, subject_id, access, granted_by, created_at)) = row else {
            return Err(MemoryError::Internal(
                "memory grant vanished immediately after write".into(),
            ));
        };
        Ok(MemoryGrantDto {
            id,
            collection_id,
            subject_type,
            subject_id,
            access,
            granted_by,
            created_at,
        })
    }

    pub async fn revoke_memory(&self, tenant_id: &str, grant_id: &str) -> Result<(), MemoryError> {
        let rows = self
            .db
            .execute("DELETE FROM one_memory_grants WHERE tenant_id = ? AND id = ?", &db_params![tenant_id, grant_id])
            .await?;
        if rows == 0 {
            return Err(MemoryError::NotFound("memory grant not found".into()));
        }
        Ok(())
    }

    pub async fn list_grants(&self, tenant_id: &str, collection_id: &str) -> Result<Vec<MemoryGrantDto>, MemoryError> {
        self.load_collection(tenant_id, collection_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("memory collection not found".into()))?;
        let rows: Vec<(String, String, String, String, String, String, i64)> = self.db.fetch_all_as::<(String, String, String, String, String, String, i64)>(
            "SELECT id, collection_id, subject_type, subject_id, access, granted_by, created_at \
             FROM one_memory_grants WHERE tenant_id = ? AND collection_id = ? ORDER BY created_at ASC, id ASC",
        &db_params![tenant_id, collection_id])
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, collection_id, subject_type, subject_id, access, granted_by, created_at)| MemoryGrantDto {
                    id,
                    collection_id,
                    subject_type,
                    subject_id,
                    access,
                    granted_by,
                    created_at,
                },
            )
            .collect())
    }

    /// Grant coverage: of the tenant's members, how many can read at least
    /// one active memory. The governance signal for "is memory actually
    /// reaching people, or is it written and never seen".
    pub async fn grant_coverage(&self, tenant_id: &str) -> Result<GrantCoverageDto, MemoryError> {
        let members: Vec<(String, String, Option<String>)> =
            self.db.fetch_all_as::<(String, String, Option<String>)>("SELECT user_id, role, department_id FROM one_user_org WHERE tenant_id = ?", &db_params![tenant_id])
                .await?;
        let member_count = members.len() as i64;
        if member_count == 0 {
            return Ok(GrantCoverageDto {
                member_count: 0,
                covered_count: 0,
                coverage_percent: 0.0,
            });
        }
        let grants = self.load_grants(tenant_id).await?;
        let rows = self.load_collections(tenant_id).await?;
        // `readable_by` takes the admin flag per member, so the models are
        // kept separate from any single member's answer.
        let models: Vec<Collection> = rows.iter().map(collection_row_to_model).collect();
        let with_active: HashSet<String> = self
            .db
            .fetch_all_scalar::<String>(
                "SELECT DISTINCT collection_id FROM one_memory_items WHERE tenant_id = ? AND status = 'active'",
                &db_params![tenant_id],
            )
            .await?
            .into_iter()
            .collect();
        let mut covered_count: i64 = 0;
        for (user_id, role, department_id) in &members {
            let is_admin = is_admin_role(role);
            let covered = models.iter().any(|c| {
                c.readable_by(user_id, is_admin, department_id.as_deref(), &grants) && with_active.contains(&c.id)
            });
            if covered {
                covered_count += 1;
            }
        }
        let coverage_percent = (covered_count as f64 / member_count as f64 * 100.0 * 100.0).round() / 100.0;
        Ok(GrantCoverageDto {
            member_count,
            covered_count,
            coverage_percent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (dream_core_db::Database, MemoryService) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        crate::migrate::run_one_memory_migrations(&dream_core_db::DbPool::Sqlite(db.pool().clone())).await.unwrap();
        let service = MemoryService::new(dream_core_db::DbPool::Sqlite(db.pool().clone()));
        (db, service)
    }

    async fn seed_membership(pool: &sqlx::SqlitePool, user_id: &str, tenant_id: &str, role: &str) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 role TEXT NOT NULL DEFAULT 'member', department_id TEXT, created_at INTEGER NOT NULL DEFAULT 0, \
                 updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));\
             CREATE TABLE IF NOT EXISTS one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
                 updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, role) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(tenant_id)
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_department(pool: &sqlx::SqlitePool, user_id: &str, tenant_id: &str, department_id: &str) {
        sqlx::query("UPDATE one_user_org SET department_id = ? WHERE user_id = ? AND tenant_id = ?")
            .bind(department_id)
            .bind(user_id)
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn make_collection(
        service: &MemoryService,
        caller_id: &str,
        scope: &str,
        department_id: Option<&str>,
        name: &str,
    ) -> MemoryCollectionDto {
        service
            .create_collection("t1", caller_id, "org_admin", scope, department_id, None, name, "")
            .await
            .unwrap()
    }

    async fn add_item(service: &MemoryService, caller_id: &str, collection_id: &str, content: &str, importance: f64) {
        service
            .add_item(
                "t1",
                caller_id,
                "org_admin",
                collection_id,
                content,
                importance,
                None,
                &[],
            )
            .await
            .unwrap();
    }

    async fn active_count(service: &MemoryService, collection_id: &str) -> usize {
        service
            .list_items("t1", "admin1", "org_admin", collection_id, 500)
            .await
            .unwrap()
            .into_iter()
            .filter(|item| item.status == "active")
            .count()
    }

    #[tokio::test]
    async fn create_collection_validates_scope_invariants() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "m1", "t1", "member").await;

        // Unknown scope is refused, not silently stored.
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "team", None, None, "x", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // Department tier must name its department.
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "department", None, None, "x", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // Global carries neither binding.
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "global", Some("d1"), None, "x", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "global", None, Some("admin1"), "x", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // Personal must be owned by the caller — nobody mints one for another.
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "personal", None, Some("m1"), "x", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_collection("t1", "admin1", "org_admin", "global", None, None, "  ", "")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // Non-admins may only create their own personal collections.
        assert_eq!(
            service
                .create_collection("t1", "m1", "member", "global", None, None, "x", "")
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        // Omitted personal owner defaults to the caller.
        let personal = service
            .create_collection("t1", "m1", "member", "personal", None, None, "my memory", "")
            .await
            .unwrap();
        assert_eq!(personal.owner_user_id.as_deref(), Some("m1"));
        assert_eq!(personal.scope, "personal");
    }

    #[tokio::test]
    async fn collection_visibility_follows_the_three_tiers() {
        let (db, service) = setup().await;
        for (user, role) in [("admin1", "org_admin"), ("m1", "member"), ("m2", "member")] {
            seed_membership(db.pool(), user, "t1", role).await;
        }
        seed_department(db.pool(), "m1", "t1", "d1").await;
        seed_department(db.pool(), "m2", "t1", "d2").await;

        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;
        let dept1 = make_collection(&service, "admin1", "department", Some("d1"), "d1 memory").await;
        let dept2 = make_collection(&service, "admin1", "department", Some("d2"), "d2 memory").await;
        let mine_m1 = service
            .create_collection("t1", "m1", "member", "personal", None, None, "m1 personal", "")
            .await
            .unwrap();
        let mine_m2 = service
            .create_collection("t1", "m2", "member", "personal", None, None, "m2 personal", "")
            .await
            .unwrap();

        // A member sees global + their own department + their own personal —
        // never another department's memory or another member's personal.
        let seen: Vec<String> = service
            .list_collections("t1", "m1", "member")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(seen, vec![global.id.clone(), dept1.id.clone(), mine_m1.id.clone()]);
        let seen_m2: Vec<String> = service
            .list_collections("t1", "m2", "member")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(seen_m2, vec![global.id, dept2.id, mine_m2.id]);
        // The admin inventory is the tenant's everything.
        assert_eq!(
            service
                .list_collections("t1", "admin1", "org_admin")
                .await
                .unwrap()
                .len(),
            5
        );
    }

    #[tokio::test]
    async fn department_collection_hidden_from_unrelated_members() {
        let (db, service) = setup().await;
        for (user, role) in [("admin1", "org_admin"), ("m1", "member"), ("m2", "member")] {
            seed_membership(db.pool(), user, "t1", role).await;
        }
        seed_department(db.pool(), "m1", "t1", "d1").await;
        seed_department(db.pool(), "m2", "t1", "d2").await;
        let dept1 = make_collection(&service, "admin1", "department", Some("d1"), "d1 memory").await;
        add_item(&service, "admin1", &dept1.id, "the d1 launch plan", 0.8).await;

        // Missing vs forbidden stay distinguishable.
        assert_eq!(
            service
                .list_items("t1", "m2", "member", &dept1.id, 100)
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        assert_eq!(
            service
                .list_items("t1", "m2", "member", "memc_missing", 100)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
        assert!(
            service
                .get_collection("t1", "m2", "member", &dept1.id)
                .await
                .unwrap()
                .is_none()
        );

        // Search never leaks: m2's readable set has no such content, m1 finds it.
        assert!(
            service
                .search_items("t1", "m2", "member", "launch", None, 50)
                .await
                .unwrap()
                .is_empty()
        );
        let found = service
            .search_items("t1", "m1", "member", "launch", None, 50)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].collection_id, dept1.id);
    }

    #[tokio::test]
    async fn global_writes_need_admin_or_write_grant() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "m1", "t1", "member").await;
        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;

        // A plain member cannot write shared knowledge.
        assert_eq!(
            service
                .add_item("t1", "m1", "member", &global.id, "hello", 0.5, None, &[])
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        // A read grant opens the collection but not the pen.
        service
            .grant_memory("t1", &global.id, "member", "m1", "read", "admin1")
            .await
            .unwrap();
        assert_eq!(
            service
                .add_item("t1", "m1", "member", &global.id, "hello", 0.5, None, &[])
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        // The write grant does, and the admin never needed one.
        service
            .grant_memory("t1", &global.id, "member", "m1", "write", "admin1")
            .await
            .unwrap();
        let item = service
            .add_item("t1", "m1", "member", &global.id, "hello from m1", 0.5, None, &[])
            .await
            .unwrap();
        assert_eq!(item.status, "active");
        add_item(&service, "admin1", &global.id, "admin note", 0.5).await;
    }

    #[tokio::test]
    async fn personal_collection_is_owner_only() {
        let (db, service) = setup().await;
        for (user, role) in [("admin1", "org_admin"), ("m1", "member"), ("m2", "member")] {
            seed_membership(db.pool(), user, "t1", role).await;
        }
        let personal = service
            .create_collection("t1", "m1", "member", "personal", None, None, "m1 personal", "")
            .await
            .unwrap();
        service
            .add_item(
                "t1",
                "m1",
                "member",
                &personal.id,
                "i prefer concise answers",
                0.9,
                None,
                &[],
            )
            .await
            .unwrap();

        // Another member can neither read nor write it.
        assert_eq!(
            service
                .list_items("t1", "m2", "member", &personal.id, 100)
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        assert_eq!(
            service
                .add_item("t1", "m2", "member", &personal.id, "sneaky", 0.5, None, &[])
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        // The owner needs no grant, and admins see everything.
        assert_eq!(
            service
                .list_items("t1", "m1", "member", &personal.id, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service
                .list_items("t1", "admin1", "org_admin", &personal.id, 100)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn refine_merges_duplicate_content() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;
        add_item(&service, "admin1", &global.id, "quarterly roadmap notes", 0.8).await;
        add_item(&service, "admin1", &global.id, "quarterly roadmap notes", 0.8).await;
        add_item(&service, "admin1", &global.id, "different note", 0.8).await;

        let job = service.run_refine_job("t1", &global.id).await.unwrap();
        assert_eq!(job.status, "done");
        assert!(job.merged_count >= 1);
        assert_eq!(job.trimmed_count, 0);

        // Soft deletion: the duplicate is still listed, marked trimmed.
        let items = service
            .list_items("t1", "admin1", "org_admin", &global.id, 100)
            .await
            .unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items.iter().filter(|i| i.status == "trimmed").count(), 1);
        assert_eq!(active_count(&service, &global.id).await, 2);
        // Search answers from active memories only.
        let hits = service
            .search_items("t1", "admin1", "org_admin", "roadmap", Some(&global.id), 50)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn refine_trims_low_importance_to_the_floor() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;
        for i in 0..21 {
            add_item(&service, "admin1", &global.id, &format!("low value note {i}"), 0.1).await;
        }
        let job = service.run_refine_job("t1", &global.id).await.unwrap();
        assert_eq!(job.merged_count, 0);
        assert_eq!(job.trimmed_count, 1);
        assert_eq!(active_count(&service, &global.id).await, 20);
    }

    #[tokio::test]
    async fn grant_coverage_counts_members_who_can_reach_memory() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "a1", "t1", "org_admin").await;
        seed_membership(db.pool(), "m2", "t1", "member").await;
        seed_membership(db.pool(), "m3", "t1", "member").await;
        seed_department(db.pool(), "a1", "t1", "d1").await;
        seed_department(db.pool(), "m2", "t1", "d2").await;
        seed_department(db.pool(), "m3", "t1", "d3").await;

        let dept1 = make_collection(&service, "a1", "department", Some("d1"), "d1 memory").await;
        add_item(&service, "a1", &dept1.id, "d1 knows the deploy runbook", 0.8).await;

        // Three members, only a1 can read an active memory: 1/3 = 33.33%.
        let coverage = service.grant_coverage("t1").await.unwrap();
        assert_eq!(coverage.member_count, 3);
        assert_eq!(coverage.covered_count, 1);
        assert!((coverage.coverage_percent - 33.33).abs() < 0.01);

        // Granting m2 read coverage lifts the metric.
        service
            .grant_memory("t1", &dept1.id, "member", "m2", "read", "a1")
            .await
            .unwrap();
        let coverage = service.grant_coverage("t1").await.unwrap();
        assert_eq!(coverage.covered_count, 2);
        assert!((coverage.coverage_percent - 66.67).abs() < 0.01);
    }

    #[tokio::test]
    async fn grant_upsert_overwrites_access_in_place() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "m1", "t1", "member").await;
        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;

        service
            .grant_memory("t1", &global.id, "member", "m1", "write", "admin1")
            .await
            .unwrap();
        let overwritten = service
            .grant_memory("t1", &global.id, "member", "m1", "read", "admin1")
            .await
            .unwrap();
        let grants = service.list_grants("t1", &global.id).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].id, overwritten.id);
        assert_eq!(grants[0].access, "read");

        // Vocabularies are validated, and revocation is final.
        assert_eq!(
            service
                .grant_memory("t1", &global.id, "team", "m1", "read", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .grant_memory("t1", &global.id, "member", "m1", "admin", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        service.revoke_memory("t1", &grants[0].id).await.unwrap();
        assert!(service.list_grants("t1", &global.id).await.unwrap().is_empty());
        assert_eq!(
            service.revoke_memory("t1", &grants[0].id).await.unwrap_err().code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn update_and_delete_follow_collection_ownership() {
        let (db, service) = setup().await;
        for (user, role) in [("admin1", "org_admin"), ("m1", "member"), ("m2", "member")] {
            seed_membership(db.pool(), user, "t1", role).await;
        }
        seed_department(db.pool(), "m1", "t1", "d1").await;
        let dept1 = make_collection(&service, "admin1", "department", Some("d1"), "d1 memory").await;
        let personal = service
            .create_collection("t1", "m1", "member", "personal", None, None, "m1 personal", "")
            .await
            .unwrap();

        // Shared memory is curated: members inside the department still
        // cannot rename or delete it.
        assert_eq!(
            service
                .update_collection("t1", "m1", "member", &dept1.id, Some("renamed"), None)
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        assert_eq!(
            service
                .delete_collection("t1", "m1", "member", &dept1.id)
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        // The admin can; the owner can edit their own personal collection.
        let renamed = service
            .update_collection("t1", "admin1", "org_admin", &dept1.id, Some("d1 curated"), None)
            .await
            .unwrap();
        assert_eq!(renamed.name, "d1 curated");
        service
            .update_collection("t1", "m1", "member", &personal.id, Some("my corner"), None)
            .await
            .unwrap();
        // Deleting wipes items and grants with it.
        service
            .delete_collection("t1", "m1", "member", &personal.id)
            .await
            .unwrap();
        assert_eq!(
            service
                .list_items("t1", "m1", "member", &personal.id, 100)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn ensure_personal_collection_creates_once_then_reuses() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "m1", "t1", "member").await;

        let a = service
            .ensure_personal_collection("t1", "m1", "member", "对话记忆")
            .await
            .unwrap();
        let b = service
            .ensure_personal_collection("t1", "m1", "member", "对话记忆")
            .await
            .unwrap();
        assert_eq!(a, b, "second call must reuse the same collection");

        // It is a real personal collection owned by m1.
        let cols = service.list_collections("t1", "m1", "member").await.unwrap();
        let c = cols.iter().find(|c| c.id == a).unwrap();
        assert_eq!(c.scope, "personal");
        assert_eq!(c.owner_user_id.as_deref(), Some("m1"));

        // And add_item into it works (the extraction pipeline's write).
        service
            .add_item("t1", "m1", "member", &a, "我用 pnpm 不用 npm", 0.7, Some("conv_1"), &[])
            .await
            .unwrap();
        assert_eq!(service.list_items("t1", "m1", "member", &a, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn search_relevant_tokenises_and_ranks_by_hit_count() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "m1", "t1", "member").await;
        let global = make_collection(&service, "admin1", "global", None, "kb").await;
        add_item(
            &service,
            "admin1",
            &global.id,
            "deploy uses the blue-green pipeline",
            0.5,
        )
        .await;
        add_item(&service, "admin1", &global.id, "the staging database is read-only", 0.5).await;
        add_item(&service, "admin1", &global.id, "unrelated note about lunch", 0.5).await;

        // A whole-sentence query never matches as one LIKE, but tokenised it does.
        let hits = service
            .search_relevant("t1", "m1", "member", "how does deploy work with the pipeline", 5)
            .await
            .unwrap();
        assert!(!hits.is_empty(), "tokenised search must find the deploy note");
        assert_eq!(hits[0].content, "deploy uses the blue-green pipeline");

        // Empty / whitespace query → nothing, no error.
        assert!(
            service
                .search_relevant("t1", "m1", "member", "   ", 5)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn search_scopes_to_readable_collections_and_active_items() {
        let (db, service) = setup().await;
        for (user, role) in [("admin1", "org_admin"), ("m1", "member"), ("m2", "member")] {
            seed_membership(db.pool(), user, "t1", role).await;
        }
        seed_department(db.pool(), "m1", "t1", "d1").await;
        let global = make_collection(&service, "admin1", "global", None, "company knowledge").await;
        let dept1 = make_collection(&service, "admin1", "department", Some("d1"), "d1 memory").await;
        add_item(&service, "admin1", &global.id, "onboarding checklist", 0.9).await;
        add_item(&service, "admin1", &dept1.id, "d1 onboarding rota", 0.9).await;
        add_item(&service, "admin1", &global.id, "retired onboarding checklist", 0.1).await;
        // Push the global collection past the active floor so refinement
        // trims the low-importance item — the only way a memory retires.
        for i in 0..19 {
            add_item(&service, "admin1", &global.id, &format!("filler note {i}"), 0.9).await;
        }
        let job = service.run_refine_job("t1", &global.id).await.unwrap();
        assert_eq!(job.trimmed_count, 1);

        // m1 sees both readable collections; the trimmed duplicate never answers.
        let hits = service
            .search_items("t1", "m1", "member", "onboarding", None, 50)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        // Scoped search within a readable collection.
        let scoped = service
            .search_items("t1", "m1", "member", "onboarding", Some(&global.id), 50)
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].collection_id, global.id);
        // Scoped search into an unreadable collection is refused, not emptied.
        assert_eq!(
            service
                .search_items("t1", "m2", "member", "onboarding", Some(&dept1.id), 50)
                .await
                .unwrap_err()
                .code(),
            "FORBIDDEN"
        );
        assert_eq!(
            service
                .search_items("t1", "m1", "member", "onboarding", Some("memc_missing"), 50)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn refine_and_grants_need_a_real_collection() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        assert_eq!(
            service.run_refine_job("t1", "memc_missing").await.unwrap_err().code(),
            "NOT_FOUND"
        );
        assert_eq!(
            service
                .grant_memory("t1", "memc_missing", "member", "m1", "read", "admin1")
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
        assert!(service.list_grants("t1", "memc_missing").await.unwrap_err().code() == "NOT_FOUND");
    }
}
