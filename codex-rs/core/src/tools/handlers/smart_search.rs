use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use codex_tools::ToolName;
use codex_tools::ToolSpec;
use llm_code_sdk::tools::Tool;
use llm_code_sdk::tools::smart::MRSearchTool;
use serde::Deserialize;
use serde_json::Value;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::smart_search_spec::SmartSearchToolOptions;
use crate::tools::handlers::smart_search_spec::create_smart_search_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const DEFAULT_SMART_SEARCH_LIMIT: usize = 20;
const DEFAULT_SMART_SEARCH_DAYS: usize = 7;
const MAX_SMART_SEARCH_LIMIT: usize = 100;
const MAX_SMART_SEARCH_DAYS: usize = 365;

pub struct SmartSearchHandler {
    options: SmartSearchToolOptions,
}

impl SmartSearchHandler {
    pub(crate) fn new(options: SmartSearchToolOptions) -> Self {
        Self { options }
    }
}

#[derive(Deserialize)]
struct SmartSearchArgs {
    query: Option<String>,
    limit: Option<usize>,
    days: Option<usize>,
    #[serde(default)]
    environment_id: Option<String>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for SmartSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("smart_search")
    }

    fn spec(&self) -> ToolSpec {
        create_smart_search_tool(self.options)
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
                "smart_search handler received unsupported payload".to_string(),
            ));
        };
        let args: SmartSearchArgs = parse_arguments(&arguments)?;
        let Some(environment) =
            resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
        else {
            return Err(FunctionCallError::RespondToModel(
                "smart_search is unavailable in this session".to_string(),
            ));
        };
        if environment.environment.is_remote() {
            return Err(FunctionCallError::RespondToModel(
                "smart_search currently supports local environments only".to_string(),
            ));
        }

        let project_root = canonicalize(environment.cwd.as_path(), "project root")?;
        let output = tokio::task::spawn_blocking(move || smart_search_with_sdk(project_root, args))
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("smart_search task failed: {err}"))
            })??;
        let success = !output.is_error;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output.text,
            /*success*/ Some(success),
        )))
    }
}

impl CoreToolRuntime for SmartSearchHandler {}

struct SmartSearchOutput {
    text: String,
    is_error: bool,
}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve {label} `{}`: {err}",
            path.display()
        ))
    })
}

fn smart_search_with_sdk(
    project_root: PathBuf,
    args: SmartSearchArgs,
) -> Result<SmartSearchOutput, FunctionCallError> {
    let input = smart_search_input(args);
    let tool = MRSearchTool::new(project_root);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to create smart_search runtime: {err}"
            ))
        })?;
    let result = runtime.block_on(async move { tool.call(input).await });
    Ok(SmartSearchOutput {
        text: result.to_content_string(),
        is_error: result.is_error(),
    })
}

fn smart_search_input(args: SmartSearchArgs) -> HashMap<String, Value> {
    let mut input = HashMap::new();
    if let Some(query) = args.query
        && !query.trim().is_empty()
    {
        input.insert("query".to_string(), Value::String(query));
    }
    let limit = args
        .limit
        .unwrap_or(DEFAULT_SMART_SEARCH_LIMIT)
        .min(MAX_SMART_SEARCH_LIMIT);
    input.insert("limit".to_string(), serde_json::json!(limit));
    let days = args
        .days
        .unwrap_or(DEFAULT_SMART_SEARCH_DAYS)
        .min(MAX_SMART_SEARCH_DAYS);
    input.insert("days".to_string(), serde_json::json!(days));
    input
}

#[cfg(test)]
#[path = "smart_search_tests.rs"]
mod tests;
