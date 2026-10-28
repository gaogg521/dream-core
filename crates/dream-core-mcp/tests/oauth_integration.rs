//! Integration tests for McpOAuthService with real SQLite.
//!
//! Tests from test-plan §4 (OAuth) at the service layer.
//! These tests exercise check_status, logout, get_authenticated_servers,
//! and get_token with a real DB and controlled loopback OAuth HTTP server.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dream_core_db::{IOAuthTokenRepository, SqliteOAuthTokenRepository, UpsertOAuthTokenParams};
use dream_core_mcp::McpOAuthService;

const TEST_USER_ID: &str = "system_default_user";

struct OAuthFixture {
    base: String,
    exchanges: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for OAuthFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn oauth_fixture() -> OAuthFixture {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let metadata_base = base.clone();
    let exchanges = Arc::new(AtomicUsize::new(0));
    let token_exchanges = exchanges.clone();
    let app = Router::new()
        .route("/{resource}/.well-known/oauth-authorization-server", get(move || {
            let base = metadata_base.clone();
            async move { Json(serde_json::json!({"authorization_endpoint":format!("{base}/authorize"), "token_endpoint":format!("{base}/token")})) }
        }))
        .route("/token", post(move |request: axum::extract::Request| {
            let exchanges = token_exchanges.clone();
            async move {
                exchanges.fetch_add(1, Ordering::SeqCst);
                let bytes = axum::body::to_bytes(request.into_body(), 8192).await.unwrap();
                let body = std::str::from_utf8(&bytes).unwrap();
                if body.contains("grant_type=refresh_token") {
                    return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid_grant"})));
                }
                assert!(body.contains("code_verifier="));
                (axum::http::StatusCode::OK, Json(serde_json::json!({"access_token":"fixture_access", "token_type":"Bearer", "expires_in":3600})))
            }
        }));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    OAuthFixture { base, exchanges, task }
}

fn callback_parts(url: &str) -> (String, String) {
    let parsed = reqwest::Url::parse(url).unwrap();
    let parameter = |name: &str| {
        parsed
            .query_pairs()
            .find(|(key, _)| key == name)
            .unwrap()
            .1
            .into_owned()
    };
    assert_eq!(parameter("code_challenge_method"), "S256");
    (parameter("redirect_uri"), parameter("state"))
}

async fn wait_authenticated(service: &McpOAuthService, url: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if service
                .check_oauth_status(TEST_USER_ID, url)
                .await
                .unwrap()
                .authenticated
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn callbacks_are_bound_to_their_resource_and_survive_invalid_requests() {
    let fixture = oauth_fixture().await;
    let (service, _) = make_service().await;
    let first = format!("{}/first", fixture.base);
    let second = format!("{}/second", fixture.base);
    let a = service
        .login(TEST_USER_ID, &first)
        .await
        .unwrap()
        .authorization_url
        .unwrap();
    let b = service
        .login(TEST_USER_ID, &second)
        .await
        .unwrap()
        .authorization_url
        .unwrap();
    let (redirect_a, state_a) = callback_parts(&a);
    let (redirect_b, state_b) = callback_parts(&b);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let wrong = client
        .get(&redirect_a)
        .query(&[("code", "fixture_code"), ("state", &state_b)])
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(fixture.exchanges.load(Ordering::SeqCst), 0);
    assert!(
        !service
            .check_oauth_status(TEST_USER_ID, &first)
            .await
            .unwrap()
            .authenticated
    );
    assert!(
        !service
            .check_oauth_status(TEST_USER_ID, &second)
            .await
            .unwrap()
            .authenticated
    );
    client
        .get(&redirect_b)
        .query(&[("code", "fixture_code"), ("state", &state_b)])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_authenticated(&service, &second).await;
    assert!(
        !service
            .check_oauth_status(TEST_USER_ID, &first)
            .await
            .unwrap()
            .authenticated
    );
    client
        .get(&redirect_a)
        .query(&[("code", "fixture_code"), ("state", &state_a)])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_authenticated(&service, &first).await;
    assert_eq!(fixture.exchanges.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn denial_releases_login_and_requires_an_explicit_retry() {
    let fixture = oauth_fixture().await;
    let (service, _) = make_service().await;
    let resource = format!("{}/first", fixture.base);
    let first = service
        .login(TEST_USER_ID, &resource)
        .await
        .unwrap()
        .authorization_url
        .unwrap();
    let (redirect, state) = callback_parts(&first);
    reqwest::Client::new()
        .get(redirect)
        .query(&[("error", "access_denied"), ("state", state.as_str())])
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(fixture.exchanges.load(Ordering::SeqCst), 0);
    assert!(
        !service
            .check_oauth_status(TEST_USER_ID, &resource)
            .await
            .unwrap()
            .authenticated
    );
    let retry = service
        .login(TEST_USER_ID, &resource)
        .await
        .unwrap()
        .authorization_url
        .unwrap();
    assert_ne!(callback_parts(&retry).1, state);
}

#[tokio::test]
async fn failed_refresh_never_returns_expired_credentials_but_keeps_valid_grace_period() {
    let fixture = oauth_fixture().await;
    let (service, repo) = make_service().await;
    for (path, expiry, expected) in [
        ("expired", 1, None),
        ("valid", dream_core_common::now_ms() + 120_000, Some("old_access")),
    ] {
        let url = format!("{}/{path}", fixture.base);
        repo.upsert(UpsertOAuthTokenParams {
            user_id: TEST_USER_ID,
            server_url: &url,
            access_token: "old_access",
            refresh_token: Some("bad-refresh"),
            client_info: None,
            token_type: "bearer",
            expires_at: Some(expiry),
        })
        .await
        .unwrap();
        assert_eq!(
            service.get_token(TEST_USER_ID, &url).await.unwrap().as_deref(),
            expected
        );
    }
    assert_eq!(fixture.exchanges.load(Ordering::SeqCst), 2);
}

async fn make_service() -> (McpOAuthService, Arc<dyn IOAuthTokenRepository>) {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let repo: Arc<dyn IOAuthTokenRepository> = Arc::new(SqliteOAuthTokenRepository::new(db.pool().clone(), [0x66; 32]));
    let svc = McpOAuthService::new(repo.clone(), reqwest::Client::new());
    // Keep db alive by leaking it (integration test only).
    std::mem::forget(db);
    (svc, repo)
}

// ---------------------------------------------------------------------------
// OA-1: Unauthenticated server returns false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_status_unauthenticated_returns_false() {
    let (svc, _repo) = make_service().await;
    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://new-server.example.com")
        .await
        .unwrap();
    assert!(!status.authenticated);
}

// ---------------------------------------------------------------------------
// OA-2: Authenticated server returns true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_status_authenticated_returns_true() {
    let (svc, repo) = make_service().await;

    // Seed a valid token.
    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://mcp.example.com",
        access_token: "access_123",
        refresh_token: Some("refresh_456"),
        client_info: None,
        token_type: "bearer",
        // Expires in the far future.
        expires_at: Some(dream_core_common::now_ms() + 3_600_000),
    })
    .await
    .unwrap();

    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://mcp.example.com")
        .await
        .unwrap();
    assert!(status.authenticated);
}

// ---------------------------------------------------------------------------
// OA-2b: Expired token treated as unauthenticated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_status_expired_token_returns_false() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://expired.example.com",
        access_token: "old_token",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        // Already expired.
        expires_at: Some(1000),
    })
    .await
    .unwrap();

    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://expired.example.com")
        .await
        .unwrap();
    assert!(!status.authenticated);
}

// ---------------------------------------------------------------------------
// OA-2c: Token with no expiry treated as valid
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_status_no_expiry_treated_as_valid() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://no-expiry.example.com",
        access_token: "no_exp_token",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: None,
    })
    .await
    .unwrap();

    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://no-expiry.example.com")
        .await
        .unwrap();
    assert!(status.authenticated);
}

// ---------------------------------------------------------------------------
// OA-3: Get all authenticated URLs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_authenticated_servers_returns_all_urls() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://a.example.com",
        access_token: "tok_a",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: None,
    })
    .await
    .unwrap();

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://b.example.com",
        access_token: "tok_b",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: None,
    })
    .await
    .unwrap();

    let urls = svc.get_authenticated_servers(TEST_USER_ID).await.unwrap();
    assert_eq!(urls.len(), 2);
    assert!(urls.contains(&"https://a.example.com".to_string()));
    assert!(urls.contains(&"https://b.example.com".to_string()));
}

#[tokio::test]
async fn get_authenticated_servers_empty_when_no_tokens() {
    let (svc, _repo) = make_service().await;
    let urls = svc.get_authenticated_servers(TEST_USER_ID).await.unwrap();
    assert!(urls.is_empty());
}

// ---------------------------------------------------------------------------
// OA-5: Login with invalid URL (no OAuth endpoints discoverable)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_invalid_url_returns_error() {
    let (svc, _repo) = make_service().await;
    // This URL won't have .well-known endpoints.
    let result = svc.login(TEST_USER_ID, "https://127.0.0.1:1").await;
    // Should return an McpError::OAuth about discovery failure.
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// OA-6: Logout deletes stored token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_deletes_stored_token() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://logout.example.com",
        access_token: "to_delete",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: None,
    })
    .await
    .unwrap();

    // Verify token exists.
    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://logout.example.com")
        .await
        .unwrap();
    assert!(status.authenticated);

    // Logout.
    svc.logout(TEST_USER_ID, "https://logout.example.com").await.unwrap();

    // Verify token is gone.
    let status = svc
        .check_oauth_status(TEST_USER_ID, "https://logout.example.com")
        .await
        .unwrap();
    assert!(!status.authenticated);
}

// ---------------------------------------------------------------------------
// OA-7: Logout is idempotent for non-authenticated URL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_idempotent_for_unauthenticated() {
    let (svc, _repo) = make_service().await;
    // Should not error.
    svc.logout(TEST_USER_ID, "https://never-authed.example.com")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// get_token tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_token_returns_none_for_unknown_url() {
    let (svc, _repo) = make_service().await;
    let token = svc
        .get_token(TEST_USER_ID, "https://unknown.example.com")
        .await
        .unwrap();
    assert!(token.is_none());
}

#[tokio::test]
async fn get_token_returns_access_token_when_valid() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://valid.example.com",
        access_token: "my_access_token",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: Some(dream_core_common::now_ms() + 3_600_000),
    })
    .await
    .unwrap();

    let token = svc.get_token(TEST_USER_ID, "https://valid.example.com").await.unwrap();
    assert_eq!(token.as_deref(), Some("my_access_token"));
}

#[tokio::test]
async fn get_token_rejects_expired_token_when_no_refresh_token() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://expired.example.com",
        access_token: "old_access",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: Some(1000),
    })
    .await
    .unwrap();

    // Expired credentials must not be sent to the resource server.
    let token = svc
        .get_token(TEST_USER_ID, "https://expired.example.com")
        .await
        .unwrap();
    assert!(token.is_none());
}

#[tokio::test]
async fn get_token_returns_no_expiry_token() {
    let (svc, repo) = make_service().await;

    repo.upsert(UpsertOAuthTokenParams {
        user_id: TEST_USER_ID,
        server_url: "https://noexp.example.com",
        access_token: "forever_token",
        refresh_token: None,
        client_info: None,
        token_type: "bearer",
        expires_at: None,
    })
    .await
    .unwrap();

    let token = svc.get_token(TEST_USER_ID, "https://noexp.example.com").await.unwrap();
    assert_eq!(token.as_deref(), Some("forever_token"));
}
