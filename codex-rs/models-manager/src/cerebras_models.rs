use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;

use crate::model_info::BASE_INSTRUCTIONS;

const CEREBRAS_API_KEY_ENV_VAR: &str = "CEREBRAS_API_KEY";
const CEREBRAS_PROVIDER_ID: &str = "cerebras";
pub(crate) const CEREBRAS_GEMMA4_MODEL: &str = "gemma-4-31b";

pub(crate) fn append_detected_cerebras_models(models: &mut Vec<ModelInfo>) {
    append_cerebras_models(models, cerebras_api_key_available());
}

#[cfg(test)]
pub(crate) fn append_cerebras_models_for_tests(
    models: &mut Vec<ModelInfo>,
    api_key_available: bool,
) {
    append_cerebras_models(models, api_key_available);
}

pub(crate) fn detected_cerebras_model_provider_id(model: &str) -> Option<&'static str> {
    (model == CEREBRAS_GEMMA4_MODEL).then_some(CEREBRAS_PROVIDER_ID)
}

fn append_cerebras_models(models: &mut Vec<ModelInfo>, api_key_available: bool) {
    if !api_key_available
        || models
            .iter()
            .any(|model| model.slug == CEREBRAS_GEMMA4_MODEL)
    {
        return;
    }

    models.push(gemma4_model_info());
}

fn cerebras_api_key_available() -> bool {
    std::env::var(CEREBRAS_API_KEY_ENV_VAR).is_ok_and(|value| !value.trim().is_empty())
}

fn gemma4_model_info() -> ModelInfo {
    ModelInfo {
        slug: CEREBRAS_GEMMA4_MODEL.to_string(),
        display_name: "Gemma 4 31B".to_string(),
        description: Some("Gemma 4 served by Cerebras.".to_string()),
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: ConfigShellToolType::Default,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority: 89,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: BASE_INSTRUCTIONS.to_string(),
        model_messages: None,
        supports_reasoning_summaries: false,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(131_072),
        max_context_window: Some(131_072),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec![InputModality::Text, InputModality::Image],
        used_fallback_model_metadata: false,
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

#[cfg(test)]
#[path = "cerebras_models_tests.rs"]
mod tests;
