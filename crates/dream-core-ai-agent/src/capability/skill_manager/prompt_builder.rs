use super::{SkillDefinition, SkillIndex};

/// Hard cap for one skill's index line (description part), in characters.
///
/// Imported skills carry long contract descriptions — trigger words, negative
/// boundaries, preconditions, several hundred characters each. With the
/// always-on custom-skill set a catalog can hold 100+ skills, and this index
/// rides along in the FIRST MESSAGE of every prompt-protocol conversation.
/// The index only owes the model enough to decide "this skill might match";
/// the full description is inside SKILL.md, which loads on demand.
const INDEX_DESCRIPTION_CAP: usize = 120;

/// One-line digest of a skill description for the conversation index.
///
/// Keeps the leading function sentence (everything up to the first sentence
/// break — CJK `。` or Latin `. `), then hard-caps at [`INDEX_DESCRIPTION_CAP`]
/// characters with an ellipsis. The trigger vocabulary and boundaries that
/// follow live in SKILL.md and are read when the skill is actually loaded.
fn index_description_digest(description: &str) -> String {
    let trimmed = description.trim();
    let sentence = match trimmed.find('。') {
        Some(idx) if idx > 0 => &trimmed[..idx],
        _ => trimmed,
    };
    let sentence = match sentence.find(". ") {
        Some(idx) if idx > 0 => &sentence[..idx],
        _ => sentence,
    };
    if sentence.chars().count() <= INDEX_DESCRIPTION_CAP {
        sentence.to_string()
    } else {
        let cut: String = sentence.chars().take(INDEX_DESCRIPTION_CAP).collect();
        format!("{cut}…")
    }
}

/// Build a formatted text block listing available skills for injection.
///
/// The output includes skill names with one-line description digests (see
/// [`index_description_digest`]) and instructions on how to request loading
/// via `[LOAD_SKILL: name]`.
pub fn build_skills_index_text(skills: &[SkillIndex]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut lines = Vec::with_capacity(skills.len() + 4);
    lines.push("## Available Skills".to_string());
    lines.push(String::new());
    lines.push("To load a skill, include `[LOAD_SKILL: skill-name]` in your response.".to_string());
    lines.push(String::new());

    for skill in skills {
        lines.push(format!(
            "- **{}**: {}",
            skill.name,
            index_description_digest(&skill.description)
        ));
    }

    lines.join("\n")
}

/// Build system instructions text with full skill content (for Gemini).
pub fn build_system_instructions(base_instructions: &str, skills: &[SkillDefinition]) -> String {
    if skills.is_empty() {
        return base_instructions.to_string();
    }

    let mut parts = vec![base_instructions.to_string()];

    for skill in skills {
        if let Some(body) = &skill.body {
            parts.push(format!("\n## Skill: {}\n\n{}", skill.name, body));
        }
    }

    parts.join("\n")
}

/// Prepare the first message with skills index prefix (for ACP/Codex).
///
/// Prepends `[Assistant Rules]` block with skill index to the user content.
pub fn prepare_first_message_with_skills_index(
    content: &str,
    skills: &[SkillIndex],
    preset_context: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    let index_text = build_skills_index_text(skills);
    let has_rules = !index_text.is_empty() || preset_context.is_some();

    if has_rules {
        parts.push("[Assistant Rules]".to_string());

        if let Some(ctx) = preset_context
            && !ctx.is_empty()
        {
            parts.push(ctx.to_string());
        }

        if !index_text.is_empty() {
            parts.push(index_text);
        }

        parts.push("[/Assistant Rules]".to_string());
        parts.push(String::new());
    }

    parts.push(content.to_string());
    parts.join("\n")
}

/// Build system instructions with skills index only (for Gemini index-only mode).
///
/// Unlike [`build_system_instructions`] which injects full skill bodies,
/// this variant injects only the skill index (name + description) and
/// the `[LOAD_SKILL]` protocol, allowing the agent to request full content on demand.
pub fn build_system_instructions_with_skills_index(base_instructions: &str, skills: &[SkillIndex]) -> String {
    let index_text = build_skills_index_text(skills);
    if index_text.is_empty() {
        return base_instructions.to_string();
    }

    format!("{base_instructions}\n\n{index_text}")
}

/// Prepare the first message with full skill content (for Gemini).
///
/// Prepends `[Assistant Rules]` block with complete skill bodies.
pub fn prepare_first_message(content: &str, skills: &[SkillDefinition], preset_context: Option<&str>) -> String {
    let mut parts = Vec::new();
    let has_rules = !skills.is_empty() || preset_context.is_some();

    if has_rules {
        parts.push("[Assistant Rules]".to_string());

        if let Some(ctx) = preset_context
            && !ctx.is_empty()
        {
            parts.push(ctx.to_string());
        }

        for skill in skills {
            if let Some(body) = &skill.body {
                parts.push(format!("## Skill: {}\n\n{}", skill.name, body));
            }
        }

        parts.push("[/Assistant Rules]".to_string());
        parts.push(String::new());
    }

    parts.push(content.to_string());
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Skills index text
    // -----------------------------------------------------------------------

    #[test]
    fn build_skills_index_text_empty() {
        assert!(build_skills_index_text(&[]).is_empty());
    }

    #[test]
    fn build_skills_index_text_with_skills() {
        let skills = vec![
            SkillIndex {
                name: "review".into(),
                description: "Code review".into(),
            },
            SkillIndex {
                name: "debug".into(),
                description: "Debugging helper".into(),
            },
        ];
        let text = build_skills_index_text(&skills);
        assert!(text.contains("## Available Skills"));
        assert!(text.contains("[LOAD_SKILL: skill-name]"));
        assert!(text.contains("- **review**: Code review"));
        assert!(text.contains("- **debug**: Debugging helper"));
    }

    /// The contract descriptions imported skills carry run to several hundred
    /// characters; the index must carry only the function sentence so a
    /// 100-skill catalog stays affordable in every first message.
    #[test]
    fn build_skills_index_text_digests_long_contract_descriptions() {
        let skills = vec![SkillIndex {
            name: "fund-analysis".into(),
            description: "基金组合诊断与单基评估。触发：用户说“基金分析/帮我看看持仓/fund analysis”时使用；不要用于生成 Excel 文件（改用 officecli-xlsx）。需要用户提供：持仓明细。".into(),
        }];
        let text = build_skills_index_text(&skills);
        assert!(text.contains("- **fund-analysis**: 基金组合诊断与单基评估"));
        assert!(!text.contains("触发"), "index must drop the trigger tail");
        assert!(
            !text.contains("officecli-xlsx"),
            "index must drop the negative boundary"
        );
    }

    #[test]
    fn build_skills_index_text_caps_sentenceless_descriptions() {
        let long = "A very long function description without any sentence break ".repeat(10);
        let skills = vec![SkillIndex {
            name: "long".into(),
            description: long.clone(),
        }];
        let text = build_skills_index_text(&skills);
        let line = text.lines().find(|l| l.starts_with("- **long**")).unwrap();
        let digest = line.trim_start_matches("- **long**: ");
        assert!(digest.chars().count() <= 121, "cap + ellipsis");
        assert!(digest.ends_with('…'));
    }

    #[test]
    fn build_skills_index_text_keeps_latin_first_sentence() {
        let skills = vec![SkillIndex {
            name: "scan".into(),
            description: "Security review for code and dependencies. Triggers: audit. Do not use for reports.".into(),
        }];
        let text = build_skills_index_text(&skills);
        assert!(text.contains("- **scan**: Security review for code and dependencies"));
        assert!(!text.contains("Triggers"));
    }

    // -----------------------------------------------------------------------
    // First message preparation
    // -----------------------------------------------------------------------

    #[test]
    fn prepare_first_message_with_index_no_skills() {
        let result = prepare_first_message_with_skills_index("Hello", &[], None);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn prepare_first_message_with_index_and_context() {
        let skills = vec![SkillIndex {
            name: "test".into(),
            description: "Testing".into(),
        }];
        let result = prepare_first_message_with_skills_index("Hello", &skills, Some("Be concise."));
        assert!(result.contains("[Assistant Rules]"));
        assert!(result.contains("Be concise."));
        assert!(result.contains("- **test**: Testing"));
        assert!(result.contains("[/Assistant Rules]"));
        assert!(result.ends_with("Hello"));
    }

    #[test]
    fn prepare_first_message_with_full_skills() {
        let skills = vec![SkillDefinition {
            name: "review".into(),
            description: "Review".into(),
            location: PathBuf::new(),
            source: dream_core_extension::SkillSource::Custom,
            relative_location: None,
            body: Some("Full review instructions here.".into()),
        }];
        let result = prepare_first_message("Hello", &skills, None);
        assert!(result.contains("[Assistant Rules]"));
        assert!(result.contains("## Skill: review"));
        assert!(result.contains("Full review instructions here."));
        assert!(result.contains("[/Assistant Rules]"));
        assert!(result.ends_with("Hello"));
    }

    #[test]
    fn prepare_first_message_no_skills_no_context() {
        let result = prepare_first_message("Hello", &[], None);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn prepare_first_message_context_only() {
        let result = prepare_first_message_with_skills_index("Hello", &[], Some("Rules here."));
        assert!(result.contains("[Assistant Rules]"));
        assert!(result.contains("Rules here."));
        assert!(result.ends_with("Hello"));
    }

    // -----------------------------------------------------------------------
    // System instructions builder
    // -----------------------------------------------------------------------

    #[test]
    fn build_system_instructions_no_skills() {
        let result = build_system_instructions("Base prompt", &[]);
        assert_eq!(result, "Base prompt");
    }

    #[test]
    fn build_system_instructions_with_skills() {
        let skills = vec![SkillDefinition {
            name: "helper".into(),
            description: "A helper".into(),
            location: PathBuf::new(),
            source: dream_core_extension::SkillSource::Custom,
            relative_location: None,
            body: Some("Helper body content.".into()),
        }];
        let result = build_system_instructions("Base prompt", &skills);
        assert!(result.starts_with("Base prompt"));
        assert!(result.contains("## Skill: helper"));
        assert!(result.contains("Helper body content."));
    }

    #[test]
    fn build_system_instructions_with_skills_index_no_skills() {
        let result = build_system_instructions_with_skills_index("Base prompt", &[]);
        assert_eq!(result, "Base prompt");
    }

    #[test]
    fn build_system_instructions_with_skills_index_includes_index() {
        let skills = vec![SkillIndex {
            name: "helper".into(),
            description: "A helper skill".into(),
        }];
        let result = build_system_instructions_with_skills_index("Base prompt", &skills);
        assert!(result.starts_with("Base prompt"));
        assert!(result.contains("## Available Skills"));
        assert!(result.contains("- **helper**: A helper skill"));
        assert!(result.contains("[LOAD_SKILL: skill-name]"));
    }

    #[test]
    fn build_system_instructions_skips_unloaded_skills() {
        let skills = vec![SkillDefinition {
            name: "unloaded".into(),
            description: "Not loaded".into(),
            location: PathBuf::new(),
            source: dream_core_extension::SkillSource::Custom,
            relative_location: None,
            body: None,
        }];
        let result = build_system_instructions("Base", &skills);
        assert_eq!(result, "Base");
    }
}
