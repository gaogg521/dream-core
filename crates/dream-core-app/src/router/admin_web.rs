//! Serve the enterprise admin console at `/admin` on the same origin as the API.
//!
//! Production gateways (see dream-en's `deploy/Caddyfile.example`) already do this
//! with Caddy `try_files`. Local eval and single-binary installs have no gateway,
//! so dreamcore hosts the SPA itself:
//!
//! 1. `ONE_ADMIN_WEB_DIR` — directory that contains `index.html` (Vite `base: '/admin/'`).
//! 2. Walk from the executable toward a sibling `dream-en/admin-web/dist`.
//! 3. `ONE_ADMIN_UI_ORIGIN` — reverse-proxy to a running Vite (default port 25810).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use tower_http::services::{ServeDir, ServeFile};

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

enum AdminUiSource {
    Dir(PathBuf),
    Proxy(String),
}

/// Router mounted on the main (and admin) process so `GET /admin` is the SPA,
/// not the JSON `NOT_FOUND` boundary fallback.
pub fn admin_console_router() -> Router {
    match resolve_admin_ui_source() {
        Some(AdminUiSource::Dir(dir)) => {
            tracing::info!(path = %dir.display(), "admin console: serving static files at /admin");
            static_spa_router(dir)
        }
        Some(AdminUiSource::Proxy(origin)) => {
            tracing::info!(origin = %origin, "admin console: reverse-proxying /admin");
            proxy_spa_router(origin)
        }
        None => {
            tracing::warn!(
                "admin console: no assets (set ONE_ADMIN_WEB_DIR to admin-web/dist, or ONE_ADMIN_UI_ORIGIN to the Vite origin)"
            );
            Router::new()
        }
    }
}

fn static_spa_router(dir: PathBuf) -> Router {
    let index = dir.join("index.html");
    let serve = ServeDir::new(dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index));
    Router::new()
        .route("/admin", get(|| async { Redirect::temporary("/admin/") }))
        .nest_service("/admin/", serve)
}

fn proxy_spa_router(origin: String) -> Router {
    Router::new()
        .route("/admin", any(proxy_admin_ui))
        .route("/admin/", any(proxy_admin_ui))
        .route("/admin/{*rest}", any(proxy_admin_ui))
        .with_state(origin)
}

fn resolve_admin_ui_source() -> Option<AdminUiSource> {
    if let Ok(dir) = std::env::var("ONE_ADMIN_WEB_DIR") {
        let path = PathBuf::from(dir.trim());
        if dir_has_index(&path) {
            return Some(AdminUiSource::Dir(path));
        }
        tracing::warn!(path = %path.display(), "ONE_ADMIN_WEB_DIR has no index.html");
    }
    if let Some(dir) = discover_admin_web_dir() {
        return Some(AdminUiSource::Dir(dir));
    }
    let origin = std::env::var("ONE_ADMIN_UI_ORIGIN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    origin.map(AdminUiSource::Proxy)
}

fn dir_has_index(dir: &Path) -> bool {
    dir.join("index.html").is_file()
}

fn discover_admin_web_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("admin-web"));
        candidates.push(dir.join("admin-web").join("dist"));
        let mut walk = dir.to_path_buf();
        for _ in 0..8 {
            candidates.push(walk.join("dream-en").join("admin-web").join("dist"));
            candidates.push(walk.join("admin-web").join("dist"));
            if !walk.pop() {
                break;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut walk = cwd;
        for _ in 0..8 {
            candidates.push(walk.join("dream-en").join("admin-web").join("dist"));
            candidates.push(walk.join("admin-web").join("dist"));
            if !walk.pop() {
                break;
            }
        }
    }
    candidates.into_iter().find(|p| dir_has_index(p))
}

async fn proxy_admin_ui(State(origin): State<String>, req: Request) -> Response {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("admin-ui proxy client")
    });

    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());
    let url = format!("{}{path_and_query}", origin.trim_end_matches('/'));

    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let headers = req.headers().clone();
    let body = match axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => Bytes::new(),
    };

    let mut builder = client.request(method, url);
    for (name, value) in headers.iter() {
        if HOP_BY_HOP.iter().any(|h| name.as_str().eq_ignore_ascii_case(h)) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    match builder.body(body.to_vec()).send().await {
        Ok(upstream) => forward_upstream(upstream).await,
        Err(err) => {
            tracing::warn!(error = %err, "admin console proxy upstream failed");
            (StatusCode::BAD_GATEWAY, "admin UI upstream unavailable").into_response()
        }
    }
}

async fn forward_upstream(upstream: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out_headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if HOP_BY_HOP.iter().any(|h| name.as_str().eq_ignore_ascii_case(h)) {
            continue;
        }
        if name.as_str().eq_ignore_ascii_case("content-encoding") {
            continue;
        }
        if let Ok(v) = HeaderValue::from_bytes(value.as_bytes()) {
            out_headers.append(name, v);
        }
    }
    let bytes = upstream.bytes().await.unwrap_or_default();
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    if !response.headers().contains_key(header::CONTENT_TYPE) && status.is_success() {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    response
}
