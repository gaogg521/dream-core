use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shell operation types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Vscode,
    Terminal,
    Explorer,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenFileRequest {
    pub file_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShowItemInFolderRequest {
    pub file_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenExternalRequest {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckToolInstalledRequest {
    pub tool: ToolType,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckToolInstalledResponse {
    pub installed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenFolderWithRequest {
    pub folder_path: String,
    pub tool: ToolType,
}

// ---------------------------------------------------------------------------
// Speech-to-text types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechToTextProvider {
    Openai,
    /// The broker-backed default (mode D): dream-core resolves this
    /// deployment's install id and forwards the audio, never holding a
    /// vendor key itself. See `crates/dream-core-shell/src/stt_hosted.rs`.
    Hosted,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeechToTextResult {
    pub text: String,
    pub model: String,
    pub provider: SpeechToTextProvider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAISpeechToTextConfig {
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpeechToTextConfig {
    pub enabled: bool,
    pub provider: SpeechToTextProvider,
    #[serde(default)]
    pub auto_send: Option<bool>,
    #[serde(default)]
    pub openai: Option<OpenAISpeechToTextConfig>,
}

// ---------------------------------------------------------------------------
// STT streaming WebSocket protocol types
// ---------------------------------------------------------------------------

/// Client → server control frames for the STT streaming WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SttStreamClientMessage {
    Start {
        format: String,
        #[serde(rename = "sampleRate")]
        sample_rate: u32,
        channels: u32,
        #[serde(rename = "languageHint", default)]
        language_hint: Option<String>,
    },
    Stop,
}

/// Server → client frames for the STT streaming WebSocket.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SttStreamServerMessage {
    Ready,
    Partial { text: String },
    Final { text: String },
    Done,
    Error { code: String, msg: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- ToolType --

    #[test]
    fn tool_type_serializes_lowercase() {
        assert_eq!(serde_json::to_value(ToolType::Vscode).unwrap(), "vscode");
        assert_eq!(serde_json::to_value(ToolType::Terminal).unwrap(), "terminal");
        assert_eq!(serde_json::to_value(ToolType::Explorer).unwrap(), "explorer");
    }

    #[test]
    fn tool_type_deserializes_lowercase() {
        let v: ToolType = serde_json::from_str(r#""vscode""#).unwrap();
        assert_eq!(v, ToolType::Vscode);
        let t: ToolType = serde_json::from_str(r#""terminal""#).unwrap();
        assert_eq!(t, ToolType::Terminal);
        let e: ToolType = serde_json::from_str(r#""explorer""#).unwrap();
        assert_eq!(e, ToolType::Explorer);
    }

    #[test]
    fn tool_type_rejects_unknown() {
        let result = serde_json::from_str::<ToolType>(r#""unknown""#);
        assert!(result.is_err());
    }

    // -- Shell request types --

    #[test]
    fn open_file_request_snake_case() {
        let raw = json!({ "file_path": "/tmp/test.txt" });
        let req: OpenFileRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.file_path, "/tmp/test.txt");
    }

    #[test]
    fn open_file_request_missing_field() {
        let result = serde_json::from_value::<OpenFileRequest>(json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn show_item_in_folder_request_snake_case() {
        let raw = json!({ "file_path": "/home/user/doc.pdf" });
        let req: ShowItemInFolderRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.file_path, "/home/user/doc.pdf");
    }

    #[test]
    fn open_external_request_parses() {
        let raw = json!({ "url": "https://example.com" });
        let req: OpenExternalRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.url, "https://example.com");
    }

    #[test]
    fn check_tool_installed_request_parses() {
        let raw = json!({ "tool": "vscode" });
        let req: CheckToolInstalledRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.tool, ToolType::Vscode);
    }

    #[test]
    fn check_tool_installed_response_serializes() {
        let resp = CheckToolInstalledResponse { installed: true };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["installed"], true);
    }

    #[test]
    fn open_folder_with_request_snake_case() {
        let raw = json!({ "folder_path": "/tmp", "tool": "terminal" });
        let req: OpenFolderWithRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.folder_path, "/tmp");
        assert_eq!(req.tool, ToolType::Terminal);
    }

    // -- SpeechToTextProvider --

    #[test]
    fn stt_provider_serializes_lowercase() {
        assert_eq!(serde_json::to_value(SpeechToTextProvider::Openai).unwrap(), "openai");
        assert_eq!(serde_json::to_value(SpeechToTextProvider::Hosted).unwrap(), "hosted");
    }

    #[test]
    fn stt_provider_deserializes_lowercase() {
        let o: SpeechToTextProvider = serde_json::from_str(r#""openai""#).unwrap();
        assert_eq!(o, SpeechToTextProvider::Openai);
        let h: SpeechToTextProvider = serde_json::from_str(r#""hosted""#).unwrap();
        assert_eq!(h, SpeechToTextProvider::Hosted);
    }

    /// A config persisted by an older build with `provider: "deepgram"` must
    /// not crash the app on load — it should be rejected the same clean way
    /// any other unrecognized value is, so `load_stt_config`'s malformed-
    /// config fallback (disabled) kicks in rather than a panic.
    #[test]
    fn stt_provider_rejects_retired_deepgram_value() {
        let result = serde_json::from_str::<SpeechToTextProvider>(r#""deepgram""#);
        assert!(result.is_err());
    }

    #[test]
    fn stt_provider_rejects_unknown() {
        let result = serde_json::from_str::<SpeechToTextProvider>(r#""azure""#);
        assert!(result.is_err());
    }

    // -- SpeechToTextResult --

    #[test]
    fn stt_result_serializes_with_language() {
        let result = SpeechToTextResult {
            text: "hello world".to_owned(),
            model: "whisper-1".to_owned(),
            provider: SpeechToTextProvider::Openai,
            language: Some("en".to_owned()),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["text"], "hello world");
        assert_eq!(json["model"], "whisper-1");
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["language"], "en");
    }

    #[test]
    fn stt_result_omits_null_language() {
        let result = SpeechToTextResult {
            text: "test".to_owned(),
            model: "qwen3-asr-flash".to_owned(),
            provider: SpeechToTextProvider::Hosted,
            language: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("language").is_none());
    }

    // -- SpeechToTextConfig --

    #[test]
    fn stt_config_full_openai() {
        let raw = json!({
            "enabled": true,
            "provider": "openai",
            "auto_send": true,
            "openai": {
                "api_key": "sk-test",
                "base_url": "https://api.openai.com",
                "model": "whisper-1",
                "language": "en",
                "prompt": "technical terms",
                "temperature": 0.2
            }
        });
        let config: SpeechToTextConfig = serde_json::from_value(raw).unwrap();
        assert!(config.enabled);
        assert_eq!(config.provider, SpeechToTextProvider::Openai);
        assert_eq!(config.auto_send, Some(true));
        let openai = config.openai.unwrap();
        assert_eq!(openai.api_key, "sk-test");
        assert_eq!(openai.base_url.as_deref(), Some("https://api.openai.com"));
        assert_eq!(openai.model, "whisper-1");
        assert_eq!(openai.language.as_deref(), Some("en"));
        assert_eq!(openai.prompt.as_deref(), Some("technical terms"));
        assert_eq!(openai.temperature, Some(0.2));
    }

    #[test]
    fn stt_config_hosted() {
        let raw = json!({
            "enabled": true,
            "provider": "hosted"
        });
        let config: SpeechToTextConfig = serde_json::from_value(raw).unwrap();
        assert!(config.enabled);
        assert_eq!(config.provider, SpeechToTextProvider::Hosted);
        assert!(config.openai.is_none());
    }

    #[test]
    fn stt_config_minimal() {
        let raw = json!({
            "enabled": false,
            "provider": "openai"
        });
        let config: SpeechToTextConfig = serde_json::from_value(raw).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.provider, SpeechToTextProvider::Openai);
        assert!(config.auto_send.is_none());
        assert!(config.openai.is_none());
    }

    #[test]
    fn stt_config_missing_required_field() {
        let raw = json!({ "enabled": true });
        let result = serde_json::from_value::<SpeechToTextConfig>(raw);
        assert!(result.is_err());
    }

    // -- SttStreamClientMessage --

    #[test]
    fn stt_stream_client_start_full() {
        let raw = json!({
            "type": "start",
            "format": "pcm16",
            "sampleRate": 24000,
            "channels": 1,
            "languageHint": "zh"
        });
        let msg: SttStreamClientMessage = serde_json::from_value(raw).unwrap();
        match msg {
            SttStreamClientMessage::Start {
                format,
                sample_rate,
                channels,
                language_hint,
            } => {
                assert_eq!(format, "pcm16");
                assert_eq!(sample_rate, 24000);
                assert_eq!(channels, 1);
                assert_eq!(language_hint, Some("zh".to_owned()));
            }
            _ => panic!("expected Start variant"),
        }
    }

    #[test]
    fn stt_stream_client_start_without_language_hint() {
        let raw = json!({
            "type": "start",
            "format": "pcm16",
            "sampleRate": 16000,
            "channels": 1
        });
        let msg: SttStreamClientMessage = serde_json::from_value(raw).unwrap();
        match msg {
            SttStreamClientMessage::Start { language_hint, .. } => {
                assert_eq!(language_hint, None);
            }
            _ => panic!("expected Start variant"),
        }
    }

    #[test]
    fn stt_stream_client_stop() {
        let raw = json!({ "type": "stop" });
        let msg: SttStreamClientMessage = serde_json::from_value(raw).unwrap();
        assert!(matches!(msg, SttStreamClientMessage::Stop));
    }

    #[test]
    fn stt_stream_client_garbage_type_rejected() {
        let raw = json!({ "type": "garbage" });
        let result = serde_json::from_value::<SttStreamClientMessage>(raw);
        assert!(result.is_err());
    }

    // -- SttStreamServerMessage --

    #[test]
    fn stt_stream_server_ready_serializes() {
        let msg = SttStreamServerMessage::Ready;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json, json!({ "type": "ready" }));
    }

    #[test]
    fn stt_stream_server_partial_serializes() {
        let msg = SttStreamServerMessage::Partial { text: "hel".to_owned() };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json, json!({ "type": "partial", "text": "hel" }));
    }

    #[test]
    fn stt_stream_server_final_serializes() {
        let msg = SttStreamServerMessage::Final {
            text: "hello world".to_owned(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json, json!({ "type": "final", "text": "hello world" }));
    }

    #[test]
    fn stt_stream_server_done_serializes() {
        let msg = SttStreamServerMessage::Done;
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json, json!({ "type": "done" }));
    }

    #[test]
    fn stt_stream_server_error_serializes() {
        let msg = SttStreamServerMessage::Error {
            code: "STT_STREAM_UNSUPPORTED".to_owned(),
            msg: "not supported".to_owned(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            json!({ "type": "error", "code": "STT_STREAM_UNSUPPORTED", "msg": "not supported" })
        );
    }

    // -- OpenAISpeechToTextConfig --

    #[test]
    fn openai_config_minimal() {
        let raw = json!({
            "api_key": "sk-key",
            "model": "whisper-1"
        });
        let config: OpenAISpeechToTextConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.api_key, "sk-key");
        assert_eq!(config.model, "whisper-1");
        assert!(config.base_url.is_none());
        assert!(config.language.is_none());
        assert!(config.prompt.is_none());
        assert!(config.temperature.is_none());
    }
}
