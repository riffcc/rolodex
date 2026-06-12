use std::path::Path;
use std::path::PathBuf;

use codex_tools::ToolName;
use codex_tools::ToolSpec;
use llm_code_sdk::tools::smart::SmartWriteTool;
use llm_code_sdk::tools::smart::StructuralEdit;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::smart_write_spec::SmartWriteToolOptions;
use crate::tools::handlers::smart_write_spec::create_smart_write_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const SMART_WRITE_PREVIEW_LIMIT: usize = 2_000;

pub struct SmartWriteHandler {
    options: SmartWriteToolOptions,
}

impl SmartWriteHandler {
    pub(crate) fn new(options: SmartWriteToolOptions) -> Self {
        Self { options }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SmartWriteOperation {
    ReplaceFunction,
    ReplaceSymbol,
    InsertAfter,
    Delete,
    ReplaceLines,
}

#[derive(Deserialize)]
struct SmartWriteArgs {
    path: String,
    operation: SmartWriteOperation,
    target: String,
    content: String,
    after: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    environment_id: Option<String>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for SmartWriteHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("smart_write")
    }

    fn spec(&self) -> ToolSpec {
        create_smart_write_tool(self.options)
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation { turn, payload, .. } = invocation;
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "smart_write handler received unsupported payload".to_string(),
            ));
        };
        let args: SmartWriteArgs = parse_arguments(&arguments)?;
        let Some(environment) =
            resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
        else {
            return Err(FunctionCallError::RespondToModel(
                "smart_write is unavailable in this session".to_string(),
            ));
        };
        if environment.environment.is_remote() {
            return Err(FunctionCallError::RespondToModel(
                "smart_write currently supports local environments only".to_string(),
            ));
        }

        let project_root = canonicalize(environment.cwd.as_path(), "project root")?;
        let requested_path = environment.cwd.join(&args.path);
        let path = canonicalize(requested_path.as_path(), "source file")?;
        validate_file_inside_root(&project_root, &path, "smart_write")?;

        let output = tokio::task::spawn_blocking(move || write_with_sdk(project_root, path, args))
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("smart_write task failed: {err}"))
            })??;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output,
            /*success*/ Some(true),
        )))
    }
}

impl CoreToolRuntime for SmartWriteHandler {}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve {label} `{}`: {err}",
            path.display()
        ))
    })
}

fn validate_file_inside_root(
    project_root: &Path,
    path: &Path,
    tool_name: &str,
) -> Result<(), FunctionCallError> {
    if !path.starts_with(project_root) {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} path `{}` is outside project root `{}`",
            path.display(),
            project_root.display()
        )));
    }
    if !path.is_file() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} path `{}` is not a file",
            path.display()
        )));
    }
    Ok(())
}

fn write_with_sdk(
    project_root: PathBuf,
    path: PathBuf,
    args: SmartWriteArgs,
) -> Result<String, FunctionCallError> {
    let relative = path.strip_prefix(&project_root).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve project-relative path: {err}"))
    })?;
    let relative = relative.to_string_lossy();
    let edit = structural_edit(
        args.operation,
        &args.target,
        &args.content,
        args.after.as_deref(),
    )?;
    let tool = SmartWriteTool::new(project_root).with_dry_run(args.dry_run);
    let new_content = tool
        .apply_edit(&relative, &edit)
        .map_err(FunctionCallError::RespondToModel)?;
    Ok(format_smart_write_output(&new_content, args.dry_run))
}

fn structural_edit(
    operation: SmartWriteOperation,
    target: &str,
    content: &str,
    after: Option<&str>,
) -> Result<StructuralEdit, FunctionCallError> {
    match operation {
        SmartWriteOperation::ReplaceFunction => {
            Ok(StructuralEdit::replace_function(target, content))
        }
        SmartWriteOperation::ReplaceSymbol => Ok(StructuralEdit::replace_symbol(target, content)),
        SmartWriteOperation::InsertAfter => Ok(StructuralEdit::insert_after(
            after.unwrap_or(target),
            content,
        )),
        SmartWriteOperation::Delete => Ok(StructuralEdit::delete_symbol(target)),
        SmartWriteOperation::ReplaceLines => {
            let (start, end) = parse_line_range(target)?;
            Ok(StructuralEdit::replace_lines(start, end, content))
        }
    }
}

fn parse_line_range(target: &str) -> Result<(usize, usize), FunctionCallError> {
    let Some((start, end)) = target.split_once(':') else {
        return Err(FunctionCallError::RespondToModel(
            "replace_lines target must be a 1-based inclusive range like `10:20`".to_string(),
        ));
    };
    let start = start.parse::<usize>().map_err(|_| {
        FunctionCallError::RespondToModel(format!(
            "replace_lines start line `{start}` is not a positive integer"
        ))
    })?;
    let end = end.parse::<usize>().map_err(|_| {
        FunctionCallError::RespondToModel(format!(
            "replace_lines end line `{end}` is not a positive integer"
        ))
    })?;
    if start == 0 || end == 0 || start > end {
        return Err(FunctionCallError::RespondToModel(format!(
            "replace_lines target `{target}` must use positive lines with start <= end"
        )));
    }
    Ok((start, end))
}

fn format_smart_write_output(new_content: &str, dry_run: bool) -> String {
    let action = if dry_run { "previewed" } else { "applied" };
    let preview = truncate_preview(new_content, SMART_WRITE_PREVIEW_LIMIT);
    format!("Edit {action} successfully.\n\n{preview}")
}

fn truncate_preview(content: &str, limit: usize) -> String {
    if content.len() <= limit {
        return content.to_string();
    }

    let mut end = limit;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}...\n(truncated, {} total chars)",
        &content[..end],
        content.len()
    )
}

#[cfg(test)]
#[path = "smart_write_tests.rs"]
mod tests;
