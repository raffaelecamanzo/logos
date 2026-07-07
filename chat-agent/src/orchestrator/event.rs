//! Orchestrator events and the sink they are emitted to ([S-173], [FR-UI-19]).
//!
//! The plan→act→observe→replan loop emits a structured [`OrchestratorEvent`] at
//! every transition — the plan produced each round, each step starting and being
//! observed, an honest budget halt, and the final answer. They are serializable
//! so the SSE seam (S-170) can stream them to the Chat view verbatim; S-173 only
//! defines and emits them through an [`EventSink`].
//!
//! [`CapturingSink`] is the in-memory sink the tests assert against; S-170 will
//! add a channel-backed sink that forwards to the SSE response.
//!
//! [S-173]: ../../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md

use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::budget::BudgetBound;
use super::plan::{PlanStep, StepRole};

/// A transition in the orchestrated turn, suitable for SSE streaming ([FR-UI-19]).
///
/// Serialized tagged on `"event"` so the stream carries a stable discriminant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum OrchestratorEvent {
    /// The planner produced a step plan. `round` is 0 for the initial plan and
    /// increments for each replan ([ADR-41] max-replans).
    Plan {
        /// The planning round (0 = initial plan, ≥1 = a replan).
        round: u32,
        /// The steps the planner laid out this round.
        steps: Vec<PlanStep>,
    },
    /// A step is about to be dispatched to its subagent.
    StepStarted {
        /// The step's index within its plan.
        index: usize,
        /// The subagent the step is routed to.
        role: StepRole,
        /// The instruction the subagent received.
        instruction: String,
    },
    /// A step completed and its observation was recorded to the scratchpad.
    StepObserved {
        /// The step's index within its plan.
        index: usize,
        /// The subagent that handled the step.
        role: StepRole,
        /// A short summary of what the step observed.
        summary: String,
    },
    /// The turn halted at a budget-tree bound — the honest halt ([NFR-CC-04]),
    /// naming which bound was reached.
    Halted {
        /// The count of planning rounds whose steps completed when the halt
        /// occurred. For a tool-call halt this is the round whose step tripped the
        /// bound; for a **replan** halt it is **one beyond** the last emitted
        /// [`Plan`](OrchestratorEvent::Plan)`.round` — the refused over-budget plan
        /// is never emitted, so a consumer correlating rounds sees no `Plan` event
        /// at this index.
        round: u32,
        /// Which budget-tree bound was reached.
        bound: BudgetBound,
    },
    /// A chunk of the final answer, streamed as the tool-less Synthesizer
    /// generates it token by token ([FR-UI-19] token-level streaming). Consumers
    /// append each `delta` to the answer in flight; the terminal
    /// [`FinalAnswer`](OrchestratorEvent::FinalAnswer) carries the authoritative
    /// full text the turn persists, so a consumer reconciles to it at the end
    /// (the deltas are the live preview, the final answer is the record of truth).
    AnswerDelta {
        /// The next chunk of answer text, to append to what has streamed so far.
        delta: String,
    },
    /// The planner produced the final grounded answer; the turn is complete.
    FinalAnswer {
        /// The answer returned to the user.
        answer: String,
    },
}

/// Where the orchestrator emits its [`OrchestratorEvent`]s.
///
/// `emit` takes `&self` so a sink can forward over a shared channel (S-170's SSE
/// sender) without `&mut` plumbing through the async loop. Must be `Send + Sync`
/// so the run future stays `Send` for a multi-threaded runtime.
pub trait EventSink: Send + Sync {
    /// Record one orchestrator event.
    fn emit(&self, event: OrchestratorEvent);
}

/// The unit sink discards every event — for callers that only want the outcome.
impl EventSink for () {
    fn emit(&self, _event: OrchestratorEvent) {}
}

/// An in-memory [`EventSink`] that accumulates every emitted event.
///
/// Cloneable and `Send + Sync` (events sit behind an `Arc<Mutex<…>>`), so a clone
/// can be handed to the orchestrator while the test retains one to inspect the
/// recorded sequence afterward.
#[derive(Debug, Clone, Default)]
pub struct CapturingSink {
    events: Arc<Mutex<Vec<OrchestratorEvent>>>,
}

impl CapturingSink {
    /// A fresh, empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot of the events recorded so far, in emission order.
    pub fn events(&self) -> Vec<OrchestratorEvent> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl EventSink for CapturingSink {
    fn emit(&self, event: OrchestratorEvent) {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event);
    }
}

/// Fan one orchestrator event out to several [`EventSink`]s at once — the
/// composition the SSE seam (S-170) uses to **stream a turn to the client AND
/// persist its scratchpad in the same pass**, with no change to
/// [`Orchestrator::run`](super::Orchestrator::run).
///
/// The orchestrator's loop emits through a single `&impl EventSink`; wrapping the
/// streaming SSE sender and the [`ScratchpadSink`](crate::memory::ScratchpadSink)
/// in one `FanOut` lets both observe every event in emission order. It borrows the
/// member sinks (`&dyn EventSink`) rather than owning them, so a turn can keep its
/// own handle on each (e.g. to read the scratchpad sink's
/// [`first_error`](crate::memory::ScratchpadSink::first_error) afterward).
///
/// `emit` is infallible per the [`EventSink`] contract: a member sink that needs
/// to report a failure (a dropped SSE receiver, a failed memory write) captures it
/// out-of-band, so one sink's trouble never starves the others of the event.
pub struct FanOut<'a> {
    sinks: Vec<&'a dyn EventSink>,
}

impl<'a> FanOut<'a> {
    /// Compose a fan-out over the given sinks; every emitted event reaches each,
    /// in order.
    pub fn new(sinks: Vec<&'a dyn EventSink>) -> Self {
        Self { sinks }
    }
}

impl EventSink for FanOut<'_> {
    fn emit(&self, event: OrchestratorEvent) {
        // Clone for every sink but the last, which takes the event by move — so the
        // common two-sink case (SSE + scratchpad) makes exactly one clone, and a
        // single-sink fan-out makes none.
        if let Some((last, rest)) = self.sinks.split_last() {
            for sink in rest {
                sink.emit(event.clone());
            }
            last.emit(event);
        }
    }
}
