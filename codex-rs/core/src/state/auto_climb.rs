//! The Auto-mode climber guard brain.
//!
//! Decides whether one loop iteration of an Auto turn was a valid climb
//! step (continue the autonomous loop) or a ralph signature (halt and yield to
//! the user). The invariant: a step is valid iff the agent declared its phase
//! AND produced net-new state — a plan change or a tool call with a novel
//! (name, args) fingerprint.
//!
//! Re-calling the same tool with identical arguments is the ralph signature.
//! Fingerprinting the *call* (not the output content) catches it at one central
//! dispatch site without having to fingerprint every tool's output.

use std::collections::HashSet;
use std::hash::Hash;
use std::hash::Hasher;

use codex_protocol::phase_tool::Phase;

#[derive(Debug, Default)]
pub(crate) struct AutoClimbState {
    /// Phase declared in the current iteration. Reset each iteration.
    phase: Option<Phase>,
    /// Whether `update_plan` ran this iteration.
    plan_changed: bool,
    /// Whether a tool call with a novel (name, args) fingerprint ran this
    /// iteration.
    novel_call: bool,
    /// Every (tool_name, args) content hash seen this session. Accumulates
    /// across iterations so a repeated call is detected as non-novel.
    seen_call_fingerprints: HashSet<u64>,
}

impl AutoClimbState {
    /// Reset the per-iteration signals at the top of each loop iteration.
    pub(crate) fn reset_for_iteration(&mut self) {
        self.phase = None;
        self.plan_changed = false;
        self.novel_call = false;
    }

    pub(crate) fn record_phase(&mut self, phase: Phase) {
        self.phase = Some(phase);
    }

    pub(crate) fn record_plan_change(&mut self) {
        self.plan_changed = true;
    }

    /// Record a tool call. Returns `true` if its (name, args) fingerprint is
    /// novel this session, and flags the iteration as having a novel call.
    pub(crate) fn record_call(&mut self, tool_name: &str, args: &str) -> bool {
        let fingerprint = fingerprint_call(tool_name, args);
        let novel = self.seen_call_fingerprints.insert(fingerprint);
        if novel {
            self.novel_call = true;
        }
        novel
    }

    /// A valid climb step requires a declared phase AND net-new state.
    pub(crate) fn is_valid_climb_step(&self) -> bool {
        self.phase.is_some() && (self.plan_changed || self.novel_call)
    }
}

/// Stable content hash of a (tool_name, args) pair. The `0u8` separator keeps
/// `("ab", "c")` distinct from `("a", "bc")`.
fn fingerprint_call(tool_name: &str, args: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    0u8.hash(&mut hasher);
    args.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_plus_novel_call_is_valid() {
        let mut s = AutoClimbState::default();
        s.reset_for_iteration();
        s.record_phase(Phase::Execute);
        assert!(s.record_call("smart_read", r#"{"path":"a.rs"}"#));
        assert!(s.is_valid_climb_step());
    }

    #[test]
    fn phase_plus_plan_change_is_valid() {
        let mut s = AutoClimbState::default();
        s.reset_for_iteration();
        s.record_phase(Phase::Reason);
        s.record_plan_change();
        assert!(s.is_valid_climb_step());
    }

    #[test]
    fn no_phase_declared_is_ralph() {
        let mut s = AutoClimbState::default();
        s.reset_for_iteration();
        s.record_call("smart_read", r#"{"path":"a.rs"}"#);
        assert!(
            !s.is_valid_climb_step(),
            "an undeclared phase is a ralph signature even with a novel call"
        );
    }

    #[test]
    fn repeated_call_is_ralph() {
        let mut s = AutoClimbState::default();
        s.reset_for_iteration();
        s.record_phase(Phase::Explore);
        assert!(s.record_call("smart_read", r#"{"path":"a.rs"}"#));
        assert!(
            s.is_valid_climb_step(),
            "first read in explore phase is a valid climb step"
        );

        // Next iteration: same phase, same call — no novel state.
        s.reset_for_iteration();
        s.record_phase(Phase::Explore);
        assert!(!s.record_call("smart_read", r#"{"path":"a.rs"}"#));
        assert!(
            !s.is_valid_climb_step(),
            "re-reading the same path with no other progress is a ralph signature"
        );
    }

    #[test]
    fn novel_call_after_repeat_is_valid_again() {
        let mut s = AutoClimbState::default();
        s.reset_for_iteration();
        s.record_phase(Phase::Explore);
        s.record_call("smart_read", r#"{"path":"a.rs"}"#);

        s.reset_for_iteration();
        s.record_phase(Phase::Explore);
        // Different args → novel → valid, even though the tool name repeats.
        assert!(s.record_call("smart_read", r#"{"path":"b.rs"}"#));
        assert!(s.is_valid_climb_step());
    }

    #[test]
    fn reset_clears_per_iteration_signals() {
        let mut s = AutoClimbState::default();
        s.record_phase(Phase::Execute);
        s.record_plan_change();
        s.reset_for_iteration();
        assert!(!s.is_valid_climb_step(), "reset must clear phase + plan change");
    }
}
