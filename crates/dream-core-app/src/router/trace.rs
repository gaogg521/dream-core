//! HTTP request access-log layer.

use std::time::Instant;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use dream_core_common::{ApiErrorLogContext, generate_short_id};

const REQUEST_ID_HEADER: &str = "x-request-id";
const MAX_QUERY_KEYS: usize = 16;

pub(super) fn with_access_log(router: Router) -> Router {
    router.layer(middleware::from_fn(access_log))
}

async fn access_log(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query_keys = query_keys(request.uri().query());
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(generate_short_id);

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let latency_ms = started.elapsed().as_millis() as u64;
    // Domain crates (org, devops, billing, …) render their own JSON error
    // bodies and never attach `ApiErrorLogContext`, so their 4xx/5xx lines
    // used to log `error_code=""`. Fall back to the body they already sent.
    let (response, body_context) = if status >= 400 && response.extensions().get::<ApiErrorLogContext>().is_none() {
        error_from_json_body(response).await
    } else {
        (response, None)
    };
    let error_context = response.extensions().get::<ApiErrorLogContext>();
    let (error_code, error_message) = match (error_context, body_context.as_ref()) {
        (Some(context), _) => (context.code, context.message.as_str()),
        (None, Some((code, message))) => (code.as_str(), message.as_str()),
        (None, None) => ("", ""),
    };

    if status >= 500 {
        tracing::error!(
            request_id = %request_id,
            method = %method,
            path = %path,
            query_keys = %query_keys,
            status,
            latency_ms,
            error_code,
            error_message,
            "http response"
        );
    } else if status >= 400 {
        tracing::warn!(
            request_id = %request_id,
            method = %method,
            path = %path,
            query_keys = %query_keys,
            status,
            latency_ms,
            error_code,
            error_message,
            "http response"
        );
    } else {
        tracing::info!(
            request_id = %request_id,
            method = %method,
            path = %path,
            query_keys = %query_keys,
            status,
            latency_ms,
            "http response"
        );
    }

    response
}

/// Error bodies larger than this are not inspected (and are passed through
/// untouched); every `ErrorResponse` is far smaller.
const MAX_ERROR_BODY: usize = 16 * 1024;
const MAX_LOGGED_MESSAGE: usize = 300;

/// Read `{"code": …, "error": …}` from a JSON error response, returning the
/// response with an identical body. Non-JSON, streamed or oversized bodies are
/// returned as they were, with no context.
async fn error_from_json_body(response: Response) -> (Response, Option<(String, String)>) {
    let is_json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    // `Json(...)` bodies are fully buffered and report an exact size; a
    // streamed body has no upper bound and must never be drained here.
    let small = axum::body::HttpBody::size_hint(response.body())
        .upper()
        .is_some_and(|len| len as usize <= MAX_ERROR_BODY);
    if !is_json || !small {
        return (response, None);
    }
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_ERROR_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return (Response::from_parts(parts, axum::body::Body::empty()), None),
    };
    let context = serde_json::from_slice::<serde_json::Value>(&bytes).ok().map(|value| {
        let field = |name: &str| value.get(name).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let mut message = field("error");
        if message.len() > MAX_LOGGED_MESSAGE {
            let mut cut = MAX_LOGGED_MESSAGE;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
        }
        (field("code"), message)
    });
    (Response::from_parts(parts, axum::body::Body::from(bytes)), context)
}

fn query_keys(query: Option<&str>) -> String {
    query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter_map(|pair| {
            let key = pair.split_once('=').map_or(pair, |(key, _)| key).trim();
            (!key.is_empty()).then_some(key)
        })
        .take(MAX_QUERY_KEYS)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn domain_json_errors_are_read_for_the_log_and_passed_through_intact() {
        let body = r#"{"success":false,"error":"Bad request: department has sub-departments","code":"BAD_REQUEST"}"#;
        let response = Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let (response, context) = error_from_json_body(response).await;
        assert_eq!(
            context,
            Some((
                "BAD_REQUEST".to_owned(),
                "Bad request: department has sub-departments".to_owned()
            ))
        );
        let sent = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(sent, body.as_bytes());
    }

    #[tokio::test]
    async fn non_json_error_bodies_are_left_alone() {
        let response = Response::builder()
            .status(502)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from("data: x"))
            .unwrap();
        let (response, context) = error_from_json_body(response).await;
        assert_eq!(context, None);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "data: x"
        );
    }

    #[test]
    fn query_keys_omit_values() {
        assert_eq!(
            query_keys(Some("path=/Users/alice/project&token=secret&flag&empty=")),
            "path,token,flag,empty"
        );
    }
}
