//! Black-box tests for the mode-B metered-proxy relay routes
//! (`/api/providers/metered/*`). A `wiremock` server stands in for
//! `dream-trial-broker`; these verify dream-core forwards the install id and
//! maps the broker's status codes onto `ApiResponse` / `ApiError`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use dream_core_realtime::BroadcastEventBus;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use dream_core_db::{
    Database, SqliteClientPreferenceRepository, SqliteFeedbackDiagnosticsRepository, SqliteProviderRepository,
    SqliteSettingsRepository, init_database_memory,
};
use dream_core_system::{
    ClientPrefService, FeedbackDiagnosticsService, MeteredAccessService, ModelFetchService, ProtocolDetectionService,
    ProviderService, RuntimePrepareService, SettingsService, SystemRouterState, VersionCheckService, system_routes,
};

const TEST_KEY: [u8; 32] = [0x42; 32];

fn build_state(db: &Database, broker_url: Option<String>) -> SystemRouterState {
    let provider_repo = Arc::new(SqliteProviderRepository::new(db.pool().clone()));
    let http_client = reqwest::Client::new();
    SystemRouterState {
        settings_service: SettingsService::new(Arc::new(SqliteSettingsRepository::new(db.pool().clone()))),
        client_pref_service: ClientPrefService::new(Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()))),
        provider_service: ProviderService::new(provider_repo.clone(), TEST_KEY),
        managed_provider_sync: None,
        trial_key_service: dream_core_system::TrialKeyService::new(
            None,
            http_client.clone(),
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone())),
        ),
        metered_access_service: MeteredAccessService::new(
            broker_url,
            http_client.clone(),
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone())),
        ),
        content_inspection: std::sync::Arc::new(dream_core_system::ContentInspectionService::new()),
        model_fetch_service: ModelFetchService::new(provider_repo, TEST_KEY, http_client.clone()),
        protocol_detection_service: ProtocolDetectionService::new(http_client.clone()),
        version_check_service: VersionCheckService::new(http_client, "0.1.0".to_owned()),
        runtime_prepare_service: RuntimePrepareService::new(Arc::new(BroadcastEventBus::new(16))),
        feedback_diagnostics_service: FeedbackDiagnosticsService::new(Arc::new(
            SqliteFeedbackDiagnosticsRepository::new(db.pool().clone()),
        )),
    }
}

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().method("GET").uri(uri).body(Body::empty()).unwrap()
}

async fn json_of(resp: axum::response::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn claim_forwards_the_install_id_and_relays_the_broker_answer() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/claim"))
        .and(body_partial_json(json!({ "vendor": "baoyun" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "vendor": "baoyun",
            "base_url": "https://broker.example/v1/metered/proxy/baoyun",
            "device_token": "dtk_abc",
            "models": ["deepseek-chat"],
            "currency": "CNY",
            "free_grant_cents": 1000,
            "remaining_cents": 1000,
        })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, body) = json_of(
        app.oneshot(post("/api/providers/metered/claim", json!({ "vendor": "baoyun" })))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["device_token"], "dtk_abc");
    assert_eq!(
        body["data"]["base_url"],
        "https://broker.example/v1/metered/proxy/baoyun"
    );
    assert_eq!(body["data"]["remaining_cents"], 1000);

    // The broker actually saw an install_id (dream-core minted one).
    let received = &broker.received_requests().await.unwrap()[0];
    let sent: Value = serde_json::from_slice(&received.body).unwrap();
    assert!(
        sent["install_id"].as_str().is_some_and(|s| s.starts_with("install")),
        "dream-core must forward its own install id, got {sent}"
    );
}

#[tokio::test]
async fn an_unknown_vendor_from_the_broker_is_a_404() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/claim"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "metered_vendor_unknown" })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, body) = json_of(
        app.oneshot(post("/api/providers/metered/claim", json!({ "vendor": "nope" })))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn quota_reads_the_local_ledger_via_the_broker() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/quota/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "vendor": "baoyun",
            "currency": "CNY",
            "free_grant_cents": 1000,
            "purchased_cents": 5900,
            "consumed_cents": 220,
            "remaining_cents": 6680,
            "exhausted": false,
        })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, body) = json_of(
        app.oneshot(get("/api/providers/metered/quota?vendor=baoyun"))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["remaining_cents"], 6680);
    assert_eq!(body["data"]["exhausted"], false);
}

#[tokio::test]
async fn quota_for_an_install_that_never_claimed_is_a_404() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/quota/status"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "metered_account_unknown" })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, _) = json_of(
        app.oneshot(get("/api/providers/metered/quota?vendor=baoyun"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creating_an_order_relays_the_gateway_payment_payload() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/orders"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "ord-1",
            "vendor": "baoyun",
            "package_id": "59",
            "amount_cents": 5900,
            "credit_cents": 5900,
            "currency": "CNY",
            "status": "pending",
            "gateway": "mock",
            "payment": { "kind": "mock", "pay_url": "mock://pay/ord-1" },
        })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, body) = json_of(
        app.oneshot(post(
            "/api/providers/metered/orders",
            json!({ "vendor": "baoyun", "package_id": "59" }),
        ))
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], "ord-1");
    assert_eq!(body["data"]["payment"]["pay_url"], "mock://pay/ord-1");
}

#[tokio::test]
async fn an_unknown_package_is_a_400() {
    let broker = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/metered/orders"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "metered_package_unknown" })))
        .mount(&broker)
        .await;

    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, Some(broker.uri())));

    let (status, _) = json_of(
        app.oneshot(post(
            "/api/providers/metered/orders",
            json!({ "vendor": "baoyun", "package_id": "999" }),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn with_no_broker_configured_the_routes_report_plainly() {
    let db = init_database_memory().await.unwrap();
    let app = system_routes(build_state(&db, None));

    let (status, body) = json_of(
        app.oneshot(post("/api/providers/metered/claim", json!({ "vendor": "baoyun" })))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}
