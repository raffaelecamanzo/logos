//! The step-execution seam the planner routes to ([S-173], [S-174]).
//!
//! S-173 owns the plan→act→observe→replan loop, the budget tree, and the events;
//! it routes each [`PlanStep`] to a [`StepExecutor`]. The **fixed subagent
//! roster** that implements this trait — Graph-Navigator, Governance-Analyst,
//! Source-Reader, Synthesizer, each a `rig` `Agent` over its agent-core tool
//! subset — is [S-174]. Keeping the executor behind a trait lets the loop, the
//! budget tree, and the events be built and tested now (with a scripted executor)
//! and the real roster plugged in next iteration without touching the loop.
//!
//! Every tool call a subagent makes must be charged through the
//! [`StepContext`] so the budget tree bounds the turn honestly ([NFR-CC-04]); on
//! a budget halt the executor returns [`StepError::Budget`] and the loop reports
//! the bound rather than the subagent fabricating a result.
//!
//! [S-173]: ../../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [S-174]: ../../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::future::Future;

use agent_core::ToolBudget;
use serde::Serialize;

use super::budget::{BudgetBound, BudgetTree};
use super::plan::PlanStep;

/// What a subagent observed running its step — recorded to the per-turn
/// scratchpad and rendered back to the planner on the next round.
///
/// S-173 carries a short text summary (the seam the planner and, later, the
/// Synthesizer read); S-175 persists the scratchpad to `chat.db`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepObservation {
    /// A short summary of the step's grounded result.
    pub summary: String,
}

impl StepObservation {
    /// Construct an observation from its summary text.
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

/// Why a step did not produce an observation.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// A budget-tree bound was reached charging a tool call — the honest halt
    /// ([NFR-CC-04]). The loop turns this into an honest halt outcome naming the
    /// bound, not an error.
    #[error(transparent)]
    Budget(#[from] BudgetBound),

    /// The subagent failed for a non-budget reason (a provider/tool error). Honest
    /// **turn-fatal** failure — never a fabricated result. Reserved for structural
    /// faults (tools failed to load, an inconsistent conversation) and **all**
    /// Synthesizer faults: when the answer composer itself is down there is nothing
    /// to degrade to, so the turn aborts honestly ([CR-060], [NFR-CC-04]).
    #[error("subagent step failed: {0}")]
    Failed(String),

    /// A **recoverable** runtime fault the loop routes around ([CR-060] Layer 3,
    /// [FR-UI-28]): a provider fault surviving the agent-core retry seam ([S-240]),
    /// a model that returned neither a tool call nor text, or a bounded-summarization
    /// provider fault. Unlike [`StepError::Failed`] this is **not** turn-fatal — the
    /// loop degrades the step to a `[unavailable — …]` scratchpad observation and
    /// continues to its remaining planned steps, letting the tool-less Synthesizer
    /// answer best-effort over what was gathered. An empty or all-`[unavailable]`
    /// scratchpad still yields an honest bare halt; nothing is fabricated
    /// ([NFR-CC-04]).
    ///
    /// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    /// [FR-UI-28]: ../../../docs/specs/requirements/FR-UI-28.md
    /// [S-240]: ../../../docs/planning/journal.md#s-240-provider-call-retry-with-backoff-in-agent-core
    #[error("subagent step unavailable: {0}")]
    Unavailable(String),
}

/// Receives the final answer's text as the tool-less Synthesizer streams it,
/// chunk by chunk ([FR-UI-19] token-level streaming).
///
/// The orchestrator wires a sink (only for a Synthesizer step) that forwards each
/// chunk as an [`OrchestratorEvent::AnswerDelta`](super::OrchestratorEvent::AnswerDelta)
/// to the SSE stream; every other step runs with no answer sink, so its
/// intermediate prose never streams as answer text. `Send + Sync` so a
/// `&dyn AnswerSink` can ride the `Send` step future.
pub trait AnswerSink: Send + Sync {
    /// Emit one chunk of answer text — appended to what has streamed so far.
    fn answer_delta(&self, delta: &str);
}

/// The per-step budget handle a [`StepExecutor`] charges its tool calls through.
///
/// Owns a fresh per-subagent [`ToolBudget`] (capped at the tree's
/// `max_subagent_tool_calls`) and borrows the shared [`BudgetTree`]. A subagent
/// calls [`charge_tool_call`](StepContext::charge_tool_call) **before** invoking
/// each tool; on a spent budget it must not run the tool ([NFR-CC-04]).
///
/// A Synthesizer step additionally carries an [`AnswerSink`] so it can stream the
/// final answer's tokens as it generates them ([FR-UI-19]).
pub struct StepContext<'t> {
    tree: &'t BudgetTree,
    subagent: ToolBudget,
    answer: Option<&'t dyn AnswerSink>,
}

impl<'t> StepContext<'t> {
    /// Open a step context over `tree`, minting this step's per-subagent budget.
    /// No answer sink — [`emit_answer_delta`](StepContext::emit_answer_delta) is a
    /// no-op (the path every non-Synthesizer step takes).
    pub(crate) fn new(tree: &'t BudgetTree) -> Self {
        Self {
            subagent: tree.new_subagent_budget(),
            tree,
            answer: None,
        }
    }

    /// Open a step context over `tree` that streams the answer through `answer` —
    /// the Synthesizer step's context, so its prose is the user-facing answer
    /// streamed token by token ([FR-UI-19]).
    pub(crate) fn with_answer_sink(tree: &'t BudgetTree, answer: &'t dyn AnswerSink) -> Self {
        Self {
            subagent: tree.new_subagent_budget(),
            tree,
            answer: Some(answer),
        }
    }

    /// Stream one chunk of the final answer, if this step has an [`AnswerSink`]
    /// wired (a Synthesizer step). A no-op for every other step ([FR-UI-19]).
    pub fn emit_answer_delta(&self, delta: &str) {
        if let Some(answer) = self.answer {
            answer.answer_delta(delta);
        }
    }

    /// The shared budget tree (for a subagent loop that charges the global
    /// ceiling itself — e.g. when dispatching through agent-core's
    /// [`BoundedDispatcher`](agent_core::BoundedDispatcher), S-174).
    pub fn budget_tree(&self) -> &BudgetTree {
        self.tree
    }

    /// This step's per-subagent [`ToolBudget`] — hand it to a
    /// [`BoundedDispatcher`](agent_core::BoundedDispatcher) to enforce the
    /// per-subagent cap on the tool set (S-174).
    pub fn subagent_budget(&self) -> &ToolBudget {
        &self.subagent
    }

    /// Charge one tool call against both the global ceiling and this step's
    /// per-subagent cap, returning the first bound reached ([NFR-CC-04]). The
    /// primary seam a subagent charges through before invoking a tool.
    pub fn charge_tool_call(&self) -> Result<(), BudgetBound> {
        self.tree.charge_tool_call(&self.subagent)
    }
}

/// Handles one plan step on behalf of its [`StepRole`](super::plan::StepRole).
///
/// Implemented by the fixed subagent roster ([S-174]); the orchestrator is
/// generic over it so the roster plugs in without changing the loop. Tests
/// supply a scripted executor that charges the [`StepContext`] to exercise the
/// budget tree.
///
/// The returned future is `Send` so the orchestrator's run future stays `Send`
/// for a multi-threaded runtime.
pub trait StepExecutor: Send + Sync {
    /// Run `step`, charging every tool call through `ctx`. Returns the step's
    /// observation, or an honest failure / budget halt ([NFR-CC-04]).
    fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> impl Future<Output = Result<StepObservation, StepError>> + Send;
}
