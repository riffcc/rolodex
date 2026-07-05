use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::phase_spec::create_declare_phase_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::phase_tool::DeclarePhaseArgs;
use codex_protocol::protocol::EventMsg;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde_json::Value as JsonValue;

pub struct PhaseHandler;

pub struct PhaseToolOutput {
    phase: String,
}

const PHASE_DECLARED_MESSAGE: &str = "Phase declared";

impl ToolOutput for PhaseToolOutput {
    fn log_preview(&self) -> String {
        format!("{PHASE_DECLARED_MESSAGE}: {}", self.phase)
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        let mut output = FunctionCallOutputPayload::from_text(PHASE_DECLARED_MESSAGE.to_string());
        output.success = Some(true);

        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output,
        }
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        JsonValue::Object(serde_json::Map::new())
    }
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for PhaseHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("declare_phase")
    }

    fn spec(&self) -> ToolSpec {
        create_declare_phase_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id: _,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "declare_phase handler received unsupported payload".to_string(),
                ));
            }
        };

        let args = parse_declare_phase_arguments(&arguments)?;
        // Record the declared phase for the Agentic-mode climber guard. Cheap
        // and inert outside Agentic mode (the guard only reads it there).
        session.record_agentic_phase(args.phase.clone()).await;
        let phase = format!("{:?}", args.phase).to_lowercase();
        session
            .send_event(turn.as_ref(), EventMsg::PhaseDeclared(args))
            .await;

        Ok(boxed_tool_output(PhaseToolOutput { phase }))
    }
}

impl CoreToolRuntime for PhaseHandler {}

fn parse_declare_phase_arguments(arguments: &str) -> Result<DeclarePhaseArgs, FunctionCallError> {
    serde_json::from_str::<DeclarePhaseArgs>(arguments).map_err(|e| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {e}"))
    })
}
