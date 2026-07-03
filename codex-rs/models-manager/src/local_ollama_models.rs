use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::process::Command;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use crate::model_info::BASE_INSTRUCTIONS;

const OLLAMA_PORT: u16 = 11434;
const OLLAMA_GEMMA4_MODEL: &str = "gemma4:12b";

pub(crate) fn append_detected_ollama_models(models: &mut Vec<ModelInfo>) {
    for model in detected_ollama_models() {
        if !models.iter().any(|existing| existing.slug == model.slug) {
            models.push(model.clone());
        }
    }
}

#[cfg(test)]
pub(crate) fn append_ollama_models(models: &mut Vec<ModelInfo>, ollama_supported: bool) {
    if !ollama_supported || models.iter().any(|model| model.slug == OLLAMA_GEMMA4_MODEL) {
        return;
    }

    models.push(gemma4_model_info());
}

pub(crate) fn detected_ollama_model_provider_id(model: &str) -> Option<&'static str> {
    detected_ollama_models()
        .iter()
        .any(|candidate| candidate.slug == model)
        .then_some("ollama")
}

fn detected_ollama_models() -> &'static Vec<ModelInfo> {
    static MODELS: OnceLock<Vec<ModelInfo>> = OnceLock::new();
    MODELS.get_or_init(|| {
        let slugs = ollama_model_slugs();
        if slugs.is_empty() {
            return Vec::new();
        }
        slugs
            .into_iter()
            .map(|slug| ollama_model_info(&slug))
            .collect()
    })
}

fn ollama_model_slugs() -> Vec<String> {
    let mut slugs = ollama_model_slugs_from_api();
    if slugs.is_empty() {
        slugs = ollama_model_slugs_from_command();
    }
    slugs.sort();
    slugs.dedup();
    slugs
}

fn ollama_model_slugs_from_api() -> Vec<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], OLLAMA_PORT));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(150)) else {
        return Vec::new();
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    if stream
        .write_all(b"GET /api/tags HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return Vec::new();
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return Vec::new();
    }
    let Some((_, body)) = response.split_once("\r\n\r\n") else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct TagsResponse {
        models: Vec<TagModel>,
    }
    #[derive(serde::Deserialize)]
    struct TagModel {
        name: String,
    }
    serde_json::from_str::<TagsResponse>(body)
        .map(|tags| {
            tags.models
                .into_iter()
                .map(|model| model.name)
                .filter(|name| !name.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn ollama_model_slugs_from_command() -> Vec<String> {
    let Ok(output) = Command::new("ollama")
        .arg("list")
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
fn gemma4_model_info() -> ModelInfo {
    ollama_model_info(OLLAMA_GEMMA4_MODEL)
}

fn ollama_model_info(slug: &str) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: ollama_display_name(slug),
        description: Some("Local model via Ollama.".to_string()),
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: ConfigShellToolType::Default,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority: 90,
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
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(128_000),
        max_context_window: Some(128_000),
        auto_compact_token_limit: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec![InputModality::Text],
        used_fallback_model_metadata: false,
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

fn ollama_display_name(slug: &str) -> String {
    if slug == OLLAMA_GEMMA4_MODEL {
        return "Gemma 4 12B".to_string();
    }
    slug.split_once(':')
        .map(|(name, tag)| format!("{} {}", title_case_model_name(name), tag.to_uppercase()))
        .unwrap_or_else(|| title_case_model_name(slug))
}

fn title_case_model_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "local_ollama_models_tests.rs"]
mod tests;
