//! Platform infrastructure config service (P1-3 container runtime + P2-2
//! realtime collaboration). All logic lives here; routes only transform
//! request/response.
//!
//! Both configs are per-project-group singletons keyed by tenant_id, with any
//! credential encrypted at rest (same helper as one-org's SMTP password).
//! Storing a config runs nothing — a real `ContainerRuntime` /
//! `CollaborationProvider` (wired at the app layer) does the actual work; until
//! then a probe reports "not configured" via the Noop defaults.

use std::sync::Arc;

use sqlx::SqlitePool;

use dream_core_common::{decrypt_string, encrypt_string, generate_prefixed_id, now_ms};

use crate::collaboration::{
    CollaborationProvider, CollaborationSettings, CollaborationStatus, NoopCollaborationProvider,
};
use crate::container::{ContainerRuntime, ContainerSettings, ContainerStatus, NoopContainerRuntime};
use crate::error::PlatformError;
use crate::ip_allowlist::ip_allowed;
use crate::models::{
    CollaborationConfigDto, ContainerConfigDto, EffectiveGrantDto, IpAllowlistConfigDto, ResourceGrantDto,
    SiemConfigDto,
};
use crate::siem::{NoopSiemExporter, SiemExporter, SiemSettings, SiemStatus};

/// Valid `subject_type` values for a resource grant (E5 four-dimensional
/// matrix): a specific member, or every member under a department (resolved
/// at read time by walking the department tree — see
/// `PlatformService::effective_resource_ids`).
pub const GRANT_SUBJECT_TYPES: [&str; 2] = ["member", "department"];

/// Valid `resource_type` values. Mirrors the four registries
/// `dream-domain-devops` already has a `scope`/`visibility` column on
/// (skills, MCP servers, model channels) plus digital employees
/// (`dream-domain-employee`, no admin-facing registry yet). `model` and
/// `channel` are one dimension here (`model_channel`) because they are one
/// table upstream — a channel offers a set of models as a unit.
pub const GRANT_RESOURCE_TYPES: [&str; 4] = ["skill", "mcp", "employee", "model_channel"];

/// `resource_id` sentinel meaning "every resource of this type, now and
/// whenever a new one is added" — the escape hatch for "give this department
/// every skill" without one row per skill.
pub const GRANT_ALL_RESOURCES: &str = "*";

/// The caller's resolved enterprise membership (active tenant + role).
#[derive(Debug, Clone)]
pub struct PlatformActor {
    pub tenant_id: String,
    pub role: String,
}

pub struct PlatformService {
    pool: SqlitePool,
    /// Encrypts stored registry/collaboration credentials (same key/helper as
    /// one-org's SMTP password and provider API keys).
    encryption_key: [u8; 32],
    container_runtime: Arc<dyn ContainerRuntime>,
    collaboration_provider: Arc<dyn CollaborationProvider>,
    siem_exporter: Arc<dyn SiemExporter>,
}

fn is_admin_role(role: &str) -> bool {
    matches!(role, "org_admin" | "system_admin" | "admin")
}

impl PlatformService {
    pub fn new(pool: SqlitePool, encryption_key: [u8; 32]) -> Self {
        Self {
            pool,
            encryption_key,
            container_runtime: Arc::new(NoopContainerRuntime),
            collaboration_provider: Arc::new(NoopCollaborationProvider),
            siem_exporter: Arc::new(NoopSiemExporter),
        }
    }

    /// Swap in a real `ContainerRuntime` once a container client is wired at the
    /// app layer. Chainable at construction time.
    pub fn with_container_runtime(mut self, runtime: Arc<dyn ContainerRuntime>) -> Self {
        self.container_runtime = runtime;
        self
    }

    /// Swap in a real `CollaborationProvider` once a backend is wired.
    pub fn with_collaboration_provider(mut self, provider: Arc<dyn CollaborationProvider>) -> Self {
        self.collaboration_provider = provider;
        self
    }

    /// Swap in a real `SiemExporter` once log forwarding is wired.
    pub fn with_siem_exporter(mut self, exporter: Arc<dyn SiemExporter>) -> Self {
        self.siem_exporter = exporter;
        self
    }

    /// Resolve the caller's active-tenant membership (tenant + role) from
    /// one-org's `one_user_org` table (same SQLite pool). `Ok(None)` when the
    /// user has no membership row — personal edition / standalone owner — or
    /// when the table itself does not exist (one-org never initialized). Role
    /// is scoped to the active tenant (active membership first, else
    /// most-recently-joined), mirroring `dream_domain_devops::user_org_role`.
    pub async fn resolve_actor(&self, user_id: &str) -> Result<Option<PlatformActor>, PlatformError> {
        let result = sqlx::query_as::<_, (String, String)>(
            "SELECT uo.tenant_id, uo.role FROM one_user_org uo WHERE uo.user_id = ? \
             ORDER BY (uo.tenant_id = (SELECT tenant_id FROM one_active_tenant WHERE user_id = uo.user_id)) DESC, \
                      uo.created_at DESC, uo.tenant_id ASC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await;
        match result {
            Ok(Some((tenant_id, role))) => Ok(Some(PlatformActor { tenant_id, role })),
            Ok(None) => Ok(None),
            // Table missing = one-org never initialized = standalone.
            Err(sqlx::Error::Database(e)) if e.message().contains("no such table") => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve the caller and require enterprise membership with an admin role.
    /// Personal edition (no membership) → `NotInEnterprise`; non-admin →
    /// `Forbidden`.
    pub async fn require_admin(&self, user_id: &str) -> Result<PlatformActor, PlatformError> {
        match self.resolve_actor(user_id).await? {
            None => Err(PlatformError::NotInEnterprise),
            Some(actor) if !is_admin_role(&actor.role) => {
                Err(PlatformError::Forbidden("Administrator role required".into()))
            }
            Some(actor) => Ok(actor),
        }
    }

    // --- Container runtime config (P1-3) ---

    pub async fn get_container_config(&self, tenant_id: &str) -> Result<ContainerConfigDto, PlatformError> {
        type Row = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            i64,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT runtime_kind, endpoint, default_image, registry, registry_secret_encrypted, enabled, updated_at \
             FROM one_container_config WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some((runtime_kind, endpoint, default_image, registry, registry_secret_encrypted, enabled, updated_at)) => {
                ContainerConfigDto {
                    runtime_kind,
                    endpoint,
                    default_image,
                    registry,
                    has_registry_secret: registry_secret_encrypted.is_some(),
                    enabled,
                    updated_at: Some(updated_at),
                }
            }
            None => ContainerConfigDto {
                runtime_kind: None,
                endpoint: None,
                default_image: None,
                registry: None,
                has_registry_secret: false,
                enabled: false,
                updated_at: None,
            },
        })
    }

    /// `registry_secret` absent/empty = keep the stored one (if any).
    #[allow(clippy::too_many_arguments)]
    pub async fn set_container_config(
        &self,
        tenant_id: &str,
        runtime_kind: Option<&str>,
        endpoint: Option<&str>,
        default_image: Option<&str>,
        registry: Option<&str>,
        registry_secret: Option<&str>,
        enabled: bool,
    ) -> Result<ContainerConfigDto, PlatformError> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT registry_secret_encrypted FROM one_container_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let secret_encrypted = match registry_secret {
            Some(s) if !s.is_empty() => {
                Some(encrypt_string(s, &self.encryption_key).map_err(|e| PlatformError::Internal(e.to_string()))?)
            }
            _ => existing,
        };
        sqlx::query(
            "INSERT INTO one_container_config \
                 (tenant_id, runtime_kind, endpoint, default_image, registry, registry_secret_encrypted, enabled, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(tenant_id) DO UPDATE SET runtime_kind = excluded.runtime_kind, endpoint = excluded.endpoint, \
                 default_image = excluded.default_image, registry = excluded.registry, \
                 registry_secret_encrypted = excluded.registry_secret_encrypted, enabled = excluded.enabled, \
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(runtime_kind)
        .bind(endpoint)
        .bind(default_image)
        .bind(registry)
        .bind(&secret_encrypted)
        .bind(enabled)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        self.get_container_config(tenant_id).await
    }

    /// Decrypted registry credential, for a real `ContainerRuntime` to consume.
    /// `None` when unset or decryption fails (never panics).
    pub async fn container_registry_secret(&self, tenant_id: &str) -> Result<Option<String>, PlatformError> {
        let encrypted: Option<String> =
            sqlx::query_scalar("SELECT registry_secret_encrypted FROM one_container_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        Ok(encrypted.and_then(|e| decrypt_string(&e, &self.encryption_key).ok()))
    }

    /// Probe the container runtime through whatever is wired
    /// (`NoopContainerRuntime` by default — reports "not configured").
    pub async fn probe_container(&self, tenant_id: &str) -> Result<ContainerStatus, PlatformError> {
        let cfg = self.get_container_config(tenant_id).await?;
        let secret = self.container_registry_secret(tenant_id).await?;
        Ok(self
            .container_runtime
            .probe(ContainerSettings {
                runtime_kind: cfg.runtime_kind.as_deref(),
                endpoint: cfg.endpoint.as_deref(),
                default_image: cfg.default_image.as_deref(),
                registry: cfg.registry.as_deref(),
                registry_secret: secret.as_deref(),
            })
            .await)
    }

    // --- Realtime collaboration config (P2-2) ---

    pub async fn get_collaboration_config(&self, tenant_id: &str) -> Result<CollaborationConfigDto, PlatformError> {
        type Row = (Option<String>, Option<String>, Option<String>, bool, bool, i64);
        let row: Option<Row> = sqlx::query_as(
            "SELECT provider, endpoint, secret_encrypted, presence, enabled, updated_at \
             FROM one_collaboration_config WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some((provider, endpoint, secret_encrypted, presence, enabled, updated_at)) => CollaborationConfigDto {
                provider,
                endpoint,
                has_secret: secret_encrypted.is_some(),
                presence,
                enabled,
                updated_at: Some(updated_at),
            },
            None => CollaborationConfigDto {
                provider: None,
                endpoint: None,
                has_secret: false,
                presence: false,
                enabled: false,
                updated_at: None,
            },
        })
    }

    /// `secret` absent/empty = keep the stored one (if any).
    pub async fn set_collaboration_config(
        &self,
        tenant_id: &str,
        provider: Option<&str>,
        endpoint: Option<&str>,
        secret: Option<&str>,
        presence: bool,
        enabled: bool,
    ) -> Result<CollaborationConfigDto, PlatformError> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT secret_encrypted FROM one_collaboration_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let secret_encrypted = match secret {
            Some(s) if !s.is_empty() => {
                Some(encrypt_string(s, &self.encryption_key).map_err(|e| PlatformError::Internal(e.to_string()))?)
            }
            _ => existing,
        };
        sqlx::query(
            "INSERT INTO one_collaboration_config \
                 (tenant_id, provider, endpoint, secret_encrypted, presence, enabled, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(tenant_id) DO UPDATE SET provider = excluded.provider, endpoint = excluded.endpoint, \
                 secret_encrypted = excluded.secret_encrypted, presence = excluded.presence, \
                 enabled = excluded.enabled, updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(provider)
        .bind(endpoint)
        .bind(&secret_encrypted)
        .bind(presence)
        .bind(enabled)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        self.get_collaboration_config(tenant_id).await
    }

    /// Decrypted collaboration credential, for a real `CollaborationProvider`.
    pub async fn collaboration_secret(&self, tenant_id: &str) -> Result<Option<String>, PlatformError> {
        let encrypted: Option<String> =
            sqlx::query_scalar("SELECT secret_encrypted FROM one_collaboration_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        Ok(encrypted.and_then(|e| decrypt_string(&e, &self.encryption_key).ok()))
    }

    /// Probe the collaboration backend (`NoopCollaborationProvider` by default).
    pub async fn probe_collaboration(&self, tenant_id: &str) -> Result<CollaborationStatus, PlatformError> {
        let cfg = self.get_collaboration_config(tenant_id).await?;
        let secret = self.collaboration_secret(tenant_id).await?;
        Ok(self
            .collaboration_provider
            .probe(CollaborationSettings {
                provider: cfg.provider.as_deref(),
                endpoint: cfg.endpoint.as_deref(),
                secret: secret.as_deref(),
                presence: cfg.presence,
            })
            .await)
    }

    // --- IP allowlist (P1-4) ---

    pub async fn get_ip_allowlist(&self, tenant_id: &str) -> Result<IpAllowlistConfigDto, PlatformError> {
        let row: Option<(Option<String>, bool, i64)> =
            sqlx::query_as("SELECT cidrs, enabled, updated_at FROM one_ip_allowlist_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(match row {
            Some((cidrs, enabled, updated_at)) => IpAllowlistConfigDto {
                cidrs: cidrs
                    .filter(|s| !s.trim().is_empty())
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                enabled,
                updated_at: Some(updated_at),
            },
            None => IpAllowlistConfigDto {
                cidrs: Vec::new(),
                enabled: false,
                updated_at: None,
            },
        })
    }

    pub async fn set_ip_allowlist(
        &self,
        tenant_id: &str,
        cidrs: &[String],
        enabled: bool,
    ) -> Result<IpAllowlistConfigDto, PlatformError> {
        let cidrs_json = serde_json::to_string(cidrs).map_err(|e| PlatformError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO one_ip_allowlist_config (tenant_id, cidrs, enabled, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(tenant_id) DO UPDATE SET cidrs = excluded.cidrs, enabled = excluded.enabled, \
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(&cidrs_json)
        .bind(enabled)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        self.get_ip_allowlist(tenant_id).await
    }

    /// Whether `ip` may reach `tenant_id`'s server per the allowlist. When the
    /// allowlist is disabled, everyone is allowed (the reserved default — no
    /// blocking). When enabled, the IP must match a configured CIDR/address.
    pub async fn is_ip_allowed(&self, tenant_id: &str, ip: &str) -> Result<bool, PlatformError> {
        let cfg = self.get_ip_allowlist(tenant_id).await?;
        Ok(!cfg.enabled || ip_allowed(&cfg.cidrs, ip))
    }

    // --- SIEM export (P1-4) ---

    pub async fn get_siem_config(&self, tenant_id: &str) -> Result<SiemConfigDto, PlatformError> {
        type SiemRow = (Option<String>, Option<String>, Option<String>, bool, i64);
        let row: Option<SiemRow> = sqlx::query_as(
            "SELECT kind, endpoint, secret_encrypted, enabled, updated_at FROM one_siem_config WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some((kind, endpoint, secret_encrypted, enabled, updated_at)) => SiemConfigDto {
                kind,
                endpoint,
                has_secret: secret_encrypted.is_some(),
                enabled,
                updated_at: Some(updated_at),
            },
            None => SiemConfigDto {
                kind: None,
                endpoint: None,
                has_secret: false,
                enabled: false,
                updated_at: None,
            },
        })
    }

    /// `secret` absent/empty = keep the stored one (if any).
    pub async fn set_siem_config(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        endpoint: Option<&str>,
        secret: Option<&str>,
        enabled: bool,
    ) -> Result<SiemConfigDto, PlatformError> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT secret_encrypted FROM one_siem_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let secret_encrypted = match secret {
            Some(s) if !s.is_empty() => {
                Some(encrypt_string(s, &self.encryption_key).map_err(|e| PlatformError::Internal(e.to_string()))?)
            }
            _ => existing,
        };
        sqlx::query(
            "INSERT INTO one_siem_config (tenant_id, kind, endpoint, secret_encrypted, enabled, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(tenant_id) DO UPDATE SET kind = excluded.kind, endpoint = excluded.endpoint, \
                 secret_encrypted = excluded.secret_encrypted, enabled = excluded.enabled, \
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(kind)
        .bind(endpoint)
        .bind(&secret_encrypted)
        .bind(enabled)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        self.get_siem_config(tenant_id).await
    }

    /// Decrypted SIEM token, for a real `SiemExporter` to consume.
    pub async fn siem_secret(&self, tenant_id: &str) -> Result<Option<String>, PlatformError> {
        let encrypted: Option<String> =
            sqlx::query_scalar("SELECT secret_encrypted FROM one_siem_config WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        Ok(encrypted.and_then(|e| decrypt_string(&e, &self.encryption_key).ok()))
    }

    /// Probe the SIEM endpoint (`NoopSiemExporter` by default).
    pub async fn probe_siem(&self, tenant_id: &str) -> Result<SiemStatus, PlatformError> {
        let cfg = self.get_siem_config(tenant_id).await?;
        let secret = self.siem_secret(tenant_id).await?;
        Ok(self
            .siem_exporter
            .probe(SiemSettings {
                kind: cfg.kind.as_deref(),
                endpoint: cfg.endpoint.as_deref(),
                secret: secret.as_deref(),
            })
            .await)
    }

    // --- Resource authorization matrix (E5) ---

    fn validate_grant_kinds(subject_type: &str, resource_type: &str) -> Result<(), PlatformError> {
        if !GRANT_SUBJECT_TYPES.contains(&subject_type) {
            return Err(PlatformError::BadRequest(format!(
                "unknown subject type '{subject_type}'"
            )));
        }
        if !GRANT_RESOURCE_TYPES.contains(&resource_type) {
            return Err(PlatformError::BadRequest(format!(
                "unknown resource type '{resource_type}'"
            )));
        }
        Ok(())
    }

    /// Grant `subject` (a member or department) access to `resource`.
    /// Idempotent: granting the same tuple twice returns the existing row
    /// rather than erroring or duplicating — an admin re-checking a box in the
    /// matrix UI is not an error.
    #[allow(clippy::too_many_arguments)]
    pub async fn grant_resource(
        &self,
        tenant_id: &str,
        subject_type: &str,
        subject_id: &str,
        resource_type: &str,
        resource_id: &str,
        granted_by: &str,
    ) -> Result<ResourceGrantDto, PlatformError> {
        Self::validate_grant_kinds(subject_type, resource_type)?;
        if subject_id.trim().is_empty() {
            return Err(PlatformError::BadRequest("subject id must not be empty".into()));
        }
        if resource_id.trim().is_empty() {
            return Err(PlatformError::BadRequest("resource id must not be empty".into()));
        }

        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM one_resource_grants \
             WHERE tenant_id = ? AND subject_type = ? AND subject_id = ? \
               AND resource_type = ? AND resource_id = ?",
        )
        .bind(tenant_id)
        .bind(subject_type)
        .bind(subject_id)
        .bind(resource_type)
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await?;
        let id = match existing {
            Some((id,)) => id,
            None => {
                let id = generate_prefixed_id("grant");
                sqlx::query(
                    "INSERT INTO one_resource_grants \
                     (id, tenant_id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(tenant_id)
                .bind(subject_type)
                .bind(subject_id)
                .bind(resource_type)
                .bind(resource_id)
                .bind(granted_by)
                .bind(now_ms())
                .execute(&self.pool)
                .await?;
                id
            }
        };
        self.get_grant(tenant_id, &id)
            .await?
            .ok_or_else(|| PlatformError::Internal("grant vanished immediately after insert".into()))
    }

    async fn get_grant(&self, tenant_id: &str, id: &str) -> Result<Option<ResourceGrantDto>, PlatformError> {
        type Row = (String, String, String, String, String, String, i64);
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at \
             FROM one_resource_grants WHERE tenant_id = ? AND id = ?",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at)| ResourceGrantDto {
                id,
                subject_type,
                subject_id,
                resource_type,
                resource_id,
                granted_by,
                created_at,
            },
        ))
    }

    /// Revoke one grant by id. `NotFound` if it does not exist (or belongs to
    /// a different tenant — the `tenant_id` filter is load-bearing: without it
    /// an admin in one project group could revoke another group's grant just
    /// by guessing its id).
    pub async fn revoke_resource(&self, tenant_id: &str, grant_id: &str) -> Result<(), PlatformError> {
        let result = sqlx::query("DELETE FROM one_resource_grants WHERE id = ? AND tenant_id = ?")
            .bind(grant_id)
            .bind(tenant_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(PlatformError::NotFound("grant not found".into()));
        }
        Ok(())
    }

    /// List grants, optionally filtered — the matrix UI's row view
    /// (`subject_type`+`subject_id`) or column view (`resource_type`, and
    /// optionally `resource_id`).
    pub async fn list_grants(
        &self,
        tenant_id: &str,
        subject_type: Option<&str>,
        subject_id: Option<&str>,
        resource_type: Option<&str>,
    ) -> Result<Vec<ResourceGrantDto>, PlatformError> {
        let mut sql = "SELECT id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at \
                        FROM one_resource_grants WHERE tenant_id = ?"
            .to_owned();
        if subject_type.is_some() {
            sql.push_str(" AND subject_type = ?");
        }
        if subject_id.is_some() {
            sql.push_str(" AND subject_id = ?");
        }
        if resource_type.is_some() {
            sql.push_str(" AND resource_type = ?");
        }
        sql.push_str(" ORDER BY created_at DESC");

        type Row = (String, String, String, String, String, String, i64);
        let mut query = sqlx::query_as::<_, Row>(&sql).bind(tenant_id);
        if let Some(v) = subject_type {
            query = query.bind(v);
        }
        if let Some(v) = subject_id {
            query = query.bind(v);
        }
        if let Some(v) = resource_type {
            query = query.bind(v);
        }
        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, subject_type, subject_id, resource_type, resource_id, granted_by, created_at)| ResourceGrantDto {
                    id,
                    subject_type,
                    subject_id,
                    resource_type,
                    resource_id,
                    granted_by,
                    created_at,
                },
            )
            .collect())
    }

    /// A member's department, then every ancestor up to the root — read-only
    /// query against one-org's shared tables, same precedent as
    /// `resolve_actor` reading `one_user_org` directly rather than taking a
    /// dependency on `dream-domain-org`.
    ///
    /// Bounded to 64 hops as defense in depth: `set_department_parent`
    /// already refuses to create a cycle, so this is a backstop against a
    /// tree that got corrupted some other way, not the primary guard.
    async fn department_ancestry(&self, tenant_id: &str, user_id: &str) -> Result<Vec<String>, PlatformError> {
        let mut current: Option<String> =
            sqlx::query_scalar("SELECT department_id FROM one_user_org WHERE tenant_id = ? AND user_id = ?")
                .bind(tenant_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();

        let mut chain = Vec::new();
        let mut hops = 0;
        while let Some(department_id) = current {
            if hops >= 64 {
                tracing::warn!(tenant_id, user_id, "department ancestry exceeded 64 hops; truncating");
                break;
            }
            hops += 1;
            chain.push(department_id.clone());
            current = sqlx::query_scalar("SELECT parent_id FROM one_departments WHERE tenant_id = ? AND id = ?")
                .bind(tenant_id)
                .bind(&department_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        }
        Ok(chain)
    }

    /// Resolve what `user_id` may reach for `resource_type`: their own direct
    /// grants, plus every grant on their department or any ancestor of it.
    /// `all: true` (a wildcard grant anywhere in that set) means every
    /// resource of this type is reachable — callers should treat
    /// `resource_ids` as irrelevant in that case, not "empty = nothing".
    pub async fn effective_resource_ids(
        &self,
        tenant_id: &str,
        user_id: &str,
        resource_type: &str,
    ) -> Result<EffectiveGrantDto, PlatformError> {
        if !GRANT_RESOURCE_TYPES.contains(&resource_type) {
            return Err(PlatformError::BadRequest(format!(
                "unknown resource type '{resource_type}'"
            )));
        }

        let department_ids = self.department_ancestry(tenant_id, user_id).await?;
        let mut resource_ids = std::collections::HashSet::new();
        let mut all = false;

        let member_grants: Vec<(String,)> = sqlx::query_as(
            "SELECT resource_id FROM one_resource_grants \
             WHERE tenant_id = ? AND subject_type = 'member' AND subject_id = ? AND resource_type = ?",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(resource_type)
        .fetch_all(&self.pool)
        .await?;
        for (resource_id,) in member_grants {
            if resource_id == GRANT_ALL_RESOURCES {
                all = true;
            } else {
                resource_ids.insert(resource_id);
            }
        }

        if !all && !department_ids.is_empty() {
            let placeholders = vec!["?"; department_ids.len()].join(", ");
            let sql = format!(
                "SELECT resource_id FROM one_resource_grants \
                 WHERE tenant_id = ? AND subject_type = 'department' AND resource_type = ? \
                   AND subject_id IN ({placeholders})"
            );
            let mut query = sqlx::query_as::<_, (String,)>(&sql).bind(tenant_id).bind(resource_type);
            for department_id in department_ids {
                query = query.bind(department_id);
            }
            for (resource_id,) in query.fetch_all(&self.pool).await? {
                if resource_id == GRANT_ALL_RESOURCES {
                    all = true;
                    break;
                }
                resource_ids.insert(resource_id);
            }
        }

        Ok(EffectiveGrantDto {
            all,
            resource_ids: if all {
                Vec::new()
            } else {
                resource_ids.into_iter().collect()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (dream_core_db::Database, PlatformService) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        crate::migrate::run_one_platform_migrations(db.pool()).await.unwrap();
        let service = PlatformService::new(db.pool().clone(), [7u8; 32]);
        (db, service)
    }

    /// Personal edition (no one_user_org table/row): resolve → None, admin gate
    /// rejects with NotInEnterprise. The red line — platform config is gated
    /// off entirely without an enterprise.
    #[tokio::test]
    async fn personal_edition_has_no_membership_and_is_gated() {
        let (_db, service) = setup().await;
        assert!(service.resolve_actor("nobody").await.unwrap().is_none());
        assert_eq!(
            service.require_admin("nobody").await.unwrap_err().code(),
            "NOT_IN_ENTERPRISE"
        );
    }

    async fn seed_membership(pool: &SqlitePool, user_id: &str, tenant_id: &str, role: &str) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 role TEXT NOT NULL DEFAULT 'member', department_id TEXT, created_at INTEGER NOT NULL DEFAULT 0, \
                 updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));\
             CREATE TABLE IF NOT EXISTS one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
                 updated_at INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE IF NOT EXISTS one_departments (id TEXT PRIMARY KEY NOT NULL, tenant_id TEXT NOT NULL, \
                 parent_id TEXT, name TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT 0, \
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

    #[tokio::test]
    async fn admin_gate_accepts_admin_rejects_member() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "member1", "t1", "member").await;
        assert_eq!(service.require_admin("admin1").await.unwrap().tenant_id, "t1");
        assert_eq!(service.require_admin("member1").await.unwrap_err().code(), "FORBIDDEN");
    }

    #[tokio::test]
    async fn container_config_roundtrips_redacts_and_stubs_probe() {
        let (_db, service) = setup().await;
        // Absent by default.
        let cfg = service.get_container_config("t1").await.unwrap();
        assert!(!cfg.enabled && !cfg.has_registry_secret);

        let saved = service
            .set_container_config(
                "t1",
                Some("docker"),
                Some("unix:///var/run/docker.sock"),
                Some("ghcr.io/acme/agent:latest"),
                Some("ghcr.io"),
                Some("reg_secret"),
                true,
            )
            .await
            .unwrap();
        assert_eq!(saved.runtime_kind.as_deref(), Some("docker"));
        assert!(saved.has_registry_secret);
        // The DTO never carries the plaintext/ciphertext secret.
        assert!(!serde_json::to_string(&saved).unwrap().contains("reg_secret"));

        // Omitting the secret on a later save keeps the stored one.
        let updated = service
            .set_container_config("t1", Some("docker"), None, None, Some("ghcr.io"), None, false)
            .await
            .unwrap();
        assert!(updated.has_registry_secret && !updated.enabled);
        assert_eq!(
            service.container_registry_secret("t1").await.unwrap().as_deref(),
            Some("reg_secret")
        );

        // Default Noop runtime reports "not configured".
        assert_eq!(service.probe_container("t1").await.unwrap().status, "not_configured");
    }

    #[tokio::test]
    async fn collaboration_config_roundtrips_redacts_and_stubs_probe() {
        let (_db, service) = setup().await;
        let cfg = service.get_collaboration_config("t1").await.unwrap();
        assert!(!cfg.enabled && !cfg.has_secret && !cfg.presence);

        let saved = service
            .set_collaboration_config(
                "t1",
                Some("external"),
                Some("wss://collab.acme.com"),
                Some("relay_tok"),
                true,
                true,
            )
            .await
            .unwrap();
        assert_eq!(saved.provider.as_deref(), Some("external"));
        assert!(saved.has_secret && saved.presence);
        assert!(!serde_json::to_string(&saved).unwrap().contains("relay_tok"));

        let updated = service
            .set_collaboration_config(
                "t1",
                Some("external"),
                Some("wss://collab.acme.com"),
                None,
                false,
                false,
            )
            .await
            .unwrap();
        assert!(updated.has_secret && !updated.presence && !updated.enabled);
        assert_eq!(
            service.collaboration_secret("t1").await.unwrap().as_deref(),
            Some("relay_tok")
        );

        assert_eq!(
            service.probe_collaboration("t1").await.unwrap().status,
            "not_configured"
        );
    }

    #[tokio::test]
    async fn ip_allowlist_roundtrips_and_enforces_when_enabled() {
        let (_db, service) = setup().await;
        // Disabled by default → everyone allowed.
        assert!(service.is_ip_allowed("t1", "8.8.8.8").await.unwrap());

        let saved = service
            .set_ip_allowlist("t1", &["10.0.0.0/8".to_owned(), "192.168.1.5".to_owned()], true)
            .await
            .unwrap();
        assert_eq!(saved.cidrs.len(), 2);
        assert!(saved.enabled);

        // Enabled → only matching IPs allowed.
        assert!(service.is_ip_allowed("t1", "10.3.4.5").await.unwrap());
        assert!(service.is_ip_allowed("t1", "192.168.1.5").await.unwrap());
        assert!(!service.is_ip_allowed("t1", "8.8.8.8").await.unwrap());

        // Toggling off re-allows everyone (no blocking).
        service
            .set_ip_allowlist("t1", &["10.0.0.0/8".to_owned()], false)
            .await
            .unwrap();
        assert!(service.is_ip_allowed("t1", "8.8.8.8").await.unwrap());
    }

    #[tokio::test]
    async fn siem_config_roundtrips_redacts_and_stubs_probe() {
        let (_db, service) = setup().await;
        let cfg = service.get_siem_config("t1").await.unwrap();
        assert!(!cfg.enabled && !cfg.has_secret);

        let saved = service
            .set_siem_config(
                "t1",
                Some("splunk"),
                Some("https://splunk.acme.com:8088"),
                Some("hec_token"),
                true,
            )
            .await
            .unwrap();
        assert_eq!(saved.kind.as_deref(), Some("splunk"));
        assert!(saved.has_secret);
        assert!(!serde_json::to_string(&saved).unwrap().contains("hec_token"));

        // Omitting the token on a later save keeps the stored one.
        let updated = service
            .set_siem_config("t1", Some("splunk"), Some("https://splunk.acme.com:8088"), None, false)
            .await
            .unwrap();
        assert!(updated.has_secret && !updated.enabled);
        assert_eq!(service.siem_secret("t1").await.unwrap().as_deref(), Some("hec_token"));

        assert_eq!(service.probe_siem("t1").await.unwrap().status, "not_configured");
    }

    // --- E5 resource authorization matrix ---

    async fn seed_department(pool: &SqlitePool, tenant_id: &str, id: &str, parent_id: Option<&str>, name: &str) {
        sqlx::query("INSERT INTO one_departments (id, tenant_id, parent_id, name, created_at, updated_at) VALUES (?, ?, ?, ?, 0, 0)")
            .bind(id)
            .bind(tenant_id)
            .bind(parent_id)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn assign_department(pool: &SqlitePool, user_id: &str, tenant_id: &str, department_id: &str) {
        sqlx::query("UPDATE one_user_org SET department_id = ? WHERE user_id = ? AND tenant_id = ?")
            .bind(department_id)
            .bind(user_id)
            .bind(tenant_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn grant_resource_is_idempotent_and_lists() {
        let (_db, service) = setup().await;
        let first = service
            .grant_resource("t1", "member", "alice", "skill", "sk_1", "admin1")
            .await
            .unwrap();
        let second = service
            .grant_resource("t1", "member", "alice", "skill", "sk_1", "admin1")
            .await
            .unwrap();
        assert_eq!(first.id, second.id, "re-granting the same tuple must not duplicate it");

        let grants = service.list_grants("t1", None, None, None).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].resource_id, "sk_1");
        assert_eq!(grants[0].granted_by, "admin1");
    }

    #[tokio::test]
    async fn grant_resource_rejects_unknown_kinds() {
        let (_db, service) = setup().await;
        assert_eq!(
            service
                .grant_resource("t1", "robot", "alice", "skill", "sk_1", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .grant_resource("t1", "member", "alice", "spaceship", "sk_1", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn revoke_resource_removes_it_and_404s_on_a_second_attempt() {
        let (_db, service) = setup().await;
        let grant = service
            .grant_resource("t1", "member", "alice", "mcp", "mcp_1", "admin1")
            .await
            .unwrap();

        service.revoke_resource("t1", &grant.id).await.unwrap();
        assert!(service.list_grants("t1", None, None, None).await.unwrap().is_empty());

        assert_eq!(
            service.revoke_resource("t1", &grant.id).await.unwrap_err().code(),
            "NOT_FOUND"
        );
    }

    /// Revoking must be scoped to the caller's own tenant — otherwise an
    /// admin of one project group could revoke another group's grant just by
    /// guessing its id.
    #[tokio::test]
    async fn revoke_resource_is_scoped_to_tenant() {
        let (_db, service) = setup().await;
        let grant = service
            .grant_resource("t1", "member", "alice", "mcp", "mcp_1", "admin1")
            .await
            .unwrap();
        assert_eq!(
            service.revoke_resource("t2", &grant.id).await.unwrap_err().code(),
            "NOT_FOUND"
        );
        // Still there — the cross-tenant attempt above must not have deleted it.
        assert_eq!(service.list_grants("t1", None, None, None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn effective_resource_ids_resolves_a_direct_member_grant() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        service
            .grant_resource("t1", "member", "alice", "model_channel", "ch_1", "admin1")
            .await
            .unwrap();

        let effective = service
            .effective_resource_ids("t1", "alice", "model_channel")
            .await
            .unwrap();
        assert!(!effective.all);
        assert_eq!(effective.resource_ids, vec!["ch_1".to_owned()]);

        // A resource type with no grant at all resolves to nothing reachable.
        let none = service.effective_resource_ids("t1", "alice", "skill").await.unwrap();
        assert!(!none.all && none.resource_ids.is_empty());
    }

    /// The whole point of the department dimension: a grant on a department
    /// two levels above a member must still reach them.
    #[tokio::test]
    async fn effective_resource_ids_resolves_through_department_ancestry() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        seed_department(db.pool(), "t1", "d_company", None, "总公司").await;
        seed_department(db.pool(), "t1", "d_eng", Some("d_company"), "研发中心").await;
        seed_department(db.pool(), "t1", "d_backend", Some("d_eng"), "后端组").await;
        assign_department(db.pool(), "alice", "t1", "d_backend").await;

        // Granted on the grandparent, not alice's own department.
        service
            .grant_resource("t1", "department", "d_company", "skill", "sk_1", "admin1")
            .await
            .unwrap();

        let effective = service.effective_resource_ids("t1", "alice", "skill").await.unwrap();
        assert!(!effective.all);
        assert_eq!(effective.resource_ids, vec!["sk_1".to_owned()]);

        // A sibling never under d_company must not see it.
        seed_membership(db.pool(), "bob", "t1", "member").await;
        seed_department(db.pool(), "t1", "d_sales", None, "销售部").await;
        assign_department(db.pool(), "bob", "t1", "d_sales").await;
        let bob_view = service.effective_resource_ids("t1", "bob", "skill").await.unwrap();
        assert!(bob_view.resource_ids.is_empty());
    }

    #[tokio::test]
    async fn effective_resource_ids_wildcard_short_circuits_the_explicit_list() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        service
            .grant_resource("t1", "member", "alice", "skill", "sk_1", "admin1")
            .await
            .unwrap();
        service
            .grant_resource("t1", "member", "alice", "skill", GRANT_ALL_RESOURCES, "admin1")
            .await
            .unwrap();

        let effective = service.effective_resource_ids("t1", "alice", "skill").await.unwrap();
        assert!(effective.all, "a wildcard grant means every resource of this type");
        assert!(
            effective.resource_ids.is_empty(),
            "the explicit list is moot once `all` is true"
        );
    }
}
