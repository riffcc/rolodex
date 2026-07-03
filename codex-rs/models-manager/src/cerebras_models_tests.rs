use super::*;
use codex_protocol::openai_models::ModelPreset;
use pretty_assertions::assert_eq;

#[test]
fn adds_gemma4_when_cerebras_api_key_is_available() {
    let mut models = Vec::new();

    append_cerebras_models_for_tests(&mut models, /*api_key_available*/ true);

    assert_eq!(models.len(), 1);
    let preset = ModelPreset::from(models.remove(0));
    assert_eq!(preset.model, "gemma-4-31b");
    assert_eq!(preset.display_name, "Gemma 4 31B");
    assert!(preset.show_in_picker);
    assert!(preset.supported_in_api);
    assert_eq!(preset.supported_reasoning_efforts, Vec::new());
}

#[test]
fn leaves_models_unchanged_without_cerebras_api_key() {
    let mut models = Vec::new();

    append_cerebras_models_for_tests(&mut models, /*api_key_available*/ false);

    assert_eq!(models, Vec::new());
}

#[test]
fn detects_cerebras_api_token_alias() {
    assert!(cerebras_api_key_available_with_env(|name| {
        (name == CEREBRAS_API_TOKEN_ENV_VAR).then_some("token".to_string())
    }));
}

#[test]
fn ignores_empty_cerebras_api_token_alias() {
    assert!(!cerebras_api_key_available_with_env(|name| {
        (name == CEREBRAS_API_TOKEN_ENV_VAR).then_some("   ".to_string())
    }));
}

#[test]
fn does_not_duplicate_existing_gemma4_entry() {
    let mut models = vec![gemma4_model_info()];

    append_cerebras_models_for_tests(&mut models, /*api_key_available*/ true);

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "gemma-4-31b");
}

#[test]
fn maps_gemma4_slug_to_cerebras_provider_independent_of_env() {
    assert_eq!(
        detected_cerebras_model_provider_id("gemma-4-31b"),
        Some("cerebras")
    );
}
