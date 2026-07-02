use super::*;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

#[test]
fn smart_write_spec_exposes_structural_operations() {
    let ToolSpec::Function(tool) = create_smart_write_tool(SmartWriteToolOptions {
        include_environment_id: false,
    }) else {
        panic!("smart_write should be a function tool");
    };

    assert_eq!(tool.name, "smart_write");
    assert!(tool.description.contains("SmartWrite"));
    assert!(tool.description.contains("surgical"));
}
