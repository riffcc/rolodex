use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct SearchToolOptions {
    pub include_environment_id: bool,
}

pub fn create_search_tool(options: SearchToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some(
                "Code search query. Prefix matching is supported by the llm-code-sdk index."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(
                "Maximum number of results to return. Defaults to 10 and is capped at 50."
                    .to_string(),
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
        name: "search".to_string(),
        description: "Search the current project with llm-code-sdk's SearchTool and return file paths ranked by relevance."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["query".to_string()]),
            /*additional_properties*/ Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "search_spec_tests.rs"]
mod tests;
