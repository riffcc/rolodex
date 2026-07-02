use super::*;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

#[test]
fn smart_search_spec_exposes_mr_search_as_smart_search() {
    let ToolSpec::Function(tool) = create_smart_search_tool(SmartSearchToolOptions {
        include_environment_id: false,
    }) else {
        panic!("smart_search should be a function tool");
    };

    assert_eq!(tool.name, "smart_search");
    assert!(tool.description.contains("MRSearchTool"));
    assert!(tool.description.contains("SmartSearch"));
}
