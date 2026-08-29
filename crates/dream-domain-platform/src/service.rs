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

use dream_core_common::constants::API_KEY_TOKEN_PREFIX;
use dream_core_common::{decrypt_string, encrypt_string, generate_id_with_length, generate_prefixed_id, now_ms};
use sha2::{Digest, Sha256};

use crate::collaboration::{
    CollaborationProvider, CollaborationSettings, CollaborationStatus, NoopCollaborationProvider,
};
use crate::container::{ContainerRuntime, ContainerSettings, ContainerStatus, NoopContainerRuntime};
use crate::error::PlatformError;
use crate::ip_allowlist::ip_allowed;
use crate::models::{
    ApiKeyDto, CollaborationConfigDto, ContainerConfigDto, EffectiveGrantDto, FileVaultDto, FileVaultObjectDto,
    FileVaultReconcileEntry, IpAllowlistConfigDto, MyNotificationDto, MyNotificationsDto, NewApiKeyDto,
    NotificationDto, ResourceGrantDto, SceneDto, SecurityPolicyDto, SiemConfigDto,
};
use crate::siem::{NoopSiemExporter, SiemExporter, SiemSettings, SiemStatus};

/// Valid `subject_type` values for a resource grant (E5 four-dimensional
/// matrix): a specific member, every member under a department (resolved at
/// read time by walking the department tree), or every member of a scene
/// (E5 "场景管理" — see `PlatformService::scene_ids_for_member`). All three
/// are resolved together by `PlatformService::effective_resource_ids`.
pub const GRANT_SUBJECT_TYPES: [&str; 3] = ["member", "department", "scene"];

/// Valid `resource_type` values. Mirrors the four registries
/// `dream-domain-devops` already has a `scope`/`visibility` column on
/// (skills, MCP servers, model channels, RAG knowledge documents). `model`
/// and `channel` are one dimension here (`model_channel`) because they are
/// one table upstream — a channel offers a set of models as a unit.
/// `knowledge` is the "知识库" half of E5's "知识与记忆治理" item — RAG
/// documents (`dream_domain_devops::RagDocumentDto`) already exist and
/// already carry `scope`/`visibility`, so this only needed a new
/// `resource_type` value, not a new registry. There is no `resource_type`
/// for "记忆集合" (memory collections): unlike knowledge documents, no
/// memory-collection feature exists anywhere in this codebase to grant
/// access to — governing a feature that isn't built would be fabricating
/// both, not just the governance layer.
///
/// `employee` is deliberately NOT in this list. Digital employees
/// (`dream-domain-employee`) are an owner-centric asset (only `private` or
/// tenant-wide `shared`, see `EmployeeService::set_visibility`), not a
/// registry with per-subject grants like the other four — bolting the
/// authorization matrix onto it is a product decision that hasn't been
/// made, not a wiring gap. `grant_resource` rejects it explicitly (see
/// `EMPLOYEE_NOT_SUPPORTED_MESSAGE`) instead of silently accepting and
/// doing nothing.
pub const GRANT_RESOURCE_TYPES: [&str; 4] = ["skill", "mcp", "model_channel", "knowledge"];

/// Returned by `grant_resource`/`validate_grant_kinds` for `resource_type ==
/// "employee"` — distinguishable from the generic "unknown resource type"
/// message so callers (and admins reading the error) can tell "this type
/// doesn't exist" apart from "this type exists but isn't supported yet".
const EMPLOYEE_NOT_SUPPORTED_MESSAGE: &str = "resource type 'employee' is not supported yet";

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
    /// Root directory for the personal file vault (P2-4). Objects live at
    /// `<root>/<tenant>/<user>/…`; the ledger stays in SQLite. Wired at the
    /// app layer (`<data_dir>/file-vault`); `None` → uploads and
    /// reconciliation report "not configured" instead of touching disk.
    storage_root: Option<std::path::PathBuf>,
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
            storage_root: None,
        }
    }

    /// Swap in a real `ContainerRuntime` once a container client is wired at the
    /// app layer. Chainable at construction time.
    pub fn with_container_runtime(mut self, runtime: Arc<dyn ContainerRuntime>) -> Self {
        self.container_runtime = runtime;
        self
    }

    /// Point the personal file vault (P2-4) at its storage root —
    /// `<data_dir>/file-vault` at the app layer. Chainable at construction
    /// time.
    pub fn with_storage_root(mut self, root: std::path::PathBuf) -> Self {
        self.storage_root = Some(root);
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

    /// Resolve the caller and require any enterprise membership (any role) —
    /// the gate for member-facing self-service routes (the in-app
    /// notification inbox). Personal edition (no membership) →
    /// `NotInEnterprise`.
    pub async fn require_member(&self, user_id: &str) -> Result<PlatformActor, PlatformError> {
        match self.resolve_actor(user_id).await? {
            None => Err(PlatformError::NotInEnterprise),
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
        if resource_type == "employee" {
            return Err(PlatformError::BadRequest(EMPLOYEE_NOT_SUPPORTED_MESSAGE.into()));
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
        // Store the TRIMMED ids, not the raw ones: validating `.trim()` and
        // then binding the original would let `" * "` pass the non-empty check
        // and persist as a grant that matches neither `GRANT_ALL_RESOURCES` nor
        // any real resource id — a permanently dead row the matrix UI renders
        // as active. Same hazard for a padded `subject_id` vs `one_user_org`.
        let subject_id = subject_id.trim();
        let resource_id = resource_id.trim();
        if subject_id.is_empty() {
            return Err(PlatformError::BadRequest("subject id must not be empty".into()));
        }
        if resource_id.is_empty() {
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

    /// Every grant's `resource_id` for one `subject_type` across a set of
    /// subject ids, folded into the caller's running `(all, resource_ids)`
    /// state. Shared by `effective_resource_ids` across its three subject
    /// dimensions (member / department / scene) so each is one query shape
    /// instead of three near-duplicates. No-ops (and issues no query) once
    /// `*all` is already true or `subject_ids` is empty — the caller decides
    /// whether to bother calling this at all past that point.
    async fn fold_grants_for_subjects(
        &self,
        tenant_id: &str,
        subject_type: &str,
        subject_ids: &[String],
        resource_type: &str,
        all: &mut bool,
        resource_ids: &mut std::collections::HashSet<String>,
    ) -> Result<(), PlatformError> {
        if *all || subject_ids.is_empty() {
            return Ok(());
        }
        let placeholders = vec!["?"; subject_ids.len()].join(", ");
        let sql = format!(
            "SELECT resource_id FROM one_resource_grants \
             WHERE tenant_id = ? AND subject_type = ? AND resource_type = ? AND subject_id IN ({placeholders})"
        );
        let mut query = sqlx::query_as::<_, (String,)>(&sql)
            .bind(tenant_id)
            .bind(subject_type)
            .bind(resource_type);
        for subject_id in subject_ids {
            query = query.bind(subject_id);
        }
        for (resource_id,) in query.fetch_all(&self.pool).await? {
            if resource_id == GRANT_ALL_RESOURCES {
                *all = true;
                return Ok(());
            }
            resource_ids.insert(resource_id);
        }
        Ok(())
    }

    /// Resolve what `user_id` may reach for `resource_type`: their own direct
    /// grants, every grant on their department or any ancestor of it, and
    /// every grant on a scene (E5) they belong to. `all: true` (a wildcard
    /// grant anywhere in that set) means every resource of this type is
    /// reachable — callers should treat `resource_ids` as irrelevant in that
    /// case, not "empty = nothing".
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
        let scene_ids = self.scene_ids_for_member(tenant_id, user_id).await?;
        let mut resource_ids = std::collections::HashSet::new();
        let mut all = false;

        self.fold_grants_for_subjects(
            tenant_id,
            "member",
            std::slice::from_ref(&user_id.to_owned()),
            resource_type,
            &mut all,
            &mut resource_ids,
        )
        .await?;
        self.fold_grants_for_subjects(
            tenant_id,
            "department",
            &department_ids,
            resource_type,
            &mut all,
            &mut resource_ids,
        )
        .await?;
        self.fold_grants_for_subjects(
            tenant_id,
            "scene",
            &scene_ids,
            resource_type,
            &mut all,
            &mut resource_ids,
        )
        .await?;

        Ok(EffectiveGrantDto {
            all,
            resource_ids: if all {
                Vec::new()
            } else {
                resource_ids.into_iter().collect()
            },
        })
    }

    // --- Scene management (E5) ---

    /// The 5 reference-product built-ins, seeded as empty templates (name +
    /// description + job-function tags, no resource grants — see the
    /// migration's own doc comment for why they can't ship pre-populated).
    /// An admin fills each in via `grant_resource("scene", scene_id, ...)`.
    const BUILTIN_SCENES: [(&'static str, &'static str, &'static [&'static str]); 5] = [
        ("办公", "日常办公协作场景", &["行政", "文秘"]),
        ("IT运维", "IT 基础设施运维场景", &["运维工程师"]),
        ("网络安全", "安全监控与响应场景", &["安全工程师"]),
        ("新媒体运营", "内容创作与社媒运营场景", &["内容运营", "社媒运营"]),
        ("市场营销", "市场推广与营销分析场景", &["市场专员"]),
    ];

    /// Idempotent: `(tenant_id, name)` is UNIQUE, so calling this on a tenant
    /// that already has its built-ins is a no-op. Called from `list_scenes`
    /// rather than any tenant-creation hook — this crate takes no dependency
    /// on dream-domain-org (see the module docs), so there is no "a new
    /// project group was just created" signal to hang this off of.
    async fn seed_builtin_scenes(&self, tenant_id: &str) -> Result<(), PlatformError> {
        let now = now_ms();
        for (name, description, job_functions) in Self::BUILTIN_SCENES {
            let job_functions_json =
                serde_json::to_string(job_functions).map_err(|e| PlatformError::Internal(e.to_string()))?;
            sqlx::query(
                "INSERT INTO one_scenes (id, tenant_id, name, description, job_functions, built_in, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, 1, ?, ?) \
                 ON CONFLICT(tenant_id, name) DO NOTHING",
            )
            .bind(generate_prefixed_id("scene"))
            .bind(tenant_id)
            .bind(name)
            .bind(description)
            .bind(&job_functions_json)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn scene_row_to_dto(&self, row: SceneRow) -> Result<SceneDto, PlatformError> {
        let member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_scene_members WHERE scene_id = ?")
            .bind(&row.0)
            .fetch_one(&self.pool)
            .await?;
        Ok(SceneDto {
            id: row.0,
            name: row.2,
            description: row.3,
            job_functions: serde_json::from_str(&row.4).unwrap_or_default(),
            built_in: row.5,
            member_count,
            created_at: row.6,
            updated_at: row.7,
        })
    }

    pub async fn list_scenes(&self, tenant_id: &str) -> Result<Vec<SceneDto>, PlatformError> {
        self.seed_builtin_scenes(tenant_id).await?;
        let rows: Vec<SceneRow> = sqlx::query_as(
            "SELECT id, tenant_id, name, description, job_functions, built_in, created_at, updated_at \
             FROM one_scenes WHERE tenant_id = ? ORDER BY built_in DESC, name ASC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.scene_row_to_dto(row).await?);
        }
        Ok(out)
    }

    pub async fn create_scene(
        &self,
        tenant_id: &str,
        name: &str,
        description: Option<&str>,
        job_functions: &[String],
    ) -> Result<SceneDto, PlatformError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PlatformError::BadRequest("scene name must not be empty".into()));
        }
        let id = generate_prefixed_id("scene");
        let now = now_ms();
        let job_functions_json =
            serde_json::to_string(job_functions).map_err(|e| PlatformError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO one_scenes (id, tenant_id, name, description, job_functions, built_in, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(name)
        .bind(description)
        .bind(&job_functions_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                PlatformError::BadRequest(format!("a scene named '{name}' already exists"))
            }
            _ => PlatformError::from(e),
        })?;
        self.get_scene(tenant_id, &id)
            .await?
            .ok_or_else(|| PlatformError::Internal("scene vanished immediately after insert".into()))
    }

    async fn get_scene(&self, tenant_id: &str, scene_id: &str) -> Result<Option<SceneDto>, PlatformError> {
        let row: Option<SceneRow> = sqlx::query_as(
            "SELECT id, tenant_id, name, description, job_functions, built_in, created_at, updated_at \
             FROM one_scenes WHERE tenant_id = ? AND id = ?",
        )
        .bind(tenant_id)
        .bind(scene_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(self.scene_row_to_dto(row).await?)),
            None => Ok(None),
        }
    }

    pub async fn update_scene(
        &self,
        tenant_id: &str,
        scene_id: &str,
        name: &str,
        description: Option<&str>,
        job_functions: &[String],
    ) -> Result<SceneDto, PlatformError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PlatformError::BadRequest("scene name must not be empty".into()));
        }
        let job_functions_json =
            serde_json::to_string(job_functions).map_err(|e| PlatformError::Internal(e.to_string()))?;
        let result = sqlx::query(
            "UPDATE one_scenes SET name = ?, description = ?, job_functions = ?, updated_at = ? \
             WHERE tenant_id = ? AND id = ?",
        )
        .bind(name)
        .bind(description)
        .bind(&job_functions_json)
        .bind(now_ms())
        .bind(tenant_id)
        .bind(scene_id)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                PlatformError::BadRequest(format!("a scene named '{name}' already exists"))
            }
            _ => PlatformError::from(e),
        })?;
        if result.rows_affected() == 0 {
            return Err(PlatformError::NotFound("scene not found".into()));
        }
        self.get_scene(tenant_id, scene_id)
            .await?
            .ok_or_else(|| PlatformError::Internal("scene vanished immediately after update".into()))
    }

    /// Refuses a built-in scene — it can be edited (including its resource
    /// grants) but not removed, same posture as every other "seeded default"
    /// in this codebase.
    pub async fn delete_scene(&self, tenant_id: &str, scene_id: &str) -> Result<(), PlatformError> {
        let built_in: Option<(bool,)> =
            sqlx::query_as("SELECT built_in FROM one_scenes WHERE tenant_id = ? AND id = ?")
                .bind(tenant_id)
                .bind(scene_id)
                .fetch_optional(&self.pool)
                .await?;
        match built_in {
            None => return Err(PlatformError::NotFound("scene not found".into())),
            Some((true,)) => return Err(PlatformError::BadRequest("a built-in scene cannot be deleted".into())),
            Some((false,)) => {}
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM one_scene_members WHERE scene_id = ?")
            .bind(scene_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM one_resource_grants WHERE tenant_id = ? AND subject_type = 'scene' AND subject_id = ?",
        )
        .bind(tenant_id)
        .bind(scene_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM one_scenes WHERE tenant_id = ? AND id = ?")
            .bind(tenant_id)
            .bind(scene_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Add `user_id` to `scene_id`'s roster. Idempotent — already being a
    /// member is not an error, same posture as `grant_resource`.
    pub async fn add_scene_member(&self, tenant_id: &str, scene_id: &str, user_id: &str) -> Result<(), PlatformError> {
        self.require_scene(tenant_id, scene_id).await?;
        sqlx::query(
            "INSERT INTO one_scene_members (scene_id, tenant_id, user_id, added_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(scene_id, user_id) DO NOTHING",
        )
        .bind(scene_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_scene_member(
        &self,
        tenant_id: &str,
        scene_id: &str,
        user_id: &str,
    ) -> Result<(), PlatformError> {
        self.require_scene(tenant_id, scene_id).await?;
        sqlx::query("DELETE FROM one_scene_members WHERE scene_id = ? AND user_id = ?")
            .bind(scene_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_scene_members(&self, tenant_id: &str, scene_id: &str) -> Result<Vec<String>, PlatformError> {
        self.require_scene(tenant_id, scene_id).await?;
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT user_id FROM one_scene_members WHERE scene_id = ? ORDER BY added_at ASC")
                .bind(scene_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn require_scene(&self, tenant_id: &str, scene_id: &str) -> Result<(), PlatformError> {
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM one_scenes WHERE tenant_id = ? AND id = ?")
            .bind(tenant_id)
            .bind(scene_id)
            .fetch_one(&self.pool)
            .await?;
        if !exists {
            return Err(PlatformError::NotFound("scene not found".into()));
        }
        Ok(())
    }

    /// Every scene `user_id` belongs to — the third subject dimension
    /// `effective_resource_ids` folds in alongside their own grants and
    /// their department ancestry.
    async fn scene_ids_for_member(&self, tenant_id: &str, user_id: &str) -> Result<Vec<String>, PlatformError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT scene_id FROM one_scene_members WHERE tenant_id = ? AND user_id = ?")
                .bind(tenant_id)
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    // --- Security policy baseline (E5) ---

    pub async fn get_security_policy(&self, tenant_id: &str) -> Result<SecurityPolicyDto, PlatformError> {
        let row: Option<SecurityPolicyRow> = sqlx::query_as(
            "SELECT tier, terminal_tools_require_approval, destructive_commands_blocked, blocked_command_patterns, \
                    external_network_denied_by_default, message_scan_enabled, message_redact_enabled, \
                    send_rate_limit_per_minute, updated_at \
             FROM one_security_policy WHERE tenant_id = ?",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match row {
            Some(row) => Self::security_policy_row_to_dto(row),
            // No row yet = the 'relaxed' tier's values, same convention as
            // `get_container_config`/`get_siem_config` reporting "off" when
            // nothing was ever saved.
            None => Self::security_policy_preset("relaxed").expect("relaxed is a known tier"),
        })
    }

    fn security_policy_row_to_dto(row: SecurityPolicyRow) -> SecurityPolicyDto {
        SecurityPolicyDto {
            tier: row.0,
            terminal_tools_require_approval: row.1,
            destructive_commands_blocked: row.2,
            blocked_command_patterns: serde_json::from_str(&row.3).unwrap_or_default(),
            external_network_denied_by_default: row.4,
            message_scan_enabled: row.5,
            message_redact_enabled: row.6,
            send_rate_limit_per_minute: row.7,
            updated_at: Some(row.8),
        }
    }

    /// The 3 reference-product tiers' field values. `None` for an unknown
    /// tier name (validated by callers before use).
    ///
    /// 宽松/relaxed: every check off, matches "quick verification" in the
    /// reference product. 标准/standard: terminal tools need approval,
    /// destructive commands blocked, a moderate send rate limit. 严格/strict:
    /// standard's checks plus external network denied by default and DLP
    /// message scan/redact on, tighter rate limit. The specific rate-limit
    /// numbers (30/min, 20/min) are this crate's own reasonable defaults —
    /// the reference product's own copy names the *categories* it limits,
    /// not specific numbers — and are editable per tenant via
    /// `set_security_policy` same as every other field.
    fn security_policy_preset(tier: &str) -> Option<SecurityPolicyDto> {
        let dto = match tier {
            "relaxed" => SecurityPolicyDto {
                tier: "relaxed".to_owned(),
                terminal_tools_require_approval: false,
                destructive_commands_blocked: false,
                blocked_command_patterns: Vec::new(),
                external_network_denied_by_default: false,
                message_scan_enabled: false,
                message_redact_enabled: false,
                send_rate_limit_per_minute: None,
                updated_at: None,
            },
            "standard" => SecurityPolicyDto {
                tier: "standard".to_owned(),
                terminal_tools_require_approval: true,
                destructive_commands_blocked: true,
                blocked_command_patterns: Self::default_blocked_command_patterns(),
                external_network_denied_by_default: false,
                message_scan_enabled: false,
                message_redact_enabled: false,
                send_rate_limit_per_minute: Some(30),
                updated_at: None,
            },
            "strict" => SecurityPolicyDto {
                tier: "strict".to_owned(),
                terminal_tools_require_approval: true,
                destructive_commands_blocked: true,
                blocked_command_patterns: Self::default_blocked_command_patterns(),
                external_network_denied_by_default: true,
                message_scan_enabled: true,
                message_redact_enabled: true,
                send_rate_limit_per_minute: Some(20),
                updated_at: None,
            },
            _ => return None,
        };
        Some(dto)
    }

    fn default_blocked_command_patterns() -> Vec<String> {
        ["rm -rf", "shutdown", "mkfs", "sudo", "kubectl delete"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Apply one of the 3 built-in tiers wholesale, overwriting every field
    /// with that tier's preset. For a field-by-field override, use
    /// `set_security_policy` instead — that one sets `tier = "custom"`.
    pub async fn apply_security_policy_tier(
        &self,
        tenant_id: &str,
        tier: &str,
    ) -> Result<SecurityPolicyDto, PlatformError> {
        let preset = Self::security_policy_preset(tier)
            .ok_or_else(|| PlatformError::BadRequest(format!("unknown tier '{tier}'")))?;
        self.upsert_security_policy(tenant_id, &preset).await
    }

    /// Field-by-field override. Always stores `tier = "custom"` — even when
    /// the resulting fields happen to match a preset exactly, because the
    /// point of `tier` is "how did this baseline get here", and a hand-edit
    /// is not the same provenance as `apply_security_policy_tier` even if it
    /// lands on the same values. An admin who wants the tier label back picks
    /// the tier explicitly.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_security_policy(
        &self,
        tenant_id: &str,
        terminal_tools_require_approval: bool,
        destructive_commands_blocked: bool,
        blocked_command_patterns: &[String],
        external_network_denied_by_default: bool,
        message_scan_enabled: bool,
        message_redact_enabled: bool,
        send_rate_limit_per_minute: Option<i64>,
    ) -> Result<SecurityPolicyDto, PlatformError> {
        let dto = SecurityPolicyDto {
            tier: "custom".to_owned(),
            terminal_tools_require_approval,
            destructive_commands_blocked,
            blocked_command_patterns: blocked_command_patterns.to_vec(),
            external_network_denied_by_default,
            message_scan_enabled,
            message_redact_enabled,
            send_rate_limit_per_minute,
            updated_at: None,
        };
        self.upsert_security_policy(tenant_id, &dto).await
    }

    async fn upsert_security_policy(
        &self,
        tenant_id: &str,
        dto: &SecurityPolicyDto,
    ) -> Result<SecurityPolicyDto, PlatformError> {
        let patterns_json =
            serde_json::to_string(&dto.blocked_command_patterns).map_err(|e| PlatformError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO one_security_policy \
                 (tenant_id, tier, terminal_tools_require_approval, destructive_commands_blocked, \
                  blocked_command_patterns, external_network_denied_by_default, message_scan_enabled, \
                  message_redact_enabled, send_rate_limit_per_minute, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(tenant_id) DO UPDATE SET \
                 tier = excluded.tier, \
                 terminal_tools_require_approval = excluded.terminal_tools_require_approval, \
                 destructive_commands_blocked = excluded.destructive_commands_blocked, \
                 blocked_command_patterns = excluded.blocked_command_patterns, \
                 external_network_denied_by_default = excluded.external_network_denied_by_default, \
                 message_scan_enabled = excluded.message_scan_enabled, \
                 message_redact_enabled = excluded.message_redact_enabled, \
                 send_rate_limit_per_minute = excluded.send_rate_limit_per_minute, \
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(&dto.tier)
        .bind(dto.terminal_tools_require_approval)
        .bind(dto.destructive_commands_blocked)
        .bind(&patterns_json)
        .bind(dto.external_network_denied_by_default)
        .bind(dto.message_scan_enabled)
        .bind(dto.message_redact_enabled)
        .bind(dto.send_rate_limit_per_minute)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        self.get_security_policy(tenant_id).await
    }

    // --- Open-integration API keys (E5) ---

    /// Shown chars of the plaintext secret kept as `key_prefix`, so the admin
    /// console can tell keys apart ("sk_live_ab12…") without ever
    /// re-displaying the full secret.
    const API_KEY_PREFIX_LEN: usize = 12;

    fn hash_api_key_secret(secret: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Mint a new key. The plaintext `secret` is generated here, hashed for
    /// storage, and returned exactly once in `NewApiKeyDto` — this is the
    /// only call in this crate that ever sees it in the clear.
    ///
    /// Authenticating a request against `one_api_keys` is [`Self::authenticate_api_key`],
    /// consulted from `dream-core-auth`'s middleware via the `ApiKeyGate` port
    /// (`dream-core-auth` cannot depend on this crate directly).
    pub async fn create_api_key(
        &self,
        tenant_id: &str,
        name: &str,
        allowed_paths: &[String],
        rate_limit_per_minute: Option<i64>,
        created_by: &str,
    ) -> Result<NewApiKeyDto, PlatformError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PlatformError::BadRequest("API key name must not be empty".into()));
        }

        let id = generate_prefixed_id("apikey");
        let secret = format!("{API_KEY_TOKEN_PREFIX}{}", generate_id_with_length(Some(48)));
        let key_prefix: String = secret.chars().take(Self::API_KEY_PREFIX_LEN).collect();
        let key_hash = Self::hash_api_key_secret(&secret);
        let allowed_paths_json =
            serde_json::to_string(allowed_paths).map_err(|e| PlatformError::Internal(e.to_string()))?;
        let now = now_ms();

        sqlx::query(
            "INSERT INTO one_api_keys \
                 (id, tenant_id, name, key_prefix, key_hash, allowed_paths, rate_limit_per_minute, status, \
                  created_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(name)
        .bind(&key_prefix)
        .bind(&key_hash)
        .bind(&allowed_paths_json)
        .bind(rate_limit_per_minute)
        .bind(created_by)
        .bind(now)
        .execute(&self.pool)
        .await?;

        let key = self
            .get_api_key(tenant_id, &id)
            .await?
            .ok_or_else(|| PlatformError::Internal("API key vanished immediately after insert".into()))?;
        Ok(NewApiKeyDto { key, secret })
    }

    /// Validate a raw bearer token against `one_api_keys` and, if it
    /// authorizes `request_path`, resolve the user to act as.
    ///
    /// The resolved identity is the key's `created_by` (the admin who minted
    /// it) — an API key acts AS a real user, the same way a JWT session does,
    /// so every downstream tenant/permission lookup that already keys off
    /// `user_id` (resource grants, `TenantResolver`, …) needs no new plumbing.
    ///
    /// Looked up by hash across all tenants (not scoped by `tenant_id`,
    /// unlike every other method here) because the caller doesn't know its
    /// tenant in advance — the hash itself, a SHA-256 of 48 random hex
    /// characters, is the credential's effective unique identifier.
    pub async fn authenticate_api_key(
        &self,
        secret: &str,
        request_path: &str,
    ) -> Result<ApiKeyAuthOutcome, PlatformError> {
        let key_hash = Self::hash_api_key_secret(secret);
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT id, created_by, allowed_paths FROM one_api_keys WHERE key_hash = ? AND status = 'active'",
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        let Some((id, created_by, allowed_paths_json)) = row else {
            return Ok(ApiKeyAuthOutcome::Invalid);
        };

        let allowed_paths: Vec<String> = serde_json::from_str(&allowed_paths_json).unwrap_or_default();
        if !api_key_path_allowed(request_path, &allowed_paths) {
            return Ok(ApiKeyAuthOutcome::PathNotAllowed);
        }

        sqlx::query("UPDATE one_api_keys SET last_used_at = ? WHERE id = ?")
            .bind(now_ms())
            .bind(&id)
            .execute(&self.pool)
            .await?;

        Ok(ApiKeyAuthOutcome::Authenticated { user_id: created_by })
    }

    fn api_key_row_to_dto(row: ApiKeyRow) -> ApiKeyDto {
        ApiKeyDto {
            id: row.0,
            name: row.1,
            key_prefix: row.2,
            allowed_paths: serde_json::from_str(&row.3).unwrap_or_default(),
            rate_limit_per_minute: row.4,
            status: row.5,
            created_by: row.6,
            created_at: row.7,
            revoked_at: row.8,
            last_used_at: row.9,
        }
    }

    async fn get_api_key(&self, tenant_id: &str, id: &str) -> Result<Option<ApiKeyDto>, PlatformError> {
        let row: Option<ApiKeyRow> = sqlx::query_as(
            "SELECT id, name, key_prefix, allowed_paths, rate_limit_per_minute, status, created_by, created_at, \
                    revoked_at, last_used_at \
             FROM one_api_keys WHERE tenant_id = ? AND id = ?",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Self::api_key_row_to_dto))
    }

    pub async fn list_api_keys(&self, tenant_id: &str) -> Result<Vec<ApiKeyDto>, PlatformError> {
        let rows: Vec<ApiKeyRow> = sqlx::query_as(
            "SELECT id, name, key_prefix, allowed_paths, rate_limit_per_minute, status, created_by, created_at, \
                    revoked_at, last_used_at \
             FROM one_api_keys WHERE tenant_id = ? ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Self::api_key_row_to_dto).collect())
    }

    /// Revoke every active key a user minted, across all tenants. Called when
    /// that user is removed from a company (via one-org's `CredentialRevoker`
    /// seam), because an API key is a bearer credential with no session
    /// generation to rotate: `invalidate_user_tokens` kills the leaver's JWTs
    /// but their key would keep authenticating as them indefinitely, since
    /// `authenticate_api_key` only checks `status = 'active'` and the user row
    /// itself is untouched by removal. Same hazard model channel tokens have,
    /// and the same answer.
    ///
    /// Returns the number of keys revoked so the caller can log it.
    pub async fn revoke_api_keys_for_user(&self, user_id: &str) -> Result<u64, PlatformError> {
        let result = sqlx::query(
            "UPDATE one_api_keys SET status = 'revoked', revoked_at = ? \
             WHERE created_by = ? AND status = 'active'",
        )
        .bind(now_ms())
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Idempotent: revoking an already-revoked key is not an error, same
    /// posture as `grant_resource`/`add_scene_member`. Only a key that never
    /// existed for this tenant is `NotFound`.
    pub async fn revoke_api_key(&self, tenant_id: &str, id: &str) -> Result<(), PlatformError> {
        let current: Option<(String,)> =
            sqlx::query_as("SELECT status FROM one_api_keys WHERE tenant_id = ? AND id = ?")
                .bind(tenant_id)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        match current {
            None => Err(PlatformError::NotFound("API key not found".into())),
            Some((status,)) if status == "revoked" => Ok(()),
            Some(_) => {
                sqlx::query(
                    "UPDATE one_api_keys SET status = 'revoked', revoked_at = ? WHERE tenant_id = ? AND id = ?",
                )
                .bind(now_ms())
                .bind(tenant_id)
                .bind(id)
                .execute(&self.pool)
                .await?;
                Ok(())
            }
        }
    }

    // --- In-app notifications (P2-3 站内消息) ---

    /// Compose and send one notification. A `broadcast` reaches every member
    /// of the tenant (now and in the future — see the migration's comment for
    /// why there are no recipient rows); a `targeted` one only the listed
    /// users, every one of which must already be a member of this tenant.
    pub async fn create_notification(
        &self,
        tenant_id: &str,
        kind: &str,
        category: &str,
        title: &str,
        body: &str,
        recipient_ids: &[String],
        created_by: &str,
    ) -> Result<NotificationDto, PlatformError> {
        let kind = match kind {
            "broadcast" | "targeted" => kind,
            other => {
                return Err(PlatformError::BadRequest(format!(
                    "unknown notification kind '{other}'"
                )));
            }
        };
        let title = title.trim();
        let body = body.trim();
        if title.is_empty() {
            return Err(PlatformError::BadRequest("notification title must not be empty".into()));
        }
        if body.is_empty() {
            return Err(PlatformError::BadRequest("notification body must not be empty".into()));
        }

        let id = generate_prefixed_id("ntf");
        let now = now_ms();
        if kind == "targeted" {
            if recipient_ids.is_empty() {
                return Err(PlatformError::BadRequest(
                    "a targeted notification requires at least one recipient".into(),
                ));
            }
            let mut tx = self.pool.begin().await?;
            // Reject (rather than silently drop) recipients that are not
            // members — an admin told "sent" while half the list silently
            // received nothing would be a fake success, the exact failure
            // mode delivery-gaps T4 was about.
            for user_id in recipient_ids {
                let member: Option<(String,)> =
                    sqlx::query_as("SELECT user_id FROM one_user_org WHERE tenant_id = ? AND user_id = ?")
                        .bind(tenant_id)
                        .bind(user_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                if member.is_none() {
                    return Err(PlatformError::BadRequest(format!(
                        "recipient '{user_id}' is not a member of this tenant"
                    )));
                }
            }
            sqlx::query(
                "INSERT INTO one_notifications (id, tenant_id, kind, category, title, body, created_by, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(tenant_id)
            .bind(kind)
            .bind(category.trim())
            .bind(title)
            .bind(body)
            .bind(created_by)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            for user_id in recipient_ids {
                sqlx::query(
                    "INSERT OR IGNORE INTO one_notification_recipients (notification_id, user_id) VALUES (?, ?)",
                )
                .bind(&id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
        } else {
            sqlx::query(
                "INSERT INTO one_notifications (id, tenant_id, kind, category, title, body, created_by, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(tenant_id)
            .bind(kind)
            .bind(category.trim())
            .bind(title)
            .bind(body)
            .bind(created_by)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }

        self.get_notification(tenant_id, &id)
            .await?
            .ok_or_else(|| PlatformError::Internal("notification vanished immediately after insert".into()))
    }

    async fn get_notification(&self, tenant_id: &str, id: &str) -> Result<Option<NotificationDto>, PlatformError> {
        let row: Option<(String, String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, kind, category, title, body, created_by, created_at \
             FROM one_notifications WHERE tenant_id = ? AND id = ?",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, kind, category, title, body, created_by, created_at)) = row else {
            return Ok(None);
        };
        let (recipient_count, read_count) = self.notification_audience_counts(tenant_id, &id, &kind).await?;
        Ok(Some(NotificationDto {
            id,
            kind,
            category,
            title,
            body,
            recipient_count,
            read_count,
            created_by,
            created_at,
        }))
    }

    /// Audience size and how much of it has read one notification. A
    /// broadcast's audience is the tenant's *current* roster (members who
    /// joined after the send are part of it, by the same rule that shows the
    /// notification to them); a targeted one's audience is its recipient rows.
    async fn notification_audience_counts(
        &self,
        tenant_id: &str,
        id: &str,
        kind: &str,
    ) -> Result<(i64, i64), PlatformError> {
        let recipient_count: i64 = if kind == "broadcast" {
            sqlx::query_scalar("SELECT COUNT(*) FROM one_user_org WHERE tenant_id = ?")
                .bind(tenant_id)
                .fetch_one(&self.pool)
                .await?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM one_notification_recipients WHERE notification_id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?
        };
        let read_count: i64 = if kind == "broadcast" {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM one_notification_reads rd \
                 JOIN one_user_org uo ON uo.user_id = rd.user_id AND uo.tenant_id = ? \
                 WHERE rd.notification_id = ?",
            )
            .bind(tenant_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM one_notification_reads rd \
                 JOIN one_notification_recipients r ON r.notification_id = rd.notification_id AND r.user_id = rd.user_id \
                 WHERE rd.notification_id = ?",
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await?
        };
        Ok((recipient_count, read_count))
    }

    /// The admin's sent history, newest first.
    pub async fn list_notifications(&self, tenant_id: &str) -> Result<Vec<NotificationDto>, PlatformError> {
        let rows: Vec<(String, String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, kind, category, title, body, created_by, created_at \
             FROM one_notifications WHERE tenant_id = ? ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, kind, category, title, body, created_by, created_at) in rows {
            let (recipient_count, read_count) = self.notification_audience_counts(tenant_id, &id, &kind).await?;
            out.push(NotificationDto {
                id,
                kind,
                category,
                title,
                body,
                recipient_count,
                read_count,
                created_by,
                created_at,
            });
        }
        Ok(out)
    }

    /// Withdraw a sent notification. Removes the message and its recipient
    /// rows; read rows for it disappear with the message (they reference its
    /// id and nothing else), which is the intent — the notification no
    /// longer exists for anyone.
    pub async fn delete_notification(&self, tenant_id: &str, id: &str) -> Result<(), PlatformError> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query("DELETE FROM one_notifications WHERE tenant_id = ? AND id = ?")
            .bind(tenant_id)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(PlatformError::NotFound("notification not found".into()));
        }
        sqlx::query("DELETE FROM one_notification_recipients WHERE notification_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM one_notification_reads WHERE notification_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The calling member's inbox: every notification of their active tenant
    /// addressed to them (all broadcasts + targeted ones listing them),
    /// newest first, with their personal read stamp and the unread count the
    /// home page's to-do card wants. `limit` caps the returned rows only —
    /// `unread_count` always counts the full visible set.
    pub async fn list_my_notifications(
        &self,
        tenant_id: &str,
        user_id: &str,
        limit: i64,
    ) -> Result<MyNotificationsDto, PlatformError> {
        let rows: Vec<(String, String, String, String, String, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT n.id, n.kind, n.category, n.title, n.body, n.created_by, n.created_at, rd.read_at \
             FROM one_notifications n \
             LEFT JOIN one_notification_reads rd ON rd.notification_id = n.id AND rd.user_id = ? \
             WHERE n.tenant_id = ? \
               AND (n.kind = 'broadcast' \
                    OR EXISTS (SELECT 1 FROM one_notification_recipients r \
                               WHERE r.notification_id = n.id AND r.user_id = ?)) \
             ORDER BY n.created_at DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let unread_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM one_notifications n \
             WHERE n.tenant_id = ? \
               AND (n.kind = 'broadcast' \
                    OR EXISTS (SELECT 1 FROM one_notification_recipients r \
                               WHERE r.notification_id = n.id AND r.user_id = ?)) \
               AND NOT EXISTS (SELECT 1 FROM one_notification_reads rd \
                               WHERE rd.notification_id = n.id AND rd.user_id = ?)",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(MyNotificationsDto {
            notifications: rows
                .into_iter()
                .map(
                    |(id, kind, category, title, body, created_by, created_at, read_at)| MyNotificationDto {
                        id,
                        kind,
                        category,
                        title,
                        body,
                        created_by,
                        created_at,
                        read_at,
                    },
                )
                .collect(),
            unread_count,
        })
    }

    /// Mark notifications read for one member. An empty `ids` marks every
    /// visible unread one — the inbox's "mark all read". Ids the caller
    /// cannot see (another tenant's, a targeted one not addressed to them)
    /// are silently skipped rather than erroring, same posture as the other
    /// idempotent mutations in this service.
    pub async fn mark_notifications_read(
        &self,
        tenant_id: &str,
        user_id: &str,
        ids: &[String],
    ) -> Result<(), PlatformError> {
        let visibility = "n.tenant_id = ? \
             AND (n.kind = 'broadcast' \
                  OR EXISTS (SELECT 1 FROM one_notification_recipients r \
                             WHERE r.notification_id = n.id AND r.user_id = ?))";
        if ids.is_empty() {
            sqlx::query(&format!(
                "INSERT OR IGNORE INTO one_notification_reads (notification_id, user_id, read_at) \
                 SELECT n.id, ?, ? FROM one_notifications n WHERE {visibility} \
                   AND NOT EXISTS (SELECT 1 FROM one_notification_reads rd \
                                   WHERE rd.notification_id = n.id AND rd.user_id = ?)"
            ))
            .bind(user_id)
            .bind(now_ms())
            .bind(tenant_id)
            .bind(user_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
            return Ok(());
        }
        for id in ids {
            let visible: Option<(String,)> = sqlx::query_as(&format!(
                "SELECT n.id FROM one_notifications n WHERE n.id = ? AND {visibility}"
            ))
            .bind(id)
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
            if visible.is_some() {
                sqlx::query(
                    "INSERT OR IGNORE INTO one_notification_reads (notification_id, user_id, read_at) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(user_id)
                .bind(now_ms())
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    // --- Personal file vault (P2-4 个人文件仓库) ---

    /// Vault state for one member. No settings row = available / unlimited —
    /// the row is created lazily on first freeze/quota touch (see the
    /// migration's comment), so absence must not read as frozen.
    async fn vault_settings(&self, tenant_id: &str, user_id: &str) -> Result<(String, Option<i64>), PlatformError> {
        let row: Option<(String, Option<i64>)> = sqlx::query_as(
            "SELECT status, quota_bytes FROM one_file_vault_settings WHERE tenant_id = ? AND user_id = ?",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.unwrap_or_else(|| ("available".to_owned(), None)))
    }

    /// One member's own vault view: status, quota, usage, object count.
    pub async fn my_vault(&self, tenant_id: &str, user_id: &str) -> Result<FileVaultDto, PlatformError> {
        let (status, quota_bytes) = self.vault_settings(tenant_id, user_id).await?;
        let (usage_bytes, object_count): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*) FROM one_file_vault_objects \
             WHERE tenant_id = ? AND user_id = ? AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(FileVaultDto {
            user_id: user_id.to_owned(),
            status,
            quota_bytes,
            usage_bytes,
            object_count,
        })
    }

    fn vault_user_dir(&self, tenant_id: &str, user_id: &str) -> Result<std::path::PathBuf, PlatformError> {
        let root = self
            .storage_root
            .as_ref()
            .ok_or_else(|| PlatformError::Internal("file vault storage root is not configured".into()))?;
        Ok(root.join(tenant_id).join(user_id))
    }

    /// Store one uploaded object. Refuses a frozen vault (compliance hold on
    /// new data — existing objects stay readable and deletable) and a full
    /// quota; the disk write happens after the ledger insert would have been
    /// validated, and a failed disk write aborts the whole call so the
    /// ledger and the bytes never disagree from this path.
    pub async fn upload_vault_object(
        &self,
        tenant_id: &str,
        user_id: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<FileVaultObjectDto, PlatformError> {
        let (status, quota_bytes) = self.vault_settings(tenant_id, user_id).await?;
        if status == "frozen" {
            return Err(PlatformError::Forbidden("file vault is frozen".into()));
        }
        let (usage_bytes,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM one_file_vault_objects \
             WHERE tenant_id = ? AND user_id = ? AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        if let Some(quota) = quota_bytes {
            if usage_bytes + bytes.len() as i64 > quota {
                return Err(PlatformError::BadRequest("file vault quota exceeded".into()));
            }
        }
        let file_name = sanitize_vault_file_name(file_name);
        if file_name.is_empty() {
            return Err(PlatformError::BadRequest("file name must not be empty".into()));
        }

        let id = generate_prefixed_id("vf");
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let storage_name = format!("{id}__{file_name}");
        let dir = self.vault_user_dir(tenant_id, user_id)?;
        let storage_key = format!("{tenant_id}/{user_id}/{storage_name}");

        sqlx::query(
            "INSERT INTO one_file_vault_objects (id, tenant_id, user_id, file_name, size_bytes, sha256, storage_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(&file_name)
        .bind(bytes.len() as i64)
        .bind(&sha256)
        .bind(&storage_key)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;

        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&storage_name), bytes)) {
            // The ledger row must not outlive its bytes: roll the row back so
            // usage stays truthful for the reconcile pass.
            let _ = sqlx::query("DELETE FROM one_file_vault_objects WHERE id = ?")
                .bind(&id)
                .execute(&self.pool)
                .await;
            return Err(PlatformError::Internal(format!("failed to store vault object: {e}")));
        }

        Ok(FileVaultObjectDto {
            id,
            file_name,
            size_bytes: bytes.len() as i64,
            sha256,
            created_at: now_ms(),
            deleted_at: None,
        })
    }

    /// The caller's own stored objects, newest first, including tombstones
    /// (they are the owner's audit trail).
    pub async fn list_my_vault_objects(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Vec<FileVaultObjectDto>, PlatformError> {
        self.list_vault_objects(tenant_id, user_id).await
    }

    async fn list_vault_objects(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Vec<FileVaultObjectDto>, PlatformError> {
        let rows: Vec<(String, String, i64, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, file_name, size_bytes, sha256, created_at, deleted_at \
             FROM one_file_vault_objects WHERE tenant_id = ? AND user_id = ? ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, file_name, size_bytes, sha256, created_at, deleted_at)| FileVaultObjectDto {
                    id,
                    file_name,
                    size_bytes,
                    sha256,
                    created_at,
                    deleted_at,
                },
            )
            .collect())
    }

    /// Resolve one object for download. Only the owner (or a tenant admin —
    /// separate admin route) may read it; the storage path is joined from the
    /// ledger row's stored key, never from request input.
    pub async fn read_vault_object(
        &self,
        tenant_id: &str,
        user_id: &str,
        id: &str,
    ) -> Result<(FileVaultObjectDto, Vec<u8>), PlatformError> {
        let row: Option<(String, i64, String, String, i64)> = sqlx::query_as(
            "SELECT file_name, size_bytes, storage_key, sha256, created_at FROM one_file_vault_objects \
             WHERE tenant_id = ? AND user_id = ? AND id = ? AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((file_name, size_bytes, storage_key, sha256, created_at)) = row else {
            return Err(PlatformError::NotFound("vault object not found".into()));
        };
        let root = self
            .storage_root
            .as_ref()
            .ok_or_else(|| PlatformError::Internal("file vault storage root is not configured".into()))?;
        let bytes = std::fs::read(root.join(&storage_key))
            .map_err(|e| PlatformError::Internal(format!("failed to read vault object: {e}")))?;
        Ok((
            FileVaultObjectDto {
                id: id.to_owned(),
                file_name,
                size_bytes,
                sha256,
                created_at,
                deleted_at: None,
            },
            bytes,
        ))
    }

    /// Owner deletes one of their own objects: tombstone the row, remove the
    /// bytes. A failed disk removal still tombstones — the reconcile pass
    /// reports leftovers, and a delete that 500s forever because a file
    /// vanished out-of-band would be worse.
    pub async fn delete_vault_object(&self, tenant_id: &str, user_id: &str, id: &str) -> Result<(), PlatformError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT storage_key FROM one_file_vault_objects \
             WHERE tenant_id = ? AND user_id = ? AND id = ? AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((storage_key,)) = row else {
            return Err(PlatformError::NotFound("vault object not found".into()));
        };
        sqlx::query("UPDATE one_file_vault_objects SET deleted_at = ? WHERE id = ?")
            .bind(now_ms())
            .bind(id)
            .execute(&self.pool)
            .await?;
        if let Some(root) = self.storage_root.as_ref() {
            let _ = std::fs::remove_file(root.join(&storage_key));
        }
        Ok(())
    }

    /// Every member's vault for the admin governance page: the union of
    /// current members and anyone who ever stored an object (a departed
    /// member's frozen vault with files still shows up — that is exactly the
    /// row an admin needs to see when offboarding).
    pub async fn admin_list_vaults(&self, tenant_id: &str) -> Result<Vec<FileVaultDto>, PlatformError> {
        let rows: Vec<(String, Option<String>, Option<i64>, i64, i64)> = sqlx::query_as(
            "SELECT u.user_id, s.status, s.quota_bytes, COALESCE(o.usage_bytes, 0), COALESCE(o.object_count, 0) \
             FROM (SELECT user_id FROM one_user_org WHERE tenant_id = ? \
                   UNION SELECT DISTINCT user_id FROM one_file_vault_objects WHERE tenant_id = ?) u \
             LEFT JOIN one_file_vault_settings s ON s.tenant_id = ? AND s.user_id = u.user_id \
             LEFT JOIN (SELECT user_id, SUM(size_bytes) AS usage_bytes, COUNT(*) AS object_count \
                        FROM one_file_vault_objects WHERE tenant_id = ? AND deleted_at IS NULL GROUP BY user_id) o \
                      ON o.user_id = u.user_id \
             ORDER BY u.user_id",
        )
        .bind(tenant_id)
        .bind(tenant_id)
        .bind(tenant_id)
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(user_id, status, quota_bytes, usage_bytes, object_count)| FileVaultDto {
                    user_id,
                    status: status.unwrap_or_else(|| "available".to_owned()),
                    quota_bytes,
                    usage_bytes,
                    object_count,
                },
            )
            .collect())
    }

    /// Freeze or release one member's vault. Freezing blocks new uploads
    /// only — see the migration's comment for why existing objects stay
    /// readable and deletable.
    pub async fn admin_set_vault_status(
        &self,
        tenant_id: &str,
        user_id: &str,
        status: &str,
    ) -> Result<(), PlatformError> {
        if !matches!(status, "available" | "frozen") {
            return Err(PlatformError::BadRequest(format!("unknown vault status '{status}'")));
        }
        sqlx::query(
            "INSERT INTO one_file_vault_settings (tenant_id, user_id, status, quota_bytes, updated_at) \
             VALUES (?, ?, ?, NULL, ?) \
             ON CONFLICT(tenant_id, user_id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(status)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set (or clear, `None`) one member's vault quota. Passes an explicit
    /// status so a quota edit on a frozen vault does not silently unfreeze it.
    pub async fn admin_set_vault_quota(
        &self,
        tenant_id: &str,
        user_id: &str,
        quota_bytes: Option<i64>,
    ) -> Result<(), PlatformError> {
        if let Some(q) = quota_bytes {
            if q < 0 {
                return Err(PlatformError::BadRequest("quota must not be negative".into()));
            }
        }
        let (status, _) = self.vault_settings(tenant_id, user_id).await?;
        sqlx::query(
            "INSERT INTO one_file_vault_settings (tenant_id, user_id, status, quota_bytes, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(tenant_id, user_id) DO UPDATE SET quota_bytes = excluded.quota_bytes, \
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(status)
        .bind(quota_bytes)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The reconcile pass ("对账"): walk the tenant's storage directories and
    /// compare what is on disk against the ledger. Three mismatch classes per
    /// member — ledger rows missing bytes, bytes missing a ledger row, and
    /// rows whose on-disk size disagrees. A not-configured storage root is an
    /// error, not an empty report: an admin asking for reconciliation must
    /// not be told "everything matches" when nothing was checked.
    pub async fn admin_reconcile_vaults(&self, tenant_id: &str) -> Result<Vec<FileVaultReconcileEntry>, PlatformError> {
        let root = self
            .storage_root
            .as_ref()
            .ok_or_else(|| PlatformError::Internal("file vault storage root is not configured".into()))?;
        let tenant_dir = root.join(tenant_id);
        let mut entries: Vec<FileVaultReconcileEntry> = Vec::new();

        // Ledger side, grouped by member: id + file_name + size per live row.
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT user_id, id, file_name, size_bytes FROM one_file_vault_objects \
             WHERE tenant_id = ? AND deleted_at IS NULL ORDER BY user_id, id",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        let mut ledger: std::collections::BTreeMap<String, Vec<(String, String, i64)>> = Default::default();
        for (user_id, id, file_name, size_bytes) in rows {
            ledger.entry(user_id).or_default().push((id, file_name, size_bytes));
        }

        // Disk side: `<root>/<tenant>/<user>/<object_id>__<name>`.
        let mut on_disk: std::collections::BTreeMap<String, Vec<(String, u64)>> = Default::default();
        if let Ok(user_dirs) = std::fs::read_dir(&tenant_dir) {
            for user_dir in user_dirs.flatten() {
                let user_id = user_dir.file_name().to_string_lossy().to_string();
                if let Ok(files) = std::fs::read_dir(user_dir.path()) {
                    for file in files.flatten() {
                        let Ok(meta) = file.metadata() else { continue };
                        if !meta.is_file() {
                            continue;
                        }
                        let name = file.file_name().to_string_lossy().to_string();
                        let object_id = name.split("__").next().unwrap_or(&name).to_owned();
                        on_disk
                            .entry(user_id.clone())
                            .or_default()
                            .push((object_id, meta.len()));
                    }
                }
            }
        }

        for user_id in ledger
            .keys()
            .chain(on_disk.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let ledger_rows = ledger.get(user_id).cloned().unwrap_or_default();
            let disk_files = on_disk.get(user_id).cloned().unwrap_or_default();
            let mut entry = FileVaultReconcileEntry {
                user_id: user_id.to_owned(),
                missing_on_disk: Vec::new(),
                missing_in_ledger: Vec::new(),
                size_mismatches: Vec::new(),
            };
            for (id, file_name, size_bytes) in &ledger_rows {
                match disk_files.iter().find(|(disk_id, _)| disk_id == id) {
                    None => entry.missing_on_disk.push(format!("{file_name} ({id})")),
                    Some((_, disk_size)) if *disk_size != *size_bytes as u64 => {
                        entry.size_mismatches.push(format!("{file_name} ({id})"));
                    }
                    Some(_) => {}
                }
            }
            for (disk_id, _) in &disk_files {
                if !ledger_rows.iter().any(|(id, _, _)| id == disk_id) {
                    entry.missing_in_ledger.push(disk_id.clone());
                }
            }
            if !entry.missing_on_disk.is_empty()
                || !entry.missing_in_ledger.is_empty()
                || !entry.size_mismatches.is_empty()
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Admin listing of one member's stored objects (including tombstones).
    pub async fn admin_list_vault_objects(
        &self,
        tenant_id: &str,
        user_id: &str,
    ) -> Result<Vec<FileVaultObjectDto>, PlatformError> {
        self.list_vault_objects(tenant_id, user_id).await
    }
}

/// Keep only the final path component of an uploaded file name and strip
/// characters that make the storage name unpleasant or unsafe. The storage
/// name is `<object_id>__<sanitized>` under an id-scoped directory, so this
/// is defense in depth — the directory layout already prevents traversal.
fn sanitize_vault_file_name(raw: &str) -> String {
    let name = std::path::Path::new(raw)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

type ApiKeyRow = (
    String,
    String,
    String,
    String,
    Option<i64>,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
);

/// Result of [`PlatformService::authenticate_api_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyAuthOutcome {
    /// No active key matches this secret's hash — an invalid or revoked
    /// credential. Deliberately not distinguished from "revoked" to the
    /// caller: both mean "this token does not authenticate".
    Invalid,
    /// The key is valid but its `allowed_paths` does not cover the request.
    PathNotAllowed,
    /// The key is valid and authorizes this path; resolves to `created_by`.
    Authenticated { user_id: String },
}

/// Whether `request_path` is covered by any pattern in `allowed_paths`.
/// Patterns are either an exact path or a `*`-suffixed prefix (the only
/// shape `PlatformService::create_api_key` accepts admins entering, e.g.
/// `/api/one/devops/*`). An empty `allowed_paths` list matches nothing —
/// see `one_api_keys`'s migration comment: a key with no paths scoped in
/// must never be mistaken for an unrestricted one.
fn api_key_path_allowed(request_path: &str, allowed_paths: &[String]) -> bool {
    allowed_paths.iter().any(|pattern| match pattern.strip_suffix('*') {
        Some(prefix) => request_path.starts_with(prefix),
        None => request_path == pattern,
    })
}

type SecurityPolicyRow = (String, bool, bool, String, bool, bool, bool, Option<i64>, i64);

type SceneRow = (String, String, String, Option<String>, String, bool, i64, i64);

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

    /// The "知识库" half of E5's "知识与记忆治理" item: RAG documents already
    /// exist (`dream_domain_devops::RagDocumentDto`) and only needed a new
    /// `resource_type` value to be grantable, not a new registry.
    #[tokio::test]
    async fn knowledge_is_a_valid_grant_resource_type() {
        let (_db, service) = setup().await;
        let grant = service
            .grant_resource("t1", "member", "alice", "knowledge", "doc_1", "admin1")
            .await
            .unwrap();
        assert_eq!(grant.resource_type, "knowledge");
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

    /// `employee` used to silently pass validation, write a row, and return
    /// 200 while nothing ever consulted it — worse than not having the
    /// feature. It must now be rejected with a message distinguishable from
    /// the generic "unknown resource type" (the type isn't unknown, it's
    /// deliberately unsupported), and the remaining four types must keep
    /// working exactly as before.
    #[tokio::test]
    async fn grant_resource_rejects_employee_with_a_distinct_message() {
        let (_db, service) = setup().await;
        let err = service
            .grant_resource("t1", "member", "alice", "employee", "emp_1", "admin1")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "BAD_REQUEST");
        assert_eq!(
            err.to_string(),
            "Bad request: resource type 'employee' is not supported yet"
        );
        assert_ne!(
            err.to_string(),
            "Bad request: unknown resource type 'employee'",
            "employee must not be reported as merely unknown"
        );
        assert!(service.list_grants("t1", None, None, None).await.unwrap().is_empty());

        for resource_type in ["skill", "mcp", "knowledge", "model_channel"] {
            service
                .grant_resource("t1", "member", "alice", resource_type, "res_1", "admin1")
                .await
                .unwrap_or_else(|e| panic!("{resource_type} must still be grantable: {e}"));
        }
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

    // --- E5 scene management ---

    #[tokio::test]
    async fn list_scenes_seeds_the_five_builtins_idempotently() {
        let (_db, service) = setup().await;
        let first = service.list_scenes("t1").await.unwrap();
        assert_eq!(first.len(), 5);
        assert!(first.iter().all(|s| s.built_in && s.member_count == 0));
        let names: std::collections::HashSet<_> = first.iter().map(|s| s.name.as_str()).collect();
        for expected in ["办公", "IT运维", "网络安全", "新媒体运营", "市场营销"] {
            assert!(names.contains(expected), "missing built-in scene {expected}");
        }

        // A second tenant gets its own five — seeding is per-tenant, not global.
        let t2 = service.list_scenes("t2").await.unwrap();
        assert_eq!(t2.len(), 5);

        // Re-listing t1 must not duplicate its built-ins.
        let second = service.list_scenes("t1").await.unwrap();
        assert_eq!(second.len(), 5);
    }

    #[tokio::test]
    async fn create_scene_rejects_a_duplicate_name_and_empty_name() {
        let (_db, service) = setup().await;
        service
            .create_scene("t1", "Sales Team", Some("desc"), &["销售".to_owned()])
            .await
            .unwrap();
        assert_eq!(
            service
                .create_scene("t1", "Sales Team", None, &[])
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service.create_scene("t1", "   ", None, &[]).await.unwrap_err().code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn update_and_delete_scene_roundtrip_and_protect_builtins() {
        let (_db, service) = setup().await;
        let custom = service.create_scene("t1", "Custom", None, &[]).await.unwrap();

        let updated = service
            .update_scene(
                "t1",
                &custom.id,
                "Custom Renamed",
                Some("new desc"),
                &["tag1".to_owned()],
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Custom Renamed");
        assert_eq!(updated.description.as_deref(), Some("new desc"));
        assert_eq!(updated.job_functions, vec!["tag1".to_owned()]);

        service.delete_scene("t1", &custom.id).await.unwrap();
        assert_eq!(
            service
                .update_scene("t1", &custom.id, "x", None, &[])
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND",
            "deleted scene should be gone"
        );

        let builtins = service.list_scenes("t1").await.unwrap();
        let builtin_id = &builtins[0].id;
        assert_eq!(
            service.delete_scene("t1", builtin_id).await.unwrap_err().code(),
            "BAD_REQUEST",
            "a built-in scene must not be deletable"
        );
    }

    #[tokio::test]
    async fn scene_membership_is_idempotent_and_listable() {
        let (_db, service) = setup().await;
        let scene = service.create_scene("t1", "Ops", None, &[]).await.unwrap();

        service.add_scene_member("t1", &scene.id, "alice").await.unwrap();
        service.add_scene_member("t1", &scene.id, "alice").await.unwrap(); // idempotent
        service.add_scene_member("t1", &scene.id, "bob").await.unwrap();

        let members = service.list_scene_members("t1", &scene.id).await.unwrap();
        assert_eq!(members, vec!["alice".to_owned(), "bob".to_owned()]);
        assert_eq!(
            service
                .list_scenes("t1")
                .await
                .unwrap()
                .iter()
                .find(|s| s.id == scene.id)
                .unwrap()
                .member_count,
            2
        );

        service.remove_scene_member("t1", &scene.id, "alice").await.unwrap();
        assert_eq!(
            service.list_scene_members("t1", &scene.id).await.unwrap(),
            vec!["bob".to_owned()]
        );
    }

    /// The whole point of scenes: joining one reaches whatever the scene has
    /// been granted, without a per-member grant.
    #[tokio::test]
    async fn effective_resource_ids_resolves_through_scene_membership() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        let scene = service.create_scene("t1", "Ops", None, &[]).await.unwrap();
        service
            .grant_resource("t1", "scene", &scene.id, "mcp", "mcp_1", "admin1")
            .await
            .unwrap();

        // Not a member yet: nothing reachable.
        let before = service.effective_resource_ids("t1", "alice", "mcp").await.unwrap();
        assert!(before.resource_ids.is_empty());

        service.add_scene_member("t1", &scene.id, "alice").await.unwrap();
        let after = service.effective_resource_ids("t1", "alice", "mcp").await.unwrap();
        assert_eq!(after.resource_ids, vec!["mcp_1".to_owned()]);

        // Leaving the scene must revoke what it granted.
        service.remove_scene_member("t1", &scene.id, "alice").await.unwrap();
        let gone = service.effective_resource_ids("t1", "alice", "mcp").await.unwrap();
        assert!(gone.resource_ids.is_empty());
    }

    /// Deleting a non-built-in scene must clean up both its roster and
    /// whatever it was granted — an orphaned `one_resource_grants` row
    /// pointing at a scene that no longer exists would be silently inert but
    /// is exactly the kind of stale state an audit later trips over.
    #[tokio::test]
    async fn delete_scene_cascades_members_and_grants() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        let scene = service.create_scene("t1", "Ops", None, &[]).await.unwrap();
        service.add_scene_member("t1", &scene.id, "alice").await.unwrap();
        service
            .grant_resource("t1", "scene", &scene.id, "mcp", "mcp_1", "admin1")
            .await
            .unwrap();

        service.delete_scene("t1", &scene.id).await.unwrap();

        assert!(
            service.list_scene_members("t1", &scene.id).await.is_err(),
            "scene itself is gone"
        );
        let remaining_grants = service
            .list_grants("t1", Some("scene"), Some(&scene.id), None)
            .await
            .unwrap();
        assert!(
            remaining_grants.is_empty(),
            "grants on the deleted scene must be cleaned up too"
        );
    }

    // --- E5 security policy baseline ---

    #[tokio::test]
    async fn get_security_policy_defaults_to_relaxed_when_unset() {
        let (_db, service) = setup().await;
        let policy = service.get_security_policy("t1").await.unwrap();
        assert_eq!(policy.tier, "relaxed");
        assert!(!policy.terminal_tools_require_approval);
        assert!(!policy.destructive_commands_blocked);
        assert!(policy.blocked_command_patterns.is_empty());
        assert!(!policy.external_network_denied_by_default);
        assert!(!policy.message_scan_enabled);
        assert_eq!(policy.send_rate_limit_per_minute, None);
        assert_eq!(policy.updated_at, None, "no row was ever written for this tenant");
    }

    #[tokio::test]
    async fn apply_security_policy_tier_sets_the_standard_and_strict_presets() {
        let (_db, service) = setup().await;

        let standard = service.apply_security_policy_tier("t1", "standard").await.unwrap();
        assert_eq!(standard.tier, "standard");
        assert!(standard.terminal_tools_require_approval);
        assert!(standard.destructive_commands_blocked);
        assert!(!standard.blocked_command_patterns.is_empty());
        assert!(!standard.external_network_denied_by_default, "strict-only check");
        assert!(!standard.message_scan_enabled, "strict-only check");
        assert!(standard.updated_at.is_some());

        let strict = service.apply_security_policy_tier("t1", "strict").await.unwrap();
        assert_eq!(strict.tier, "strict");
        assert!(strict.external_network_denied_by_default);
        assert!(strict.message_scan_enabled);
        assert!(strict.message_redact_enabled);
        assert!(
            strict.send_rate_limit_per_minute < standard.send_rate_limit_per_minute,
            "strict must rate-limit at least as tightly as standard"
        );
    }

    #[tokio::test]
    async fn apply_security_policy_tier_rejects_an_unknown_tier() {
        let (_db, service) = setup().await;
        assert_eq!(
            service
                .apply_security_policy_tier("t1", "paranoid")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn set_security_policy_is_a_custom_override_independent_of_any_tier() {
        let (_db, service) = setup().await;
        service.apply_security_policy_tier("t1", "strict").await.unwrap();

        // A hand-edit after applying a tier must relabel it 'custom', even
        // though only one field actually changed from the strict preset —
        // the label is about provenance, not incidentally matching a preset.
        let custom = service
            .set_security_policy(
                "t1",
                true,
                false,
                &["custom-pattern".to_owned()],
                true,
                true,
                true,
                Some(99),
            )
            .await
            .unwrap();
        assert_eq!(custom.tier, "custom");
        assert!(!custom.destructive_commands_blocked);
        assert_eq!(custom.blocked_command_patterns, vec!["custom-pattern".to_owned()]);
        assert_eq!(custom.send_rate_limit_per_minute, Some(99));

        // Reading it back gets the same values, not the tier preset's.
        let reread = service.get_security_policy("t1").await.unwrap();
        assert_eq!(reread.tier, "custom");
        assert_eq!(reread.blocked_command_patterns, vec!["custom-pattern".to_owned()]);
    }

    /// Two tenants' baselines must not bleed into each other.
    #[tokio::test]
    async fn security_policy_is_scoped_per_tenant() {
        let (_db, service) = setup().await;
        service.apply_security_policy_tier("t1", "strict").await.unwrap();
        let t2 = service.get_security_policy("t2").await.unwrap();
        assert_eq!(
            t2.tier, "relaxed",
            "an untouched tenant must not inherit another tenant's tier"
        );
    }

    // --- E5 open-integration API keys ---

    #[tokio::test]
    async fn create_api_key_returns_the_secret_once_and_never_again() {
        let (_db, service) = setup().await;
        let created = service
            .create_api_key("t1", "CI bot", &["/api/one/devops/*".to_owned()], Some(60), "admin1")
            .await
            .unwrap();
        assert!(created.secret.starts_with("sk_live_"));
        assert!(
            created.secret.starts_with(&created.key.key_prefix),
            "the shown prefix must actually be a prefix of the real secret"
        );
        assert_eq!(created.key.status, "active");
        assert_eq!(created.key.allowed_paths, vec!["/api/one/devops/*".to_owned()]);
        assert_eq!(created.key.rate_limit_per_minute, Some(60));
        assert_eq!(created.key.created_by, "admin1");

        // The list/get-shaped view (ApiKeyDto) has no field the secret or its
        // hash could leak through — this is enforced by the type itself, not
        // by a runtime redaction step, so there's nothing to assert beyond
        // "it's the type we expect" — but do assert the JSON wire shape has
        // no `secret`/`keyHash` key, in case that ever changes.
        let listed = service.list_api_keys("t1").await.unwrap();
        assert_eq!(listed.len(), 1);
        let as_json = serde_json::to_string(&listed[0]).unwrap();
        assert!(
            !as_json.contains(&created.secret),
            "the list view must never carry the plaintext secret"
        );
        assert!(
            !as_json.to_lowercase().contains("hash"),
            "the list view must never carry the hash either"
        );
    }

    #[test]
    fn hash_api_key_secret_is_deterministic_and_distinguishes_inputs() {
        let a = PlatformService::hash_api_key_secret("secret-a");
        let b = PlatformService::hash_api_key_secret("secret-a");
        let c = PlatformService::hash_api_key_secret("secret-b");
        assert_eq!(a, b, "hashing the same secret twice must produce the same hash");
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn create_api_key_rejects_an_empty_name() {
        let (_db, service) = setup().await;
        assert_eq!(
            service
                .create_api_key("t1", "   ", &[], None, "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn revoke_api_key_is_idempotent_and_404s_only_for_a_truly_unknown_id() {
        let (_db, service) = setup().await;
        let created = service.create_api_key("t1", "Key", &[], None, "admin1").await.unwrap();

        service.revoke_api_key("t1", &created.key.id).await.unwrap();
        let after = service.list_api_keys("t1").await.unwrap();
        assert_eq!(after[0].status, "revoked");
        assert!(after[0].revoked_at.is_some());

        // Revoking again must not error.
        service.revoke_api_key("t1", &created.key.id).await.unwrap();

        assert_eq!(
            service.revoke_api_key("t1", "no-such-key").await.unwrap_err().code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn api_keys_are_scoped_per_tenant() {
        let (_db, service) = setup().await;
        service.create_api_key("t1", "Key", &[], None, "admin1").await.unwrap();
        assert!(service.list_api_keys("t2").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn authenticate_api_key_accepts_a_matching_secret_on_an_allowed_path() {
        let (_db, service) = setup().await;
        let created = service
            .create_api_key("t1", "CI bot", &["/api/one/devops/*".to_owned()], None, "admin1")
            .await
            .unwrap();

        let outcome = service
            .authenticate_api_key(&created.secret, "/api/one/devops/skills")
            .await
            .unwrap();
        assert_eq!(
            outcome,
            ApiKeyAuthOutcome::Authenticated {
                user_id: "admin1".to_owned()
            }
        );
    }

    #[tokio::test]
    async fn authenticate_api_key_rejects_an_unknown_secret() {
        let (_db, service) = setup().await;
        let outcome = service
            .authenticate_api_key("sk_live_not-a-real-key", "/api/one/devops/skills")
            .await
            .unwrap();
        assert_eq!(outcome, ApiKeyAuthOutcome::Invalid);
    }

    #[tokio::test]
    async fn authenticate_api_key_rejects_a_revoked_key() {
        let (_db, service) = setup().await;
        let created = service
            .create_api_key("t1", "CI bot", &["/api/one/devops/*".to_owned()], None, "admin1")
            .await
            .unwrap();
        service.revoke_api_key("t1", &created.key.id).await.unwrap();

        let outcome = service
            .authenticate_api_key(&created.secret, "/api/one/devops/skills")
            .await
            .unwrap();
        assert_eq!(outcome, ApiKeyAuthOutcome::Invalid);
    }

    #[tokio::test]
    async fn authenticate_api_key_rejects_a_path_outside_allowed_paths() {
        let (_db, service) = setup().await;
        let created = service
            .create_api_key("t1", "CI bot", &["/api/one/devops/*".to_owned()], None, "admin1")
            .await
            .unwrap();

        let outcome = service
            .authenticate_api_key(&created.secret, "/api/one/billing/usage")
            .await
            .unwrap();
        assert_eq!(outcome, ApiKeyAuthOutcome::PathNotAllowed);
    }

    /// A key created with no `allowed_paths` (the default) must authorize
    /// nothing, not everything — see `one_api_keys`'s migration comment.
    #[tokio::test]
    async fn authenticate_api_key_with_no_allowed_paths_authorizes_nothing() {
        let (_db, service) = setup().await;
        let created = service.create_api_key("t1", "Key", &[], None, "admin1").await.unwrap();

        let outcome = service
            .authenticate_api_key(&created.secret, "/api/one/devops/skills")
            .await
            .unwrap();
        assert_eq!(outcome, ApiKeyAuthOutcome::PathNotAllowed);
    }

    /// A removed member's API key must stop authenticating. It has no session
    /// generation to rotate, so without this it outlives every other
    /// credential revocation path.
    #[tokio::test]
    async fn revoking_a_users_api_keys_stops_them_authenticating() {
        let (_db, service) = setup().await;
        let mine = service
            .create_api_key("t1", "leaver key", &["/api/one/devops/*".to_owned()], None, "leaver")
            .await
            .unwrap();
        let theirs = service
            .create_api_key("t1", "stayer key", &["/api/one/devops/*".to_owned()], None, "stayer")
            .await
            .unwrap();

        assert_eq!(service.revoke_api_keys_for_user("leaver").await.unwrap(), 1);

        assert_eq!(
            service
                .authenticate_api_key(&mine.secret, "/api/one/devops/skills")
                .await
                .unwrap(),
            ApiKeyAuthOutcome::Invalid
        );
        // Another member's key is untouched.
        assert_eq!(
            service
                .authenticate_api_key(&theirs.secret, "/api/one/devops/skills")
                .await
                .unwrap(),
            ApiKeyAuthOutcome::Authenticated {
                user_id: "stayer".to_owned()
            }
        );
    }

    /// Idempotent, and revokes across every tenant the user minted keys in —
    /// removal is per-user, and the caller does not know their tenants.
    #[tokio::test]
    async fn revoking_a_users_api_keys_spans_tenants_and_is_idempotent() {
        let (_db, service) = setup().await;
        service.create_api_key("t1", "k1", &[], None, "leaver").await.unwrap();
        service.create_api_key("t2", "k2", &[], None, "leaver").await.unwrap();

        assert_eq!(service.revoke_api_keys_for_user("leaver").await.unwrap(), 2);
        // Second call finds nothing still active.
        assert_eq!(service.revoke_api_keys_for_user("leaver").await.unwrap(), 0);
        assert_eq!(service.revoke_api_keys_for_user("never-existed").await.unwrap(), 0);
    }

    /// A padded id must be stored trimmed, or it becomes a grant that matches
    /// neither `GRANT_ALL_RESOURCES` nor any real resource id while the admin
    /// console still renders it as active.
    #[tokio::test]
    async fn grant_resource_stores_trimmed_ids() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        let grant = service
            .grant_resource("t1", "member", "  alice  ", "skill", "  *  ", "admin1")
            .await
            .unwrap();
        assert_eq!(grant.subject_id, "alice");
        assert_eq!(grant.resource_id, GRANT_ALL_RESOURCES);

        // And it resolves as the wildcard it was meant to be.
        let effective = service.effective_resource_ids("t1", "alice", "skill").await.unwrap();
        assert!(effective.all, "a trimmed '*' must resolve as a wildcard grant");
    }

    #[tokio::test]
    async fn authenticate_api_key_matches_an_exact_path_without_a_wildcard() {
        let (_db, service) = setup().await;
        let created = service
            .create_api_key("t1", "Key", &["/api/one/billing/usage".to_owned()], None, "admin1")
            .await
            .unwrap();

        assert_eq!(
            service
                .authenticate_api_key(&created.secret, "/api/one/billing/usage")
                .await
                .unwrap(),
            ApiKeyAuthOutcome::Authenticated {
                user_id: "admin1".to_owned()
            }
        );
        assert_eq!(
            service
                .authenticate_api_key(&created.secret, "/api/one/billing/usage/extra")
                .await
                .unwrap(),
            ApiKeyAuthOutcome::PathNotAllowed
        );
    }

    // --- In-app notifications (P2-3 站内消息) ---

    #[tokio::test]
    async fn broadcast_reaches_every_member_and_late_joiners_too() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        let sent = service
            .create_notification("t1", "broadcast", "公告", "维护窗口", "周六 02:00-04:00", &[], "admin1")
            .await
            .unwrap();
        // A broadcast's audience is the whole roster, evaluated at read time.
        assert_eq!(sent.recipient_count, 2);

        let alice = service.list_my_notifications("t1", "alice", 100).await.unwrap();
        assert_eq!(alice.notifications.len(), 1);
        assert_eq!(alice.unread_count, 1);

        // A member who joins AFTER the send still sees the broadcast — the
        // reason broadcast rows carry no recipient snapshot.
        seed_membership(db.pool(), "bob", "t1", "member").await;
        let bob = service.list_my_notifications("t1", "bob", 100).await.unwrap();
        assert_eq!(bob.notifications.len(), 1);
        assert_eq!(bob.unread_count, 1);
    }

    #[tokio::test]
    async fn targeted_reaches_only_its_recipients_and_rejects_non_members() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        seed_membership(db.pool(), "mallory", "t2", "member").await;

        let sent = service
            .create_notification(
                "t1",
                "targeted",
                "安全",
                "凭证轮换",
                "请轮换你的 API Key",
                &["alice".to_owned()],
                "admin1",
            )
            .await
            .unwrap();
        assert_eq!(sent.recipient_count, 1);

        let alice = service.list_my_notifications("t1", "alice", 100).await.unwrap();
        assert_eq!(alice.unread_count, 1);
        // A non-recipient member of the same tenant sees nothing…
        let carol = service.list_my_notifications("t1", "carol", 100).await.unwrap();
        assert!(carol.notifications.is_empty());
        // …and a targeted message can never be aimed at a non-member.
        let err = service
            .create_notification("t1", "targeted", "", "hi", "body", &["mallory".to_owned()], "admin1")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "BAD_REQUEST");
        // A targeted send with no recipients is rejected, not a silent no-op.
        assert_eq!(
            service
                .create_notification("t1", "targeted", "", "hi", "body", &[], "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_notification("t1", "wallpaper", "", "hi", "body", &[], "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn read_marks_are_per_user_and_mark_all_is_scoped_to_visibility() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        seed_membership(db.pool(), "bob", "t1", "member").await;
        service
            .create_notification("t1", "broadcast", "", "one", "body", &[], "admin1")
            .await
            .unwrap();
        service
            .create_notification("t1", "broadcast", "", "two", "body", &[], "admin1")
            .await
            .unwrap();

        service.mark_notifications_read("t1", "alice", &[]).await.unwrap();
        let alice = service.list_my_notifications("t1", "alice", 100).await.unwrap();
        assert_eq!(alice.unread_count, 0);
        // Bob's state is untouched — read marks are per user.
        let bob = service.list_my_notifications("t1", "bob", 100).await.unwrap();
        assert_eq!(bob.unread_count, 2);
        // Marking again is idempotent.
        service.mark_notifications_read("t1", "alice", &[]).await.unwrap();
        assert_eq!(
            service
                .list_my_notifications("t1", "alice", 100)
                .await
                .unwrap()
                .unread_count,
            0
        );
        // The admin's sent history reflects the audience reads.
        let history = service.list_notifications("t1").await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].read_count, 1);
        assert_eq!(history[0].recipient_count, 3);
    }

    #[tokio::test]
    async fn deleting_a_notification_removes_it_for_everyone() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        seed_membership(db.pool(), "alice", "t1", "member").await;
        let sent = service
            .create_notification("t1", "broadcast", "", "oops", "wrong message", &[], "admin1")
            .await
            .unwrap();
        service.mark_notifications_read("t1", "alice", &[]).await.unwrap();

        service.delete_notification("t1", &sent.id).await.unwrap();
        assert!(service.list_notifications("t1").await.unwrap().is_empty());
        let alice = service.list_my_notifications("t1", "alice", 100).await.unwrap();
        assert!(alice.notifications.is_empty() && alice.unread_count == 0);
        // Unknown id → NotFound, not a silent success.
        assert_eq!(
            service
                .delete_notification("t1", "ntf_missing")
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn notification_title_and_body_must_not_be_blank() {
        let (_db, service) = setup().await;
        assert_eq!(
            service
                .create_notification("t1", "broadcast", "", "  ", "body", &[], "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_notification("t1", "broadcast", "", "title", "   ", &[], "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    // --- Personal file vault (P2-4 个人文件仓库) ---

    async fn setup_vault(tag: &str) -> (dream_core_db::Database, PlatformService, std::path::PathBuf) {
        let (db, _) = setup().await;
        let root = std::env::temp_dir().join(format!("one-platform-vault-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let service = PlatformService::new(db.pool().clone(), [7u8; 32]).with_storage_root(root.clone());
        (db, service, root)
    }

    #[tokio::test]
    async fn vault_upload_stores_bytes_ledger_and_usage_in_agreement() {
        let (_db, service, _root) = setup_vault("roundtrip").await;
        let stored = service
            .upload_vault_object("t1", "alice", "notes.txt", b"hello vault")
            .await
            .unwrap();
        assert_eq!(stored.file_name, "notes.txt");

        let vault = service.my_vault("t1", "alice").await.unwrap();
        assert_eq!(vault.usage_bytes, 11);
        assert_eq!(vault.object_count, 1);
        assert_eq!(vault.status, "available");
        assert_eq!(vault.quota_bytes, None);

        // Reading back yields the exact bytes that went in.
        let (dto, bytes) = service.read_vault_object("t1", "alice", &stored.id).await.unwrap();
        assert_eq!(bytes, b"hello vault");
        assert_eq!(dto.file_name, "notes.txt");

        // A member with no vault activity reads as available / unlimited.
        let bob = service.my_vault("t1", "bob").await.unwrap();
        assert_eq!(bob.status, "available");
        assert_eq!(bob.usage_bytes, 0);
    }

    #[tokio::test]
    async fn frozen_vault_blocks_uploads_but_keeps_existing_objects_usable() {
        let (_db, service, _root) = setup_vault("frozen").await;
        let stored = service
            .upload_vault_object("t1", "alice", "keep.txt", b"keep me")
            .await
            .unwrap();
        service.admin_set_vault_status("t1", "alice", "frozen").await.unwrap();

        let err = service
            .upload_vault_object("t1", "alice", "new.txt", b"nope")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "FORBIDDEN");

        // The hold is on new data, not on the member's own history.
        let (_, bytes) = service.read_vault_object("t1", "alice", &stored.id).await.unwrap();
        assert_eq!(bytes, b"keep me");
        service.delete_vault_object("t1", "alice", &stored.id).await.unwrap();
        assert_eq!(service.my_vault("t1", "alice").await.unwrap().object_count, 0);

        // Releasing the vault lets uploads through again, and unknown
        // statuses are rejected.
        service
            .admin_set_vault_status("t1", "alice", "available")
            .await
            .unwrap();
        assert!(
            service
                .upload_vault_object("t1", "alice", "new.txt", b"ok")
                .await
                .is_ok()
        );
        assert_eq!(
            service
                .admin_set_vault_status("t1", "alice", "shredded")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn vault_quota_blocks_the_upload_that_would_exceed_it() {
        let (_db, service, _root) = setup_vault("quota").await;
        service.admin_set_vault_quota("t1", "alice", Some(10)).await.unwrap();
        service
            .upload_vault_object("t1", "alice", "six.bin", b"123456")
            .await
            .unwrap();
        assert_eq!(
            service
                .upload_vault_object("t1", "alice", "six.bin", b"123456")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // Exactly filling the quota is fine; a negative quota is not.
        assert!(
            service
                .upload_vault_object("t1", "alice", "four.bin", b"1234")
                .await
                .is_ok()
        );
        assert_eq!(
            service
                .admin_set_vault_quota("t1", "alice", Some(-1))
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn reconcile_reports_missing_orphan_and_size_mismatches() {
        let (_db, service, root) = setup_vault("reconcile").await;
        let a = service
            .upload_vault_object("t1", "alice", "a.txt", b"aaaaa")
            .await
            .unwrap();
        let b = service
            .upload_vault_object("t1", "alice", "b.txt", b"bbbbb")
            .await
            .unwrap();
        let c = service
            .upload_vault_object("t1", "alice", "c.txt", b"ccccc")
            .await
            .unwrap();

        // a: file vanishes out-of-band → missing on disk.
        std::fs::remove_file(root.join("t1").join("alice").join(format!("{}__a.txt", a.id))).unwrap();
        // b: truncated in place → size mismatch.
        std::fs::write(root.join("t1").join("alice").join(format!("{}__b.txt", b.id)), b"b").unwrap();
        // Orphan: bytes with no ledger row → missing in ledger.
        std::fs::write(root.join("t1").join("alice").join("orphan__orphan.bin"), b"zz").unwrap();

        let report = service.admin_reconcile_vaults("t1").await.unwrap();
        assert_eq!(report.len(), 1, "only alice's vault is inconsistent: {report:?}");
        let entry = &report[0];
        assert_eq!(entry.user_id, "alice");
        assert_eq!(entry.missing_on_disk, vec![format!("a.txt ({})", a.id)]);
        assert_eq!(entry.size_mismatches, vec![format!("b.txt ({})", b.id)]);
        assert_eq!(entry.missing_in_ledger.len(), 1);
        assert!(entry.missing_in_ledger[0].starts_with("orphan"));

        // c is intact, so a clean tenant yields an empty report.
        assert!(service.read_vault_object("t1", "alice", &c.id).await.is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn vault_file_names_cannot_traverse_and_tombstones_survive() {
        let (_db, service, root) = setup_vault("sanitize").await;
        // Traversal components are dropped by the file-name extraction, not
        // by relying on platform-specific separators.
        let stored = service
            .upload_vault_object("t1", "alice", "../../../evil?.txt", b"safe")
            .await
            .unwrap();
        assert_eq!(stored.file_name, "evil_.txt");
        // The object landed inside the member's own directory.
        assert!(
            root.join("t1")
                .join("alice")
                .join(format!("{}__evil_.txt", stored.id))
                .is_file()
        );

        service.delete_vault_object("t1", "alice", &stored.id).await.unwrap();
        // Tombstoned: gone from live reads and from usage, kept in the ledger.
        assert_eq!(
            service
                .read_vault_object("t1", "alice", &stored.id)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
        assert_eq!(service.my_vault("t1", "alice").await.unwrap().usage_bytes, 0);
        let listed = service.list_my_vault_objects("t1", "alice").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].deleted_at.is_some());
        // Deleting it again is a NotFound, not a double tombstone.
        assert_eq!(
            service
                .delete_vault_object("t1", "alice", &stored.id)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
    }
}
