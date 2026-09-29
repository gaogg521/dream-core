//! Broker-backed realtime STT upstream.
//!
//! Core owns no DashScope credential. It opens a WebSocket only to the
//! configured Dream broker; that broker authenticates the vendor connection
//! and translates the Qwen streaming protocol into our small STT protocol.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::SttError;
use crate::stt_stream::{UpstreamEvent, UpstreamFactory, UpstreamStream};

pub struct HostedRealtimeUpstreamFactory {
    service: dream_core_system::HostedSttService,
}

impl HostedRealtimeUpstreamFactory {
    pub fn new(service: dream_core_system::HostedSttService) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl UpstreamFactory for HostedRealtimeUpstreamFactory {
    async fn connect(
        &self,
        _config: &dream_core_api_types::SpeechToTextConfig,
        sample_rate: u32,
        language_hint: Option<&str>,
    ) -> Result<Box<dyn UpstreamStream>, SttError> {
        let (url, install_id) = self
            .service
            .stream_endpoint()
            .await
            .map_err(|error| SttError::RequestFailed(error.to_string()))?;
        let request = url
            .into_client_request()
            .map_err(|error| SttError::RequestFailed(format!("invalid hosted STT stream URL: {error}")))?;
        let connector = crate::stt_stream_tls::build_ws_connector()?;
        let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
            .await
            .map_err(|error| SttError::RequestFailed(format!("hosted STT stream connection failed: {error}")))?;
        let mut start = serde_json::json!({
            "type": "start",
            "install_id": install_id,
            "sampleRate": sample_rate,
        });
        if let Some(language) = language_hint.map(str::trim).filter(|value| !value.is_empty()) {
            start["languageHint"] = serde_json::Value::String(language.to_owned());
        }
        ws.send(Message::Text(start.to_string().into()))
            .await
            .map_err(|error| SttError::RequestFailed(format!("hosted STT stream start failed: {error}")))?;
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => match parse_frame(text.as_str()) {
                    BrokerFrame::Ready => break,
                    BrokerFrame::Error(message) => return Err(SttError::RequestFailed(message)),
                    BrokerFrame::Ignore => {}
                    _ => {
                        return Err(SttError::RequestFailed(
                            "hosted STT stream did not acknowledge start".into(),
                        ));
                    }
                },
                Some(Ok(Message::Close(_))) | None => {
                    return Err(SttError::RequestFailed("hosted STT stream closed before ready".into()));
                }
                Some(Err(error)) => {
                    return Err(SttError::RequestFailed(format!(
                        "hosted STT stream error before ready: {error}"
                    )));
                }
                _ => {}
            }
        }
        Ok(Box::new(HostedRealtimeStream { ws }))
    }
}

pub struct HostedRealtimeStream {
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
}

#[async_trait::async_trait]
impl UpstreamStream for HostedRealtimeStream {
    async fn send_audio(&mut self, pcm: &[u8]) -> Result<(), SttError> {
        self.ws
            .send(Message::Binary(pcm.to_vec().into()))
            .await
            .map_err(|error| SttError::RequestFailed(format!("hosted STT audio send failed: {error}")))
    }

    async fn finish(&mut self) -> Result<(), SttError> {
        self.ws
            .send(Message::Text(r#"{"type":"stop"}"#.into()))
            .await
            .map_err(|error| SttError::RequestFailed(format!("hosted STT finish failed: {error}")))
    }

    async fn next_event(&mut self) -> Option<Result<UpstreamEvent, SttError>> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => match parse_frame(text.as_str()) {
                    BrokerFrame::Partial(text) => return Some(Ok(UpstreamEvent::Partial(text))),
                    BrokerFrame::Final(text) => return Some(Ok(UpstreamEvent::Final(text))),
                    BrokerFrame::Done => return Some(Ok(UpstreamEvent::Closed)),
                    BrokerFrame::Error(message) => return Some(Err(SttError::RequestFailed(message))),
                    BrokerFrame::Ready | BrokerFrame::Ignore => {}
                },
                Some(Ok(Message::Close(_))) | None => return Some(Ok(UpstreamEvent::Closed)),
                Some(Err(error)) => {
                    return Some(Err(SttError::RequestFailed(format!(
                        "hosted STT stream error: {error}"
                    ))));
                }
                _ => {}
            }
        }
    }
}

enum BrokerFrame {
    Ready,
    Partial(String),
    Final(String),
    Done,
    Error(String),
    Ignore,
}

fn parse_frame(text: &str) -> BrokerFrame {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return BrokerFrame::Ignore;
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("ready") => BrokerFrame::Ready,
        Some("partial") => BrokerFrame::Partial(
            value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned(),
        ),
        Some("final") => BrokerFrame::Final(
            value
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned(),
        ),
        Some("done") => BrokerFrame::Done,
        Some("error") => BrokerFrame::Error(
            value
                .get("msg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("hosted STT request failed")
                .to_owned(),
        ),
        _ => BrokerFrame::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_broker_transcript_frames() {
        assert!(matches!(parse_frame(r#"{"type":"ready"}"#), BrokerFrame::Ready));
        assert!(
            matches!(parse_frame(r#"{"type":"partial","text":"hel"}"#), BrokerFrame::Partial(text) if text == "hel")
        );
        assert!(
            matches!(parse_frame(r#"{"type":"final","text":"hello"}"#), BrokerFrame::Final(text) if text == "hello")
        );
    }
}
