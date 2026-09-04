//! Digital-employee catalog (P1-2, align-openocta): the prebuilt ops/office
//! personas an administrator adopts into their tenant.
//!
//! Two layers, on purpose:
//!
//! 1. The GLOBAL reference table `one_employee_catalog` (migration 009) holds
//!    the 28 catalog definitions. It has no `tenant_id` — every tenant
//!    instantiates from the same curated content, and that content is
//!    versioned with the codebase rather than living as per-install data.
//! 2. Per-tenant adoption lives in `one_personal_agents`, in two shapes:
//!
//!    - **Placeholder rows** (the "seed"): `origin='catalog'`,
//!      `visibility='shared'`, owner = the [`CATALOG_OWNER_SENTINEL`]
//!      (`'catalog'`), persona stored in `automation_config.instructions` so
//!      the run path (`build_run_prompt`) picks it up unchanged. They exist
//!      because the admin registry page lists `one_personal_agents` rows and
//!      the T12 grant matrix (`one_employee_grants`) attaches to employee
//!      ids — a catalog entry can only be "visible in the registry and
//!      authorizable to a department/member" if a real row exists to attach
//!      to. Seeding is lazy (on catalog list / instantiate) and idempotent.
//!    - **Formal instances** (the "instantiate"): an admin-triggered NEW row
//!      owned by the initiating admin, `origin='catalog'`,
//!      `visibility='shared'`. The placeholder stays behind so grants made
//!      against it and the catalog listing keep working; the two are told
//!      apart by `owner_user_id` (sentinel vs. real user), which is the only
//!      column that distinguishes them — both carry `origin='catalog'`.
//!
//! Seed idempotency mechanics: unlike the platform crate's builtin scenes,
//! which lean on a `UNIQUE(tenant_id, name)` constraint with
//! `ON CONFLICT DO NOTHING`, `one_personal_agents` has no such constraint to
//! lean on (a member may legitimately name their own employee like a catalog
//! entry). Each placeholder insert is therefore guarded by an in-statement
//! `WHERE NOT EXISTS (… tenant + origin + sentinel-owner + name …)`, which is
//! row-level idempotent and self-heals a placeholder an operator deleted by
//! hand — same spirit as `seed_builtin_scenes` re-inserting on every list.
//!
//! Instantiate idempotency: the brief's "existing instance = `origin='catalog'
//! AND name=<catalog key>`" is realized as `(tenant_id, origin='catalog',
//! name = catalog entry name, owner_user_id != sentinel)`. The `name` column
//! carries the entry's display name (a formal employee should be named "K8s
//! 运维", not the slug `k8s-ops`), names are unique across the catalog
//! content, and the owner clause is what keeps this lookup from matching the
//! still-seeded placeholder instead of the instance.

use std::collections::HashMap;

use dream_core_db::{DbPool, db_params};

use dream_core_common::now_ms;

use crate::service::is_missing_table_error;

use crate::error::EmployeeError;
use crate::models::{CatalogEntryDto, CatalogEntryRow, PersonalAgentRow};

/// `owner_user_id` sentinel marking a seeded catalog placeholder — no real
/// user owns it, which is exactly what lets `owner_user_id != 'catalog'`
/// select formal instances only (see the module docs).
pub(crate) const CATALOG_OWNER_SENTINEL: &str = "catalog";
/// `origin` value shared by placeholders and formal instances.
pub(crate) const CATALOG_ORIGIN: &str = "catalog";

/// Backend recorded on seeded/instantiated catalog employees. Catalog content
/// is backend-agnostic; `claude` resolves through the agent registry to a
/// runnable ACP backend (same default every fixture and admin creation path
/// uses), and the admin rebinds persona/model after instantiating like on any
/// other employee.
const CATALOG_AGENT_TYPE: &str = "claude";

fn catalog_agent_id() -> String {
    let uuid = uuid::Uuid::now_v7().simple().to_string();
    format!("pa_{uuid}")
}

/// Lazily and idempotently ensure this tenant has one placeholder row per
/// catalog entry. Reads the entries from `one_employee_catalog` (not a Rust
/// mirror constant) so the migration is the single source of truth and the
/// two can never drift. Called from every catalog read/write path — cheap
/// (28 guarded inserts against a small table) and self-healing.
async fn seed_catalog_for_tenant(pool: &DbPool, tenant_id: &str) -> Result<(), EmployeeError> {
    let entries = pool.fetch_all_as::<CatalogEntryRow>("SELECT * FROM one_employee_catalog ORDER BY id ASC", &[])
        .await?;
    let now = now_ms() as i64;
    for entry in entries {
        let config = serde_json::json!({ "instructions": entry.persona }).to_string();
        pool.execute(
            "INSERT INTO one_personal_agents \
                 (id, owner_user_id, tenant_id, name, description, agent_type, automation_config, \
                  visibility, origin, created_at, updated_at) \
             SELECT ?, ?, ?, ?, ?, ?, ?, 'shared', ?, ?, ? \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM one_personal_agents \
                 WHERE tenant_id = ? AND origin = ? AND owner_user_id = ? AND name = ?)",
        &db_params![catalog_agent_id(), CATALOG_OWNER_SENTINEL, tenant_id, &entry.name, &entry.description, CATALOG_AGENT_TYPE, &config, CATALOG_ORIGIN, now, now, tenant_id, CATALOG_ORIGIN, CATALOG_OWNER_SENTINEL, &entry.name])
        .await?;
    }
    Ok(())
}

/// The admin catalog page: every catalog entry plus THIS tenant's adoption
/// status. Seeding runs first (lazy per-tenant seed), then per-entry status
/// is assembled in memory from three small queries instead of one query per
/// entry (28 entries × N+1 would still be cheap, but the batch shape matches
/// `list_tags_for_resources`' convention).
pub(crate) async fn list_catalog(pool: &DbPool, tenant_id: &str) -> Result<Vec<CatalogEntryDto>, EmployeeError> {
    seed_catalog_for_tenant(pool, tenant_id).await?;

    let entries = pool.fetch_all_as::<CatalogEntryRow>("SELECT * FROM one_employee_catalog ORDER BY id ASC", &[])
        .await?;

    // Sentinel-owned placeholders, keyed by entry name (names are unique
    // across the catalog content).
    let placeholders: HashMap<String, String> = pool.fetch_all_as::<(String, String)>(
        "SELECT name, id FROM one_personal_agents \
         WHERE tenant_id = ? AND origin = ? AND owner_user_id = ?",
    &db_params![tenant_id, CATALOG_ORIGIN, CATALOG_OWNER_SENTINEL])
    .await?
    .into_iter()
    .collect();

    // Formal instances: earliest per entry name wins (instantiate is
    // idempotent, so there is normally exactly one; ordering keeps the
    // mapping stable even if an old duplicate predates the guard).
    let mut instances: HashMap<String, String> = HashMap::new();
    let rows = pool.fetch_all_as::<(String, String)>(
        "SELECT name, id FROM one_personal_agents \
         WHERE tenant_id = ? AND origin = ? AND owner_user_id != ? \
         ORDER BY created_at ASC, id ASC",
    &db_params![tenant_id, CATALOG_ORIGIN, CATALOG_OWNER_SENTINEL])
    .await?;
    for (name, id) in rows {
        instances.entry(name).or_insert(id);
    }

    // Grant counts per employee id, batched — the "authorization summary"
    // column of the catalog page (placeholder + instance both count: grants
    // may pre-date instantiation, see the module docs). Grants now live in
    // the unified matrix (migration 012); a missing table (personal builds)
    // simply means nothing is granted.
    let grants: HashMap<String, i64> = match pool
        .fetch_all_as::<(String, i64)>(
            "SELECT resource_id, COUNT(*) FROM one_resource_grants \
             WHERE tenant_id = ? AND resource_type = 'employee' GROUP BY resource_id",
            &db_params![tenant_id],
        )
        .await
    {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) if is_missing_table_error(&e) => HashMap::new(),
        Err(e) => return Err(e.into()),
    };

    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let recommended_skills = serde_json::from_str::<Vec<String>>(&entry.recommended_skills).unwrap_or_default();
        let placeholder_id = placeholders.get(&entry.name);
        let instance_id = instances.get(&entry.name);
        let grant_count = placeholder_id.map_or(0, |id| grants.get(id).copied().unwrap_or(0))
            + instance_id.map_or(0, |id| grants.get(id).copied().unwrap_or(0));
        out.push(CatalogEntryDto {
            id: entry.id,
            key: entry.key,
            name: entry.name,
            description: entry.description,
            persona: entry.persona,
            recommended_skills,
            instantiated: instance_id.is_some(),
            instance_id: instance_id.cloned(),
            grant_count,
        });
    }
    Ok(out)
}

/// Instantiate a catalog entry as this tenant's formal digital employee
/// (owner = the initiating admin). Idempotent: re-instantiating an entry that
/// already has an instance returns that instance unchanged (see the module
/// docs for the exact idempotency key).
pub(crate) async fn instantiate_catalog_entry(
    pool: &DbPool,
    tenant_id: &str,
    admin_user_id: &str,
    catalog_id: &str,
) -> Result<PersonalAgentRow, EmployeeError> {
    // Seeding first so instantiate works without a prior catalog list call —
    // the placeholder is not strictly required for the instance itself, but
    // keeping both paths through the same seed keeps the tenant mirror
    // complete no matter which endpoint an admin hits first.
    seed_catalog_for_tenant(pool, tenant_id).await?;

    let entry = pool.fetch_optional_as::<CatalogEntryRow>("SELECT * FROM one_employee_catalog WHERE id = ?", &db_params![catalog_id])
        .await?
        .ok_or_else(|| EmployeeError::BadRequest(format!("catalog entry '{catalog_id}' not found")))?;

    // Idempotency: an instance (owner ≠ sentinel) for this entry already
    // exists → return it instead of creating a second one.
    if let Some(existing) = pool.fetch_optional_as::<PersonalAgentRow>(
        "SELECT * FROM one_personal_agents \
         WHERE tenant_id = ? AND origin = ? AND name = ? AND owner_user_id != ? \
         ORDER BY created_at ASC, id ASC LIMIT 1",
    &db_params![tenant_id, CATALOG_ORIGIN, &entry.name, CATALOG_OWNER_SENTINEL])
    .await?
    {
        return Ok(existing);
    }

    let id = catalog_agent_id();
    let now = now_ms() as i64;
    let config = serde_json::json!({ "instructions": entry.persona }).to_string();
    pool.execute(
        "INSERT INTO one_personal_agents \
             (id, owner_user_id, tenant_id, name, description, agent_type, automation_config, \
              visibility, origin, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'shared', ?, ?, ?)",
    &db_params![&id, admin_user_id, tenant_id, &entry.name, &entry.description, CATALOG_AGENT_TYPE, &config, CATALOG_ORIGIN, now, now])
    .await?;

    pool.fetch_optional_as::<PersonalAgentRow>("SELECT * FROM one_personal_agents WHERE id = ?", &db_params![&id])
        .await?
        .ok_or_else(|| EmployeeError::Internal("catalog instance vanished immediately after insert".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::run_one_employee_migrations;
    // Both are crate-private free functions in service.rs (made pub(crate)
    // for testability, same convention as the other free functions there).
    use crate::service::{grant_employee_access, is_missing_table_error, select_agent_for_use};

    async fn test_pool() -> dream_core_db::DbPool {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(&dream_core_db::DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        // The unified grants matrix lives in dream-domain-platform's
        // migrations; this crate's tests create its minimal shape directly
        // (personal builds without the platform never have it, and the read
        // paths treat that as "no grants").
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_resource_grants (\
                 id TEXT PRIMARY KEY NOT NULL,\
                 tenant_id TEXT NOT NULL,\
                 subject_type TEXT NOT NULL,\
                 subject_id TEXT NOT NULL,\
                 resource_type TEXT NOT NULL,\
                 resource_id TEXT NOT NULL,\
                 permission TEXT NOT NULL DEFAULT 'use',\
                 granted_by TEXT NOT NULL,\
                 created_at INTEGER NOT NULL,\
                 UNIQUE(tenant_id, subject_type, subject_id, resource_type, resource_id));",
        )
        .execute(db.pool())
        .await
        .unwrap();
        dream_core_db::DbPool::Sqlite(db.pool().clone())
    }

    async fn count_rows(pool: &DbPool, where_clause: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM one_personal_agents WHERE {where_clause}");
        pool.fetch_one_scalar(&sql, &[]).await.unwrap()
    }

    async fn placeholder_count(pool: &DbPool, tenant: &str) -> i64 {
        count_rows(
            pool,
            &format!("tenant_id = '{tenant}' AND origin = 'catalog' AND owner_user_id = 'catalog'"),
        )
        .await
    }

    #[tokio::test]
    async fn catalog_list_seeds_placeholder_rows_idempotently() {
        let pool = test_pool().await;

        let first = list_catalog(&pool, "t1").await.unwrap();
        assert_eq!(first.len(), 28, "the migration ships 28 catalog entries");
        assert!(first.windows(2).all(|w| w[0].id < w[1].id), "entries are ordered by id");
        assert!(
            first
                .iter()
                .all(|e| !e.instantiated && e.instance_id.is_none() && e.grant_count == 0)
        );
        // The seeded placeholder carries the persona where the run path reads it.
        let config: String = pool
            .fetch_one_scalar(
                "SELECT automation_config FROM one_personal_agents \
             WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' LIMIT 1",
                &[],
            )
            .await
            .unwrap();
        assert!(config.contains("instructions"));

        // Second list: no duplicate entries, no duplicate placeholders.
        let second = list_catalog(&pool, "t1").await.unwrap();
        assert_eq!(second.len(), 28);
        assert_eq!(placeholder_count(&pool, "t1").await, 28);

        // Self-healing: a hand-deleted placeholder is re-seeded on the next
        // list, same spirit as seed_builtin_scenes re-inserting on every call.
        pool.execute(
            "DELETE FROM one_personal_agents WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' AND name = ?",
            &db_params![&first[0].name],
        )
        .await
        .unwrap();
        assert_eq!(placeholder_count(&pool, "t1").await, 27);
        list_catalog(&pool, "t1").await.unwrap();
        assert_eq!(
            placeholder_count(&pool, "t1").await,
            28,
            "deleted placeholder is re-seeded"
        );

        // Every catalog entry has a matching placeholder (no drift between
        // the migration content and the seeding loop).
        for entry in &second {
            let exists: bool = pool
                .fetch_one_scalar(
                    "SELECT COUNT(*) > 0 FROM one_personal_agents \
                 WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' AND name = ?",
                    &db_params![&entry.name],
                )
                .await
                .unwrap();
            assert!(exists, "placeholder missing for entry {}", entry.key);
        }
    }

    #[tokio::test]
    async fn instantiate_creates_a_formal_shared_instance() {
        let pool = test_pool().await;
        let entry = &list_catalog(&pool, "t1").await.unwrap()[0];

        let instance = instantiate_catalog_entry(&pool, "t1", "admin1", &entry.id)
            .await
            .unwrap();
        assert_eq!(instance.owner_user_id, "admin1");
        assert_eq!(instance.tenant_id, "t1");
        assert_eq!(instance.origin, "catalog");
        assert_eq!(instance.visibility, "shared");
        assert_eq!(
            instance.name, entry.name,
            "instance carries the human-readable entry name"
        );
        assert_eq!(instance.published, 1);
        let config: serde_json::Value = serde_json::from_str(&instance.automation_config).unwrap();
        assert_eq!(
            config["instructions"], entry.persona,
            "persona rides in automation_config.instructions"
        );

        // The placeholder is untouched — grants and the registry listing keep
        // working against it.
        assert_eq!(placeholder_count(&pool, "t1").await, 28);
    }

    #[tokio::test]
    async fn instantiate_is_idempotent() {
        let pool = test_pool().await;
        let entry = &list_catalog(&pool, "t1").await.unwrap()[1];

        let first = instantiate_catalog_entry(&pool, "t1", "admin1", &entry.id)
            .await
            .unwrap();
        let second = instantiate_catalog_entry(&pool, "t1", "admin1", &entry.id)
            .await
            .unwrap();
        assert_eq!(first.id, second.id, "re-instantiating returns the existing instance");
        assert_eq!(
            count_rows(
                &pool,
                "tenant_id = 't1' AND origin = 'catalog' AND owner_user_id != 'catalog'"
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn catalog_list_reports_instantiation_and_grant_status() {
        let pool = test_pool().await;
        let entries = list_catalog(&pool, "t1").await.unwrap();
        let entry = &entries[2];
        assert!(!entries[2].instantiated);

        let instance = instantiate_catalog_entry(&pool, "t1", "admin1", &entry.id)
            .await
            .unwrap();
        // A grant pre-dating instantiation, attached to the placeholder row.
        let placeholder_id: String = pool
            .fetch_one_scalar(
                "SELECT id FROM one_personal_agents \
             WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' AND name = ?",
                &db_params![&entry.name],
            )
            .await
            .unwrap();
        grant_employee_access(&pool, "t1", "member", "u1", &placeholder_id, "use", "admin1")
            .await
            .unwrap();

        let after = list_catalog(&pool, "t1").await.unwrap();
        let target = after.iter().find(|e| e.id == entry.id).unwrap();
        assert!(target.instantiated);
        assert_eq!(target.instance_id.as_deref(), Some(instance.id.as_str()));
        assert_eq!(target.grant_count, 1, "grant on the placeholder counts");
        // Unrelated entries stay not-instantiated and ungranted.
        let untouched = after.iter().find(|e| e.id != entry.id).unwrap();
        assert!(!untouched.instantiated);
        assert_eq!(untouched.grant_count, 0);
    }

    #[tokio::test]
    async fn catalog_is_tenant_scoped() {
        let pool = test_pool().await;
        let entry = &list_catalog(&pool, "t1").await.unwrap()[3];

        instantiate_catalog_entry(&pool, "t1", "admin1", &entry.id)
            .await
            .unwrap();
        let placeholder_id: String = pool
            .fetch_one_scalar(
                "SELECT id FROM one_personal_agents \
             WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' AND name = ?",
                &db_params![&entry.name],
            )
            .await
            .unwrap();
        grant_employee_access(&pool, "t1", "member", "u1", &placeholder_id, "use", "admin1")
            .await
            .unwrap();

        // t2 gets its own independent seed and its own status view.
        let t2 = list_catalog(&pool, "t2").await.unwrap();
        assert_eq!(t2.len(), 28);
        assert_eq!(placeholder_count(&pool, "t2").await, 28);
        let target = t2.iter().find(|e| e.id == entry.id).unwrap();
        assert!(!target.instantiated, "t1's instance must not leak into t2");
        assert_eq!(target.instance_id, None);
        assert_eq!(target.grant_count, 0, "t1's grant must not leak into t2");
        assert_eq!(
            count_rows(&pool, "origin = 'catalog' AND owner_user_id != 'catalog'").await,
            1,
            "exactly one instance exists, and it belongs to t1"
        );
    }

    #[tokio::test]
    async fn instantiate_unknown_catalog_id_is_rejected() {
        let pool = test_pool().await;
        let result = instantiate_catalog_entry(&pool, "t1", "admin1", "empcat_nope").await;
        assert!(matches!(result, Err(EmployeeError::BadRequest(_))));
    }

    /// The seed's authorization semantics (the whole point of the sentinel
    /// owner): the placeholder is `shared` but, per T12, a non-owner can only
    /// reach it through an explicit grant in the matrix — the catalog is
    /// visible in the registry, use stays a governance action.
    #[tokio::test]
    async fn seeded_placeholder_needs_explicit_grant_for_non_owner_use() {
        let pool = test_pool().await;
        list_catalog(&pool, "t1").await.unwrap();
        let placeholder_id: String = pool
            .fetch_one_scalar(
                "SELECT id FROM one_personal_agents \
             WHERE tenant_id = 't1' AND origin = 'catalog' AND owner_user_id = 'catalog' LIMIT 1",
                &[],
            )
            .await
            .unwrap();

        assert!(
            select_agent_for_use(&pool, "u1", "t1", &placeholder_id)
                .await
                .unwrap()
                .is_none(),
            "no grant → not usable, even though the placeholder is 'shared'"
        );
        grant_employee_access(&pool, "t1", "member", "u1", &placeholder_id, "use", "admin1")
            .await
            .unwrap();
        assert!(
            select_agent_for_use(&pool, "u1", "t1", &placeholder_id)
                .await
                .unwrap()
                .is_some(),
            "explicit grant → usable"
        );
    }
}
