//! The plan vocabulary the planner emits and the loop executes ([S-173],
//! [ADR-41]).
//!
//! The LLM-driven planner decomposes a request into an explicit **step plan**: a
//! sequence of [`PlanStep`]s, each routed to one specialized-subagent
//! [`StepRole`]. Its reply is a [`PlannerDecision`] — either a (re)plan or a
//! final answer — parsed from the model's JSON output (see [`planner`]).
//!
//! [S-173]: ../../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [`planner`]: super::planner

use logos_core::config::ChatRole;
use serde::{Deserialize, Serialize};

/// The role a plan step is routed to — one of the four fixed specialized
/// subagents of the [ADR-41] roster (the planner itself is not a step target).
///
/// The wire names match the `[chat.models]` per-role override keys ([FR-CF-06])
/// so a step's role maps directly to its configured model via
/// [`as_chat_role`](StepRole::as_chat_role) (S-174 resolves the subagent model
/// that way).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRole {
    /// Code-graph navigation (search/context/node/callers/callees/impact/…).
    GraphNavigator,
    /// Architecture-governance / quality (scan/check_rules/hotspots/…).
    GovernanceAnalyst,
    /// Sandboxed filesystem source reading (read/grep/glob).
    SourceReader,
    /// Tool-less final-answer synthesis from the turn's scratchpad.
    Synthesizer,
}

impl StepRole {
    /// The matching [`ChatRole`] for per-role model resolution
    /// ([`ChatConfig::model_for_role`](logos_core::config::ChatConfig::model_for_role),
    /// [FR-CF-06]). The planner role has no step form, so this maps only the four
    /// subagent roles.
    pub fn as_chat_role(self) -> ChatRole {
        match self {
            StepRole::GraphNavigator => ChatRole::GraphNavigator,
            StepRole::GovernanceAnalyst => ChatRole::GovernanceAnalyst,
            StepRole::SourceReader => ChatRole::SourceReader,
            StepRole::Synthesizer => ChatRole::Synthesizer,
        }
    }
}

/// One step of a plan: which subagent handles it and the instruction it is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// The specialized subagent this step is routed to.
    pub role: StepRole,
    /// The natural-language instruction the subagent receives.
    pub instruction: String,
}

impl PlanStep {
    /// Construct a step routed to `role` with `instruction`.
    pub fn new(role: StepRole, instruction: impl Into<String>) -> Self {
        Self {
            role,
            instruction: instruction.into(),
        }
    }
}

/// What the planner decided this round: produce/replace the step plan, or finish.
///
/// Parsed from the planner `Agent`'s JSON reply under the `"action"` tag:
///
/// ```json
/// { "action": "plan",  "steps": [ { "role": "graph_navigator", "instruction": "…" } ] }
/// { "action": "final", "answer": "…the grounded answer…" }
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlannerDecision {
    /// A (re)plan: the steps to run before the planner is consulted again.
    Plan {
        /// The ordered steps to execute this round.
        steps: Vec<PlanStep>,
    },
    /// The turn is complete; carries the final grounded answer.
    Final {
        /// The synthesized answer to return to the user.
        answer: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_role_round_trips_through_its_wire_names() {
        for (role, wire) in [
            (StepRole::GraphNavigator, "\"graph_navigator\""),
            (StepRole::GovernanceAnalyst, "\"governance_analyst\""),
            (StepRole::SourceReader, "\"source_reader\""),
            (StepRole::Synthesizer, "\"synthesizer\""),
        ] {
            assert_eq!(serde_json::to_string(&role).unwrap(), wire);
            assert_eq!(serde_json::from_str::<StepRole>(wire).unwrap(), role);
        }
    }

    #[test]
    fn planner_decision_parses_a_plan() {
        let decision: PlannerDecision = serde_json::from_str(
            r#"{ "action": "plan", "steps": [
                { "role": "graph_navigator", "instruction": "find Engine callers" },
                { "role": "synthesizer", "instruction": "summarize" }
            ] }"#,
        )
        .unwrap();
        match decision {
            PlannerDecision::Plan { steps } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].role, StepRole::GraphNavigator);
                assert_eq!(steps[1].role, StepRole::Synthesizer);
            }
            other => panic!("expected a plan, got {other:?}"),
        }
    }

    #[test]
    fn planner_decision_parses_a_final_answer() {
        let decision: PlannerDecision =
            serde_json::from_str(r#"{ "action": "final", "answer": "grounded" }"#).unwrap();
        assert_eq!(
            decision,
            PlannerDecision::Final {
                answer: "grounded".to_string()
            }
        );
    }
}
