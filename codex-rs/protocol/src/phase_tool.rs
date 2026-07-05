//! Types for the `declare_phase` tool.
//!
//! `declare_phase` is the structural handle the Auto-mode climber reads to
//! distinguish a legitimate slow-phase turn (context loading — produces no plan
//! step on purpose) from a ralph loop (wheel-spin). The model declares its
//! cognitive phase each turn; the climber guard treats a phase transition as a
//! valid climb step.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// The two cognitive phases of the chess-master loop.
///
/// `Slow` is the loading phase: context gathering, reads, reasoning. It looks
/// inert but is genuine progress. `Fast` is the explosive execution phase:
/// plan steps fall in rapid succession.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Slow,
    Fast,
}

/// Arguments for the `declare_phase` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DeclarePhaseArgs {
    /// The cognitive phase the agent is entering with this turn.
    pub phase: Phase,
    /// Optional one-line intent: what this phase move is for (e.g. "loading
    /// context around the auth refactor" or "executing the plan").
    #[serde(default)]
    pub intent: Option<String>,
}
