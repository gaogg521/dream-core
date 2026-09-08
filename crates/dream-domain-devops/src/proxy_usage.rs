//! Usage metering for the model proxy (P1-2).
//!
//! Company channels were the one LLM path with no accounting at all: the
//! member's local dreamcore is a personal build (no billing plane compiled
//! in), and the proxy itself only ever forwarded bytes. This module closes
//! that hole at the proxy — the one place every company-channel call has to
//! pass through, whatever the member's client does locally.
//!
//! Three pieces:
//!
//! * [`ProxyUsageRecorder`] — the fire-and-forget seam. dream-app wires a
//!   billing-backed implementation under the `enterprise` feature; personal
//!   builds leave the slot empty and the tap below compiles to a pure
//!   pass-through.
//! * [`UsageTap`] — a byte-side parser that sits on the upstream response
//!   stream. It forwards every byte unchanged and, in parallel, accumulates
//!   just enough to lift the `model` and the `usage` block out of an SSE
//!   stream or a JSON body. Wire formats differ per vendor, so the merge
//!   rules are deliberately generic (see `absorb_event`).
//! * [`prepare_forward_body`] — a bounded peek at JSON request bodies, which
//!   (a) learns the requested model as a fallback attribution and (b) adds
//!   `stream_options.include_usage` to OpenAI-shaped streaming requests so
//!   their final frame carries token counts. Without (b) an OpenAI-style
//!   stream reports no usage at all and the row would be permanently blind.
//!
//! # Honesty rules
//!
//! A row is recorded only for an upstream 2xx whose body fully streamed
//! through. Anything else — a transport error, a 4xx/5xx, a client that hangs
//! up mid-stream — records nothing: half a usage block is worse than none.
//! And when a stream simply never reports tokens (a vendor that ignores
//! `include_usage`), the row still lands with the model it does know, so the
//! call is visible even where the cost is not.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::{Body, Bytes};
// StreamExt only, deliberately: importing TryStreamExt as well would make
// `.next()` ambiguous on the `Result`-item streams below.
use futures_util::{Stream, StreamExt};

/// Cap on the backlog an SSE tap may hold while hunting for a frame boundary.
/// Real frames are tens of bytes; anything over this is not SSE and the
/// backlog is dropped rather than grown.
const SSE_BUFFER_CAP: usize = 1024 * 1024;

/// Cap on how much of a JSON response body the tap will hold for end-of-body
/// parsing. Chat completions land well under it; over the cap the tap gives
/// up (the forwarded bytes are unaffected either way).
const JSON_PARSE_CAP: usize = 8 * 1024 * 1024;

/// Cap on how much of a JSON request body the proxy may read to learn the
/// model / inject `stream_options`. Beyond it the request is forwarded
/// streamingly and unprobed — large media payloads keep their pass-through.
const REQUEST_PROBE_LIMIT: usize = 8 * 1024 * 1024;

/// One completed upstream model call seen by the proxy, ready for the
/// billing plane.
///
/// `channel_id` is the raw registry channel id (`one_provider_registry.id`);
/// implementations map it to the persisted attribution form before writing
/// (`prov_chan_<channel_id>`, the convention `record_turn` documents).
#[derive(Clone)]
pub struct ProxyUsageEvent {
    pub user_id: String,
    pub channel_id: String,
    /// Conversation the client attributed this call to (C1-4), from the
    /// `x-dream-conversation-id` request header. `None` = unattributed.
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Wall time from the moment the tap was built to the end of the upstream
    /// stream. It is the whole point of the per-call trace — without it the
    /// admin console's latency percentiles have nothing to compute over for
    /// proxied traffic, which on an enterprise deployment is most of it.
    ///
    /// Measured, not estimated: `None` only if the clock went backwards.
    pub duration_ms: Option<i64>,
}

/// Sink for model-proxy usage (P1-2). Fire-and-forget by contract — same rule
/// as the conversation crate's `UsageRecorder`: implementations spawn their
/// own async work and MUST NOT block or fail the proxied call. Wired to
/// one-billing in dream-app; `None` in personal builds (no rows at all).
pub trait ProxyUsageRecorder: Send + Sync {
    fn record_proxy_usage(&self, event: ProxyUsageEvent);
}

/// What to hand to reqwest after the request-side probe.
pub enum PreparedBody {
    /// The whole body fit under the probe cap (and was possibly rewritten to
    /// request usage in the response stream).
    Bytes(Bytes),
    /// Forward as a stream — either the probe was skipped entirely, or the
    /// body outgrew the cap and the probed prefix is chained in front of the
    /// unread remainder.
    Stream(futures_util::stream::BoxStream<'static, std::io::Result<Bytes>>),
}

/// Peeks at an outbound request body long enough to attribute a model and —
/// for OpenAI-shaped streaming requests — to ask the vendor for usage in the
/// final SSE frame. Everything the probe does not consume (other platforms,
/// non-JSON bodies, oversized payloads) continues downstream as a stream,
/// byte-for-byte as before.
///
/// The rewrite is deliberately narrow: only a body that already speaks OpenAI
/// chat completions (`"stream": true` + a `messages` array, and no
/// `stream_options` of its own) is touched. Anthropic rejects the field
/// outright and Bedrock takes its own buffered path, so both skip the probe
/// by platform before ever reading a byte.
pub async fn prepare_forward_body(body: Body, platform: &str, content_type: &str) -> (PreparedBody, Option<String>) {
    if platform == "anthropic" || platform == "bedrock" || !content_type.contains("json") {
        return (
            PreparedBody::Stream(Box::pin(
                body.into_data_stream()
                    .map(|result| result.map_err(std::io::Error::other)),
            )),
            None,
        );
    }

    // The probe has to consume bytes to see them, and consumed bytes cannot go
    // back into `body` — so whatever is read is chained in front of the
    // remainder on the way out. Requests that finish under the cap are simply
    // the whole body, possibly rewritten.
    let mut data_stream = body
        .into_data_stream()
        .map(|result| result.map_err(std::io::Error::other));
    let mut prefix: Vec<u8> = Vec::new();
    let complete = loop {
        match data_stream.next().await {
            Some(Ok(bytes)) => {
                prefix.extend_from_slice(&bytes);
                if prefix.len() > REQUEST_PROBE_LIMIT {
                    break false;
                }
            }
            // The body failed mid-read: forward what was read in front of the
            // still-failing stream and let the error surface from the upstream
            // call, exactly as it would have without the probe.
            Some(Err(err)) => {
                tracing::debug!(error = %err, "model_proxy: request body read failed during usage probe");
                break false;
            }
            None => break true,
        }
    };

    if !complete {
        let rest = futures_util::stream::once(async move { Ok(Bytes::from(prefix)) })
            .chain(data_stream)
            .boxed();
        return (PreparedBody::Stream(rest), None);
    }

    let mut request_model = None;
    let forward = match serde_json::from_slice::<serde_json::Value>(&prefix) {
        Ok(mut value) => {
            request_model = value
                .get("model")
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .map(str::to_owned);
            if inject_stream_options(&mut value) {
                serde_json::to_vec(&value)
                    .map(Bytes::from)
                    .unwrap_or_else(|_| Bytes::from(prefix))
            } else {
                Bytes::from(prefix)
            }
        }
        // Not JSON (or not yet parseable): forward verbatim.
        Err(_) => Bytes::from(prefix),
    };
    (PreparedBody::Bytes(forward), request_model)
}

/// Adds `stream_options.include_usage = true` to an OpenAI chat-completions
/// streaming request, returning whether the body was modified.
///
/// Without this the vendor's stream reports no usage at all and the proxy
/// could never meter OpenAI-style channels. The field is part of the
/// de-facto OpenAI-compatible contract (OpenAI, vLLM, OpenRouter, DeepSeek,
/// DashScope and Gemini's OpenAI-compat endpoint all honor it); a caller that
/// already set `stream_options` is left exactly as it was.
fn inject_stream_options(value: &mut serde_json::Value) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    if object.get("stream") != Some(&serde_json::Value::Bool(true)) {
        return false;
    }
    if !matches!(object.get("messages"), Some(serde_json::Value::Array(_))) {
        return false;
    }
    if object.contains_key("stream_options") {
        return false;
    }
    object.insert(
        "stream_options".to_owned(),
        serde_json::json!({ "include_usage": true }),
    );
    true
}

/// Which side-channel parse mode the response tap runs in.
#[derive(Clone, Copy, PartialEq)]
enum TapMode {
    /// `text/event-stream`: split frames on blank lines, parse each `data:`
    /// payload as it completes, keep only the trailing partial frame.
    Sse,
    /// Anything else is treated as one JSON document parsed at stream end.
    Json,
}

/// Everything the tap needs to fire one usage event at stream end.
struct TapContext {
    recorder: Arc<dyn ProxyUsageRecorder>,
    user_id: String,
    channel_id: String,
    /// Model claimed by the request body, when the probe saw one. The
    /// response's own `model` wins; this only fills the gap for responses
    /// too large to parse.
    request_model: Option<String>,
    /// Only a 2xx upstream response may produce a row.
    success: bool,
    /// Conversation the client attributed this call to (C1-4), read from the
    /// `x-dream-conversation-id` request header. `None` = the caller did not
    /// say (an old client, or a non-conversation call) — the row stays
    /// unattributed rather than guessing.
    conversation_id: Option<String>,
    /// When the tap was built — as close to "the call started" as this layer
    /// gets, which is after the request body probe and before the upstream
    /// request is sent.
    started_at: Instant,
}

/// Byte-side usage parser for one proxied response. Constructed per request;
/// inert (zero buffering) when no recorder is wired.
pub struct UsageTap {
    /// `None` = no billing plane: absorb becomes a no-op and nothing is held.
    context: Option<TapContext>,
    mode: TapMode,
    buffer: Vec<u8>,
    model: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    /// The stream errored mid-flight — never record a partial call.
    failed: bool,
}

impl UsageTap {
    pub fn new(
        recorder: Option<Arc<dyn ProxyUsageRecorder>>,
        user_id: String,
        channel_id: String,
        content_type: &str,
        upstream_success: bool,
        request_model: Option<String>,
        conversation_id: Option<String>,
    ) -> Self {
        let context = recorder.map(|recorder| TapContext {
            recorder,
            user_id,
            channel_id,
            request_model,
            conversation_id,
            success: upstream_success,
            started_at: Instant::now(),
        });
        let mode = if content_type.contains("text/event-stream") {
            TapMode::Sse
        } else {
            TapMode::Json
        };
        Self {
            context,
            mode,
            buffer: Vec::new(),
            model: None,
            input_tokens: None,
            output_tokens: None,
            failed: false,
        }
    }

    /// Feed one chunk of the upstream response. The bytes themselves are
    /// forwarded by the caller; the tap only keeps what parsing still needs.
    pub fn absorb(&mut self, bytes: &[u8]) {
        if self.context.is_none() {
            return;
        }
        match self.mode {
            TapMode::Sse => self.absorb_sse(bytes),
            TapMode::Json => self.absorb_json(bytes),
        }
    }

    /// The stream errored mid-flight; a partial call must not be recorded.
    pub fn mark_failed(&mut self) {
        self.failed = true;
    }

    /// The upstream stream ended. Resolves whatever the mode held back, then
    /// fires at most one usage event.
    pub fn finish(&mut self) {
        match self.mode {
            TapMode::Json if !self.buffer.is_empty() => {
                let buffer = std::mem::take(&mut self.buffer);
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&buffer) {
                    self.absorb_event(&value);
                }
            }
            TapMode::Sse if !self.buffer.is_empty() => {
                // A stream that ended without a trailing blank line still has
                // its last frame in the backlog. A truncated (broken) frame
                // simply fails to parse and is skipped.
                let frame = std::mem::take(&mut self.buffer);
                self.absorb_frame(&frame);
            }
            _ => {}
        }

        let Some(context) = self.context.take() else {
            return;
        };
        if self.failed || !context.success {
            return;
        }
        let model = self.model.clone().or_else(|| context.request_model.clone());
        if model.is_none() && self.input_tokens.is_none() && self.output_tokens.is_none() {
            return;
        }
        context.recorder.record_proxy_usage(ProxyUsageEvent {
            user_id: context.user_id,
            channel_id: context.channel_id,
            conversation_id: context.conversation_id,
            model,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            duration_ms: i64::try_from(context.started_at.elapsed().as_millis()).ok(),
        });
    }

    fn absorb_json(&mut self, bytes: &[u8]) {
        if self.buffer.len() + bytes.len() > JSON_PARSE_CAP {
            // Oversized response: give up on parsing. (JSON mode only ever
            // resolves at stream end, so there is nothing partial to keep.)
            self.buffer.clear();
            self.buffer.shrink_to_fit();
            return;
        }
        self.buffer.extend_from_slice(bytes);
    }

    fn absorb_sse(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        while let Some((frame_end, next_start)) = find_frame_end(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..next_start).collect();
            self.absorb_frame(&frame[..frame_end]);
        }
        if self.buffer.len() > SSE_BUFFER_CAP {
            // No blank line in sight: not actually SSE, or a hostile peer.
            // Drop the backlog instead of growing it without bound.
            self.buffer.clear();
        }
    }

    fn absorb_frame(&mut self, frame: &[u8]) {
        // SSE joins consecutive `data:` lines of one event with `\n`.
        let mut data = String::new();
        for line in frame.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(rest) = line.strip_prefix(b"data:") {
                let rest = rest.strip_prefix(b" ").unwrap_or(rest);
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(&String::from_utf8_lossy(rest));
            }
        }
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            self.absorb_event(&value);
        }
    }

    /// Merges one parsed event into the captured state.
    ///
    /// Wire shapes covered, all by the same rules:
    /// * OpenAI chat chunk — `model` on every chunk, `usage` only on the
    ///   final one (when `include_usage` was requested).
    /// * Anthropic — `message_start` carries `message.model` +
    ///   `message.usage.input_tokens`, `message_delta` carries the cumulative
    ///   `usage.output_tokens`.
    /// * OpenAI Responses API — `response.completed` nests both under
    ///   `response`.
    /// * Bedrock InvokeModel (Anthropic-style) — one JSON body with top-level
    ///   `model` + `usage`.
    ///
    /// Per-field last-write-wins: Anthropic's delta overwrites the
    /// placeholder output count from `message_start`, while the input count
    /// (absent from the delta) survives.
    fn absorb_event(&mut self, value: &serde_json::Value) {
        if self.model.is_none() {
            for probe in [Some(value), value.get("message"), value.get("response")] {
                if let Some(model) = probe.and_then(|v| v.get("model")).and_then(|m| m.as_str())
                    && !model.is_empty()
                {
                    self.model = Some(model.to_owned());
                    break;
                }
            }
        }
        let usage = value
            .get("usage")
            .or_else(|| value.get("message").and_then(|m| m.get("usage")))
            .or_else(|| value.get("response").and_then(|r| r.get("usage")))
            .and_then(|u| u.as_object());
        let Some(usage) = usage else {
            return;
        };
        // OpenAI names them prompt/completion; Anthropic (and Bedrock's
        // Anthropic-style bodies) input/output.
        let input = usage
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_i64)
            .or_else(|| usage.get("input_tokens").and_then(serde_json::Value::as_i64));
        let output = usage
            .get("completion_tokens")
            .and_then(serde_json::Value::as_i64)
            .or_else(|| usage.get("output_tokens").and_then(serde_json::Value::as_i64));
        if input.is_some() {
            self.input_tokens = input;
        }
        if output.is_some() {
            self.output_tokens = output;
        }
    }
}

/// First frame boundary in `buf`, as `(frame_end, resume_at)` — the index of
/// the separator's first byte, and where the next frame starts. Both `\n\n`
/// and `\r\n\r\n` count (the latter does not contain the former as a byte
/// pair, so both have to be searched).
fn find_frame_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = find_subslice(buf, b"\n\n").map(|i| (i, i + 2));
    if let Some(i) = find_subslice(buf, b"\r\n\r\n")
        && best.is_none_or(|(j, _)| i < j)
    {
        best = Some((i, i + 4));
    }
    best
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Wraps an upstream byte stream: every chunk is forwarded unchanged, the tap
/// watches it, and stream end fires the (at most one) usage event. Error
/// chunks mark the tap so a broken call never records.
pub struct UsageTapStream<S> {
    inner: S,
    tap: Option<UsageTap>,
}

impl<S> UsageTapStream<S> {
    pub fn new(inner: S, tap: UsageTap) -> Self {
        Self { inner, tap: Some(tap) }
    }
}

impl<S> Stream for UsageTapStream<S>
where
    S: Stream<Item = std::io::Result<Bytes>> + Unpin,
{
    type Item = std::io::Result<Bytes>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match std::pin::Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some(tap) = self.tap.as_mut() {
                    tap.absorb(&bytes);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                if let Some(tap) = self.tap.as_mut() {
                    tap.mark_failed();
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if let Some(mut tap) = self.tap.take() {
                    tap.finish();
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingSink(std::sync::Mutex<Vec<ProxyUsageEvent>>);

    impl ProxyUsageRecorder for RecordingSink {
        fn record_proxy_usage(&self, event: ProxyUsageEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn sink() -> Arc<RecordingSink> {
        Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())))
    }

    fn make_tap(recorder: &Arc<RecordingSink>, content_type: &str, success: bool) -> UsageTap {
        UsageTap::new(
            Some(recorder.clone()),
            "member-1".into(),
            "ch-1".into(),
            content_type,
            success,
            None,
            Some("conv-1".into()),
        )
    }

    fn recorded(recorder: &RecordingSink) -> Vec<ProxyUsageEvent> {
        recorder.0.lock().unwrap().clone()
    }

    /// The trace's whole reason for existing is latency, so a recorded call
    /// has to carry a measured one. `None` here would leave the admin console's
    /// percentiles with nothing to compute over for proxied traffic.
    #[test]
    fn a_recorded_call_carries_a_measured_duration() {
        let sink = sink();
        let mut tap = make_tap(&sink, "application/json", true);
        tap.absorb(br#"{"model":"gpt-4o","usage":{"prompt_tokens":10,"completion_tokens":2}}"#);
        tap.finish();

        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].duration_ms.is_some(),
            "a recorded call must report how long it took"
        );
    }

    /// C1-4: the conversation id the client stamped on the request must reach
    /// the usage event verbatim — this is what lands the spend on the right
    /// conversation in the billing plane.
    #[test]
    fn a_recorded_call_carries_the_client_conversation_id() {
        let sink = sink();
        let mut tap = make_tap(&sink, "application/json", true);
        tap.absorb(br#"{"model":"gpt-4o","usage":{"prompt_tokens":10,"completion_tokens":2}}"#);
        tap.finish();

        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].conversation_id.as_deref(),
            Some("conv-1"),
            "the client's conversation attribution must survive the tap"
        );
    }

    #[test]
    fn inert_tap_buffers_nothing_and_records_nothing() {
        let mut tap = UsageTap::new(None, "u".into(), "c".into(), "text/event-stream", true, None, None);
        tap.absorb(b"data: {\"model\":\"m\"}\n\n");
        tap.finish();
        // No recorder behind the tap: it must not have retained the bytes.
        assert!(tap.buffer.is_empty());
    }

    #[test]
    fn anthropic_stream_usage_is_captured_across_chunk_boundaries() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        // Fed one byte at a time: every frame straddles chunk boundaries, two
        // of them inside the JSON payload itself.
        let frames = "event: message_start\n\
                      data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-4\",\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\
                      \n\
                      event: content_block_delta\n\
                      data: {\"type\":\"content_block_delta\"}\n\
                      \n\
                      event: message_delta\n\
                      data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":17}}\n\
                      \n";
        for byte in frames.as_bytes() {
            tap.absorb(std::slice::from_ref(byte));
        }
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1, "one call, one row");
        let event = &events[0];
        assert_eq!(event.user_id, "member-1");
        assert_eq!(event.channel_id, "ch-1");
        assert_eq!(event.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(event.input_tokens, Some(25));
        assert_eq!(event.output_tokens, Some(17), "message_delta's cumulative count wins");
    }

    #[test]
    fn openai_stream_maps_prompt_and_completion_tokens() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        tap.absorb(b"data: {\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}],\"usage\":null}\n\n");
        tap.absorb(
            b"data: {\"model\":\"gpt-4o\",\"choices\":[],\"usage\":{\"prompt_tokens\":119,\"completion_tokens\":43,\"total_tokens\":162}}\n\n",
        );
        tap.absorb(b"data: [DONE]\n\n");
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-4o"));
        assert_eq!(events[0].input_tokens, Some(119));
        assert_eq!(events[0].output_tokens, Some(43));
    }

    /// A vendor that ignores `include_usage` still yields a visible row —
    /// with the model, and NULL tokens — rather than vanishing.
    #[test]
    fn openai_stream_without_usage_still_records_the_model() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        tap.absorb(b"data: {\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        tap.absorb(b"data: [DONE]\n\n");
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-4o"));
        assert_eq!(events[0].input_tokens, None);
        assert_eq!(events[0].output_tokens, None);
    }

    #[test]
    fn crlf_framed_sse_is_parsed() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        tap.absorb(
            b"data: {\"model\":\"m\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}\r\n\r\ndata: [DONE]\r\n\r\n",
        );
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(3));
        assert_eq!(events[0].output_tokens, Some(4));
    }

    #[test]
    fn an_sse_stream_without_a_trailing_blank_line_still_flushes_its_last_frame() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        tap.absorb(b"data: {\"usage\":{\"input_tokens\":9}}\n\ndata: {\"usage\":{\"output_tokens\":11}}");
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(9));
        assert_eq!(events[0].output_tokens, Some(11));
    }

    #[test]
    fn json_response_body_records_usage_and_model() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "application/json", true);
        let body = br#"{"id":"x","model":"gpt-4o","choices":[{"message":{"role":"assistant","content":"yo"}}],"usage":{"prompt_tokens":7,"completion_tokens":9}}"#;
        tap.absorb(&body[..40]);
        tap.absorb(&body[40..]);
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-4o"));
        assert_eq!(events[0].input_tokens, Some(7));
        assert_eq!(events[0].output_tokens, Some(9));
    }

    #[test]
    fn anthropic_style_json_body_reads_nested_message_usage() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "application/json", true);
        tap.absorb(
            br#"{"id":"msg_1","type":"message","model":"claude-sonnet-4","content":[],"usage":{"input_tokens":62,"output_tokens":8}}"#,
        );
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(events[0].input_tokens, Some(62));
        assert_eq!(events[0].output_tokens, Some(8));
    }

    #[test]
    fn an_upstream_error_is_never_recorded() {
        let recorder = sink();
        // 4xx passes through to the caller; the tap must not turn it into a
        // usage row even though the body (an error JSON) parses.
        let mut tap = make_tap(&recorder, "application/json", false);
        tap.absorb(br#"{"error":{"message":"rate limited","usage":{"prompt_tokens":1,"completion_tokens":1}}}"#);
        tap.finish();
        assert!(recorded(&recorder).is_empty());

        // Same for a stream that broke mid-flight under a 2xx.
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        tap.absorb(b"data: {\"model\":\"m\",\"usage\":{\"input_tokens\":1}}\n\n");
        tap.mark_failed();
        tap.finish();
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn a_response_with_neither_model_nor_usage_records_nothing() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "application/json", true);
        tap.absorb(b"{}");
        tap.finish();
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn json_bodies_over_the_parse_cap_are_dropped_not_held() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "application/json", true);
        let junk = vec![b'x'; JSON_PARSE_CAP + 1];
        tap.absorb(&junk);
        assert!(tap.buffer.is_empty(), "over-cap backlog is released");
        tap.finish();
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn sse_backlog_over_the_cap_is_dropped() {
        let recorder = sink();
        let mut tap = make_tap(&recorder, "text/event-stream", true);
        let junk = vec![b'x'; SSE_BUFFER_CAP + 1];
        tap.absorb(&junk);
        assert!(tap.buffer.is_empty());
        tap.finish();
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn request_model_fills_the_gap_when_the_response_names_none() {
        let recorder = sink();
        let mut tap = UsageTap::new(
            Some(recorder.clone()),
            "member-1".into(),
            "ch-1".into(),
            "application/json",
            true,
            Some("gpt-4o".into()),
            None,
        );
        tap.absorb(br#"{"usage":{"prompt_tokens":5,"completion_tokens":6}}"#);
        tap.finish();

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn stream_options_are_injected_only_into_openai_chat_stream_requests() {
        let inject = |body: &str| {
            let mut value = serde_json::from_str::<serde_json::Value>(body).unwrap();
            inject_stream_options(&mut value)
        };
        // The exact shape the rewrite exists for.
        assert!(inject(
            r#"{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}"#
        ));
        // Anything the caller already decided is left alone.
        assert!(!inject(
            r#"{"stream":true,"messages":[],"stream_options":{"include_usage":false}}"#
        ));
        // Not a streaming chat completion: no rewrite.
        assert!(!inject(r#"{"model":"gpt-4o","stream":false,"messages":[]}"#));
        assert!(!inject(r#"{"model":"text-embedding-3-small","input":"hi"}"#));
        assert!(!inject(
            r#"{"model":"gpt-4o","stream":true,"input":"responses-api-body"}"#
        ));
    }

    #[tokio::test]
    async fn openai_platform_streaming_request_gets_usage_requested() {
        let body = Body::from(r#"{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}"#);
        let (prepared, model) = prepare_forward_body(body, "openai", "application/json").await;
        let PreparedBody::Bytes(bytes) = prepared else {
            panic!("a body that fit under the probe cap must come back as bytes");
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["stream_options"]["include_usage"], serde_json::Value::Bool(true));
        assert_eq!(model.as_deref(), Some("gpt-4o"));
    }

    #[tokio::test]
    async fn anthropic_platform_requests_are_never_touched() {
        let raw: &[u8] = br#"{"model":"claude-sonnet-4","stream":true,"max_tokens":64,"messages":[]}"#;
        let body = Body::from(Bytes::from_static(raw));
        let (prepared, model) = prepare_forward_body(body, "anthropic", "application/json").await;
        let PreparedBody::Stream(stream) = prepared else {
            panic!("an unprobed platform must stream through");
        };
        let mut collected = Vec::new();
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, raw, "byte-for-byte pass-through");
        assert!(model.is_none(), "unprobed platforms report no request model");
    }

    #[tokio::test]
    async fn non_json_bodies_stream_through_unprobed() {
        let body = Body::from(Bytes::from_static(b"\x00\x01\x02 binary-ish"));
        let (prepared, model) = prepare_forward_body(body, "openai", "application/octet-stream").await;
        assert!(matches!(prepared, PreparedBody::Stream(_)));
        assert!(model.is_none());
    }

    #[tokio::test]
    async fn oversized_requests_chain_the_probed_prefix_in_front_of_the_rest() {
        let big = vec![b'a'; REQUEST_PROBE_LIMIT + 1024];
        let body = Body::from(Bytes::from(big));
        let (prepared, model) = prepare_forward_body(body, "openai", "application/json").await;
        let PreparedBody::Stream(mut stream) = prepared else {
            panic!("an over-cap body must stay a stream");
        };
        assert!(model.is_none());
        // The full payload must still go out: the probed prefix chained in
        // front of the unread remainder.
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected.len(), REQUEST_PROBE_LIMIT + 1024);
        assert!(collected.iter().all(|&b| b == b'a'));
    }

    #[tokio::test]
    async fn the_tap_stream_forwards_bytes_and_fires_once_at_end() {
        let recorder = sink();
        let tap = make_tap(&recorder, "text/event-stream", true);
        let (c1, c2, c3) = (
            "data: {\"model\":\"m\",\"usage\":{\"input_tokens\":2",
            ",\"output_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let frames: Vec<std::io::Result<Bytes>> = vec![Ok(Bytes::from(c1)), Ok(Bytes::from(c2)), Ok(Bytes::from(c3))];
        let mut stream = UsageTapStream::new(futures_util::stream::iter(frames), tap);
        let mut forwarded = Vec::new();
        while let Some(chunk) = stream.next().await {
            forwarded.extend_from_slice(&chunk.unwrap());
        }
        // What the caller receives must be byte-identical to the upstream.
        let mut expected = Vec::new();
        expected.extend_from_slice(c1.as_bytes());
        expected.extend_from_slice(c2.as_bytes());
        expected.extend_from_slice(c3.as_bytes());
        assert_eq!(forwarded, expected);

        let events = recorded(&recorder);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(2));
        assert_eq!(events[0].output_tokens, Some(5));
    }
}
