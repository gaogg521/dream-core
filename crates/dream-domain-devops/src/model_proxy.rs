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
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use futures_util::TryStreamExt;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::warn;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    self as sigv4_http, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4::SigningParams;

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
    // Not hop-by-hop in the RFC sense, and — to be precise about what is and
    // is not known — not a leak observed in production either. The caller
    // today is an agent CLI using reqwest with no cookie jar (see "Not
    // session-authenticated" above; the proxy base_url reaches it through
    // ANTHROPIC_BASE_URL), so no real request carries one.
    //
    // It is here because the proxy shares an origin with the session cookie in
    // WebUI mode, so any same-origin `fetch` of a provider base_url — a media
    // adapter moved into the renderer, say — would attach `dream-session`
    // automatically, and the forwarding code would then hand it to the vendor.
    // On the Bedrock path it would additionally be folded into the SigV4
    // signature. Dropping it costs nothing and removes the whole class.
    "cookie",
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
    uri: Uri,
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

    // Bedrock channels re-sign server-side, which requires the whole body —
    // branch off before the streaming pass-through.
    if channel.platform == "bedrock" {
        return handle_bedrock_proxy(channel, method, headers, uri, body).await;
    }

    let url = build_upstream_url(&channel.upstream_base_url, &params.path, uri.query());

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

// ---------- Bedrock channels: server-side SigV4 re-signing ----------

/// The AWS credential document stored (encrypted) in
/// `one_provider_registry.api_key_encrypted` for `platform = 'bedrock'`.
/// The column already is "the real credential" for every platform; for
/// Bedrock that credential is more than one key, so it is a JSON document.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BedrockChannelCredential {
    access_key_id: String,
    secret_access_key: String,
    #[serde(default)]
    session_token: Option<String>,
    /// Falls back to the region parsed from the channel's upstream host
    /// (`bedrock-runtime.{region}.amazonaws.com`).
    #[serde(default)]
    region: Option<String>,
}

#[derive(Debug)]
struct BedrockTarget {
    url: String,
    region: String,
    credentials: Credentials,
}

/// Parse the channel's credential document and resolve the signed request's
/// target URL (path-preserving, query preserved).
fn parse_bedrock_target(
    channel: &crate::provider_channel::ResolvedChannel,
    path: &str,
    query: Option<&str>,
) -> Result<BedrockTarget, String> {
    let credential: BedrockChannelCredential = serde_json::from_str(&channel.api_key).map_err(|_| {
        "this Bedrock channel's credential is not a valid {accessKeyId, secretAccessKey, region} document; \
             re-save it in the admin console"
            .to_string()
    })?;
    let region = match credential.region.as_deref() {
        Some(region) if !region.trim().is_empty() => region.trim().to_owned(),
        _ => region_from_bedrock_host(&channel.upstream_base_url).ok_or_else(|| {
            "this Bedrock channel's credential has no region and its upstream URL does not name one".to_string()
        })?,
    };
    let credentials = Credentials::new(
        credential.access_key_id,
        credential.secret_access_key,
        credential.session_token,
        None,
        "one-model-proxy",
    );
    Ok(BedrockTarget {
        url: build_upstream_url(&channel.upstream_base_url, path, query),
        region,
        credentials,
    })
}

/// `https://bedrock-runtime.us-east-1.amazonaws.com` → `us-east-1`. Works for
/// the `….amazonaws.com.cn` partitions too, since the region is always the
/// label right after `bedrock-runtime`. A host that is not a Bedrock regional
/// endpoint names no region — the credential document must.
/// The `host[:port]` a request to `url` will actually carry, for the SigV4
/// `host` header. The default port is omitted because that is what an HTTP
/// client puts on the wire, and SigV4 verifies the header byte-for-byte.
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    let is_https = url.starts_with("https://");
    let trimmed = authority
        .strip_suffix(if is_https { ":443" } else { ":80" })
        .unwrap_or(authority);
    Some(trimmed.to_owned())
}

fn region_from_bedrock_host(base_url: &str) -> Option<String> {
    let host = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(base_url);
    let host = host.split('/').next().unwrap_or(host);
    let mut labels = host.split('.');
    let first = labels.next()?;
    if !first.starts_with("bedrock-runtime") {
        return None;
    }
    labels.next().map(|region| region.to_owned())
}

/// `{base}/{path}[?{query}]` — the path-preserving shape every platform
/// forwards by. The query used to be dropped here (axum's `Path` extractor
/// never sees it), which only mattered once a protocol actually used one.
fn build_upstream_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let mut url = format!("{}/{}", base_url.trim_end_matches('/'), path.trim_start_matches('/'));
    if let Some(query) = query {
        if !query.is_empty() {
            url.push('?');
            url.push_str(query);
        }
    }
    url
}

/// SigV4 over exactly the request the proxy will send: method, full URL
/// (signed URLs include host and query), the non-hop-by-hop headers, and the
/// whole payload — which is why Bedrock bodies are buffered while every other
/// platform streams. `now` is a parameter so tests can pin the clock.
fn sign_bedrock_request(
    method: &str,
    url: &str,
    headers: &HeaderMap,
    body: &[u8],
    region: &str,
    credentials: &Credentials,
    now: std::time::SystemTime,
) -> Result<HeaderMap, String> {
    let mut signing_settings = SigningSettings::default();
    signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    signing_settings.signature_location = SignatureLocation::Headers;

    let identity = credentials.clone().into();
    let signing_params = SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name("bedrock")
        .time(now)
        .settings(signing_settings)
        .build()
        .map_err(|e| format!("SigV4 params error: {e}"))?;

    // Sign exactly the headers that will actually be sent, and no others.
    //
    // Passing the inbound `HeaderMap` through was wrong in three ways, all of
    // which SigV4 turns into a hard failure or a leak rather than a warning:
    //
    //  * `host` — SigV4 always signs it, and the inbound value is THIS proxy's
    //    host. reqwest then rewrites Host from the upstream URL, so what AWS
    //    verifies never matches what was signed: a guaranteed 403
    //    SignatureDoesNotMatch on every Bedrock call, with correct credentials.
    //  * `cookie` — signed AND forwarded, handing the member's `dream-session`
    //    to AWS. Now dropped for every platform (see HOP_BY_HOP).
    //  * `content-length` — signed from the inbound request while reqwest sets
    //    its own from the body we hand it.
    //
    // `authorization` is excluded by aws-sigv4 itself and then overwritten with
    // the signature, so the member's channel token never reached AWS — that one
    // was already safe.
    let mut signable_headers = HeaderMap::new();
    for (name, value) in headers.iter() {
        if !is_hop_by_hop(name.as_str()) {
            signable_headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(host) = host_of(url) {
        let host = HeaderValue::from_str(&host).map_err(|e| format!("SigV4 host header error: {e}"))?;
        signable_headers.insert(axum::http::header::HOST, host);
    }
    let header_pairs: Vec<(&str, &str)> = signable_headers
        .iter()
        .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str(), v)))
        .collect();

    let signable_request = SignableRequest::new(method, url, header_pairs.into_iter(), SignableBody::Bytes(body))
        .map_err(|e| format!("SigV4 signable request error: {e}"))?;

    let (signing_instructions, _signature) = sigv4_http::sign(signable_request, &signing_params.into())
        .map_err(|e| format!("SigV4 signing error: {e}"))?
        .into_parts();

    // Start from the filtered set, not the inbound one: what goes on the wire
    // must be exactly what was signed, or the canonical request AWS rebuilds
    // will not match.
    let mut signed_headers = signable_headers;
    for (name, value) in signing_instructions.headers() {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| format!("SigV4 header name error: {e}"))?;
        let value = HeaderValue::from_str(value).map_err(|e| format!("SigV4 header value error: {e}"))?;
        signed_headers.insert(name, value);
    }
    Ok(signed_headers)
}

async fn handle_bedrock_proxy(
    channel: crate::provider_channel::ResolvedChannel,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> Response {
    // SigV4 covers the payload hash, so the whole body must be in hand before
    // any header goes out. Bedrock invokes are JSON documents — tens of KB,
    // not the tens of MB the media pass-through streams — and the 64 MB
    // RequestBodyLimitLayer still bounds a hostile request.
    let body_bytes = match axum::body::to_bytes(body, PROXY_BODY_LIMIT).await {
        Ok(bytes) => bytes.to_vec(),
        Err(err) => {
            warn!(channel_id = %channel.id, error = %err, "model_proxy: bedrock body read failed");
            return bad_gateway("failed to read the request body");
        }
    };

    let target = match parse_bedrock_target(&channel, uri.path(), uri.query()) {
        Ok(target) => target,
        Err(message) => {
            warn!(channel_id = %channel.id, error = %message, "model_proxy: bedrock channel misconfigured");
            return bad_gateway(&message);
        }
    };

    // The caller's own credential headers are hop-by-hop and already dropped;
    // whatever survives is signed and sent verbatim.
    let signed_headers = match sign_bedrock_request(
        method.as_str(),
        &target.url,
        &headers,
        &body_bytes,
        &target.region,
        &target.credentials,
        std::time::SystemTime::now(),
    ) {
        Ok(headers) => headers,
        Err(message) => {
            warn!(channel_id = %channel.id, error = %message, "model_proxy: bedrock re-sign failed");
            return bad_gateway(&message);
        }
    };

    let client = reqwest::Client::new();
    let upstream = match client
        .request(method, &target.url)
        .headers(signed_headers)
        .body(body_bytes)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            warn!(channel_id = %channel.id, error = %err, "model_proxy: bedrock upstream request failed");
            return bad_gateway(&format!("upstream request failed: {err}"));
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
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn bad_gateway(message: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        axum::Json(serde_json::json!({
            "error": { "message": message, "type": "server_error" }
        })),
    )
        .into_response()
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

    // ---------- Bedrock channels: SigV4 re-signing ----------

    fn bedrock_channel(upstream_base_url: &str, api_key: &str) -> crate::provider_channel::ResolvedChannel {
        crate::provider_channel::ResolvedChannel {
            id: "ch-1".into(),
            platform: "bedrock".into(),
            upstream_base_url: upstream_base_url.into(),
            api_key: api_key.into(),
            user_id: "admin1".into(),
        }
    }

    const BEDROCK_CREDENTIAL_JSON: &str =
        r#"{"accessKeyId":"AKID","secretAccessKey":"shhh","sessionToken":"tok","region":"us-east-1"}"#;

    #[test]
    fn bedrock_credential_document_parses_with_region() {
        let target = parse_bedrock_target(
            &bedrock_channel(
                "https://bedrock-runtime.us-west-2.amazonaws.com",
                BEDROCK_CREDENTIAL_JSON,
            ),
            "/model/x/invoke-with-response-stream",
            None,
        )
        .expect("valid credential document should parse");
        assert_eq!(target.region, "us-east-1");
        assert_eq!(
            target.url,
            "https://bedrock-runtime.us-west-2.amazonaws.com/model/x/invoke-with-response-stream"
        );
    }

    #[test]
    fn bedrock_region_falls_back_to_the_upstream_host() {
        let without_region = r#"{"accessKeyId":"AKID","secretAccessKey":"shhh"}"#;
        let target = parse_bedrock_target(
            &bedrock_channel("https://bedrock-runtime.eu-central-1.amazonaws.com", without_region),
            "/model/x/invoke-with-response-stream",
            None,
        )
        .expect("host-named region should fill the gap");
        assert_eq!(target.region, "eu-central-1");

        // China partitions keep the region in the label after bedrock-runtime.
        let target = parse_bedrock_target(
            &bedrock_channel("https://bedrock-runtime.cn-north-1.amazonaws.com.cn", without_region),
            "/model/x/invoke-with-response-stream",
            None,
        )
        .expect("china partition should parse");
        assert_eq!(target.region, "cn-north-1");
    }

    #[test]
    fn bedrock_channel_without_any_region_is_a_configuration_error() {
        let without_region = r#"{"accessKeyId":"AKID","secretAccessKey":"shhh"}"#;
        let error = parse_bedrock_target(
            &bedrock_channel("https://gateway.internal", without_region),
            "/model/x/invoke-with-response-stream",
            None,
        )
        .unwrap_err();
        assert!(error.contains("region"));
    }

    #[test]
    fn bedrock_channel_with_a_non_json_credential_is_rejected() {
        let error = parse_bedrock_target(
            &bedrock_channel("https://bedrock-runtime.us-east-1.amazonaws.com", "sk-plain-key"),
            "/model/x/invoke-with-response-stream",
            None,
        )
        .unwrap_err();
        assert!(error.contains("re-save"));
    }

    #[test]
    fn upstream_url_preserves_the_query_string() {
        assert_eq!(
            build_upstream_url("https://api.test/v1", "/chat/completions", Some("a=1&b=2")),
            "https://api.test/v1/chat/completions?a=1&b=2"
        );
        assert_eq!(
            build_upstream_url("https://api.test/v1/", "/x", None),
            "https://api.test/v1/x"
        );
        assert_eq!(
            build_upstream_url("https://api.test", "/x", Some("")),
            "https://api.test/x"
        );
    }

    #[test]
    fn bedrock_signing_produces_sigv4_headers_over_the_full_url() {
        let target = parse_bedrock_target(
            &bedrock_channel(
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                BEDROCK_CREDENTIAL_JSON,
            ),
            "/model/claude/invoke-with-response-stream",
            None,
        )
        .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        // Pinned clock: 2026-09-04T00:00:00Z.
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_785_840_000);

        let signed = sign_bedrock_request(
            "POST",
            &target.url,
            &headers,
            br#"{"x":1}"#,
            &target.region,
            &target.credentials,
            now,
        )
        .expect("signing should succeed");

        let authorization = signed
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .expect("signed request carries an authorization header");
        assert!(authorization.starts_with("AWS4-HMAC-SHA256"), "got: {authorization}");
        assert!(authorization.contains("Credential=AKID/"), "got: {authorization}");
        assert!(authorization.contains("/bedrock/aws4_request"), "got: {authorization}");
        let amz_date = signed
            .get("x-amz-date")
            .and_then(|v| v.to_str().ok())
            .expect("x-amz-date");
        assert!(
            amz_date.starts_with("2026"),
            "x-amz-date should reflect the pinned clock, got: {amz_date}"
        );
        let payload_hash = signed
            .get("x-amz-content-sha256")
            .and_then(|v| v.to_str().ok())
            .expect("payload checksum header");
        assert_ne!(payload_hash, "UNSIGNED-PAYLOAD");
    }

    /// The previous signing test passed a clean `content-type`-only map, which
    /// is not what an inbound proxy request looks like: it carries this proxy's
    /// `host` and the caller's channel token, and SigV4 signs whatever it is
    /// handed — so `host` ended up in the signature as the proxy's while the
    /// client sent the upstream's. `cookie` is asserted here as a guard rather
    /// than a reproduction: no caller sends one today (see HOP_BY_HOP), and
    /// this keeps it that way if one ever does.
    #[test]
    fn bedrock_signing_covers_only_what_is_actually_sent() {
        let target = parse_bedrock_target(
            &bedrock_channel(
                "https://bedrock-runtime.us-east-1.amazonaws.com",
                BEDROCK_CREDENTIAL_JSON,
            ),
            "/model/claude/invoke",
            None,
        )
        .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("host", HeaderValue::from_static("172.29.128.120:25810"));
        headers.insert("authorization", HeaderValue::from_static("Bearer member-channel-token"));
        headers.insert("content-length", HeaderValue::from_static("7"));
        headers.insert("cookie", HeaderValue::from_static("dream-session=secret"));
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_785_840_000);

        let signed = sign_bedrock_request(
            "POST",
            &target.url,
            &headers,
            br#"{"x":1}"#,
            &target.region,
            &target.credentials,
            now,
        )
        .expect("signing should succeed");

        // `host` must be the upstream's. Signing this proxy's host is a
        // guaranteed 403 from AWS, because the client rewrites Host from the
        // URL before the request leaves.
        assert_eq!(
            signed.get("host").and_then(|v| v.to_str().ok()),
            Some("bedrock-runtime.us-east-1.amazonaws.com")
        );

        // The member's session cookie is this deployment's, not AWS's.
        assert!(
            signed.get("cookie").is_none(),
            "the member session cookie must not reach the vendor"
        );

        let authorization = signed
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .expect("authorization");
        assert!(authorization.starts_with("AWS4-HMAC-SHA256"), "got: {authorization}");
        let signed_list = authorization
            .split("SignedHeaders=")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .expect("SignedHeaders");
        assert!(
            signed_list.contains("host"),
            "SigV4 always signs host; got: {signed_list}"
        );
        for leaked in ["cookie", "content-length"] {
            assert!(
                !signed_list.split(';').any(|h| h == leaked),
                "'{leaked}' must not be signed — it is not what goes on the wire; got: {signed_list}"
            );
        }
    }

    #[test]
    fn host_of_drops_the_default_port_and_any_userinfo() {
        assert_eq!(
            host_of("https://bedrock-runtime.us-east-1.amazonaws.com/x").as_deref(),
            Some("bedrock-runtime.us-east-1.amazonaws.com")
        );
        // Default ports are not written on the wire, and SigV4 compares bytes.
        assert_eq!(host_of("https://example.com:443/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://example.com:80/x").as_deref(), Some("example.com"));
        // A non-default port IS part of the Host header.
        assert_eq!(
            host_of("https://example.com:8443/x").as_deref(),
            Some("example.com:8443")
        );
        assert_eq!(host_of("https://user:pw@example.com/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("https://example.com?a=1").as_deref(), Some("example.com"));
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
