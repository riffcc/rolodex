use super::*;
use pretty_assertions::assert_eq;

#[test]
fn resolves_path_with_redundant_cwd_basename_prefix() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("codex-rs");
    let file = cwd.join("sandboxing").join("mod.rs");
    std::fs::create_dir_all(file.parent().expect("test file should have parent"))?;
    std::fs::write(&file, "pub mod linux;\n")?;

    let resolved = resolve_existing_path(&cwd, "codex-rs/sandboxing/mod.rs", "source file")?;

    assert_eq!(resolved, file.canonicalize()?);
    Ok(())
}

#[test]
fn resolves_absolute_paths_without_rewriting() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("codex-rs");
    let file = cwd.join("sandboxing").join("mod.rs");
    std::fs::create_dir_all(file.parent().expect("test file should have parent"))?;
    std::fs::write(&file, "pub mod linux;\n")?;

    let resolved = resolve_existing_path(
        &cwd,
        file.to_str().expect("test path should be valid utf-8"),
        "source file",
    )?;

    assert_eq!(resolved, file.canonicalize()?);
    Ok(())
}

#[test]
fn reports_primary_path_when_resolution_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("codex-rs");
    std::fs::create_dir_all(&cwd)?;

    let error = resolve_existing_path(&cwd, "missing.rs", "source file").unwrap_err();
    let FunctionCallError::RespondToModel(message) = error else {
        panic!("expected model-facing error");
    };

    assert!(message.contains("unable to resolve source file"));
    assert!(message.contains("codex-rs/missing.rs"));
    Ok(())
}
