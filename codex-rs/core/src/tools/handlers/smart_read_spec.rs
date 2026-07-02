use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct SmartReadToolOptions {
    pub include_environment_id: bool,
}

pub fn create_smart_read_tool(options: SmartReadToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to a source file inside the current project.".to_string(),
            )),
        ),
        (
            "layer".to_string(),
            JsonSchema::string_enum(
                vec![
                    json!("smart"),
                    json!("raw"),
                    json!("ast"),
                    json!("call_graph"),
                    json!("cfg"),
                    json!("dfg"),
                    json!("pdg"),
                    json!("theory_graph"),
                ],
                Some(
                    "Analysis layer. Defaults to `smart`, which lets llm-code-sdk choose."
                        .to_string(),
                ),
            ),
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
        name: "smart_read".to_string(),
        description: "Read source code through llm-code-sdk's structural analysis layers. Use `smart` for automatic selection, or request AST, call graph, control-flow, data-flow, dependence, or Lean theory views."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string()]),
            /*additional_properties*/ Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "smart_read_spec_tests.rs"]
mod tests;
