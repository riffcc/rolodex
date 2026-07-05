use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

/// The `declare_phase` tool spec.
///
/// This is the structural handle the Agentic-mode climber reads: each turn the
/// agent declares which cognitive phase it is in (`explore`, `reason`, or
/// `execute`). A phase transition with phase-appropriate new state counts as a
/// valid climb step, so an explore-phase turn that completes no plan step is
/// not penalized as a ralph loop.
pub fn create_declare_phase_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "phase".to_string(),
            JsonSchema::string_enum(
                vec![json!("explore"), json!("reason"), json!("execute")],
                Some(
                    "The cognitive phase this turn is in: explore (loading context), reason (synthesizing a plan/decision), or execute (acting)."
                        .to_string(),
                ),
            ),
        ),
        (
            "intent".to_string(),
            JsonSchema::string(Some(
                "Optional one-line intent for this phase move (e.g. 'loading auth refactor context')."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "declare_phase".to_string(),
        description: "Declare the cognitive phase of the current turn (explore = context loading, reason = synthesis, execute = action). Call this at the start of each turn so the Agentic-mode climber can distinguish genuine progress from a stuck loop."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["phase".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::phase_tool::DeclarePhaseArgs;
    use codex_protocol::phase_tool::Phase;

    #[test]
    fn declare_phase_spec_is_well_formed() {
        let spec = create_declare_phase_tool();
        let codex_tools::ToolSpec::Function(f) = spec else {
            panic!("expected function spec");
        };
        assert_eq!(f.name, "declare_phase");
        // The serialized schema must advertise `phase` as required.
        let schema_json = serde_json::to_string(&f.parameters).unwrap();
        assert!(schema_json.contains("\"phase\""), "schema: {schema_json}");
    }

    #[test]
    fn declare_phase_args_parse_round_trip() {
        let args: DeclarePhaseArgs =
            serde_json::from_str(r#"{"phase":"explore","intent":"loading auth context"}"#).unwrap();
        assert_eq!(args.phase, Phase::Explore);
        assert_eq!(args.intent.as_deref(), Some("loading auth context"));

        let reason: DeclarePhaseArgs = serde_json::from_str(r#"{"phase":"reason"}"#).unwrap();
        assert_eq!(reason.phase, Phase::Reason);
        assert!(reason.intent.is_none());

        let execute: DeclarePhaseArgs = serde_json::from_str(r#"{"phase":"execute"}"#).unwrap();
        assert_eq!(execute.phase, Phase::Execute);
    }
}
