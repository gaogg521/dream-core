//! Local HTTP fixtures for attachment upload and platform acknowledgement tests.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    response::Response,
    routing::any,
};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

pub(crate) type Captures = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

pub(crate) async fn server(responses: Vec<serde_json::Value>) -> (String, Captures, tokio::task::JoinHandle<()>) {
    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    let remaining = Arc::new(Mutex::new(VecDeque::from(responses)));
    let saved = captures.clone();
    let app = Router::new().fallback(any(move |request: Request| {
        let remaining = remaining.clone();
        let captures = captures.clone();
        async move {
            let path = request.uri().to_string();
            let body = to_bytes(request.into_body(), 25 * 1024 * 1024).await.unwrap();
            captures.lock().await.push((path, body.to_vec()));
            let result = remaining
                .lock()
                .await
                .pop_front()
                .unwrap_or(serde_json::json!({"error":"unexpected request"}));
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(result.to_string()))
                .unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, saved, handle)
}

pub(crate) async fn file() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("employee-report-{}.csv", dream_core_common::generate_id()));
    tokio::fs::write(&path, "name,value\nnotification-proof,42\n")
        .await
        .unwrap();
    path
}
