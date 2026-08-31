//! One-shot LLM memory extraction for completed conversation turns
//! (P2-2 followups §A.6 方案 L).
//!
//! Mirrors [`crate::capability::image_description`]'s one-shot provider call
//! shape (`dream_engine_providers::create_provider` + `LlmRequest` + timeout)
//! but is text-only: no image loading, no delegate policy. The system prompt
//! is deliberately strict — memory is injected into *future* turns' agent
//! context, so a sloppy extraction poisons every later turn ("bad memories
//! are worse than none"). Facts below an importance floor are dropped by the
//! caller.

use dream_engine_config::config::Config;
use std::sync::Arc;

use dream_engine_providers::{LlmProvider, create_provider};
use dream_engine_types::llm::LlmEvent;
use dream_engine_types::llm::LlmRequest;
use dream_engine_types::message::{ContentBlock, Message, Role};

/// Extraction never needs more than a handful of short facts.
const EXTRACTION_MAX_TOKENS: u32 = 512;
/// Hard bound so a hung upstream can never stall the spawned extraction
/// task for long (the task is fire-and-forget off the turn path).
const EXTRACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Facts scored below this are noise; `importance` is the extractor's own
/// 0..1 self-assessment and the floor keeps the memory store signal-heavy.
pub const EXTRACTION_MIN_IMPORTANCE: f64 = 0.5;

const EXTRACTION_SYSTEM_PROMPT: &str = "You are a memory extractor. From the conversation turn below, extract facts about the user that should be remembered long-term: stable preferences, standing facts (name, role, team, projects), ongoing commitments, and durable context. \
Output one JSON object per fact, one per line: {\"content\": \"...\", \"importance\": 0.0-1.0, \"tags\": [\"...\"]}. \
Extract NOTHING for one-off task instructions, code, error messages, small talk, or anything that only matters for this turn. \
If nothing is worth remembering, output nothing. Output raw JSON lines only — no markdown, no commentary.";

/// One extracted fact, parsed from the extractor model's JSON-lines output.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedFact {
    pub content: String,
    pub importance: f64,
    pub tags: Vec<String>,
}

/// Extracts salient long-term facts from one completed turn.
///
/// Returns `Err` on any transport/timeout failure (the caller skips
/// extraction — never fabricates) and `Ok(vec![])` when the model judged the
/// turn to contain nothing worth remembering. Malformed JSON lines are
/// dropped individually rather than failing the batch.
pub async fn extract_facts_via_llm(
    config: &Config,
    user_message: &str,
    assistant_message: &str,
) -> Result<Vec<ExtractedFact>, String> {
    let provider: Arc<dyn LlmProvider> = create_provider(config);
    extract_facts_with_provider_with_timeout(
        provider.as_ref(),
        config,
        user_message,
        assistant_message,
        EXTRACTION_TIMEOUT,
    )
    .await
}

async fn extract_facts_with_provider_with_timeout(
    provider: &dyn LlmProvider,
    config: &Config,
    user_message: &str,
    assistant_message: &str,
    timeout: std::time::Duration,
) -> Result<Vec<ExtractedFact>, String> {
    tokio::time::timeout(
        timeout,
        extract_facts_with_provider_inner(provider, config, user_message, assistant_message),
    )
    .await
    .map_err(|_| {
        format!(
            "memory extraction model '{}' did not finish within {} seconds; nothing was extracted",
            config.model,
            timeout.as_secs()
        )
    })?
}

async fn extract_facts_with_provider_inner(
    provider: &dyn LlmProvider,
    config: &Config,
    user_message: &str,
    assistant_message: &str,
) -> Result<Vec<ExtractedFact>, String> {
    let user_block = format!("<user_message>\n{user_message}\n</user_message>");
    let assistant_block = format!("\n<assistant_message>\n{assistant_message}\n</assistant_message>");
    let request = LlmRequest {
        model: config.model.to_owned(),
        system: EXTRACTION_SYSTEM_PROMPT.to_owned(),
        messages: vec![Message::now(
            Role::User,
            vec![
                ContentBlock::Text { text: user_block },
                ContentBlock::Text {
                    text: assistant_block,
                },
            ],
        )],
        tools: Vec::new(),
        max_tokens: Some(EXTRACTION_MAX_TOKENS),
        thinking: None,
        reasoning_effort: None,
    };

    let mut stream = provider
        .stream(&request)
        .await
        .map_err(|error| format!("memory extraction model '{}' could not be reached: {error}", config.model))?;

    let mut text = String::new();
    while let Some(event) = stream.recv().await {
        match event {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::Error(error) => {
                return Err(format!(
                    "memory extraction model '{}' returned an error: {error}",
                    config.model
                ));
            }
            LlmEvent::Done { .. } => break,
            _ => {}
        }
    }

    Ok(parse_extracted_facts(&text))
}

/// Parses the extractor model's JSON-lines output. Lines that fail to parse,
/// carry a blank content, or an unparsable importance are dropped
/// individually — a partially malformed response must not cost the whole
/// batch, and must never produce a fabricated half-fact.
fn parse_extracted_facts(text: &str) -> Vec<ExtractedFact> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('-').trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(content) = value.get("content").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let importance = value
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        out.push(ExtractedFact {
            content: content.to_owned(),
            importance,
            tags,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_lines_and_drops_malformed_ones() {
        let text = concat!(
            "{\"content\":\"用户负责支付网关项目\",\"importance\":0.9,\"tags\":[\"project\"]}\n",
            "not json at all\n",
            "{\"content\":\"\"}\n",
            "{\"content\":\"喜欢简洁回复\",\"importance\":0.6}\n",
        );
        let facts = parse_extracted_facts(text);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].content, "用户负责支付网关项目");
        assert!((facts[0].importance - 0.9).abs() < f64::EPSILON);
        assert_eq!(facts[0].tags, vec!["project".to_owned()]);
        assert_eq!(facts[1].content, "喜欢简洁回复");
    }

    #[test]
    fn empty_output_yields_no_facts() {
        assert!(parse_extracted_facts("").is_empty());
        assert!(parse_extracted_facts("（没有什么值得记录的）").is_empty());
    }
}
