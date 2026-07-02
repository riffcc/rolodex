use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct SmartWriteToolOptions {
    pub include_environment_id: bool,
}

pub fn create_smart_write_tool(options: SmartWriteToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to an existing source file inside the current project.".to_string(),
            )),
        ),
        (
            "operation".to_string(),
            JsonSchema::string_enum(
                vec![
                    json!("replace_function"),
                    json!("replace_symbol"),
                    json!("insert_after"),
                    json!("delete"),
                    json!("replace_lines"),
                ],
                Some("Structural edit operation to apply.".to_string()),
            ),
        ),
        (
            "target".to_string(),
            JsonSchema::string(Some(
                "Target symbol name, or a 1-based inclusive line range like `10:20` for replace_lines."
                    .to_string(),
            )),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some(
                "Replacement or inserted content. Use an empty string for delete.".to_string(),
            )),
        ),
        (
            "after".to_string(),
            JsonSchema::string(Some(
                "For insert_after, the symbol to insert after. Defaults to target.".to_string(),
            )),
        ),
        (
            "dry_run".to_string(),
            JsonSchema::boolean(Some(
                "Preview the edit without writing the file. Defaults to false.".to_string(),
            )),
        ),
    ]);
    if options.include_environment_id {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>. Omit to use the primary environment."
                    .to_string(),
            )),
        );
    }

    ToolSpec::Function(ResponsesApiTool {
        name: "smart_write".to_string(),
        description: "Edit existing source files through llm-code-sdk's SmartWrite structural operations. Prefer this for surgical function, symbol, insertion, deletion, or line-range edits."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec![
                "path".to_string(),
                "operation".to_string(),
                "target".to_string(),
                "content".to_string(),
            ]),
            /*additional_properties*/ Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "smart_write_spec_tests.rs"]
mod tests;
