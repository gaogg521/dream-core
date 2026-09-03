//! The model proxy: the only place a company channel's credential is used.
//!
//! A member's desktop gets a normal-looking provider whose `base_url` points
//! here and whose `api_key` is their revocable channel token. Everything
//! downstream — dream chat, the ACP bridges, all three media adapter forms —
//! then works unchanged, because they only ever read a base URL and a key.
//!
//! # Why path-preserving rather than protocol-aware
//!
//! `dream-codex-bridge` decodes requests into `LlmEvent`s and re-encodes them.
//! That is right for translating between two protocols, and wrong here: media
//! alone speaks three mutually incompatible shapes (`/v1/images/generations`,
//! chat completions, and the DashScope/Ark async task APIs), so a
//! protocol-aware proxy would need a branch per shape and would break the day a
//! vendor adds a fourth.
//!
//! Instead the whole path after the channel id is forwarded verbatim:
//!
//! ```text
//! ANY /api/one/model-proxy/{channel_id}/{*path}
//!   → {channel.upstream_base_url}/{path}   + the real Authorization
//! ```
//!
//! so `/v1/chat/completions`, `/v1/images/generations` and
//! `/api/v1/services/aigc/...` all arrive intact with no per-protocol code.
//!
//! # Streaming
//!
//! Request and response bodies are streamed, never buffered. Buffering would
//! turn every streamed chat reply into a long silence followed by the whole
//! answer at once, and would hold multi-megabyte image payloads in memory on
//! the server for no reason.
//!
//! # Not session-authenticated
//!
//! The caller is an agent process, not a browser: it presents a channel token,
//! not a session cookie. Same arrangement as the Codex bridge's public route,
//! and the same reason it must sit outside the CSRF layer.

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures_util::TryStreamExt;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::warn;

use crate::state::OneDevopsRouterState;

/// Image-to-image sends reference images inline as data URIs, which routinely
/// exceeds the app-wide 10 MB default. This is a pass-through — the body is
/// never held whole — so the cap only exists to bound a hostile request.
const PROXY_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// Headers that describe *this* hop and must not be copied to the next one.
/// `authorization` and `x-api-key` are replaced with the company credential
/// (whichever of the two the destination protocol wants); `host` must follow
/// the upstream URL; the rest are connection-level.
const HOP_BY_HOP: &[&str] = &[
    "authorization",
    "x-api-key",
    "host",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| name.eq_ignore_ascii_case(h))
}

pub fn model_proxy_routes(state: OneDevopsRouterState) -> Router {
    Router::new()
        .route("/api/one/model-proxy/{channel_id}/{*path}", any(handle_proxy))
        .with_state(state)
        // Replace the app-wide default rather than layering under it: a
        // pass-through has different constraints from a JSON endpoint.
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(PROXY_BODY_LIMIT))
}

#[derive(serde::Deserialize)]
struct ProxyPath {
    channel_id: String,
    path: String,
}

/// Bearer token from the request, if it looks like one.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// `x-api-key` value from the request, if present.
fn api_key_header(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("x-api-key")?.to_str().ok()?;
    let token = raw.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// The channel token, however the caller's local transport happened to send
/// it.
///
/// The member's desktop doesn't know it's talking to this proxy instead of
/// the real vendor: it builds the request the same way it would for a direct
/// connection, which means the credential shape follows the *destination*
/// platform, not a proxy-specific convention. OpenAI-compatible transports
/// (including the `gemini` platform, which is mapped to the OpenAI-compatible
/// transport client-side) send `Authorization: Bearer`; Anthropic sends
/// `x-api-key`. Both carry the same channel token and are checked the same
/// way once extracted.
fn channel_token(headers: &HeaderMap) -> Option<String> {
    bearer_token(headers).or_else(|| api_key_header(headers))
}

/// The header name and value used to present the real vendor credential to
/// `channel.upstream_base_url`, chosen by the destination platform's
/// protocol — the one place a company channel's Anthropic secret differs in
/// transport from every other platform this proxy serves.
///
/// `gemini` is deliberately absent: it is materialized to members as the
/// OpenAI-compatible transport (see `resolve_dream_engine_url_and_compat` in
/// dream-core-ai-agent), so it already needs `Authorization: Bearer` like
/// every other non-Anthropic platform.
fn credential_header(platform: &str, api_key: &str) -> (&'static str, String) {
    if platform == "anthropic" {
        ("x-api-key", api_key.to_owned())
    } else {
        ("Authorization", format!("Bearer {api_key}"))
    }
}

/// One flat error shape for every rejection.
///
/// Deliberately indistinguishable across "no token", "revoked token", "token
/// for another channel" and "no such channel": telling them apart would let
/// anyone holding one token enumerate the company's channels.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "error": {
                "message": "not authorized for this model channel",
                "type": "invalid_request_error"
            }
        })),
    )
        .into_response()
}

async fn handle_proxy(
    State(state): State<OneDevopsRouterState>,
    Path(params): Path<ProxyPath>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(token) = channel_token(&headers) else {
        return unauthorized();
    };

    let channel = match state
        .service
        .resolve_channel_for_token(&params.channel_id, &token)
        .await
    {
        Ok(Some(channel)) => channel,
        Ok(None) => return unauthorized(),
        Err(err) => {
            // A misconfigured channel (no credential, unreadable key) is the
            // operator's problem, not the caller's — say so rather than
            // pretending they are unauthorized, which would send them hunting
            // through their own settings.
            warn!(
                channel_id = %params.channel_id,
                error = %err,
                "model_proxy: channel could not be resolved"
            );
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": { "message": err.to_string(), "type": "server_error" }
                })),
            )
                .into_response();
        }
    };

    let url = format!("{}/{}", channel.upstream_base_url, params.path.trim_start_matches('/'));

    let client = reqwest::Client::new();
    let mut request = client.request(method, &url);
    for (name, value) in headers.iter() {
        if !is_hop_by_hop(name.as_str()) {
            request = request.header(name.as_str(), value.as_bytes());
        }
    }
    let (credential_name, credential_value) = credential_header(&channel.platform, &channel.api_key);
    request = request.header(credential_name, credential_value);
    // Stream the request body straight through: an image-to-image payload can
    // be tens of megabytes and there is no reason for it to land in memory.
    request = request.body(reqwest::Body::wrap_stream(
        body.into_data_stream().map_err(std::io::Error::other),
    ));

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            // Never log the URL with credentials or the body; the channel id
            // and the user are enough to find the misconfiguration.
            warn!(
                channel_id = %channel.id,
                user_id = %channel.user_id,
                error = %err,
                "model_proxy: upstream request failed"
            );
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": { "message": format!("upstream request failed: {err}"), "type": "server_error" }
                })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers().iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_ref()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            builder = builder.header(name, value);
        }
    }

    // Stream the response too, so SSE arrives token by token instead of as one
    // long pause followed by the whole answer.
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn a_bearer_token_is_read_in_either_case() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer onech-abc"));
        assert_eq!(bearer_token(&headers).as_deref(), Some("onech-abc"));

        headers.insert("authorization", HeaderValue::from_static("bearer onech-abc"));
        assert_eq!(bearer_token(&headers).as_deref(), Some("onech-abc"));
    }

    #[test]
    fn anything_that_is_not_a_bearer_token_is_no_token() {
        let mut headers = HeaderMap::new();
        assert!(bearer_token(&headers).is_none());
        headers.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert!(bearer_token(&headers).is_none());
        headers.insert("authorization", HeaderValue::from_static("Bearer   "));
        assert!(bearer_token(&headers).is_none());
    }

    /// The caller's own Authorization must never reach upstream — it carries
    /// the channel token, and the upstream gets the company credential instead.
    #[test]
    fn the_callers_credentials_are_not_forwarded() {
        assert!(is_hop_by_hop("Authorization"));
        assert!(is_hop_by_hop("authorization"));
        assert!(is_hop_by_hop("x-api-key"));
        assert!(is_hop_by_hop("X-Api-Key"));
        assert!(is_hop_by_hop("Host"));
        assert!(is_hop_by_hop("Content-Length"));
        // …while everything the model API actually needs passes through.
        assert!(!is_hop_by_hop("content-type"));
        assert!(!is_hop_by_hop("accept"));
        assert!(!is_hop_by_hop("x-dashscope-async"));
    }

    #[test]
    fn an_x_api_key_header_is_read_as_a_channel_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("onech-abc"));
        assert_eq!(api_key_header(&headers).as_deref(), Some("onech-abc"));

        headers.insert("x-api-key", HeaderValue::from_static("   "));
        assert!(api_key_header(&headers).is_none());
    }

    /// Anthropic's client-side transport sends `x-api-key`, never `Authorization:
    /// Bearer` — the front door has to recognize whichever one the caller's
    /// local transport used for the destination platform.
    #[test]
    fn channel_token_accepts_either_bearer_or_x_api_key() {
        let mut bearer_only = HeaderMap::new();
        bearer_only.insert("authorization", HeaderValue::from_static("Bearer onech-abc"));
        assert_eq!(channel_token(&bearer_only).as_deref(), Some("onech-abc"));

        let mut api_key_only = HeaderMap::new();
        api_key_only.insert("x-api-key", HeaderValue::from_static("onech-def"));
        assert_eq!(channel_token(&api_key_only).as_deref(), Some("onech-def"));

        // Bearer wins when a caller somehow sends both — matches the order
        // `channel_token` tries them in.
        let mut both = HeaderMap::new();
        both.insert("authorization", HeaderValue::from_static("Bearer onech-bearer"));
        both.insert("x-api-key", HeaderValue::from_static("onech-apikey"));
        assert_eq!(channel_token(&both).as_deref(), Some("onech-bearer"));

        assert!(channel_token(&HeaderMap::new()).is_none());
    }

    #[test]
    fn anthropic_channels_forward_an_x_api_key_header() {
        let (name, value) = credential_header("anthropic", "real-vendor-secret");
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "real-vendor-secret");
    }

    #[test]
    fn every_other_platform_forwards_a_bearer_header() {
        for platform in ["openai", "gemini", "custom", "new-api", ""] {
            let (name, value) = credential_header(platform, "real-vendor-secret");
            assert_eq!(name, "Authorization", "platform = {platform}");
            assert_eq!(value, "Bearer real-vendor-secret", "platform = {platform}");
        }
    }
}
