use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Sink(Mutex<Vec<ProxyUsageEvent>>);
impl ProxyUsageRecorder for Sink {
    fn record_proxy_usage(&self, event: ProxyUsageEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn tap(sink: &Arc<Sink>) -> UsageTap {
    UsageTap::new(
        Some(sink.clone()),
        "member".into(),
        "channel".into(),
        "text/event-stream",
        true,
        Some("demo-model".into()),
        Some("conversation".into()),
        None,
    )
}

#[tokio::test]
async fn done_records_before_downstream_drops_without_polling_eof() {
    let sink = Arc::new(Sink::default());
    let bytes =
        Bytes::from_static(b"data: {\"usage\":{\"prompt_tokens\":80,\"completion_tokens\":5}}\n\ndata: [DONE]\n\n");
    let mut stream = UsageTapStream::new(futures_util::stream::iter(vec![Ok(bytes.clone())]), tap(&sink));
    assert_eq!(stream.next().await.unwrap().unwrap(), bytes);
    drop(stream); // This is how a client that stops on [DONE] behaves.
    let rows = sink.0.lock().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].input_tokens, Some(80));
    assert_eq!(rows[0].output_tokens, Some(5));
}

#[tokio::test]
async fn terminal_then_eof_does_not_double_count() {
    let sink = Arc::new(Sink::default());
    let bytes = Bytes::from_static(
        b"data: {\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3}}\r\n\r\ndata: [DONE]\r\n\r\n",
    );
    let mut stream = UsageTapStream::new(futures_util::stream::iter(vec![Ok(bytes)]), tap(&sink));
    while stream.next().await.is_some() {}
    assert_eq!(sink.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn interrupted_stream_without_terminal_is_not_billed() {
    let sink = Arc::new(Sink::default());
    let bytes = Bytes::from_static(b"data: {\"usage\":{\"prompt_tokens\":80,\"completion_tokens\":1}}\n\n");
    let mut stream = UsageTapStream::new(futures_util::stream::iter(vec![Ok(bytes)]), tap(&sink));
    assert!(stream.next().await.is_some());
    drop(stream);
    assert!(sink.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn anthropic_stop_and_responses_completed_are_terminal() {
    for body in [
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":2}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n",
    ] {
        let sink = Arc::new(Sink::default());
        let mut stream = UsageTapStream::new(
            futures_util::stream::iter(vec![Ok(Bytes::copy_from_slice(body.as_bytes()))]),
            tap(&sink),
        );
        assert!(stream.next().await.is_some());
        drop(stream);
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }
}
