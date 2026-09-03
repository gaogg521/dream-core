//! The canonical model platform preset list.
//!
//! Ported from dream-ui's `renderer/utils/model/modelPlatforms.ts`, which
//! until now was hand-copied a second time into dream-en's
//! `console/components/modelPlatformPresets.ts` (with `logo`/`i18nKey`
//! dropped, since the admin console renders neither). Both frontends should
//! fetch this list at `GET /api/model-platforms` instead of maintaining their
//! own copy — that is the whole point: a platform added here reaches both
//! frontends without either one shipping a code change.
//!
//! Order and content are kept identical to dream-ui's list at the time of
//! this port (30 presets) so neither frontend sees a reshuffled picker.

use dream_core_api_types::{ModelPlatformPreset, ModelPlatformsResponse};

fn preset(name: &str, value: &str, platform: &str, base_url: Option<&str>, logo_path: &str, i18n_key: Option<&str>) -> ModelPlatformPreset {
    ModelPlatformPreset {
        name: name.to_owned(),
        value: value.to_owned(),
        platform: platform.to_owned(),
        base_url: base_url.map(str::to_owned),
        logo_path: Some(logo_path.to_owned()),
        i18n_key: i18n_key.map(str::to_owned),
    }
}

/// The full preset list, in display order.
pub fn model_platform_presets() -> Vec<ModelPlatformPreset> {
    vec![
        ModelPlatformPreset {
            name: "Custom".to_owned(),
            value: "custom".to_owned(),
            platform: "custom".to_owned(),
            base_url: None,
            logo_path: None,
            i18n_key: Some("settings.platformCustom".to_owned()),
        },
        preset("Moonshot (China)", "Moonshot", "custom", Some("https://api.moonshot.cn/v1"), "ai-china/kimi.svg", None),
        preset(
            "Moonshot (Global)",
            "Moonshot-Global",
            "custom",
            Some("https://api.moonshot.ai/v1"),
            "ai-china/kimi.svg",
            None,
        ),
        ModelPlatformPreset {
            name: "New API".to_owned(),
            value: "new-api".to_owned(),
            platform: "new-api".to_owned(),
            base_url: None,
            logo_path: Some("ai-cloud/newapi.svg".to_owned()),
            i18n_key: Some("settings.platformNewApi".to_owned()),
        },
        preset("Ollama", "Ollama", "ollama", Some("http://localhost:11434"), "ai-major/ollama.svg", None),
        preset(
            "Gemini",
            "gemini",
            "gemini",
            Some("https://generativelanguage.googleapis.com"),
            "ai-major/gemini.svg",
            None,
        ),
        preset("Gemini (Vertex AI)", "gemini-vertex-ai", "gemini-vertex-ai", None, "ai-major/gemini.svg", None),
        preset("OpenAI", "OpenAI", "custom", Some("https://api.openai.com/v1"), "ai-major/openai.svg", None),
        preset("Anthropic", "Anthropic", "anthropic", Some("https://api.anthropic.com"), "ai-major/anthropic.svg", None),
        ModelPlatformPreset {
            name: "AWS Bedrock".to_owned(),
            value: "AWS-Bedrock".to_owned(),
            platform: "bedrock".to_owned(),
            base_url: None,
            logo_path: Some("ai-cloud/bedrock.svg".to_owned()),
            i18n_key: Some("settings.platformBedrock".to_owned()),
        },
        preset("DeepSeek", "DeepSeek", "custom", Some("https://api.deepseek.com/v1"), "ai-major/deepseek.svg", None),
        preset("MiniMax", "MiniMax", "custom", Some("https://api.minimaxi.com/v1"), "ai-china/minimax.png", None),
        preset("Novita", "Novita", "custom", Some("https://api.novita.ai/openai/v1"), "ai-cloud/novita.svg", None),
        preset("OpenRouter", "OpenRouter", "custom", Some("https://openrouter.ai/api/v1"), "ai-cloud/openrouter.svg", None),
        preset(
            "Dashscope",
            "Dashscope",
            "custom",
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "ai-china/qwen.svg",
            None,
        ),
        // Base URL intentionally unset — users must supply their own coding-plan
        // endpoint, so the add-model form should not pre-fill a default.
        preset("Dashscope Coding Plan", "Dashscope-Coding", "custom", Some(""), "ai-china/qwen.svg", None),
        preset("SiliconFlow-CN", "SiliconFlow-CN", "custom", Some("https://api.siliconflow.cn/v1"), "ai-cloud/siliconflow.png", None),
        preset("SiliconFlow", "SiliconFlow", "custom", Some("https://api.siliconflow.com/v1"), "ai-cloud/siliconflow.png", None),
        preset("Zhipu", "Zhipu", "custom", Some("https://open.bigmodel.cn/api/paas/v4"), "ai-china/zhipu.svg", None),
        preset("xAI", "xAI", "custom", Some("https://api.x.ai/v1"), "ai-major/xai.svg", None),
        preset("Ark", "Ark", "custom", Some("https://ark.cn-beijing.volces.com/api/v3"), "ai-china/volcengine.svg", None),
        preset("Qianfan", "Qianfan", "custom", Some("https://qianfan.baidubce.com/v2"), "ai-china/baidu.svg", None),
        preset(
            "Hunyuan",
            "Hunyuan",
            "custom",
            Some("https://api.hunyuan.cloud.tencent.com/v1"),
            "ai-china/tencent.svg",
            None,
        ),
        preset("Lingyi", "Lingyi", "custom", Some("https://api.lingyiwanwu.com/v1"), "ai-china/lingyiwanwu.svg", None),
        preset("Poe", "Poe", "custom", Some("https://api.poe.com/v1"), "ai-cloud/poe.svg", None),
        preset("PPIO", "PPIO", "custom", Some("https://api.ppinfra.com/v3/openai"), "ai-cloud/ppio.svg", None),
        preset(
            "ModelScope",
            "ModelScope",
            "custom",
            Some("https://api-inference.modelscope.cn/v1"),
            "ai-cloud/modelscope.svg",
            None,
        ),
        preset("InfiniAI", "InfiniAI", "custom", Some("https://cloud.infini-ai.com/maas/v1"), "ai-cloud/infiniai.svg", None),
        preset("Ctyun", "Ctyun", "custom", Some("https://wishub-x1.ctyun.cn/v1"), "ai-cloud/ctyun.svg", None),
        preset("StepFun", "StepFun", "custom", Some("https://api.stepfun.com/v1"), "ai-china/stepfun.svg", None),
    ]
}

pub fn model_platforms_response() -> ModelPlatformsResponse {
    ModelPlatformsResponse {
        platforms: model_platform_presets(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_a_non_empty_name_value_and_platform() {
        for preset in model_platform_presets() {
            assert!(!preset.name.is_empty());
            assert!(!preset.value.is_empty());
            assert!(!preset.platform.is_empty());
        }
    }

    #[test]
    fn preset_values_are_unique() {
        let presets = model_platform_presets();
        let mut values: Vec<&str> = presets.iter().map(|p| p.value.as_str()).collect();
        let before = values.len();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), before, "duplicate preset value");
    }

    /// Locks the count so a future edit that silently drops an entry (a
    /// copy-paste mistake in a list this long) fails loudly instead of
    /// shipping a frontend with fewer platforms than before.
    #[test]
    fn matches_the_count_ported_from_dream_ui() {
        assert_eq!(model_platform_presets().len(), 30);
    }
}
