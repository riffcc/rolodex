use super::*;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

#[test]
fn smart_read_spec_exposes_sdk_layers() {
    let ToolSpec::Function(tool) = create_smart_read_tool(SmartReadToolOptions {
        include_environment_id: false,
    }) else {
        panic!("smart_read should be a function tool");
    };

    assert_eq!(tool.name, "smart_read");
    assert!(tool.description.contains("llm-code-sdk"));
    assert!(tool.description.contains("Lean theory"));
}
