//! `secure-export` routes: watermark seeds, export events and encrypted
//! package key escrow. Handlers only transform request/response; the rules
//! live in `secure_export.rs`.
//!
//! Member routes sit under `/api/one/platform/secure-export/*` and are reached
//! by the desktop client with its bearer token; admin routes sit under
//! `/api/one/admin/platform/secure-export/*` for the console.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;
use dream_core_common::governance_caller::{GovernanceCaller, classify_governance_caller};

use crate::error::PlatformError;
use crate::rbac::{RequirePlatformAdmin, RequirePlatformAuditReader, RequirePlatformMember};
use crate::secure_export::{
    CreateSecurePackageInput, ExportEventDto, ExportEventInput, ExportEventQuery, OpenSecurePackageDto,
    ReportExportEventsDto, SecurePackageDto, SecurePackageOpenDto, SecurePackageQuery, WatermarkSeedDto,
    WatermarkVerifyDto,
};
use crate::state::OnePlatformRouterState;

/// Largest text accepted for a watermark lookup. A pasted document, not an
/// archive.
const MAX_VERIFY_TEXT_BYTES: usize = 5 * 1024 * 1024;

pub fn secure_export_routes() -> Router<OnePlatformRouterState> {
    Router::new()
        .route("/api/one/platform/secure-export/watermark-seed", post(issue_seed))
        .route("/api/one/platform/secure-export/events", post(report_events))
        .route(
            "/api/one/platform/secure-export/packages",
            get(list_my_packages).post(create_package),
        )
        .route("/api/one/platform/secure-export/packages/{id}/open", post(open_package))
        .route(
            "/api/one/platform/secure-export/packages/{id}/opens",
            get(my_package_opens),
        )
        .route(
            "/api/one/platform/secure-export/packages/{id}/revoke",
            post(revoke_my_package),
        )
        .route(
            "/api/one/admin/platform/secure-export/packages",
            get(admin_list_packages),
        )
        .route(
            "/api/one/admin/platform/secure-export/packages/{id}/opens",
            get(admin_package_opens),
        )
        .route(
            "/api/one/admin/platform/secure-export/packages/{id}/revoke",
            post(admin_revoke_package),
        )
        .route("/api/one/admin/platform/secure-export/events", get(admin_list_events))
        .route(
            "/api/one/admin/platform/secure-export/watermark/verify",
            post(admin_verify_watermark),
        )
}

/// The machine the desktop client says it is, as the runtime-node roster
/// knows it. Console and WebUI sessions have none.
fn caller_machine(headers: &HeaderMap) -> Option<String> {
    match classify_governance_caller(headers) {
        GovernanceCaller::Identified(id) => Some(id.to_owned()),
        _ => None,
    }
}

async fn issue_seed(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<WatermarkSeedDto>>, PlatformError> {
    let machine = caller_machine(&headers).unwrap_or_else(|| "unknown".to_owned());
    Ok(Json(ApiResponse::ok(
        state.service.issue_watermark_seed(&actor, &user.id, &machine).await?,
    )))
}

#[derive(Deserialize)]
struct ReportEventsBody {
    events: Vec<ExportEventInput>,
}

async fn report_events(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    Json(body): Json<ReportEventsBody>,
) -> Result<Json<ApiResponse<ReportExportEventsDto>>, PlatformError> {
    let machine = caller_machine(&headers);
    Ok(Json(ApiResponse::ok(
        state
            .service
            .report_export_events(&actor, &user.id, machine.as_deref(), body.events)
            .await?,
    )))
}

async fn create_package(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    Json(body): Json<CreateSecurePackageInput>,
) -> Result<Json<ApiResponse<SecurePackageDto>>, PlatformError> {
    let machine = caller_machine(&headers);
    Ok(Json(ApiResponse::ok(
        state
            .service
            .create_secure_package(&actor, &user.id, machine.as_deref(), body)
            .await?,
    )))
}

async fn open_package(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<OpenSecurePackageDto>>, PlatformError> {
    let machine = caller_machine(&headers);
    Ok(Json(ApiResponse::ok(
        state
            .service
            .open_secure_package(&actor, &user.id, machine.as_deref(), &id)
            .await?,
    )))
}

async fn list_my_packages(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(_actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<SecurePackageDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_my_secure_packages(&user.id).await?,
    )))
}

async fn my_package_opens(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(_actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<SecurePackageOpenDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.my_secure_package_opens(&user.id, &id).await?,
    )))
}

async fn revoke_my_package(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<SecurePackageDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.revoke_my_secure_package(&actor, &user.id, &id).await?,
    )))
}

async fn admin_list_packages(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAuditReader(actor): RequirePlatformAuditReader,
    Query(query): Query<SecurePackageQuery>,
) -> Result<Json<ApiResponse<Vec<SecurePackageDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_secure_packages(&actor.tenant_id, &query).await?,
    )))
}

async fn admin_package_opens(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAuditReader(actor): RequirePlatformAuditReader,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<SecurePackageOpenDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.admin_secure_package_opens(&actor.tenant_id, &id).await?,
    )))
}

async fn admin_revoke_package(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<SecurePackageDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .admin_revoke_secure_package(&actor.tenant_id, &user.id, &id)
            .await?,
    )))
}

async fn admin_list_events(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAuditReader(actor): RequirePlatformAuditReader,
    Query(query): Query<ExportEventQuery>,
) -> Result<Json<ApiResponse<Vec<ExportEventDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_export_events(&actor.tenant_id, &query).await?,
    )))
}

#[derive(Deserialize)]
struct VerifyBody {
    text: String,
}

async fn admin_verify_watermark(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAuditReader(actor): RequirePlatformAuditReader,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<VerifyBody>,
) -> Result<Json<ApiResponse<WatermarkVerifyDto>>, PlatformError> {
    if body.text.len() > MAX_VERIFY_TEXT_BYTES {
        return Err(PlatformError::BadRequest("text is too large to check".into()));
    }
    Ok(Json(ApiResponse::ok(
        state
            .service
            .verify_watermarks(&actor.tenant_id, &user.id, &body.text)
            .await?,
    )))
}
