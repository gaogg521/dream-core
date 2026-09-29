//! End-to-end tests for the `GET /api/stt/stream` WebSocket endpoint.
//!
//! Exercises the full app stack: auth middleware on the upgrade request,
//! preference-backed config loading, and the streaming session protocol.
//! wiremock cannot speak WebSocket, so `dream-shell`'s own OpenAI-realtime
//! integration tests use a small tokio-tungstenite server instead — this
//! file only needs the app-level auth/protocol paths, which don't require a
//! live upstream at all.
//!
//! Hang-safety: every potentially blocking await is wrapped in a 5s timeout.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dream_core_app::{AppConfig, AppServices, create_router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Await with a deadline so a regression fails fast instead of hanging the suite.
async fn within<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(TEST_TIMEOUT, fut)
        .await
        .expect("timed out after 5s")
}

// ---------------------------------------------------------------------------
// App harness
// ---------------------------------------------------------------------------

struct TestApp {
    addr: SocketAddr,
    services: AppServices,
}

async fn start_app() -> TestApp {
    let db = dream_core_db::init_database_memory().await.unwrap();
    let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();
    let router = create_router(&services).await.expect("build router");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    TestApp { addr, services }
}

/// Sign a JWT for the seeded system user — the auth middleware verifies the
/// token AND looks the user up in the DB, so the subject must exist.
fn sign_token(app: &TestApp) -> String {
    app.services.jwt_service.sign("system_default_user", "admin").unwrap()
}

/// Seed `tools.speechToText` through the same `ClientPrefService` mechanism
/// the settings API uses (same DB pool the route's service reads from).
async fn seed_stt_prefs(app: &TestApp, config: Value) {
    let repo = Arc::new(dream_core_db::SqliteClientPreferenceRepository::new(
        app.services.database.pool().clone(),
    ));
    let service = dream_core_system::ClientPrefService::new(repo);
    let mut req = dream_core_api_types::UpdateClientPreferencesRequest::new();
    req.insert("tools.speechToText".to_owned(), config);
    service.update_preferences("system_default_user", req).await.unwrap();
}

type ClientWs = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn upgrade_request(addr: SocketAddr, token: Option<&str>) -> tungstenite::http::Request<()> {
    let mut builder = tungstenite::http::Request::builder()
        .uri(format!("ws://{addr}/api/stt/stream"))
        .header("Host", addr.to_string())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key());
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    builder.body(()).unwrap()
}

async fn connect_stream(addr: SocketAddr, token: &str) -> ClientWs {
    let (ws, _) = within(tokio_tungstenite::connect_async(upgrade_request(addr, Some(token))))
        .await
        .expect("websocket handshake failed");
    ws
}

/// Read the next protocol frame (server frames are always JSON text).
async fn read_frame(ws: &mut ClientWs) -> Value {
    loop {
        match within(ws.next()).await {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(text.as_str()).unwrap(),
            Some(Ok(Message::Close(frame))) => panic!("unexpected close frame: {frame:?}"),
            Some(Ok(_)) => continue, // ping/pong
            other => panic!("unexpected websocket read result: {other:?}"),
        }
    }
}

/// Expect the server to close the connection as the next event.
async fn read_until_close(ws: &mut ClientWs) {
    match within(ws.next()).await {
        Some(Ok(Message::Close(_))) | None => (),
        Some(Ok(other)) => panic!("expected close frame, got {other:?}"),
        Some(Err(_)) => (), // server dropped the socket after Close
    }
}

fn start_frame() -> Message {
    Message::Text(r#"{"type":"start","format":"pcm16","sampleRate":16000,"channels":1}"#.into())
}

// ===========================================================================
// Tests
// ===========================================================================

// 1. Unauthenticated upgrade requests must be rejected at the handshake by
//    the same auth middleware that guards POST /api/stt (GET bypasses CSRF,
//    so the auth middleware's 401 is what reaches the client).
#[tokio::test]
async fn unauthenticated_handshake_is_rejected() {
    let app = start_app().await;

    let err = within(tokio_tungstenite::connect_async(upgrade_request(app.addr, None)))
        .await
        .expect_err("handshake must be rejected without auth");
    match err {
        tungstenite::Error::Http(response) => assert_eq!(response.status(), 401),
        other => panic!("expected HTTP rejection, got {other:?}"),
    }
}

// 1b. An invalid token must be rejected the same way.
#[tokio::test]
async fn invalid_token_handshake_is_rejected() {
    let app = start_app().await;

    let err = within(tokio_tungstenite::connect_async(upgrade_request(
        app.addr,
        Some("not-a-valid-token"),
    )))
    .await
    .expect_err("handshake must be rejected with a bogus token");
    match err {
        tungstenite::Error::Http(response) => assert_eq!(response.status(), 401),
        other => panic!("expected HTTP rejection, got {other:?}"),
    }
}

// 2. Authed connect with STT disabled in prefs: the session answers the start
//    frame with an STT_DISABLED error frame, then the server closes.
#[tokio::test]
async fn disabled_stt_yields_error_frame_then_close() {
    let app = start_app().await;
    seed_stt_prefs(&app, json!({ "enabled": false, "provider": "openai" })).await;
    let token = sign_token(&app);

    let mut ws = connect_stream(app.addr, &token).await;
    within(ws.send(start_frame())).await.unwrap();

    let frame = read_frame(&mut ws).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "STT_DISABLED");
    assert!(frame["msg"].as_str().is_some());

    read_until_close(&mut ws).await;
}

// 3. Hosted streaming reaches the broker connector. This test app deliberately
// has no broker URL, so the connector reports a request failure rather than
// falsely claiming that the hosted provider lacks a realtime protocol.
#[tokio::test]
async fn hosted_provider_without_broker_reports_request_failure() {
    let app = start_app().await;
    seed_stt_prefs(&app, json!({ "enabled": true, "provider": "hosted" })).await;
    let token = sign_token(&app);

    let mut ws = connect_stream(app.addr, &token).await;
    within(ws.send(start_frame())).await.unwrap();

    let frame = read_frame(&mut ws).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "STT_REQUEST_FAILED");

    read_until_close(&mut ws).await;
}

// 4. Protocol violation: a binary frame before `start` is rejected with
//    STT_STREAM_PROTOCOL before any config/upstream work happens.
#[tokio::test]
async fn binary_first_frame_is_protocol_error() {
    let app = start_app().await;
    let token = sign_token(&app);

    let mut ws = connect_stream(app.addr, &token).await;
    within(ws.send(Message::Binary(vec![9u8, 9, 9].into()))).await.unwrap();

    let frame = read_frame(&mut ws).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["code"], "STT_STREAM_PROTOCOL");

    read_until_close(&mut ws).await;
}
