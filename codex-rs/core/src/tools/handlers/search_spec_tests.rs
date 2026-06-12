use super::*;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

#[test]
fn search_spec_keeps_plain_search_tool_name() {
    let ToolSpec::Function(tool) = create_search_tool(SearchToolOptions {
        include_environment_id: false,
    }) else {
        panic!("search should be a function tool");
    };

    assert_eq!(tool.name, "search");
    assert!(tool.description.contains("SearchTool"));
}
