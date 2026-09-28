//! Provider-dispatch upstream factory for STT streaming.
//!
//! The route layer needs a single [`UpstreamFactory`] that works for any
//! stored config; this module composes the per-provider factories by
//! matching on `config.provider` and delegating.

use dream_core_api_types::{SpeechToTextConfig, SpeechToTextProvider};

use crate::error::SttError;
use crate::stt_stream::{UpstreamFactory, UpstreamStream};
use crate::stt_stream_openai::OpenAIRealtimeUpstreamFactory;

/// Dispatches to [`OpenAIRealtimeUpstreamFactory`] for `Openai`. The hosted
/// broker path (mode D) has no realtime protocol — the client already
/// degrades a streaming attempt to the whole-blob `/api/stt` fallback on
/// `STT_STREAM_UNSUPPORTED` and remembers not to retry streaming for it.
pub struct ProviderUpstreamFactory;

#[async_trait::async_trait]
impl UpstreamFactory for ProviderUpstreamFactory {
    async fn connect(
        &self,
        config: &SpeechToTextConfig,
        sample_rate: u32,
        language_hint: Option<&str>,
    ) -> Result<Box<dyn UpstreamStream>, SttError> {
        match config.provider {
            SpeechToTextProvider::Openai => {
                OpenAIRealtimeUpstreamFactory
                    .connect(config, sample_rate, language_hint)
                    .await
            }
            SpeechToTextProvider::Hosted => Err(SttError::StreamUnsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each provider arm must reach its concrete factory: with a missing
    /// provider config, the factory's own validation error proves dispatch.
    #[tokio::test]
    async fn dispatches_openai_to_openai_factory() {
        let config = SpeechToTextConfig {
            enabled: true,
            provider: SpeechToTextProvider::Openai,
            auto_send: None,
            openai: None,
        };
        let err = ProviderUpstreamFactory
            .connect(&config, 16000, None)
            .await
            .map(|_| ())
            .expect_err("expected OpenAI factory error");
        assert!(matches!(err, SttError::OpenaiNotConfigured));
    }

    #[tokio::test]
    async fn hosted_provider_has_no_streaming_protocol() {
        let config = SpeechToTextConfig {
            enabled: true,
            provider: SpeechToTextProvider::Hosted,
            auto_send: None,
            openai: None,
        };
        let err = ProviderUpstreamFactory
            .connect(&config, 16000, None)
            .await
            .map(|_| ())
            .expect_err("hosted has no realtime protocol");
        assert!(matches!(err, SttError::StreamUnsupported));
    }
}
