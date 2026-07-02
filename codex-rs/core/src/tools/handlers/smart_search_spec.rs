use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct SmartSearchToolOptions {
    pub include_environment_id: bool,
}

pub fn create_smart_search_tool(options: SmartSearchToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some(
                "Git history query: issue number, phrase, commit message, or omit for recent activity."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(
                "Maximum number of commits to return. Defaults to 20 and is capped at 100."
                    .to_string(),
            )),
        ),
        (
            "days".to_string(),
            JsonSchema::integer(Some(
                "For recent activity mode, how many days back to inspect. Defaults to 7 and is capped at 365."
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
        name: "smart_search".to_string(),
        description: "Expose llm-code-sdk's MRSearchTool as SmartSearch for git-history-aware project search. Omit query for recent activity."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            None,
            /*additional_properties*/ Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
#[path = "smart_search_spec_tests.rs"]
mod tests;
