//! First-run bootstrap: an enterprise build must land the deployment admin
//! (`system_default_user`) in a working default enterprise, so a fresh install
//! has no `NOT_IN_ENTERPRISE` wall on any tenant-scoped page.
//!
//! Only meaningful under `--features enterprise` — the whole file is cfg'd out
//! otherwise (the `one_*` governance tables do not exist in the personal
//! build).
#![cfg(feature = "enterprise")]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use common::{body_json, extract_csrf_token, get_request};

async fn count(pool: &sqlx::SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn first_run_provisions_a_working_default_enterprise_and_is_idempotent() {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let config = dream_core_app::AppConfig {
        identity_mode: dream_core_app::IdentityMode::AionPro,
        bootstrap_secret: Some("bootstrap-secret".to_string()),
        ..Default::default()
    };
    let services = dream_core_app::AppServices::from_config(db, &config).await.unwrap();

    // Full router build runs the one-* migrations and then the first-run
    // enterprise bootstrap.
    let _router = dream_core_app::create_router(&services).await.expect("build router");
    let pool = services.database.pool();

    // The deployment admin is now a real enterprise-tenant member.
    assert_eq!(
        count(pool, "SELECT COUNT(*) FROM one_tenants WHERE id = 'enterprise'").await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM one_user_org WHERE user_id = 'system_default_user' \
             AND tenant_id = 'enterprise' AND role = 'system_admin'",
        )
        .await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM one_active_tenant WHERE user_id = 'system_default_user' AND tenant_id = 'enterprise'",
        )
        .await,
        1
    );
    assert_eq!(
        count(pool, "SELECT COUNT(*) FROM one_enterprises WHERE origin = 'bootstrap'").await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM one_enterprise_license WHERE tier = 'enterprise'"
        )
        .await,
        1
    );
    let enterprise_id: String = sqlx::query_scalar("SELECT enterprise_id FROM one_tenants WHERE id = 'enterprise'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(!enterprise_id.is_empty());

    // A second router build over the same DB (mirrors the split
    // dreamcore / dreamcore-admin double-run, and every later restart) must
    // not duplicate anything.
    let _router2 = dream_core_app::create_router(&services).await.expect("rebuild router");
    assert_eq!(count(pool, "SELECT COUNT(*) FROM one_tenants").await, 1);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM one_enterprises").await, 1);
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM one_user_org WHERE user_id = 'system_default_user'"
        )
        .await,
        1
    );

    services.database.close().await;
}

#[tokio::test]
async fn admin_reaches_a_tenant_scoped_route_without_not_in_enterprise() {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let config = dream_core_app::AppConfig {
        identity_mode: dream_core_app::IdentityMode::WebUi,
        ..Default::default()
    };
    let services = dream_core_app::AppServices::from_config(db, &config).await.unwrap();
    let app = dream_core_app::create_router(&services).await.expect("build router");

    // Log in as the seed admin (username "admin" == system_default_user).
    let hash = dream_core_auth::hash_password("pw12345678").unwrap();
    services
        .user_repo
        .set_system_user_credentials("admin", &hash)
        .await
        .unwrap();
    let status_resp = app.clone().oneshot(get_request("/api/auth/status")).await.unwrap();
    let csrf = extract_csrf_token(&status_resp).expect("csrf cookie");
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"username":"admin","password":"pw12345678"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let token = body_json(login).await["token"].as_str().unwrap().to_owned();

    // A RequireOrgAdmin route: on a fresh un-bootstrapped deploy this returned
    // 400 NOT_IN_ENTERPRISE. It may now be 200, or 403 PASSWORD_CHANGE_REQUIRED
    // under the password gate — either way the enterprise gate itself passed.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/one/org/members")
                .header("authorization", format!("Bearer {token}"))
                .header("x-csrf-token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let json = body_json(resp).await;
    let code = json.get("code").and_then(|c| c.as_str()).unwrap_or("");
    assert_ne!(
        code, "NOT_IN_ENTERPRISE",
        "bootstrap should have cleared the enterprise gate (status {status})"
    );

    services.database.close().await;
}
