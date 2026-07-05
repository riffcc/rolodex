use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

/// The `declare_phase` tool spec.
///
/// This is the structural handle the Auto-mode climber reads: each turn the
/// agent declares which cognitive phase it is in (`slow` = loading context,
/// `fast` = executing the plan). A phase transition counts as a valid climb
/// step, so a slow-phase turn that completes no plan step is not penalized as a
/// ralph loop.
pub fn create_declare_phase_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "phase".to_string(),
            JsonSchema::string_enum(
                vec![json!("slow"), json!("fast")],
                Some(
                    "The cognitive phase this turn is in: slow (loading context) or fast (executing)."
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
        description: "Declare the cognitive phase of the current turn (slow = context loading, fast = execution). Call this at the start of each turn so the Auto-mode climber can distinguish genuine slow-phase loading from a stuck loop."
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
            serde_json::from_str(r#"{"phase":"slow","intent":"loading auth context"}"#).unwrap();
        assert_eq!(args.phase, Phase::Slow);
        assert_eq!(args.intent.as_deref(), Some("loading auth context"));

        let fast: DeclarePhaseArgs = serde_json::from_str(r#"{"phase":"fast"}"#).unwrap();
        assert_eq!(fast.phase, Phase::Fast);
        assert!(fast.intent.is_none());
    }
}
