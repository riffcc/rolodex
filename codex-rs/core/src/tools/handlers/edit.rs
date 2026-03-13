use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::client_common::tools::ResponsesApiTool;
use crate::client_common::tools::ToolSpec;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::apply_patch::run_verified_apply_patch_action;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::spec::AdditionalProperties;
use crate::tools::spec::JsonSchema;

pub struct EditHandler;

#[derive(Debug, Deserialize)]
struct EditToolArgs {
    operations: Vec<EditOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum EditOperation {
    Create {
        path: String,
        content: String,
        overwrite: Option<bool>,
    },
    Delete {
        path: String,
        ignore_missing: Option<bool>,
    },
    Move {
        from: String,
        to: String,
        overwrite: Option<bool>,
    },
    Replace {
        path: String,
        old: String,
        new: String,
        replace_all: Option<bool>,
        expected_replacements: Option<usize>,
    },
    InsertBefore {
        path: String,
        anchor: String,
        content: String,
        insert_all: Option<bool>,
        expected_occurrences: Option<usize>,
    },
    InsertAfter {
        path: String,
        anchor: String,
        content: String,
        insert_all: Option<bool>,
        expected_occurrences: Option<usize>,
    },
}

#[derive(Clone, Debug, Default)]
struct WorkingFile {
    original_content: Option<String>,
    current_content: Option<String>,
}

#[async_trait]
impl ToolHandler for EditHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tracker,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "edit handler received unsupported payload".to_string(),
                ));
            }
        };
        let args: EditToolArgs = parse_arguments(&arguments)?;
        if args.operations.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "edit requires at least one operation".to_string(),
            ));
        }

        let patch = compile_edit_operations_to_patch(&turn.cwd, &args.operations)?;
        let command = vec!["apply_patch".to_string(), patch];
        match codex_apply_patch::maybe_parse_apply_patch_verified(&command, &turn.cwd) {
            codex_apply_patch::MaybeApplyPatchVerified::Body(action) => {
                run_verified_apply_patch_action(session, turn, tracker, call_id, tool_name, action)
                    .await
            }
            codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(error) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "edit generated an invalid patch: {error}"
                )))
            }
            codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "edit generated an unparsable patch wrapper: {error:?}"
                )))
            }
            codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => {
                Err(FunctionCallError::RespondToModel(
                    "edit generated a non-apply_patch payload".to_string(),
                ))
            }
        }
    }
}

pub(crate) fn create_edit_tool() -> ToolSpec {
    let operation_schema = JsonSchema::Object {
        properties: BTreeMap::from([
            (
                "op".to_string(),
                JsonSchema::String {
                    description: Some(
                        "Operation kind: create, delete, move, replace, insert_before, or insert_after.".to_string(),
                    ),
                },
            ),
            (
                "path".to_string(),
                JsonSchema::String {
                    description: Some("Target file path, relative to the workspace when possible.".to_string()),
                },
            ),
            (
                "from".to_string(),
                JsonSchema::String {
                    description: Some("Source path for move operations.".to_string()),
                },
            ),
            (
                "to".to_string(),
                JsonSchema::String {
                    description: Some("Destination path for move operations.".to_string()),
                },
            ),
            (
                "content".to_string(),
                JsonSchema::String {
                    description: Some("Content to create or insert.".to_string()),
                },
            ),
            (
                "old".to_string(),
                JsonSchema::String {
                    description: Some("Exact text to replace.".to_string()),
                },
            ),
            (
                "new".to_string(),
                JsonSchema::String {
                    description: Some("Replacement text.".to_string()),
                },
            ),
            (
                "anchor".to_string(),
                JsonSchema::String {
                    description: Some("Exact text anchor used for insert_before or insert_after.".to_string()),
                },
            ),
            (
                "overwrite".to_string(),
                JsonSchema::Boolean {
                    description: Some("Allow create or move to overwrite an existing destination.".to_string()),
                },
            ),
            (
                "ignore_missing".to_string(),
                JsonSchema::Boolean {
                    description: Some("Allow delete to skip a missing file.".to_string()),
                },
            ),
            (
                "replace_all".to_string(),
                JsonSchema::Boolean {
                    description: Some("Replace every occurrence instead of just the first.".to_string()),
                },
            ),
            (
                "insert_all".to_string(),
                JsonSchema::Boolean {
                    description: Some("Insert around every anchor occurrence instead of just the first.".to_string()),
                },
            ),
            (
                "expected_replacements".to_string(),
                JsonSchema::Number {
                    description: Some("Optional exact replacement count guard.".to_string()),
                },
            ),
            (
                "expected_occurrences".to_string(),
                JsonSchema::Number {
                    description: Some("Optional exact anchor match count guard.".to_string()),
                },
            ),
        ]),
        required: Some(vec!["op".to_string()]),
        additional_properties: Some(AdditionalProperties::Boolean(false)),
    };

    ToolSpec::Function(ResponsesApiTool {
        name: "edit".to_string(),
        description: "Use the `edit` tool for coordinated file edits. It accepts batched structured operations like create, delete, move, replace, insert_before, and insert_after, then applies them atomically through Codex's verified edit pipeline.".to_string(),
        parameters: JsonSchema::Object {
            properties: BTreeMap::from([(
                "operations".to_string(),
                JsonSchema::Array {
                    items: Box::new(operation_schema),
                    description: Some("Ordered edit operations to apply in a single tool call.".to_string()),
                },
            )]),
            required: Some(vec!["operations".to_string()]),
            additional_properties: Some(AdditionalProperties::Boolean(false)),
        },
        strict: false,
        output_schema: None,
    })
}

fn compile_edit_operations_to_patch(
    cwd: &Path,
    operations: &[EditOperation],
) -> Result<String, FunctionCallError> {
    let mut files = BTreeMap::<PathBuf, WorkingFile>::new();
    for operation in operations {
        apply_operation(cwd, &mut files, operation)?;
    }

    let mut patch = String::from("*** Begin Patch\n");
    let mut changed_paths = 0usize;

    for (path, file) in files {
        match (&file.original_content, &file.current_content) {
            (None, None) => {}
            (None, Some(content)) => {
                append_add_file(&mut patch, cwd, &path, content)?;
                changed_paths += 1;
            }
            (Some(content), None) => {
                append_delete_file(&mut patch, cwd, &path, content);
                changed_paths += 1;
            }
            (Some(original), Some(current)) if original != current => {
                append_update_file(&mut patch, cwd, &path, original, current)?;
                changed_paths += 1;
            }
            (Some(_), Some(_)) => {}
        }
    }

    if changed_paths == 0 {
        return Err(FunctionCallError::RespondToModel(
            "edit produced no filesystem changes".to_string(),
        ));
    }

    patch.push_str("*** End Patch\n");
    Ok(patch)
}

fn apply_operation(
    cwd: &Path,
    files: &mut BTreeMap<PathBuf, WorkingFile>,
    operation: &EditOperation,
) -> Result<(), FunctionCallError> {
    match operation {
        EditOperation::Create {
            path,
            content,
            overwrite,
        } => {
            if content.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "edit cannot create an empty file at {path}; use at least one newline-terminated line"
                )));
            }
            let path = resolve_path(cwd, path);
            let file = ensure_loaded(files, &path)?;
            if file.current_content.is_some() && !overwrite.unwrap_or(false) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "create would overwrite existing file {}",
                    path.display()
                )));
            }
            file.current_content = Some(content.clone());
            Ok(())
        }
        EditOperation::Delete {
            path,
            ignore_missing,
        } => {
            let path = resolve_path(cwd, path);
            let file = ensure_loaded(files, &path)?;
            if file.current_content.is_none() && !ignore_missing.unwrap_or(false) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "delete target does not exist: {}",
                    path.display()
                )));
            }
            file.current_content = None;
            Ok(())
        }
        EditOperation::Move {
            from,
            to,
            overwrite,
        } => {
            let from = resolve_path(cwd, from);
            let to = resolve_path(cwd, to);
            if from == to {
                return Ok(());
            }

            let source_content = ensure_loaded(files, &from)?
                .current_content
                .clone()
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(format!(
                        "move source does not exist: {}",
                        from.display()
                    ))
                })?;
            let destination = ensure_loaded(files, &to)?;
            if destination.current_content.is_some() && !overwrite.unwrap_or(false) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "move destination already exists: {}",
                    to.display()
                )));
            }
            destination.current_content = Some(source_content);
            ensure_loaded(files, &from)?.current_content = None;
            Ok(())
        }
        EditOperation::Replace {
            path,
            old,
            new,
            replace_all,
            expected_replacements,
        } => {
            let path = resolve_path(cwd, path);
            let file = ensure_loaded(files, &path)?;
            let current = file.current_content.as_mut().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "replace target does not exist: {}",
                    path.display()
                ))
            })?;
            let replacements = current.matches(old).count();
            validate_match_count(
                replacements,
                *expected_replacements,
                format!("replace in {}", path.display()),
            )?;
            if replacements == 0 {
                return Err(FunctionCallError::RespondToModel(format!(
                    "replace did not find target text in {}",
                    path.display()
                )));
            }
            if replace_all.unwrap_or(false) {
                *current = current.replace(old, new);
            } else {
                *current = current.replacen(old, new, 1);
            }
            Ok(())
        }
        EditOperation::InsertBefore {
            path,
            anchor,
            content,
            insert_all,
            expected_occurrences,
        } => {
            let path = resolve_path(cwd, path);
            let file = ensure_loaded(files, &path)?;
            let current = file.current_content.as_mut().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "insert_before target does not exist: {}",
                    path.display()
                ))
            })?;
            *current = insert_relative_to_anchor(
                current,
                anchor,
                content,
                InsertPosition::Before,
                insert_all.unwrap_or(false),
                *expected_occurrences,
                &path,
            )?;
            Ok(())
        }
        EditOperation::InsertAfter {
            path,
            anchor,
            content,
            insert_all,
            expected_occurrences,
        } => {
            let path = resolve_path(cwd, path);
            let file = ensure_loaded(files, &path)?;
            let current = file.current_content.as_mut().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "insert_after target does not exist: {}",
                    path.display()
                ))
            })?;
            *current = insert_relative_to_anchor(
                current,
                anchor,
                content,
                InsertPosition::After,
                insert_all.unwrap_or(false),
                *expected_occurrences,
                &path,
            )?;
            Ok(())
        }
    }
}

fn ensure_loaded<'a>(
    files: &'a mut BTreeMap<PathBuf, WorkingFile>,
    path: &Path,
) -> Result<&'a mut WorkingFile, FunctionCallError> {
    if !files.contains_key(path) {
        let content = if path.exists() {
            Some(fs::read_to_string(path).map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "failed to read {}: {error}",
                    path.display()
                ))
            })?)
        } else {
            None
        };
        files.insert(
            path.to_path_buf(),
            WorkingFile {
                original_content: content.clone(),
                current_content: content,
            },
        );
    }
    files.get_mut(path).ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("failed to load {}", path.display()))
    })
}

fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    crate::util::resolve_path(cwd, &PathBuf::from(raw))
}

fn append_add_file(
    patch: &mut String,
    cwd: &Path,
    path: &Path,
    content: &str,
) -> Result<(), FunctionCallError> {
    if content.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "edit cannot represent an empty add for {}",
            path.display()
        )));
    }
    patch.push_str(&format!("*** Add File: {}\n", display_path(cwd, path)));
    for line in content.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    Ok(())
}

fn append_delete_file(patch: &mut String, cwd: &Path, path: &Path, _content: &str) {
    patch.push_str(&format!("*** Delete File: {}\n", display_path(cwd, path)));
}

fn append_update_file(
    patch: &mut String,
    cwd: &Path,
    path: &Path,
    original: &str,
    current: &str,
) -> Result<(), FunctionCallError> {
    if original.is_empty() && current.is_empty() {
        return Ok(());
    }

    patch.push_str(&format!("*** Update File: {}\n", display_path(cwd, path)));
    patch.push_str("@@\n");

    for line in original.lines() {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in current.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }

    if !current.is_empty() && !current.ends_with('\n') {
        patch.push_str("*** End of File\n");
    }
    if original.is_empty() && current.is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "edit cannot emit an empty update for {}",
            path.display()
        )));
    }
    Ok(())
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

#[derive(Clone, Copy)]
enum InsertPosition {
    Before,
    After,
}

fn insert_relative_to_anchor(
    current: &str,
    anchor: &str,
    content: &str,
    position: InsertPosition,
    insert_all: bool,
    expected_occurrences: Option<usize>,
    path: &Path,
) -> Result<String, FunctionCallError> {
    let occurrences = current.matches(anchor).count();
    validate_match_count(
        occurrences,
        expected_occurrences,
        format!("insert in {}", path.display()),
    )?;
    if occurrences == 0 {
        return Err(FunctionCallError::RespondToModel(format!(
            "insert anchor not found in {}",
            path.display()
        )));
    }

    if insert_all {
        return Ok(match position {
            InsertPosition::Before => current.replace(anchor, &format!("{content}{anchor}")),
            InsertPosition::After => current.replace(anchor, &format!("{anchor}{content}")),
        });
    }

    let index = current.find(anchor).ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("insert anchor not found in {}", path.display()))
    })?;
    let mut output = String::with_capacity(current.len() + content.len());
    match position {
        InsertPosition::Before => {
            output.push_str(&current[..index]);
            output.push_str(content);
            output.push_str(&current[index..]);
        }
        InsertPosition::After => {
            let anchor_end = index + anchor.len();
            output.push_str(&current[..anchor_end]);
            output.push_str(content);
            output.push_str(&current[anchor_end..]);
        }
    }
    Ok(output)
}

fn validate_match_count(
    actual: usize,
    expected: Option<usize>,
    context: String,
) -> Result<(), FunctionCallError> {
    if let Some(expected) = expected
        && actual != expected
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "{context} expected {expected} matches but found {actual}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn compile_edit_operations_supports_multi_step_flow() {
        let dir = tempdir().expect("tmpdir");
        let cwd = dir.path();
        fs::write(cwd.join("alpha.txt"), "hello\nworld\n").expect("seed file");

        let operations = vec![
            EditOperation::Replace {
                path: "alpha.txt".to_string(),
                old: "world".to_string(),
                new: "team".to_string(),
                replace_all: None,
                expected_replacements: Some(1),
            },
            EditOperation::InsertAfter {
                path: "alpha.txt".to_string(),
                anchor: "hello\n".to_string(),
                content: "there\n".to_string(),
                insert_all: None,
                expected_occurrences: Some(1),
            },
            EditOperation::Create {
                path: "beta.txt".to_string(),
                content: "fresh\nfile\n".to_string(),
                overwrite: None,
            },
        ];

        let patch = compile_edit_operations_to_patch(cwd, &operations).expect("patch");
        let argv = vec!["apply_patch".to_string(), patch];
        let verified = codex_apply_patch::maybe_parse_apply_patch_verified(&argv, cwd);
        match verified {
            codex_apply_patch::MaybeApplyPatchVerified::Body(action) => {
                assert_eq!(action.changes().len(), 2);
            }
            other => panic!("expected verified patch, got {other:?}"),
        }
    }

    #[test]
    fn insert_before_respects_expected_occurrences() {
        let result = insert_relative_to_anchor(
            "abc abc",
            "abc",
            "X",
            InsertPosition::Before,
            false,
            Some(1),
            Path::new("demo.txt"),
        );

        let err = result.expect_err("should reject wrong occurrence count");
        assert_eq!(
            err.to_string(),
            "insert in demo.txt expected 1 matches but found 2"
        );
    }

    #[test]
    fn move_compiles_to_delete_and_add() {
        let dir = tempdir().expect("tmpdir");
        let cwd = dir.path();
        fs::write(cwd.join("move-me.txt"), "payload\n").expect("seed file");

        let patch = compile_edit_operations_to_patch(
            cwd,
            &[EditOperation::Move {
                from: "move-me.txt".to_string(),
                to: "moved.txt".to_string(),
                overwrite: None,
            }],
        )
        .expect("patch");

        assert!(patch.contains("*** Delete File: move-me.txt"));
        assert!(patch.contains("*** Add File: moved.txt"));
    }
}
