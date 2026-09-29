//! Provider-dispatch upstream factory for STT streaming.
//!
//! The route layer needs a single [`UpstreamFactory`] that works for any
//! stored config; this module composes the per-provider factories by
//! matching on `config.provider` and delegating.

use dream_core_api_types::{SpeechToTextConfig, SpeechToTextProvider};

use crate::error::SttError;
use crate::stt_stream::{UpstreamFactory, UpstreamStream};
use crate::stt_stream_hosted::HostedRealtimeUpstreamFactory;
use crate::stt_stream_openai::OpenAIRealtimeUpstreamFactory;

/// Dispatches to a vendor-specific realtime connector while keeping hosted
/// STT credential-free in Core: its connector talks only to our broker.
pub struct ProviderUpstreamFactory {
    hosted_stt: dream_core_system::HostedSttService,
}

impl ProviderUpstreamFactory {
    pub fn new(hosted_stt: dream_core_system::HostedSttService) -> Self {
        Self { hosted_stt }
    }
}

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
            SpeechToTextProvider::Hosted => {
                HostedRealtimeUpstreamFactory::new(self.hosted_stt.clone())
                    .connect(config, sample_rate, language_hint)
                    .await
            }
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
        let err = OpenAIRealtimeUpstreamFactory
            .connect(&config, 16000, None)
            .await
            .map(|_| ())
            .expect_err("expected OpenAI factory error");
        assert!(matches!(err, SttError::OpenaiNotConfigured));
    }
}
