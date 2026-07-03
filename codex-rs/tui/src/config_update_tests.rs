use super::*;
use codex_model_provider_info::CEREBRAS_PROVIDER_ID;
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;
use color_eyre::eyre::WrapErr;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}

#[test]
fn gemma4_model_selection_persists_ollama_provider() {
    let edits = build_model_selection_edits("gemma4:12b", Option::<String>::None);

    let key_paths = edits
        .iter()
        .map(|edit| edit.key_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        key_paths,
        vec![
            "model",
            "model_reasoning_effort",
            "model_provider",
            "oss_provider"
        ]
    );
    assert_eq!(edits[2].value, serde_json::json!(OLLAMA_OSS_PROVIDER_ID));
    assert_eq!(edits[3].value, serde_json::json!(OLLAMA_OSS_PROVIDER_ID));
}

#[test]
fn cerebras_gemma4_model_selection_persists_cerebras_provider() {
    let edits = build_model_selection_edits("gemma-4-31b", Option::<String>::None);

    let key_paths = edits
        .iter()
        .map(|edit| edit.key_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        key_paths,
        vec!["model", "model_reasoning_effort", "model_provider"]
    );
    assert_eq!(edits[2].value, serde_json::json!(CEREBRAS_PROVIDER_ID));
}

#[test]
fn openai_model_selection_does_not_force_provider() {
    let edits = build_model_selection_edits("gpt-5.4", Option::<String>::None);

    let key_paths = edits
        .iter()
        .map(|edit| edit.key_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(key_paths, vec!["model", "model_reasoning_effort"]);
}
