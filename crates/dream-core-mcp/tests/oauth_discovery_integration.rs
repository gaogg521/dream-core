//! Real loopback HTTP and SQLite coverage for standards-based MCP OAuth.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::response::IntoResponse;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    routing::{get, post},
};
use dream_core_db::{IOAuthTokenRepository, SqliteOAuthTokenRepository, UpsertOAuthTokenParams};
use dream_core_mcp::McpOAuthService;
use serde_json::{Value, json};

const USER: &str = "system_default_user";

#[derive(Clone)]
struct FixtureState {
    base: String,
    registrations: Arc<AtomicUsize>,
    tokens: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
}

struct Fixture {
    state: FixtureState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture() -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let state = FixtureState {
        base: format!("http://{}", listener.local_addr().unwrap()),
        registrations: Arc::new(AtomicUsize::new(0)),
        tokens: Arc::new(AtomicUsize::new(0)),
        refreshes: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/modern/mcp", get(|State(s):State<FixtureState>| async move { (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE,format!("Bearer realm=\"qa\", resource_metadata=\"{}/resource-metadata\"",s.base))]) }))
        .route("/wrong/mcp", get(|State(s):State<FixtureState>| async move { (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE,format!("Bearer resource_metadata=\"{}/resource-metadata\"",s.base))]) }))
        .route("/resource-metadata", get(|State(s):State<FixtureState>| async move {Json(json!({"resource":format!("{}/modern/mcp",s.base),"authorization_servers":[format!("{}/issuer",s.base)]}))}))
        .route("/.well-known/oauth-authorization-server/issuer", get(|State(s):State<FixtureState>| async move {Json(json!({"issuer":format!("{}/issuer",s.base),"authorization_endpoint":format!("{}/authorize",s.base),"token_endpoint":format!("{}/token",s.base),"registration_endpoint":format!("{}/register",s.base),"code_challenge_methods_supported":["S256"]}))}))
        .route("/register", post(|State(s):State<FixtureState>, Json(request):Json<Value>| async move {
            assert_eq!(request["token_endpoint_auth_method"],"none");
            assert_eq!(request["response_types"],json!(["code"]));
            assert!(request["redirect_uris"][0].as_str().unwrap().starts_with("http://127.0.0.1:"));
            s.registrations.fetch_add(1,Ordering::SeqCst);
            (StatusCode::CREATED, Json(json!({"client_id":"registered-public-client","token_endpoint_auth_method":"none"})))
        }))
        .route("/token", post(token))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fixture { state, task }
}

async fn token(State(s): State<FixtureState>, request: Request) -> Json<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), 8192).await.unwrap();
    let form = reqwest::Url::parse(&format!("http://fixture/?{}", std::str::from_utf8(&bytes).unwrap())).unwrap();
    let pairs: std::collections::HashMap<_, _> = form.query_pairs().into_owned().collect();
    assert_eq!(pairs.get("client_id").unwrap(), "registered-public-client");
    assert_eq!(pairs.get("resource").unwrap(), &format!("{}/modern/mcp", s.base));
    s.tokens.fetch_add(1, Ordering::SeqCst);
    if pairs.get("grant_type").unwrap() == "refresh_token" {
        assert_eq!(pairs.get("refresh_token").unwrap(), "initial-refresh");
        s.refreshes.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        Json(
            json!({"access_token":"refreshed-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":3600}),
        )
    } else {
        assert!(pairs.contains_key("code_verifier"));
        if pairs.get("code").is_some_and(|code| code == "slow-code") {
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
        Json(
            json!({"access_token":"initial-access","refresh_token":"initial-refresh","token_type":"Bearer","expires_in":3600}),
        )
    }
}

async fn service() -> (McpOAuthService, Arc<dyn IOAuthTokenRepository>, dream_core_db::Database) {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let repo: Arc<dyn IOAuthTokenRepository> = Arc::new(SqliteOAuthTokenRepository::new(db.pool().clone(), [0x31; 32]));
    (McpOAuthService::new(repo.clone(), reqwest::Client::new()), repo, db)
}

fn parameters(url: &str) -> std::collections::HashMap<String, String> {
    reqwest::Url::parse(url).unwrap().query_pairs().into_owned().collect()
}

async fn authorize(service: &McpOAuthService, resource: &str, code: &str) {
    let login = service.login(USER, resource).await.unwrap();
    let params = parameters(&login.authorization_url.unwrap());
    assert_eq!(params["client_id"], "registered-public-client");
    assert_eq!(params["resource"], resource);
    assert_eq!(params["code_challenge_method"], "S256");
    reqwest::Client::new()
        .get(&params["redirect_uri"])
        .query(&[("state", params["state"].as_str()), ("code", code)])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}

async fn await_token(service: &McpOAuthService, resource: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if service.check_oauth_status(USER, resource).await.unwrap().authenticated {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn discovery_dcr_persists_client_binding_and_serializes_refresh_after_restart() {
    let fixture = fixture().await;
    let (service, repo, _db) = service().await;
    let resource = format!("{}/modern/mcp", fixture.state.base);
    authorize(&service, &resource, "fixture-code").await;
    await_token(&service, &resource).await;
    let row = repo.get_by_url(USER, &resource).await.unwrap().unwrap();
    let binding: Value = serde_json::from_str(row.client_info.as_deref().unwrap()).unwrap();
    assert_eq!(binding["client_id"], "registered-public-client");
    assert_eq!(binding["resource"], resource);
    repo.upsert(UpsertOAuthTokenParams {
        user_id: USER,
        server_url: &resource,
        access_token: &row.access_token,
        refresh_token: row.refresh_token.as_deref(),
        client_info: row.client_info.as_deref(),
        token_type: "bearer",
        expires_at: Some(1),
    })
    .await
    .unwrap();
    let restarted = McpOAuthService::new(repo.clone(), reqwest::Client::new());
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let s = restarted.clone();
        let r = resource.clone();
        tasks.push(tokio::spawn(async move { s.get_token(USER, &r).await.unwrap() }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().as_deref(), Some("refreshed-access"));
    }
    assert_eq!(fixture.state.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        repo.get_by_url(USER, &resource)
            .await
            .unwrap()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("rotated-refresh")
    );
}

#[tokio::test]
async fn wrong_resource_is_rejected_before_registration_or_token_exchange() {
    let fixture = fixture().await;
    let (service, _, _db) = service().await;
    assert!(
        service
            .login(USER, &format!("{}/wrong/mcp", fixture.state.base))
            .await
            .is_err()
    );
    assert_eq!(fixture.state.registrations.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.tokens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn logout_cancels_token_exchange_and_late_callbacks_cannot_reauthenticate() {
    let fixture = fixture().await;
    let (service, repo, _db) = service().await;
    let resource = format!("{}/modern/mcp", fixture.state.base);
    authorize(&service, &resource, "slow-code").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.tokens.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    service.logout(USER, &resource).await.unwrap();
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(repo.get_by_url(USER, &resource).await.unwrap().is_none());
    assert!(
        service
            .login(USER, &resource)
            .await
            .unwrap()
            .authorization_url
            .is_some()
    );
}

#[tokio::test]
async fn discovery_rejects_redirects_malformed_oversize_issuer_pkce_and_confidential_registration() {
    type BadState = (String, &'static str, Arc<AtomicUsize>);
    for mode in [
        "redirect",
        "malformed",
        "oversize",
        "issuer",
        "pkce",
        "insecure",
        "confidential",
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let app=Router::new()
            .route("/mcp",get(|State((base,_,_)):State<BadState>|async move {(StatusCode::UNAUTHORIZED,[(header::WWW_AUTHENTICATE,format!("Bearer resource_metadata=\"{base}/metadata\""))])}))
            .route("/metadata",get(|State((base,mode,_)):State<BadState>|async move {
                match mode {
                    "redirect" => (StatusCode::FOUND,[(header::LOCATION,format!("{base}/unexpected"))]).into_response(),
                    "malformed" => "invalid json".into_response(),
                    "oversize" => " ".repeat(65537).into_response(),
                    _ => Json(json!({"resource":format!("{base}/mcp"),"authorization_servers":[format!("{base}/issuer")]})).into_response(),
                }
            }))
            .route("/.well-known/oauth-authorization-server/issuer",get(|State((base,mode,_)):State<BadState>|async move {
                Json(json!({"issuer":format!("{base}/{}",if mode=="issuer"{"wrong"}else{"issuer"}),"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":if mode=="insecure"{"http://example.invalid/token".to_string()}else{format!("{base}/token")},"registration_endpoint":format!("{base}/register"),"code_challenge_methods_supported":[if mode=="pkce"{"plain"}else{"S256"}]}))
            }))
            .route("/register",post(||async {Json(json!({"client_id":"confidential","client_secret":"fixture-secret-must-not-leak","token_endpoint_auth_method":"client_secret_post"}))}))
            .route("/unexpected",get(|State((_,_,hits)):State<BadState>|async move {hits.fetch_add(1,Ordering::SeqCst);Json(json!({}))}))
            .with_state((base.clone(),mode,hits.clone()));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (service, _, _db) = service().await;
        let resource = format!("{base}/mcp");
        for _ in 0..2 {
            let error = service.login(USER, &resource).await.unwrap_err().to_string();
            assert!(!error.contains("fixture-secret-must-not-leak"));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "redirect followed in {mode}");
        assert!(!service.check_oauth_status(USER, &resource).await.unwrap().authenticated);
        task.abort();
    }
}
