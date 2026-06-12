use std::path::Path;
use std::path::PathBuf;

use codex_tools::ToolName;
use codex_tools::ToolSpec;
use llm_code_sdk::tools::SearchTool;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::search_spec::SearchToolOptions;
use crate::tools::handlers::search_spec::create_search_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const DEFAULT_SEARCH_LIMIT: usize = 10;
const MAX_SEARCH_LIMIT: usize = 50;

pub struct SearchHandler {
    options: SearchToolOptions,
}

impl SearchHandler {
    pub(crate) fn new(options: SearchToolOptions) -> Self {
        Self { options }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    limit: Option<usize>,
    #[serde(default)]
    environment_id: Option<String>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for SearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("search")
    }

    fn spec(&self) -> ToolSpec {
        create_search_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation { turn, payload, .. } = invocation;
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "search handler received unsupported payload".to_string(),
            ));
        };
        let args: SearchArgs = parse_arguments(&arguments)?;
        if args.query.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "search query is required".to_string(),
            ));
        }
        let Some(environment) =
            resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
        else {
            return Err(FunctionCallError::RespondToModel(
                "search is unavailable in this session".to_string(),
            ));
        };
        if environment.environment.is_remote() {
            return Err(FunctionCallError::RespondToModel(
                "search currently supports local environments only".to_string(),
            ));
        }

        let project_root = canonicalize(environment.cwd.as_path(), "project root")?;
        let query = args.query;
        let limit = args
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .min(MAX_SEARCH_LIMIT);
        let output =
            tokio::task::spawn_blocking(move || search_with_sdk(project_root, query, limit))
                .await
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!("search task failed: {err}"))
                })?
                .map_err(FunctionCallError::RespondToModel)?;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output,
            /*success*/ Some(true),
        )))
    }
}

impl CoreToolRuntime for SearchHandler {}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve {label} `{}`: {err}",
            path.display()
        ))
    })
}

fn search_with_sdk(project_root: PathBuf, query: String, limit: usize) -> Result<String, String> {
    let tool = SearchTool::new(&project_root);
    let count = tool.index_directory(&project_root);
    if count == 0 {
        return Err("No files to search".to_string());
    }

    let results = tool.search(&query, limit);
    if results.is_empty() {
        return Ok("No matches found".to_string());
    }

    let output = results
        .iter()
        .map(|result| {
            let terms = if result.matched_terms.is_empty() {
                String::new()
            } else {
                format!("; terms: {}", result.matched_terms.join(", "))
            };
            format!("{} (score: {:.2}{terms})", result.path, result.score)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(output)
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
