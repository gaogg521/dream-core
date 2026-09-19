#![allow(clippy::disallowed_types)]

//! HTTP surface for personal-edition backup and restore.
//!
//! A separate router with its own state rather than three more fields on
//! `SystemRouterState`: that struct is built in nine places, seven of them
//! tests that have nothing to do with backup, and widening it would edit all of
//! them to say `backup: None`.
//!
//! Paths are passed by the client, not streamed as request bodies. The backend
//! is a local service, the archive can reach gigabytes, and the file dialog
//! that chooses the location lives in the desktop app.

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Json, State};
use axum::routing::post;

use dream_core_api_types::{
    ApiResponse, ArchiveEncryptionResponse, BackupManifestResponse, BackupScopeDto, CreateBackupRequest,
    CreateBackupResponse, PreviewBackupRequest, RestoreBackupRequest, RestoreBackupResponse,
};
use dream_core_auth::CurrentUser;
use dream_core_common::ApiError;
use dream_core_db::DbPool;

use crate::backup::{BackupManifest, BackupScope, BackupService};

#[derive(Clone)]
pub struct BackupRouterState {
    pub service: BackupService,
    /// The pool serving this data directory; the catalog copy and the restore
    /// merge both run through it.
    pub pool: DbPool,
}

/// Routes:
/// - `POST /api/system/backup`         — write an archive of the selected categories
/// - `POST /api/system/backup/preview` — read an archive's manifest, apply nothing
/// - `POST /api/system/backup/restore` — merge an archive into this install
pub fn backup_routes(state: BackupRouterState) -> Router {
    Router::new()
        .route("/api/system/backup", post(create_backup))
        .route("/api/system/backup/preview", post(preview_backup))
        .route("/api/system/backup/restore", post(restore_backup))
        .with_state(state)
}

/// Authenticated like every other route on this surface. An archive that
/// includes the provider category carries decryptable API keys, so an
/// unauthenticated export would hand the install to any local caller.
async fn create_backup(
    State(state): State<BackupRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<CreateBackupRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<CreateBackupResponse>>, ApiError> {
    let Json(request) = body.map_err(ApiError::from)?;
    let destination = std::path::PathBuf::from(&request.destination);
    let manifest = state
        .service
        .export(&state.pool, &destination, from_dto(request.scope), &request.passphrase)
        .await
        .map_err(ApiError::from)?;

    let archive_bytes = std::fs::metadata(&destination)
        .map(|meta| meta.len())
        .unwrap_or_default();
    Ok(Json(ApiResponse::ok(CreateBackupResponse {
        path: destination.to_string_lossy().into_owned(),
        archive_bytes,
        manifest: to_manifest_response(&manifest),
    })))
}

async fn preview_backup(
    State(state): State<BackupRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<PreviewBackupRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<BackupManifestResponse>>, ApiError> {
    let Json(request) = body.map_err(ApiError::from)?;
    let manifest = state
        .service
        .preview(std::path::Path::new(&request.source))
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(to_manifest_response(&manifest))))
}

async fn restore_backup(
    State(state): State<BackupRouterState>,
    Extension(_user): Extension<CurrentUser>,
    body: Result<Json<RestoreBackupRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<RestoreBackupResponse>>, ApiError> {
    let Json(request) = body.map_err(ApiError::from)?;
    let outcome = state
        .service
        .restore(
            &state.pool,
            std::path::Path::new(&request.source),
            from_dto(request.scope),
            &request.passphrase,
        )
        .await
        .inspect_err(|error| {
            // An internal error reaches the client as a bare "Internal server
            // error", by design. Without this line the cause is gone: the
            // per-connection ATTACH bug presented as exactly that response and
            // nothing in the log said why.
            tracing::error!(
                source = %request.source,
                %error,
                "Backup restore failed"
            );
        })
        .map_err(ApiError::from)?;
    tracing::info!(
        tables = outcome.rows_by_table.len(),
        files = outcome.files_restored,
        orphans = outcome.orphans_removed.values().sum::<u64>(),
        "Backup restore applied"
    );
    Ok(Json(ApiResponse::ok(RestoreBackupResponse {
        rows_by_table: outcome.rows_by_table,
        files_restored: outcome.files_restored,
        orphans_removed: outcome.orphans_removed,
    })))
}

fn from_dto(dto: BackupScopeDto) -> BackupScope {
    BackupScope {
        conversations: dto.conversations,
        attachments: dto.attachments,
        providers: dto.providers,
        skills: dto.skills,
        app_settings: dto.app_settings,
    }
}

fn to_dto(scope: BackupScope) -> BackupScopeDto {
    BackupScopeDto {
        conversations: scope.conversations,
        attachments: scope.attachments,
        providers: scope.providers,
        skills: scope.skills,
        app_settings: scope.app_settings,
    }
}

fn to_manifest_response(manifest: &BackupManifest) -> BackupManifestResponse {
    BackupManifestResponse {
        format_version: manifest.format_version,
        exported_at: manifest.exported_at,
        app_version: manifest.app_version.clone(),
        scope: to_dto(manifest.scope),
        total_bytes: manifest.total_bytes,
        contains_credentials: manifest.contains_credentials,
        // This mapping is the whole bug: the field existed on the internal
        // manifest since the day encryption shipped, and was never carried
        // into the response the desktop UI actually reads. Without it, the
        // passphrase field a person could type into never appears.
        encryption: manifest
            .encryption
            .as_ref()
            .map(|encryption| ArchiveEncryptionResponse {
                cipher: encryption.cipher.clone(),
                kdf: encryption.kdf.clone(),
            }),
    }
}

#[cfg(test)]
mod manifest_response_tests {
    use super::*;
    use crate::backup_crypto::ArchiveEncryption;

    fn manifest(encryption: Option<ArchiveEncryption>) -> BackupManifest {
        BackupManifest {
            format_version: 3,
            exported_at: 0,
            app_version: "3.0.7".to_owned(),
            scope: BackupScope::all(),
            total_bytes: 0,
            contains_credentials: true,
            encryption,
        }
    }

    /// The bug this whole module exists to close: `to_manifest_response` used
    /// to build a `BackupManifestResponse` with no `encryption` field at all,
    /// so an actually-encrypted archive's preview response never told the
    /// desktop UI that it needed a passphrase. The UI's decision to show that
    /// field reads `manifest.encryption` truthiness on exactly this response
    /// -- so this asserts the WIRE JSON, not just the Rust struct, because a
    /// future refactor that keeps the field but drops it during serialization
    /// would reintroduce the same silent failure.
    #[test]
    fn an_encrypted_manifest_carries_encryption_into_the_response_the_ui_reads() {
        let encrypted = manifest(Some(ArchiveEncryption {
            cipher: "aes-256-gcm".to_owned(),
            kdf: "argon2id".to_owned(),
            salt: "unused-in-this-test".to_owned(),
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
            verifier: "unused-in-this-test".to_owned(),
        }));

        let response = to_manifest_response(&encrypted);
        assert!(
            response.encryption.is_some(),
            "an encrypted manifest must report encryption"
        );
        let json = serde_json::to_string(&response).unwrap();
        assert!(
            json.contains("\"encryption\""),
            "the field must survive serialization, not just exist on the struct"
        );
        assert!(json.contains("\"aes-256-gcm\""));
        assert!(json.contains("\"argon2id\""));
        // The client never decrypts anything itself -- salt and verifier are
        // server-side only and must not leave the machine that already has
        // the archive.
        assert!(
            !json.contains("unused-in-this-test"),
            "salt/verifier must not reach the wire"
        );
    }

    /// A version-2 archive, written before encryption existed, has nothing to
    /// report -- and the field must be ABSENT rather than `null`, matching the
    /// frontend's optional-field truthiness check.
    #[test]
    fn an_unencrypted_manifest_omits_the_field_entirely() {
        let response = to_manifest_response(&manifest(None));
        assert!(response.encryption.is_none());
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("encryption"), "absent, not present-and-null: {json}");
    }
}
