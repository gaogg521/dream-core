use dream_core_api_types::{AgentErrorCode, AgentErrorOwnership};

use super::*;

#[test]
fn aionrs_structured_malformed_tool_call_error_is_provider_error() {
    let error = DreamEngineAgentError::ToolCallMalformed { count: 3, limit: 3 };
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderInvalidRequest));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(false));
}

#[test]
fn aionrs_provider_rate_limited_appends_response_body_to_detail() {
    let error = DreamEngineAgentError::Provider(ProviderError::RateLimited {
        retry_after_ms: 5000,
        body: Some(r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#.to_owned()),
    });
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderRateLimited));
    let detail = send_error
        .stream_error()
        .detail
        .as_deref()
        .expect("rate-limited errors must carry a detail");
    assert!(
        detail.contains("Provider response: "),
        "detail should include the provider body marker; got: {detail}"
    );
    assert!(
        detail.contains("insufficient_quota"),
        "detail should surface the raw provider signal; got: {detail}"
    );
}

#[test]
fn aionrs_provider_rate_limited_without_body_falls_back_to_bare_detail() {
    let error = DreamEngineAgentError::Provider(ProviderError::RateLimited {
        retry_after_ms: 5000,
        body: None,
    });
    let send_error = engine_error_to_send_error(&error);

    let detail = send_error
        .stream_error()
        .detail
        .as_deref()
        .expect("rate-limited errors must carry a detail");
    assert!(
        !detail.contains("Provider response:"),
        "detail must not add the body marker when body is absent; got: {detail}"
    );
    assert!(
        detail.contains("Rate limited"),
        "detail should still include the base message; got: {detail}"
    );
}

#[test]
fn aionrs_provider_rate_limited_ignores_whitespace_only_body() {
    let error = DreamEngineAgentError::Provider(ProviderError::RateLimited {
        retry_after_ms: 5000,
        body: Some("   \n\t  ".to_owned()),
    });
    let send_error = engine_error_to_send_error(&error);

    let detail = send_error
        .stream_error()
        .detail
        .as_deref()
        .expect("rate-limited errors must carry a detail");
    assert!(
        !detail.contains("Provider response:"),
        "whitespace-only body should be treated as absent; got: {detail}"
    );
}

#[test]
fn aionrs_provider_connection_error_is_user_llm_provider_error() {
    let error = DreamEngineAgentError::Provider(ProviderError::Connection(
        "Signable request error: failed to create canonical request".to_owned(),
    ));
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderNetworkError));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(true));
}

#[test]
fn provider_error_summary_classifies_network_without_body() {
    let error = DreamEngineAgentError::Provider(ProviderError::Connection("connect failed".into()));
    let summary = engine_runtime_error_summary(&error);

    assert_eq!(summary.kind, "provider");
    assert_eq!(summary.provider_error_class, Some("network"));
    assert_eq!(summary.http_status, None);
}

#[test]
fn tool_call_failure_summary_classifies_loop() {
    let error = DreamEngineAgentError::ToolCallFailures { count: 3, limit: 3 };
    let summary = engine_runtime_error_summary(&error);

    assert_eq!(summary.kind, "tool_call_failures");
    assert_eq!(summary.failure_count, Some(3));
    assert_eq!(summary.failure_limit, Some(3));
}

#[test]
fn aionrs_api_connection_error_is_user_llm_provider_network_error() {
    let error = DreamEngineAgentError::Provider(ProviderError::Connection("error decoding response body".to_owned()));
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderNetworkError));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(true));
}

#[test]
fn aionrs_provider_status_error_uses_status_instead_of_message_text() {
    let error = DreamEngineAgentError::Provider(ProviderError::Api {
        status: 401,
        message: "credentials failed".to_owned(),
    });
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderAuthFailed));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(false));
}

#[test]
fn aionrs_context_too_long_is_provider_context_error() {
    let error = DreamEngineAgentError::ContextTooLong {
        input_tokens: 120_000,
        limit: 100_000,
    };
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderContextTooLarge));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(false));
}

#[test]
fn aionrs_repeated_malformed_tool_call_is_user_llm_provider_error() {
    let error = DreamEngineAgentError::ToolCallMalformed { count: 3, limit: 3 };
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderInvalidRequest));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserLlmProvider));
    assert_eq!(send_error.stream_error().retryable, Some(false));
}

#[test]
fn aionrs_tool_call_failures_are_agent_tool_call_loop_error() {
    // Fork: consecutive tool-failure breaker is a known local agent condition,
    // not UnknownUpstream (see tool_call_failure_send_error).
    let error = DreamEngineAgentError::ToolCallFailures { count: 3, limit: 3 };
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserAgentToolCallLoop));
    assert_eq!(send_error.ownership(), Some(AgentErrorOwnership::UserAgent));
    assert_eq!(send_error.stream_error().retryable, Some(true));
}

/// A key whose allowance is spent comes back as a 403, and this path maps on
/// the status alone — so it read as PERMISSION_DENIED and told the user to go
/// re-check credentials that are perfectly fine.
///
/// This is the path 1ONE CLI conversations actually take. The first version of
/// the fix only guarded the text classifier in `protocol::send_error`, and the
/// running app kept showing the wrong code because errors arrive here instead.
#[test]
fn spent_allowance_is_not_reported_as_permission_denied() {
    let error = DreamEngineAgentError::Provider(ProviderError::Api {
        status: 403,
        message: r#"{"error":{"message":"Key limit exceeded (monthly limit). Manage it using https://openrouter.ai/workspaces/default/keys/abc123","code":403}}"#
            .to_owned(),
    });
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderQuotaExhausted));
}

/// The upstream text names our OpenRouter workspace and the key's management
/// handle. It was rendered in the chat bubble, persisted to the message store,
/// and shipped in feedback attachments.
#[test]
fn spent_allowance_does_not_leak_the_upstream_text() {
    let error = DreamEngineAgentError::Provider(ProviderError::Api {
        status: 403,
        message: r#"{"error":{"message":"Key limit exceeded (monthly limit). Manage it using https://openrouter.ai/workspaces/default/keys/abc123","code":403}}"#
            .to_owned(),
    });
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(
        send_error.stream_error().detail,
        None,
        "upstream text must not reach the user for a spent allowance"
    );
    let rendered = format!("{:?}", send_error.stream_error());
    assert!(!rendered.contains("openrouter.ai/workspaces"), "workspace URL leaked");
    assert!(!rendered.contains("abc123"), "key handle leaked");
}

/// The suppression is deliberately narrow — a genuine 403 still shows the
/// provider's own words, which is usually the most useful line on screen.
#[test]
fn a_real_permission_denial_still_carries_its_upstream_text() {
    let error = DreamEngineAgentError::Provider(ProviderError::Api {
        status: 403,
        message: r#"{"error":{"message":"You do not have access to this model"}}"#.to_owned(),
    });
    let send_error = engine_error_to_send_error(&error);

    assert_eq!(send_error.code(), Some(AgentErrorCode::UserLlmProviderPermissionDenied));
    assert!(
        send_error
            .stream_error()
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("do not have access")),
        "non-suppressed codes must keep their detail"
    );
}
