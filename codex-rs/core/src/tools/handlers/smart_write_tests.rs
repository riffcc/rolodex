use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parses_replace_lines_target() -> anyhow::Result<()> {
    assert_eq!(parse_line_range("2:4")?, (2, 4));
    assert!(parse_line_range("4:2").is_err());
    assert!(parse_line_range("nope").is_err());
    Ok(())
}

#[test]
fn dry_run_does_not_write_file() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("demo.rs");
    std::fs::write(
        &path,
        "fn answer() -> u32 {\n    41\n}\n\nfn keep() -> u32 {\n    7\n}\n",
    )?;

    let output = write_with_sdk(
        temp.path().canonicalize()?,
        path.canonicalize()?,
        SmartWriteArgs {
            path: "demo.rs".to_string(),
            operation: SmartWriteOperation::ReplaceFunction,
            target: "answer".to_string(),
            content: "fn answer() -> u32 {\n    42\n}".to_string(),
            after: None,
            dry_run: true,
            environment_id: None,
        },
    )?;

    assert!(output.contains("Edit previewed successfully"));
    assert!(output.contains("42"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("demo.rs"))?,
        "fn answer() -> u32 {\n    41\n}\n\nfn keep() -> u32 {\n    7\n}\n"
    );
    Ok(())
}

#[test]
fn replace_lines_writes_file() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("demo.rs");
    std::fs::write(&path, "fn answer() -> u32 {\n    41\n}\n")?;

    let output = write_with_sdk(
        temp.path().canonicalize()?,
        path.canonicalize()?,
        SmartWriteArgs {
            path: "demo.rs".to_string(),
            operation: SmartWriteOperation::ReplaceLines,
            target: "2:2".to_string(),
            content: "    42".to_string(),
            after: None,
            dry_run: false,
            environment_id: None,
        },
    )?;

    assert!(output.contains("Edit applied successfully"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("demo.rs"))?,
        "fn answer() -> u32 {\n    42\n}\n"
    );
    Ok(())
}
