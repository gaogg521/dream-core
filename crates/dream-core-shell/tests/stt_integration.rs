#![allow(clippy::disallowed_types)]

use std::sync::Arc;

use dream_core_api_types::{OpenAISpeechToTextConfig, SpeechToTextConfig, SpeechToTextProvider};
use dream_core_db::{IClientPreferenceRepository, SqliteClientPreferenceRepository, init_database_memory};
use dream_core_shell::{SttError, SttService};
use dream_core_system::HostedSttService;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn dummy_audio() -> Vec<u8> {
    vec![0u8; 64]
}

async fn hosted_service(broker_base_url: Option<String>) -> HostedSttService {
    let db = init_database_memory().await.unwrap();
    let repo: Arc<dyn IClientPreferenceRepository> = Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
    HostedSttService::new(broker_base_url, reqwest::Client::new(), repo)
}

async fn stt_service() -> SttService {
    SttService::new(reqwest::Client::new(), hosted_service(None).await)
}

async fn stt_service_with_broker(broker_base_url: String) -> SttService {
    SttService::new(reqwest::Client::new(), hosted_service(Some(broker_base_url)).await)
}

// ---------------------------------------------------------------------------
// ST-1: OpenAI transcription — success
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st1_openai_transcribe_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("Authorization", "Bearer sk-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "hello world" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-test-key".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await
        .unwrap();

    assert_eq!(result.text, "hello world");
    assert_eq!(result.model, "whisper-1");
    assert_eq!(result.provider, SpeechToTextProvider::Openai);
}

// ---------------------------------------------------------------------------
// ST-2: Hosted (mode D) transcription — success
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st2_hosted_transcribe_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/stt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "provider": "aliyun",
            "text": "hello hosted",
            "quota": { "used_today": 1, "daily_limit": 20, "remaining": 19 }
        })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Hosted,
        auto_send: None,
        openai: None,
    };

    let result = stt_service_with_broker(mock_server.uri())
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await
        .unwrap();

    assert_eq!(result.text, "hello hosted");
    assert_eq!(result.model, "aliyun");
    assert_eq!(result.provider, SpeechToTextProvider::Hosted);
}

// ---------------------------------------------------------------------------
// ST-3: STT disabled
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st3_stt_disabled() {
    let config = SpeechToTextConfig {
        enabled: false,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: None,
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    assert!(matches!(result, Err(SttError::Disabled)));
}

// ---------------------------------------------------------------------------
// ST-5: OpenAI missing API key
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st5_openai_empty_api_key() {
    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: String::new(),
            base_url: None,
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    assert!(matches!(result, Err(SttError::OpenaiNotConfigured)));
}

#[tokio::test]
async fn st5b_openai_config_section_missing() {
    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: None,
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    assert!(matches!(result, Err(SttError::OpenaiNotConfigured)));
}

/// A self-hosted OpenAI-compatible server with no auth is a legitimate setup
/// the settings UI itself allows (API key marked optional for a custom
/// base_url) — this must reach the network, not be rejected as unconfigured.
#[tokio::test]
async fn st5c_custom_base_url_with_empty_api_key_reaches_the_network() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "ok" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: String::new(),
            base_url: Some(mock_server.uri()),
            model: "custom-model".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await
        .unwrap();
    assert_eq!(result.text, "ok");
}

// ---------------------------------------------------------------------------
// ST-6: hosted with no broker configured on this deployment
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st6_hosted_without_a_broker_is_a_request_failure() {
    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Hosted,
        auto_send: None,
        openai: None,
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    assert!(matches!(result, Err(SttError::RequestFailed(_))));
}

// ---------------------------------------------------------------------------
// ST-7: OpenAI upstream API failure (401)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st7_openai_upstream_failure() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "message": "Incorrect API key provided",
                "type": "invalid_request_error"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-invalid".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    match result {
        Err(SttError::RequestFailed(msg)) => {
            assert!(msg.contains("401"), "expected 401 in error: {msg}");
        }
        other => panic!("expected RequestFailed, got: {other:?}"),
    }
}

/// A broker-side failure (bad gateway, quota exhausted, etc.) must surface as
/// a plain, readable message through the same error path OpenAI failures use
/// — not a new error code the frontend has to special-case.
#[tokio::test]
async fn st7b_hosted_broker_rejection_surfaces_as_request_failed() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/stt"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({ "error": "rate_limited" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Hosted,
        auto_send: None,
        openai: None,
    };

    let result = stt_service_with_broker(mock_server.uri())
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await;

    assert!(matches!(result, Err(SttError::RequestFailed(_))));
}

// ---------------------------------------------------------------------------
// ST-10: configured language overrides languageHint for OpenAI
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st10_openai_config_language_overrides_language_hint() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("name=\"language\""))
        .and(body_string_contains("\r\nen\r\n"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "你好世界" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-test".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: Some("en".into()),
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", Some("zh"), &config)
        .await
        .unwrap();

    assert_eq!(result.text, "你好世界");
    assert_eq!(result.language.as_deref(), Some("en"));
}

#[tokio::test]
async fn st10c_openai_language_hint_region_is_normalized() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("name=\"language\""))
        .and(body_string_contains("\r\nen\r\n"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "hello" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-test".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(
            dummy_audio(),
            "test.webm",
            "audio/webm;codecs=opus",
            Some("en-US"),
            &config,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "hello");
    assert_eq!(result.language.as_deref(), Some("en"));
}

#[tokio::test]
async fn openai_base_url_with_v1_suffix_is_not_duplicated() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "ok" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-test".into(),
            base_url: Some(format!("{}/v1", mock_server.uri())),
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.wav", "audio/wav", None, &config)
        .await
        .unwrap();

    assert_eq!(result.text, "ok");
}

#[tokio::test]
async fn openai_mime_type_codec_params_are_removed() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("Content-Type: audio/webm\r\n"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "ok" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: None,
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-test".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: None,
            prompt: None,
            temperature: None,
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "test.webm", "audio/webm;codecs=opus", None, &config)
        .await
        .unwrap();

    assert_eq!(result.text, "ok");
}

// ---------------------------------------------------------------------------
// Additional: OpenAI with all optional params (prompt, temperature)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn openai_with_all_optional_params() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "technical terms test" })))
        .mount(&mock_server)
        .await;

    let config = SpeechToTextConfig {
        enabled: true,
        provider: SpeechToTextProvider::Openai,
        auto_send: Some(true),
        openai: Some(OpenAISpeechToTextConfig {
            api_key: "sk-full".into(),
            base_url: Some(mock_server.uri()),
            model: "whisper-1".into(),
            language: Some("en".into()),
            prompt: Some("technical terms".into()),
            temperature: Some(0.2),
        }),
    };

    let result = stt_service()
        .await
        .transcribe(dummy_audio(), "audio.m4a", "audio/mp4", None, &config)
        .await
        .unwrap();

    assert_eq!(result.text, "technical terms test");
    assert_eq!(result.model, "whisper-1");
    assert_eq!(result.provider, SpeechToTextProvider::Openai);
    assert_eq!(result.language.as_deref(), Some("en"));
}

// ---------------------------------------------------------------------------
// SttError → ApiError conversion (black-box integration test)
// ---------------------------------------------------------------------------
#[test]
fn stt_error_to_api_error_mapping() {
    use dream_core_common::ApiError;

    let err: ApiError = SttError::Disabled.into();
    assert!(matches!(err, ApiError::BadRequest(_)));

    let err: ApiError = SttError::OpenaiNotConfigured.into();
    assert!(matches!(err, ApiError::BadRequest(_)));

    let err: ApiError = SttError::RequestFailed("upstream".into()).into();
    assert!(matches!(err, ApiError::BadGateway(_)));

    let err: ApiError = SttError::Unknown("bug".into()).into();
    assert!(matches!(err, ApiError::Internal(_)));
}
