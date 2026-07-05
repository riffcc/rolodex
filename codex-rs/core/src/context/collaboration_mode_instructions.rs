use super::ContextualUserFragment;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use codex_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CollaborationModeInstructions {
    instructions: String,
}

/// The default Auto-mode instruction, used when the config does not provide
/// one. It communicates the climb contract the guard enforces, so the model
/// supplies the structural signals (declared phase + net-new state) the guard
/// reads — without relying on the user to configure it.
const DEFAULT_AUTO_INSTRUCTIONS: &str = "\
You are in Auto mode: an autonomous loop. Each turn you MUST:\n\
1. Call `declare_phase` with your current phase — `explore` (loading context),\n\
   `reason` (synthesizing a plan/decision), or `execute` (acting) — and a short intent.\n\
2. Produce net-new state: either advance the plan via `update_plan`, or make a\n\
   tool call you have not already made in this session.\n\
The loop halts and returns to the user if a turn declares no phase, or produces\n\
no plan change and only repeated tool calls. Do not re-call the same tool with\n\
the same arguments as a previous turn unless you have a concrete reason.";

impl CollaborationModeInstructions {
    pub(crate) fn from_collaboration_mode(collaboration_mode: &CollaborationMode) -> Option<Self> {
        if let Some(instructions) = collaboration_mode
            .settings
            .developer_instructions
            .as_ref()
            .filter(|instructions| !instructions.is_empty())
        {
            return Some(Self {
                instructions: instructions.clone(),
            });
        }
        // Auto mode ships a default instruction so the climber guard has the
        // declared-phase signal it needs, even without user config.
        if collaboration_mode.mode == ModeKind::Auto {
            return Some(Self {
                instructions: DEFAULT_AUTO_INSTRUCTIONS.to_string(),
            });
        }
        None
    }
}

impl ContextualUserFragment for CollaborationModeInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (COLLABORATION_MODE_OPEN_TAG, COLLABORATION_MODE_CLOSE_TAG)
    }

    fn body(&self) -> String {
        self.instructions.clone()
    }
}
