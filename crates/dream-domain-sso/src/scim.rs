//! SCIM 2.0 inbound provisioning (`/scim/v2/Users`, `/scim/v2/Groups`).
//!
//! Push-mode counterpart of [`crate::directory`]. Bearer-token auth. PATCH
//! uses SCIM's `Operations` dialect, not JSON Patch. Deactivating a user
//! (`active=false`) immediately revokes sessions through [`ScimLifecycle`].
//! DELETE runs the same offboarding hook (ownership transfer + session
//! revoke) and never hard-deletes local user rows.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use dream_core_common::now_ms;
use dream_core_db::db_params;

use crate::error::SsoError;
use crate::providers::ProviderUserInfo;
use crate::state::OneSsoRouterState;

const SCIM_USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const SCIM_GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const SCIM_LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
#[allow(dead_code)]
const SCIM_PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
const SCIM_ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const SCIM_PROVIDER: &str = "scim";

/// Session revoke + offboarding (ownership transfer). Wired by the app over
/// `SessionRevoker` / devops transfer so this crate does not depend on them.
#[async_trait]
pub trait ScimLifecycle: Send + Sync {
    async fn revoke_sessions(&self, user_id: &str);
    /// Reassign the user's assets and drop company/group seats. Must not
    /// delete the user row. Best-effort: log and continue on failure.
    async fn offboard(&self, user_id: &str);
}

pub fn scim_routes(state: OneSsoRouterState) -> Router {
    let authed = Router::new()
        .route("/scim/v2/Users", get(list_users).post(create_user))
        .route(
            "/scim/v2/Users/{id}",
            get(get_user).put(replace_user).patch(patch_user).delete(delete_user),
        )
        .route("/scim/v2/Groups", get(list_groups).post(create_group))
        .route(
            "/scim/v2/Groups/{id}",
            get(get_group).patch(patch_group).delete(delete_group),
        )
        .route("/scim/v2/ServiceProviderConfig", get(service_provider_config))
        .layer(from_fn_with_state(state.clone(), scim_bearer));
    authed.with_state(state)
}

pub async fn set_bearer_token(state: &OneSsoRouterState, token: &str) -> Result<(), SsoError> {
    let hash = token_sha256(token);
    let now = now_ms();
    let existing: Option<String> = state
        .service
        .db()
        .fetch_optional_scalar("SELECT token_sha256 FROM one_scim_settings WHERE id = 1", &[])
        .await?;
    if existing.is_some() {
        state
            .service
            .db()
            .execute(
                "UPDATE one_scim_settings SET token_sha256 = ?, enabled = 1, updated_at = ? WHERE id = 1",
                &db_params![&hash, now],
            )
            .await?;
    } else {
        state
            .service
            .db()
            .execute(
                "INSERT INTO one_scim_settings (id, token_sha256, enabled, updated_at) VALUES (1, ?, 1, ?)",
                &db_params![&hash, now],
            )
            .await?;
    }
    Ok(())
}

fn token_sha256(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    format!("{:x}", h.finalize())
}

async fn scim_bearer(State(state): State<OneSsoRouterState>, request: Request, next: Next) -> Response {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .unwrap_or("")
        .trim();
    if presented.is_empty() {
        return scim_error(StatusCode::UNAUTHORIZED, "invalidToken", "Bearer token required");
    }
    match verify_token(&state, presented).await {
        Ok(true) => next.run(request).await,
        Ok(false) => scim_error(StatusCode::UNAUTHORIZED, "invalidToken", "Bearer token mismatch"),
        Err(e) => scim_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", &e.to_string()),
    }
}

async fn verify_token(state: &OneSsoRouterState, presented: &str) -> Result<bool, SsoError> {
    let row: Option<(String, i64)> = state
        .service
        .db()
        .fetch_optional_as("SELECT token_sha256, enabled FROM one_scim_settings WHERE id = 1", &[])
        .await?;
    let Some((stored, enabled)) = row else {
        return Ok(false);
    };
    if enabled == 0 {
        return Ok(false);
    }
    let got = token_sha256(presented);
    Ok(bool::from(stored.as_bytes().ct_eq(got.as_bytes())))
}

fn scim_error(status: StatusCode, scim_type: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({
            "schemas": [SCIM_ERROR],
            "status": status.as_u16().to_string(),
            "scimType": scim_type,
            "detail": detail,
        })),
    )
        .into_response()
}

#[derive(Debug, sqlx::FromRow)]
struct ScimUserRow {
    id: String,
    external_id: String,
    user_id: String,
    user_name: String,
    display_name: Option<String>,
    active: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ScimGroupRow {
    id: String,
    external_id: Option<String>,
    display_name: String,
}

fn user_resource(row: &ScimUserRow) -> Value {
    json!({
        "schemas": [SCIM_USER],
        "id": row.id,
        "externalId": row.external_id,
        "userName": row.user_name,
        "displayName": row.display_name.clone().unwrap_or_else(|| row.user_name.clone()),
        "active": row.active != 0,
        "meta": { "resourceType": "User" },
    })
}

fn group_resource(row: &ScimGroupRow, members: Vec<Value>) -> Value {
    json!({
        "schemas": [SCIM_GROUP],
        "id": row.id,
        "externalId": row.external_id,
        "displayName": row.display_name,
        "members": members,
        "meta": { "resourceType": "Group" },
    })
}

async fn list_users(State(state): State<OneSsoRouterState>) -> Response {
    match load_users(&state).await {
        Ok(rows) => {
            let resources: Vec<Value> = rows.iter().map(user_resource).collect();
            Json(json!({
                "schemas": [SCIM_LIST],
                "totalResults": resources.len(),
                "startIndex": 1,
                "itemsPerPage": resources.len(),
                "Resources": resources,
            }))
            .into_response()
        }
        Err(e) => scim_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", &e.to_string()),
    }
}

async fn load_users(state: &OneSsoRouterState) -> Result<Vec<ScimUserRow>, SsoError> {
    state
        .service
        .db()
        .fetch_all_as(
            "SELECT id, external_id, user_id, user_name, display_name, active FROM one_scim_users",
            &[],
        )
        .await
        .map_err(Into::into)
}

async fn create_user(State(state): State<OneSsoRouterState>, Json(body): Json<Value>) -> Response {
    match create_user_inner(&state, body).await {
        Ok((status, value)) => (status, Json(value)).into_response(),
        Err(e) => map_err(e),
    }
}

async fn create_user_inner(state: &OneSsoRouterState, body: Value) -> Result<(StatusCode, Value), SsoError> {
    let user_name = claim_str(&body, "userName").ok_or_else(|| SsoError::BadRequest("userName required".into()))?;
    let external_id = claim_str(&body, "externalId").unwrap_or_else(|| user_name.clone());
    let display_name = claim_str(&body, "displayName").unwrap_or_else(|| user_name.clone());
    let active = body.get("active").and_then(json_bool).unwrap_or(true);

    if let Some(existing) = load_user_by_external(state, &external_id).await? {
        return Ok((StatusCode::OK, user_resource(&existing)));
    }

    let profile = ProviderUserInfo {
        external_id: external_id.clone(),
        preferred_username: display_name.clone(),
        org_unit_path: None,
        job_title: None,
        org_external_id: None,
    };
    let (user_id, _username, _created) = state
        .service
        .resolve_or_provision_user_named(SCIM_PROVIDER, profile)
        .await?;
    ensure_member_role(state, &user_id).await;
    let id = uuid::Uuid::now_v7().simple().to_string();
    let now = now_ms();
    state
        .service
        .db()
        .execute(
            "INSERT INTO one_scim_users (id, external_id, user_id, user_name, display_name, active, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &db_params![&id, &external_id, &user_id, &user_name, &display_name, if active { 1 } else { 0 }, now, now],
        )
        .await?;
    let row = load_user(state, &id).await?.expect("just inserted");
    Ok((StatusCode::CREATED, user_resource(&row)))
}

async fn ensure_member_role(state: &OneSsoRouterState, user_id: &str) {
    let now = now_ms();
    let _ = state
        .service
        .db()
        .execute(
            "INSERT OR IGNORE INTO one_user_org (user_id, tenant_id, role, created_at, updated_at) \
             VALUES (?, 'default', 'member', ?, ?)",
            &db_params![user_id, now, now],
        )
        .await;
}

async fn get_user(State(state): State<OneSsoRouterState>, Path(id): Path<String>) -> Response {
    match load_user(&state, &id).await {
        Ok(Some(row)) => Json(user_resource(&row)).into_response(),
        Ok(None) => scim_error(StatusCode::NOT_FOUND, "invalidValue", "User not found"),
        Err(e) => map_err(e),
    }
}

async fn load_user(state: &OneSsoRouterState, id: &str) -> Result<Option<ScimUserRow>, SsoError> {
    state
        .service
        .db()
        .fetch_optional_as(
            "SELECT id, external_id, user_id, user_name, display_name, active FROM one_scim_users WHERE id = ?",
            &db_params![id],
        )
        .await
        .map_err(Into::into)
}

async fn load_user_by_external(state: &OneSsoRouterState, external_id: &str) -> Result<Option<ScimUserRow>, SsoError> {
    state
        .service
        .db()
        .fetch_optional_as(
            "SELECT id, external_id, user_id, user_name, display_name, active FROM one_scim_users WHERE external_id = ?",
            &db_params![external_id],
        )
        .await
        .map_err(Into::into)
}

async fn replace_user(
    State(state): State<OneSsoRouterState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    match replace_user_inner(&state, &id, body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => map_err(e),
    }
}

async fn replace_user_inner(state: &OneSsoRouterState, id: &str, body: Value) -> Result<Value, SsoError> {
    let row = load_user(state, id)
        .await?
        .ok_or_else(|| SsoError::NotFound("user".into()))?;
    let user_name = claim_str(&body, "userName").unwrap_or(row.user_name.clone());
    let display_name = claim_str(&body, "displayName").or(row.display_name.clone());
    let active = body.get("active").and_then(json_bool).unwrap_or(row.active != 0);
    apply_active_change(state, &row, active).await?;
    state
        .service
        .db()
        .execute(
            "UPDATE one_scim_users SET user_name = ?, display_name = ?, active = ?, updated_at = ? WHERE id = ?",
            &db_params![
                &user_name,
                display_name.as_deref(),
                if active { 1 } else { 0 },
                now_ms(),
                id
            ],
        )
        .await?;
    Ok(user_resource(&load_user(state, id).await?.expect("updated")))
}

async fn patch_user(
    State(state): State<OneSsoRouterState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    match patch_user_inner(&state, &id, body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => map_err(e),
    }
}

async fn patch_user_inner(state: &OneSsoRouterState, id: &str, body: Value) -> Result<Value, SsoError> {
    let mut row = load_user(state, id)
        .await?
        .ok_or_else(|| SsoError::NotFound("user".into()))?;
    let ops = patch_ops(&body)?;
    let mut active = row.active != 0;
    let mut user_name = row.user_name.clone();
    let mut display_name = row.display_name.clone();
    for op in ops {
        match op.path.as_deref() {
            Some("active") | None if op.is_replace() && op.path.is_none() => {
                if let Some(v) = op
                    .value
                    .as_ref()
                    .and_then(json_bool)
                    .or_else(|| op.value.as_ref().and_then(|v| v.get("active")).and_then(json_bool))
                {
                    active = v;
                }
            }
            Some(p) if p.eq_ignore_ascii_case("active") => {
                active = op.value.as_ref().and_then(json_bool).unwrap_or(active);
            }
            Some(p) if p.eq_ignore_ascii_case("userName") => {
                if let Some(s) = op.value.as_ref().and_then(|v| v.as_str()) {
                    user_name = s.to_owned();
                }
            }
            Some(p) if p.eq_ignore_ascii_case("displayName") => {
                display_name = op.value.as_ref().and_then(|v| v.as_str()).map(str::to_owned);
            }
            _ => {}
        }
    }
    apply_active_change(state, &row, active).await?;
    state
        .service
        .db()
        .execute(
            "UPDATE one_scim_users SET user_name = ?, display_name = ?, active = ?, updated_at = ? WHERE id = ?",
            &db_params![
                &user_name,
                display_name.as_deref(),
                if active { 1 } else { 0 },
                now_ms(),
                id
            ],
        )
        .await?;
    row = load_user(state, id).await?.expect("updated");
    Ok(user_resource(&row))
}

async fn apply_active_change(state: &OneSsoRouterState, row: &ScimUserRow, active: bool) -> Result<(), SsoError> {
    if row.active != 0
        && !active
        && let Some(hook) = state.scim_lifecycle.as_ref()
    {
        hook.revoke_sessions(&row.user_id).await;
    }
    Ok(())
}

async fn delete_user(State(state): State<OneSsoRouterState>, Path(id): Path<String>) -> Response {
    match delete_user_inner(&state, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

async fn delete_user_inner(state: &OneSsoRouterState, id: &str) -> Result<(), SsoError> {
    let row = load_user(state, id)
        .await?
        .ok_or_else(|| SsoError::NotFound("user".into()))?;
    if let Some(hook) = state.scim_lifecycle.as_ref() {
        hook.offboard(&row.user_id).await;
        hook.revoke_sessions(&row.user_id).await;
    }
    state
        .service
        .db()
        .execute(
            "UPDATE one_scim_users SET active = 0, updated_at = ? WHERE id = ?",
            &db_params![now_ms(), id],
        )
        .await?;
    Ok(())
}

async fn list_groups(State(state): State<OneSsoRouterState>) -> Response {
    match load_groups(&state).await {
        Ok(rows) => {
            let mut resources = Vec::new();
            for row in &rows {
                let members = group_members(&state, &row.id).await.unwrap_or_default();
                resources.push(group_resource(row, members));
            }
            Json(json!({
                "schemas": [SCIM_LIST],
                "totalResults": resources.len(),
                "startIndex": 1,
                "itemsPerPage": resources.len(),
                "Resources": resources,
            }))
            .into_response()
        }
        Err(e) => map_err(e),
    }
}

async fn load_groups(state: &OneSsoRouterState) -> Result<Vec<ScimGroupRow>, SsoError> {
    state
        .service
        .db()
        .fetch_all_as("SELECT id, external_id, display_name FROM one_scim_groups", &[])
        .await
        .map_err(Into::into)
}

async fn create_group(State(state): State<OneSsoRouterState>, Json(body): Json<Value>) -> Response {
    match create_group_inner(&state, body).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => map_err(e),
    }
}

async fn create_group_inner(state: &OneSsoRouterState, body: Value) -> Result<Value, SsoError> {
    let display_name =
        claim_str(&body, "displayName").ok_or_else(|| SsoError::BadRequest("displayName required".into()))?;
    let external_id = claim_str(&body, "externalId");
    let id = uuid::Uuid::now_v7().simple().to_string();
    let now = now_ms();
    state
        .service
        .db()
        .execute(
            "INSERT INTO one_scim_groups (id, external_id, display_name, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            &db_params![&id, external_id.as_deref(), &display_name, now, now],
        )
        .await?;
    if let Some(members) = body.get("members").and_then(|v| v.as_array()) {
        for m in members {
            if let Some(uid) = m.get("value").and_then(|v| v.as_str()) {
                let _ = state
                    .service
                    .db()
                    .execute(
                        "INSERT OR IGNORE INTO one_scim_group_members (group_id, user_scim_id) VALUES (?, ?)",
                        &db_params![&id, uid],
                    )
                    .await;
            }
        }
    }
    let row = load_group(state, &id).await?.expect("inserted");
    let members = group_members(state, &id).await?;
    Ok(group_resource(&row, members))
}

async fn get_group(State(state): State<OneSsoRouterState>, Path(id): Path<String>) -> Response {
    match load_group(&state, &id).await {
        Ok(Some(row)) => {
            let members = group_members(&state, &id).await.unwrap_or_default();
            Json(group_resource(&row, members)).into_response()
        }
        Ok(None) => scim_error(StatusCode::NOT_FOUND, "invalidValue", "Group not found"),
        Err(e) => map_err(e),
    }
}

async fn load_group(state: &OneSsoRouterState, id: &str) -> Result<Option<ScimGroupRow>, SsoError> {
    state
        .service
        .db()
        .fetch_optional_as(
            "SELECT id, external_id, display_name FROM one_scim_groups WHERE id = ?",
            &db_params![id],
        )
        .await
        .map_err(Into::into)
}

async fn group_members(state: &OneSsoRouterState, group_id: &str) -> Result<Vec<Value>, SsoError> {
    let ids: Vec<(String,)> = state
        .service
        .db()
        .fetch_all_as(
            "SELECT user_scim_id FROM one_scim_group_members WHERE group_id = ?",
            &db_params![group_id],
        )
        .await?;
    Ok(ids
        .into_iter()
        .map(|(id,)| json!({ "value": id, "type": "User" }))
        .collect())
}

async fn patch_group(
    State(state): State<OneSsoRouterState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    match patch_group_inner(&state, &id, body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => map_err(e),
    }
}

async fn patch_group_inner(state: &OneSsoRouterState, id: &str, body: Value) -> Result<Value, SsoError> {
    let row = load_group(state, id)
        .await?
        .ok_or_else(|| SsoError::NotFound("group".into()))?;
    for op in patch_ops(&body)? {
        let path = op.path.unwrap_or_default();
        if path.eq_ignore_ascii_case("displayName") {
            if let Some(name) = op.value.as_ref().and_then(|v| v.as_str()) {
                state
                    .service
                    .db()
                    .execute(
                        "UPDATE one_scim_groups SET display_name = ?, updated_at = ? WHERE id = ?",
                        &db_params![name, now_ms(), id],
                    )
                    .await?;
            }
        } else if path.to_ascii_lowercase().starts_with("members") || path.is_empty() {
            apply_member_op(state, id, &op.op, op.value.as_ref()).await?;
        }
    }
    let _ = row;
    let row = load_group(state, id).await?.expect("group");
    let members = group_members(state, id).await?;
    Ok(group_resource(&row, members))
}

async fn apply_member_op(
    state: &OneSsoRouterState,
    group_id: &str,
    op: &str,
    value: Option<&Value>,
) -> Result<(), SsoError> {
    let op = op.to_ascii_lowercase();
    let values: Vec<&Value> = match value {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v) => vec![v],
        None => vec![],
    };
    for v in values {
        let uid = v
            .get("value")
            .and_then(|x| x.as_str())
            .or_else(|| v.as_str())
            .unwrap_or("");
        if uid.is_empty() {
            continue;
        }
        if op == "add" || op == "replace" {
            let _ = state
                .service
                .db()
                .execute(
                    "INSERT OR IGNORE INTO one_scim_group_members (group_id, user_scim_id) VALUES (?, ?)",
                    &db_params![group_id, uid],
                )
                .await;
        } else if op == "remove" {
            let _ = state
                .service
                .db()
                .execute(
                    "DELETE FROM one_scim_group_members WHERE group_id = ? AND user_scim_id = ?",
                    &db_params![group_id, uid],
                )
                .await;
        }
    }
    Ok(())
}

async fn delete_group(State(state): State<OneSsoRouterState>, Path(id): Path<String>) -> Response {
    match state
        .service
        .db()
        .execute(
            "DELETE FROM one_scim_group_members WHERE group_id = ?",
            &db_params![&id],
        )
        .await
    {
        Ok(_) => {}
        Err(e) => return map_err(e.into()),
    }
    match state
        .service
        .db()
        .execute("DELETE FROM one_scim_groups WHERE id = ?", &db_params![&id])
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e.into()),
    }
}

async fn service_provider_config() -> Json<Value> {
    Json(json!({
        "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
        "patch": { "supported": true },
        "bulk": { "supported": false },
        "filter": { "supported": false },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "OAuth Bearer Token",
            "description": "Authentication scheme using the OAuth Bearer Token Standard",
            "specUri": "http://www.rfc-editor.org/info/rfc6750",
            "primary": true
        }]
    }))
}

struct PatchOp {
    op: String,
    path: Option<String>,
    value: Option<Value>,
}

impl PatchOp {
    fn is_replace(&self) -> bool {
        self.op.eq_ignore_ascii_case("replace")
    }
}

fn patch_ops(body: &Value) -> Result<Vec<PatchOp>, SsoError> {
    let ops = body
        .get("Operations")
        .or_else(|| body.get("operations"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| SsoError::BadRequest("SCIM PATCH requires Operations".into()))?;
    Ok(ops
        .iter()
        .map(|op| PatchOp {
            op: op.get("op").and_then(|v| v.as_str()).unwrap_or("replace").to_owned(),
            path: op
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_start_matches('/').to_owned()),
            value: op.get("value").cloned(),
        })
        .collect())
}

fn claim_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn json_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn map_err(e: SsoError) -> Response {
    match e {
        SsoError::BadRequest(m) => scim_error(StatusCode::BAD_REQUEST, "invalidValue", &m),
        SsoError::NotFound(m) => scim_error(StatusCode::NOT_FOUND, "invalidValue", &m),
        SsoError::Unauthorized => scim_error(StatusCode::UNAUTHORIZED, "invalidToken", "unauthorized"),
        other => scim_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", &other.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminScimBody {
    pub bearer_token: String,
}

pub async fn admin_put_scim_token(
    State(state): State<OneSsoRouterState>,
    _admin: crate::rbac::RequireSsoAdmin,
    Json(body): Json<AdminScimBody>,
) -> Result<Json<dream_core_api_types::ApiResponse<Value>>, SsoError> {
    if body.bearer_token.trim().is_empty() {
        return Err(SsoError::BadRequest("bearerToken required".into()));
    }
    set_bearer_token(&state, body.bearer_token.trim()).await?;
    Ok(Json(dream_core_api_types::ApiResponse::ok(
        json!({ "configured": true }),
    )))
}

// Silence unused imports used only by tests / wiring.
#[allow(dead_code)]
fn _keep(h: HeaderMap, a: Arc<()>) {
    let _ = (h, a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::SsoService;
    use dream_core_auth::{CookieConfig, JwtService};
    use dream_core_db::IUserRepository;
    use http_body_util::BodyExt;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    struct Rec {
        revoked: Mutex<Vec<String>>,
        offboarded: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ScimLifecycle for Rec {
        async fn revoke_sessions(&self, user_id: &str) {
            self.revoked.lock().await.push(user_id.to_owned());
        }
        async fn offboard(&self, user_id: &str) {
            self.offboarded.lock().await.push(user_id.to_owned());
        }
    }

    async fn harness() -> (Router, Arc<Rec>, String) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        sqlx::query(
            "CREATE TABLE one_user_org (\
                 user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', \
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (user_id, tenant_id))",
        )
        .execute(db.pool())
        .await
        .unwrap();
        crate::migrate::run_one_sso_migrations(&dream_core_db::DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        let user_repo: Arc<dyn IUserRepository> = Arc::new(dream_core_db::SqliteUserRepository::new(db.pool().clone()));
        let svc = Arc::new(SsoService::new(
            dream_core_db::DbPool::Sqlite(db.pool().clone()),
            user_repo,
            Arc::new(JwtService::new("test-secret".into())),
            Arc::new(CookieConfig {
                secure: false,
                same_site: "Lax",
            }),
        ));
        let rec = Arc::new(Rec {
            revoked: Mutex::new(Vec::new()),
            offboarded: Mutex::new(Vec::new()),
        });
        let mut state = OneSsoRouterState::new(svc);
        state.scim_lifecycle = Some(rec.clone());
        set_bearer_token(&state, "scim-secret").await.unwrap();
        (scim_routes(state), rec, "scim-secret".into())
    }

    async fn call(app: Router, req: axum::http::Request<axum::body::Body>) -> (StatusCode, Value) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    fn authed(token: &str, builder: axum::http::request::Builder) -> axum::http::request::Builder {
        builder.header(header::AUTHORIZATION, format!("Bearer {token}"))
    }

    #[tokio::test]
    async fn post_user_provisions_member_and_is_idempotent_on_external_id() {
        let (app, _, token) = harness().await;
        let body = json!({
            "schemas": [SCIM_USER],
            "userName": "ada.lovelace",
            "externalId": "okta-ada",
            "displayName": "Ada Lovelace",
            "active": true
        });
        let (status, first) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(first["userName"], "ada.lovelace");
        assert_eq!(first["active"], true);
        let id = first["id"].as_str().unwrap().to_owned();

        let (status2, second) = call(
            app,
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(second["id"], id);
    }

    #[tokio::test]
    async fn patch_active_false_revokes_sessions() {
        let (app, rec, token) = harness().await;
        let body = json!({ "userName": "leaver", "externalId": "ext-leave", "active": true });
        let (_, created) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let patch = json!({
            "schemas": [SCIM_PATCH],
            "Operations": [{ "op": "Replace", "path": "active", "value": false }]
        });
        let (status, patched) = call(
            app,
            authed(&token, axum::http::Request::patch(format!("/scim/v2/Users/{id}")))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(patch.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(patched["active"], false);
        assert!(!rec.revoked.lock().await.is_empty());
    }

    #[tokio::test]
    async fn delete_runs_offboarding_and_does_not_remove_the_user_row() {
        let (app, rec, token) = harness().await;
        let body = json!({ "userName": "gone", "externalId": "ext-gone" });
        let (_, created) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_owned();
        let (status, _) = call(
            app.clone(),
            authed(&token, axum::http::Request::delete(format!("/scim/v2/Users/{id}")))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!rec.offboarded.lock().await.is_empty());
        let (get_status, got) = call(
            app,
            authed(&token, axum::http::Request::get(format!("/scim/v2/Users/{id}")))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(get_status, StatusCode::OK);
        assert_eq!(got["active"], false);
    }

    #[tokio::test]
    async fn missing_bearer_is_rejected() {
        let (app, _, _) = harness().await;
        let (status, body) = call(
            app,
            axum::http::Request::get("/scim/v2/Users")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["scimType"], "invalidToken");
    }

    #[tokio::test]
    async fn azure_style_string_false_deactivates() {
        let (app, rec, token) = harness().await;
        let (_, created) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({ "userName": "az", "externalId": "az-1" }).to_string(),
                ))
                .unwrap(),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let patch = json!({
            "Operations": [{ "op": "replace", "path": "active", "value": "False" }]
        });
        let (_, patched) = call(
            app,
            authed(&token, axum::http::Request::patch(format!("/scim/v2/Users/{id}")))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(patch.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(patched["active"], false);
        assert!(!rec.revoked.lock().await.is_empty());
    }

    #[tokio::test]
    async fn groups_round_trip() {
        let (app, _, token) = harness().await;
        let (_, user) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Users"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"userName":"m","externalId":"m1"}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        let uid = user["id"].as_str().unwrap();
        let (_, group) = call(
            app.clone(),
            authed(&token, axum::http::Request::post("/scim/v2/Groups"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"displayName":"Eng","members":[{"value": uid}]}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(group["displayName"], "Eng");
        assert_eq!(group["members"][0]["value"], uid);
    }
}
