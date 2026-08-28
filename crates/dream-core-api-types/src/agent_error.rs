use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorOwnership {
    Dream,
    UserAgent,
    UserLlmProvider,
    UnknownUpstream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentErrorCode {
    DreamConversationBusy,
    DreamStreamBroken,
    DreamStateInconsistent,
    DreamPermissionError,
    DreamInternalError,
    /// We stopped a turn that was not converging — it exhausted its budget of
    /// agentic turns, or ran past the wall-clock ceiling for one message.
    ///
    /// One code for both because the user's situation is identical either way:
    /// the request ran away, nothing came back, and the next step is to retry
    /// or rephrase. The `detail` says which limit it was.
    DreamTurnLimitReached,
    #[serde(alias = "WORKSPACE_PATH_CONTAINS_WHITESPACE_RUNTIME_UNSUPPORTED")]
    WorkspacePathRuntimeUnavailable,
    UserAgentHandshakeFailed,
    UserAgentHandshakeTimeout,
    UserAgentAcpInitFailed,
    UserAgentProtocolMismatch,
    UserAgentNotInstalled,
    UserAgentStartupFailed,
    #[serde(rename = "USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE")]
    UserAgentOpenClawGatewayUnreachable,
    UserAgentDisconnected,
    UserAgentAuthRequired,
    UserAgentSessionNotFound,
    UserAgentNoPreviousSession,
    UserAgentProtocolParseError,
    UserAgentInvalidRequest,
    UserAgentResourceNotFound,
    UserAgentProtocolError,
    UserAgentCommandNotFound,
    UserAgentMissingEnv,
    UserAgentUnsupportedMethod,
    UserAgentInvalidParams,
    /// The agent engine stopped after consecutive rounds of failing tool
    /// calls (dream `ToolCallFailures` breaker). Local diagnosis, not an
    /// unknown upstream failure.
    UserAgentToolCallLoop,
    UserLlmProviderAuthFailed,
    UserLlmProviderAwsSsoExpired,
    UserLlmProviderPermissionDenied,
    UserLlmProviderBillingRequired,
    UserLlmProviderConfigError,
    /// The key itself is valid, but its spend allowance is used up — the
    /// trial tier's monthly cap, or any key the user capped themselves.
    /// Distinct from `UserLlmProviderBillingRequired` (the *account* needs
    /// money) and from `UserLlmProviderPermissionDenied`, which is where this
    /// used to land purely because the upstream text contains "403" — and
    /// which sends the user off to check credentials that are perfectly fine.
    UserLlmProviderQuotaExhausted,
    UserLlmProviderModelNotFound,
    /// The provider config a conversation was built with has been deleted or
    /// replaced since. Distinct from `UserLlmProviderModelNotFound` (provider
    /// exists but rejects the model) — here the provider row itself is gone.
    ProviderNotFound,
    UserLlmProviderUnsupportedModel,
    UserLlmProviderEndpointNotFound,
    UserLlmProviderInvalidRequest,
    UserLlmProviderInvalidToolSchema,
    UserLlmProviderContextTooLarge,
    UserLlmProviderRateLimited,
    UserLlmProviderTimeout,
    UserLlmProviderNetworkError,
    UserLlmProviderEmptyResponse,
    UserLlmProviderGatewayError,
    UnknownUpstreamError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorResolutionKind {
    Retry,
    WaitForCurrentResponse,
    StartNewSession,
    ReconnectAgent,
    CheckAgentLogin,
    CheckAgentInstallation,
    CheckAgentVersion,
    CheckLocalCommand,
    CheckProviderCredentials,
    CheckProviderBilling,
    CheckProviderBaseUrl,
    ChangeModel,
    ReduceContext,
    SendFeedback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorResolutionTarget {
    ProviderSettings,
    AgentSettings,
    NewConversation,
    Feedback,
}

impl AgentErrorCode {
    /// Whether the upstream provider's own error text must be withheld from
    /// the end user for this code.
    ///
    /// `bound_error_detail` truncates and de-tags but explicitly does not
    /// redact, so whatever the provider wrote is persisted in the message
    /// store, rendered in the chat bubble, and attached to feedback reports.
    /// That is the right default — provider errors are usually the most
    /// useful thing on screen. It is wrong for quota exhaustion on a
    /// company-issued key: OpenRouter answers with
    /// `"Key limit exceeded (total limit). Manage it using
    /// https://openrouter.ai/workspaces/<org>/keys/<hash>"`, which names our
    /// workspace and the key's management handle, and tells the user nothing
    /// they can act on. The localized title/body for the code says everything
    /// that matters.
    pub fn hides_upstream_detail(self) -> bool {
        matches!(self, Self::UserLlmProviderQuotaExhausted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentErrorResolution {
    pub kind: AgentErrorResolutionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AgentErrorResolutionTarget>,
}

impl AgentErrorResolution {
    pub fn new(kind: AgentErrorResolutionKind, target: Option<AgentErrorResolutionTarget>) -> Self {
        Self { kind, target }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStreamErrorData {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<AgentErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<AgentErrorOwnership>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "workspacePath",
        alias = "workspace_path"
    )]
    pub workspace_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_recommended: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<AgentErrorResolution>,
}

impl AgentStreamErrorData {
    pub fn legacy(message: impl Into<String>, code: Option<AgentErrorCode>) -> Self {
        Self {
            message: message.into(),
            code,
            ownership: None,
            detail: None,
            workspace_path: None,
            retryable: None,
            feedback_recommended: None,
            resolution: None,
        }
    }

    pub fn classified(
        message: impl Into<String>,
        code: AgentErrorCode,
        ownership: AgentErrorOwnership,
        detail: Option<String>,
        retryable: bool,
        feedback_recommended: bool,
        resolution: Option<AgentErrorResolution>,
    ) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            ownership: Some(ownership),
            detail,
            workspace_path: None,
            retryable: Some(retryable),
            feedback_recommended: Some(feedback_recommended),
            resolution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classified_error_serializes_as_public_contract() {
        let payload = AgentStreamErrorData::classified(
            "The model provider rejected the request",
            AgentErrorCode::UserLlmProviderAuthFailed,
            AgentErrorOwnership::UserLlmProvider,
            Some("Provider returned 401.".into()),
            false,
            false,
            None,
        );

        let json = serde_json::to_value(payload).unwrap();
        assert_eq!(json["message"], "The model provider rejected the request");
        assert_eq!(json["code"], "USER_LLM_PROVIDER_AUTH_FAILED");
        assert_eq!(json["ownership"], "user_llm_provider");
        assert!(json.get("workspacePath").is_none());
        assert_eq!(json["retryable"], false);
        assert_eq!(json["feedback_recommended"], false);
        assert!(json.get("resolution").is_none());
    }

    #[test]
    fn classified_error_serializes_resolution() {
        let payload = AgentStreamErrorData::classified(
            "The current response is still running",
            AgentErrorCode::DreamConversationBusy,
            AgentErrorOwnership::Dream,
            Some("Conflict: Conversation is already processing a message".into()),
            true,
            false,
            Some(AgentErrorResolution::new(
                AgentErrorResolutionKind::WaitForCurrentResponse,
                Some(AgentErrorResolutionTarget::NewConversation),
            )),
        );

        let json = serde_json::to_value(payload).unwrap();
        assert_eq!(json["code"], "DREAM_CONVERSATION_BUSY");
        assert_eq!(json["resolution"]["kind"], "wait_for_current_response");
        assert_eq!(json["resolution"]["target"], "new_conversation");
    }

    #[test]
    fn openclaw_gateway_unreachable_serializes_with_compact_vendor_name() {
        let json = serde_json::to_value(AgentErrorCode::UserAgentOpenClawGatewayUnreachable).unwrap();
        assert_eq!(json, "USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE");

        let decoded: AgentErrorCode =
            serde_json::from_value(serde_json::json!("USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE")).unwrap();
        assert_eq!(decoded, AgentErrorCode::UserAgentOpenClawGatewayUnreachable);
    }

    #[test]
    fn legacy_error_payload_deserializes() {
        let json = serde_json::json!({
            "message": "legacy failure",
            "code": "UNKNOWN_UPSTREAM_ERROR"
        });

        let payload: AgentStreamErrorData = serde_json::from_value(json).unwrap();
        assert_eq!(payload.message, "legacy failure");
        assert_eq!(payload.code, Some(AgentErrorCode::UnknownUpstreamError));
        assert_eq!(payload.ownership, None);
        assert_eq!(payload.workspace_path, None);
        assert_eq!(payload.retryable, None);
        assert_eq!(payload.feedback_recommended, None);
    }

    #[test]
    fn legacy_error_payload_has_no_resolution() {
        let json = serde_json::json!({
            "message": "legacy failure",
            "code": "UNKNOWN_UPSTREAM_ERROR"
        });

        let payload: AgentStreamErrorData = serde_json::from_value(json).unwrap();
        assert_eq!(payload.resolution, None);
    }

    #[test]
    fn workspace_path_field_serializes_and_deserializes() {
        let payload = AgentStreamErrorData {
            message: "workspace path rejected".into(),
            code: Some(AgentErrorCode::WorkspacePathRuntimeUnavailable),
            ownership: Some(AgentErrorOwnership::Dream),
            detail: Some("workspace detail".into()),
            workspace_path: Some("/tmp/Archive ".into()),
            retryable: Some(false),
            feedback_recommended: Some(false),
            resolution: None,
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["code"], "WORKSPACE_PATH_RUNTIME_UNAVAILABLE");
        assert_eq!(json["workspacePath"], "/tmp/Archive ");

        let roundtrip: AgentStreamErrorData = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.workspace_path.as_deref(), Some("/tmp/Archive "));
    }
}
