//! HTTP transformations for the cookie-authenticated UI and engine capabilities.
use crate::{DevopsError, OneDevopsRouterState, office_service::OfficeService};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::Response,
    routing::{get, post},
};
use dream_core_api_types::{
    ApiResponse,
    knowledge_office::{
        KnowledgeOfficeAvailability, KnowledgeOfficeCallbackResult, KnowledgeOfficeCapability, KnowledgeOfficeSaved,
        KnowledgeOfficeSession,
    },
};
use dream_core_auth::CurrentUser;
use std::sync::Arc;

fn office(state: &OneDevopsRouterState) -> Result<Arc<OfficeService>, DevopsError> {
    state
        .office
        .clone()
        .ok_or_else(|| DevopsError::BadRequest("intranet Office editing is not configured".into()))
}

pub(crate) async fn availability(
    State(state): State<OneDevopsRouterState>,
) -> Json<ApiResponse<KnowledgeOfficeAvailability>> {
    Json(ApiResponse::ok(KnowledgeOfficeAvailability {
        enabled: state.office.is_some(),
    }))
}

pub(crate) async fn start(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<KnowledgeOfficeSession>>, DevopsError> {
    Ok(Json(ApiResponse::ok(
        office(&state)?.start(&user.id, &user.username, &id).await?,
    )))
}

pub(crate) async fn flush(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<KnowledgeOfficeSaved>>, DevopsError> {
    Ok(Json(ApiResponse::ok(office(&state)?.flush(&user.id, &id).await?)))
}

/// Exempt only these capability-authenticated engine requests from cookie CSRF.
/// Each handler verifies a purpose-bound capability and live member/document ACL.
pub fn office_engine_routes(state: OneDevopsRouterState) -> Router {
    Router::new()
        .route("/api/one/devops/office-engine/source", get(source))
        .route("/api/one/devops/office-engine/callback", post(callback))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}

async fn source(
    State(state): State<OneDevopsRouterState>,
    Query(query): Query<KnowledgeOfficeCapability>,
) -> Result<Response, DevopsError> {
    let (bytes, extension) = office(&state)?.source(&query.token).await?;
    let mime = match extension.as_str() {
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    };
    let length = bytes.len();
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    headers.insert(header::CONTENT_LENGTH, length.into());
    headers.insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    headers.insert(header::CONTENT_DISPOSITION, "attachment".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    Ok(response)
}

async fn callback(
    State(state): State<OneDevopsRouterState>,
    Query(query): Query<KnowledgeOfficeCapability>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<KnowledgeOfficeCallbackResult>, DevopsError> {
    let token = body
        .get("token")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
        })
        .ok_or_else(|| DevopsError::Forbidden("Office engine signature required".into()))?;
    office(&state)?.callback(&query.token, token).await?;
    Ok(Json(KnowledgeOfficeCallbackResult { error: 0 }))
}
