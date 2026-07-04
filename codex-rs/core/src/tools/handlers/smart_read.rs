use std::path::PathBuf;

use codex_tools::ToolName;
use codex_tools::ToolSpec;
use llm_code_sdk::tools::smart::CodeLayer;
use llm_code_sdk::tools::smart::SmartReadTool;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::handlers::smart_path::canonicalize_path;
use crate::tools::handlers::smart_path::resolve_existing_path;
use crate::tools::handlers::smart_read_spec::SmartReadToolOptions;
use crate::tools::handlers::smart_read_spec::create_smart_read_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub struct SmartReadHandler {
    options: SmartReadToolOptions,
}

impl SmartReadHandler {
    pub(crate) fn new(options: SmartReadToolOptions) -> Self {
        Self { options }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SmartReadLayer {
    #[default]
    Smart,
    Raw,
    Ast,
    CallGraph,
    Cfg,
    Dfg,
    Pdg,
    TheoryGraph,
}

impl SmartReadLayer {
    fn code_layer(self) -> Option<CodeLayer> {
        match self {
            Self::Smart => None,
            Self::Raw => Some(CodeLayer::Raw),
            Self::Ast => Some(CodeLayer::Ast),
            Self::CallGraph => Some(CodeLayer::CallGraph),
            Self::Cfg => Some(CodeLayer::Cfg),
            Self::Dfg => Some(CodeLayer::Dfg),
            Self::Pdg => Some(CodeLayer::Pdg),
            Self::TheoryGraph => Some(CodeLayer::TheoryGraph),
        }
    }
}

#[derive(Deserialize)]
struct SmartReadArgs {
    path: String,
    #[serde(default)]
    layer: SmartReadLayer,
    #[serde(default)]
    environment_id: Option<String>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for SmartReadHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("smart_read")
    }

    fn spec(&self) -> ToolSpec {
        create_smart_read_tool(self.options)
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
                "smart_read handler received unsupported payload".to_string(),
            ));
        };
        let args: SmartReadArgs = parse_arguments(&arguments)?;
        let Some(environment) =
            resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?
        else {
            return Err(FunctionCallError::RespondToModel(
                "smart_read is unavailable in this session".to_string(),
            ));
        };
        if environment.environment.is_remote() {
            return Err(FunctionCallError::RespondToModel(
                "smart_read currently supports local environments only".to_string(),
            ));
        }

        let project_root = canonicalize_path(environment.cwd.as_path(), "project root")?;
        let path = resolve_existing_path(&project_root, &args.path, "source file")?;
        if !path.starts_with(&project_root) {
            return Err(FunctionCallError::RespondToModel(format!(
                "smart_read path `{}` is outside project root `{}`",
                path.display(),
                project_root.display()
            )));
        }
        if !path.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "smart_read path `{}` is not a file",
                path.display()
            )));
        }

        let layer = args.layer;
        let output = tokio::task::spawn_blocking(move || read_with_sdk(project_root, path, layer))
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("smart_read task failed: {err}"))
            })??;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output,
            /*success*/ Some(true),
        )))
    }
}

impl CoreToolRuntime for SmartReadHandler {}

fn read_with_sdk(
    project_root: PathBuf,
    path: PathBuf,
    layer: SmartReadLayer,
) -> Result<String, FunctionCallError> {
    let relative = path.strip_prefix(&project_root).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to resolve project-relative path: {err}"))
    })?;
    let relative = relative.to_string_lossy();
    let tool = SmartReadTool::new(project_root);
    let view = match layer.code_layer() {
        Some(layer) => tool.read_at_layer(&relative, layer),
        None => tool.read_smart(&relative),
    }
    .map_err(FunctionCallError::RespondToModel)?;
    Ok(view.to_context())
}

#[cfg(test)]
#[path = "smart_read_tests.rs"]
mod tests;
