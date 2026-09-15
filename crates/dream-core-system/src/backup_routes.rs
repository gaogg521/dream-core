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
    ApiResponse, BackupManifestResponse, BackupScopeDto, CreateBackupRequest, CreateBackupResponse,
    PreviewBackupRequest, RestoreBackupRequest, RestoreBackupResponse,
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
        .export(&state.pool, &destination, from_dto(request.scope))
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
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(RestoreBackupResponse {
        rows_by_table: outcome.rows_by_table,
        files_restored: outcome.files_restored,
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
    }
}
