use super::*;

#[test]
fn search_indexes_project_and_returns_ranked_paths() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    std::fs::write(
        temp.path().join("alpha.rs"),
        "fn sdk_search_marker() -> u32 { 1 }\n",
    )?;
    std::fs::write(temp.path().join("beta.rs"), "fn other() -> u32 { 2 }\n")?;

    let output = search_with_sdk(
        temp.path().canonicalize()?,
        "sdk_search_marker".to_string(),
        10,
    )
    .map_err(anyhow::Error::msg)?;

    assert!(output.contains("alpha.rs"));
    Ok(())
}
