use super::*;
use pretty_assertions::assert_eq;

#[test]
fn reads_rust_ast_through_sdk() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    std::fs::write(temp.path().join("demo.rs"), "fn answer() -> u32 { 42 }\n")?;
    let output = read_with_sdk(
        temp.path().canonicalize()?,
        temp.path().join("demo.rs").canonicalize()?,
        SmartReadLayer::Ast,
    )?;

    assert!(output.contains("answer"));
    assert!(output.contains("demo.rs"));
    Ok(())
}

#[test]
fn layer_mapping_is_exhaustive() {
    assert_eq!(SmartReadLayer::Smart.code_layer(), None);
    assert_eq!(SmartReadLayer::Dfg.code_layer(), Some(CodeLayer::Dfg));
    assert_eq!(
        SmartReadLayer::TheoryGraph.code_layer(),
        Some(CodeLayer::TheoryGraph)
    );
}
