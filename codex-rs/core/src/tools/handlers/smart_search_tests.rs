use super::*;
use pretty_assertions::assert_eq;
use std::process::Command;

#[test]
fn smart_search_input_caps_limit_and_days() {
    let input = smart_search_input(SmartSearchArgs {
        query: Some("edge case".to_string()),
        limit: Some(500),
        days: Some(500),
        environment_id: None,
    });

    assert_eq!(input.get("query"), Some(&serde_json::json!("edge case")));
    assert_eq!(input.get("limit"), Some(&serde_json::json!(100)));
    assert_eq!(input.get("days"), Some(&serde_json::json!(365)));
}

#[test]
fn smart_search_wraps_mr_search_tool() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    run_git(temp.path(), &["init"])?;
    run_git(temp.path(), &["config", "user.email", "test@example.com"])?;
    run_git(temp.path(), &["config", "user.name", "Test User"])?;
    std::fs::write(temp.path().join("README.md"), "# Test\n")?;
    run_git(temp.path(), &["add", "."])?;
    run_git(temp.path(), &["commit", "-m", "Initial commit"])?;
    std::fs::write(temp.path().join("fix.rs"), "fn fix() {}\n")?;
    run_git(temp.path(), &["add", "."])?;
    run_git(temp.path(), &["commit", "-m", "Fixed #12345 edge case"])?;

    let output = smart_search_with_sdk(
        temp.path().canonicalize()?,
        SmartSearchArgs {
            query: Some("#12345".to_string()),
            limit: Some(5),
            days: Some(7),
            environment_id: None,
        },
    )?;

    assert!(!output.is_error);
    assert!(output.text.contains("12345"));
    assert!(output.text.contains("fix.rs"));
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
