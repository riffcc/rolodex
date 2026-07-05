//! Types for the `declare_phase` tool.
//!
//! `declare_phase` is the structural handle the Agentic-mode climber reads to
//! distinguish a legitimate progress turn from a ralph loop. The model declares
//! its cognitive phase each turn; the climber guard treats a phase transition
//! (and phase-appropriate new state) as a valid climb step.
//!
//! The three phases trace a chess-master / OODA-style cycle — intake, synthesize,
//! act — without which the two-state slow/fast flattening collapses the crucial
//! synthesis step into either "loading" or "executing".

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// The three cognitive phases of the agentic loop.
///
/// Each phase has a distinct progress currency the climber guard measures:
/// - `Explore` — context gathering (reads, reasoning). Progress = net-new
///   deduped context.
/// - `Reason` — synthesis: integrating context into a plan or decision.
///   Progress = a plan change or an explicit decision artifact.
/// - `Execute` — acting on the decision. Progress = applied changes (plan steps
///   advanced, files written, commands run).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Explore,
    Reason,
    Execute,
}

/// Arguments for the `declare_phase` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DeclarePhaseArgs {
    /// The cognitive phase the agent is entering with this turn.
    pub phase: Phase,
    /// Optional one-line intent: what this phase move is for (e.g. "loading
    /// context around the auth refactor" or "executing step 3").
    #[serde(default)]
    pub intent: Option<String>,
}
