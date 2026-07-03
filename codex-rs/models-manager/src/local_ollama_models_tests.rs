use super::*;
use codex_protocol::openai_models::ModelPreset;
use pretty_assertions::assert_eq;

#[test]
fn adds_gemma4_when_ollama_is_supported() {
    let mut models = Vec::new();

    append_ollama_models(&mut models, /*ollama_supported*/ true);

    assert_eq!(models.len(), 1);
    let preset = ModelPreset::from(models.remove(0));
    assert_eq!(preset.model, "gemma4:12b");
    assert_eq!(preset.display_name, "Gemma 4 12B");
    assert!(preset.show_in_picker);
    assert!(preset.supported_in_api);
}

#[test]
fn leaves_models_unchanged_when_ollama_is_not_supported() {
    let mut models = Vec::new();

    append_ollama_models(&mut models, /*ollama_supported*/ false);

    assert_eq!(models, Vec::new());
}

#[test]
fn does_not_duplicate_existing_gemma4_entry() {
    let mut models = vec![gemma4_model_info()];

    append_ollama_models(&mut models, /*ollama_supported*/ true);

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "gemma4:12b");
}
