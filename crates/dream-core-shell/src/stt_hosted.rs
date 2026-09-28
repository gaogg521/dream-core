//! Mode D — hosted default speech-to-text.
//!
//! Thin adapter between [`SttService`](crate::stt::SttService) and
//! [`dream_core_system::HostedSttService`], which does the actual broker
//! call. This file only maps its result/errors onto [`SttError`] the same
//! way `stt_openai.rs` maps a vendor's own errors.

use dream_core_api_types::{SpeechToTextProvider, SpeechToTextResult};
use dream_core_system::HostedSttService;

use crate::error::SttError;

pub async fn transcribe(
    hosted: &HostedSttService,
    audio_data: &[u8],
    mime_type: &str,
    language_hint: Option<&str>,
) -> Result<SpeechToTextResult, SttError> {
    let response = hosted
        .transcribe(audio_data, mime_type, language_hint)
        .await
        .map_err(map_system_error)?;

    Ok(SpeechToTextResult {
        text: response.text,
        model: response.provider,
        provider: SpeechToTextProvider::Hosted,
        language: None,
    })
}

/// Every hosted-path failure becomes `SttError::RequestFailed` with a
/// human-readable message: the frontend's `mapSpeechInputError` already
/// treats any `STT_REQUEST_FAILED` as "transcription failed, here's why" and
/// shows the message verbatim, so a not-configured broker (dev-only — every
/// packaged build has one baked in), a rate limit, and a spent daily budget
/// all reach the user as a clear sentence without inventing a new error code
/// and i18n string for each.
fn map_system_error(err: dream_core_system::SystemError) -> SttError {
    SttError::RequestFailed(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::{IClientPreferenceRepository, SqliteClientPreferenceRepository, init_database_memory};
    use std::sync::Arc;

    async fn hosted(broker_base_url: Option<String>) -> HostedSttService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        HostedSttService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn unconfigured_deployment_surfaces_as_a_request_failure() {
        let h = hosted(None).await;
        let err = transcribe(&h, &[0u8; 4], "audio/wav", None).await.unwrap_err();
        assert!(matches!(err, SttError::RequestFailed(msg) if msg.contains("not configured")));
    }
}
