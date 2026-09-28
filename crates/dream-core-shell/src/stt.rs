use dream_core_api_types::{SpeechToTextConfig, SpeechToTextProvider, SpeechToTextResult};
use dream_core_system::HostedSttService;
use reqwest::Client;

use crate::error::SttError;
use crate::{stt_hosted, stt_openai};

pub struct SttService {
    client: Client,
    hosted: HostedSttService,
}

impl SttService {
    pub fn new(client: Client, hosted: HostedSttService) -> Self {
        Self { client, hosted }
    }

    pub async fn transcribe(
        &self,
        audio_data: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language_hint: Option<&str>,
        config: &SpeechToTextConfig,
    ) -> Result<SpeechToTextResult, SttError> {
        if !config.enabled {
            return Err(SttError::Disabled);
        }

        match config.provider {
            SpeechToTextProvider::Openai => {
                let openai_config = config.openai.as_ref().ok_or(SttError::OpenaiNotConfigured)?;
                stt_openai::transcribe(
                    &self.client,
                    openai_config,
                    audio_data,
                    file_name,
                    mime_type,
                    language_hint,
                )
                .await
            }
            SpeechToTextProvider::Hosted => {
                stt_hosted::transcribe(&self.hosted, &audio_data, mime_type, language_hint).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_api_types::OpenAISpeechToTextConfig;
    use dream_core_db::{IClientPreferenceRepository, SqliteClientPreferenceRepository, init_database_memory};
    use std::sync::Arc;

    async fn make_service() -> SttService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        let hosted = HostedSttService::new(None, Client::new(), repo);
        SttService::new(Client::new(), hosted)
    }

    fn make_disabled_config() -> SpeechToTextConfig {
        SpeechToTextConfig {
            enabled: false,
            provider: SpeechToTextProvider::Openai,
            auto_send: None,
            openai: None,
        }
    }

    fn make_openai_config(api_key: &str) -> SpeechToTextConfig {
        SpeechToTextConfig {
            enabled: true,
            provider: SpeechToTextProvider::Openai,
            auto_send: None,
            openai: Some(OpenAISpeechToTextConfig {
                api_key: api_key.to_owned(),
                base_url: None,
                model: "whisper-1".into(),
                language: None,
                prompt: None,
                temperature: None,
            }),
        }
    }

    #[tokio::test]
    async fn disabled_config_returns_disabled_error() {
        let svc = make_service().await;
        let result = svc
            .transcribe(vec![0u8; 10], "test.wav", "audio/wav", None, &make_disabled_config())
            .await;
        assert!(matches!(result, Err(SttError::Disabled)));
    }

    #[tokio::test]
    async fn openai_provider_missing_config_returns_not_configured() {
        let svc = make_service().await;
        let config = SpeechToTextConfig {
            enabled: true,
            provider: SpeechToTextProvider::Openai,
            auto_send: None,
            openai: None,
        };
        let result = svc
            .transcribe(vec![0u8; 10], "test.wav", "audio/wav", None, &config)
            .await;
        assert!(matches!(result, Err(SttError::OpenaiNotConfigured)));
    }

    #[tokio::test]
    async fn openai_empty_api_key_returns_not_configured() {
        let svc = make_service().await;
        let config = make_openai_config("");
        let result = svc
            .transcribe(vec![0u8; 10], "test.wav", "audio/wav", None, &config)
            .await;
        assert!(matches!(result, Err(SttError::OpenaiNotConfigured)));
    }

    /// The hosted path with no broker configured (this test's default) must
    /// still be a clean, catchable error — never a panic.
    #[tokio::test]
    async fn hosted_provider_without_a_broker_returns_request_failed() {
        let svc = make_service().await;
        let config = SpeechToTextConfig {
            enabled: true,
            provider: SpeechToTextProvider::Hosted,
            auto_send: None,
            openai: None,
        };
        let result = svc
            .transcribe(vec![0u8; 10], "test.wav", "audio/wav", None, &config)
            .await;
        assert!(matches!(result, Err(SttError::RequestFailed(_))));
    }
}
