//! The orchestrator core — the LLM planner + plan→act→observe→replan loop +
//! budget tree ([S-173], [ADR-41], [chat-agent]).
//!
//! This is the reasoning core that turns a compound question into a bounded,
//! observable, multi-step run:
//!
//! 1. **Plan** — the [`Planner`] (a `rig` `Agent`) decomposes the request into a
//!    step plan, or returns the final answer.
//! 2. **Act** — each [`PlanStep`] is routed to a [`StepExecutor`] (the fixed
//!    subagent roster, [S-174]), bounded by the [budget tree](budget::BudgetTree).
//! 3. **Observe** — the step's [`StepObservation`] is recorded to the per-turn
//!    scratchpad.
//! 4. **Replan** — the loop consults the planner again with the accumulated
//!    observations, up to the `max_replans` bound.
//!
//! Every transition emits an [`OrchestratorEvent`] for the SSE stream (S-170).
//! Hitting any budget bound halts the turn **honestly** ([`TurnOutcome::Halted`]
//! naming the bound) — never looping unbounded, never fabricating a tool result
//! or an answer ([NFR-CC-04]).
//!
//! S-173 owns the loop, the planner, the budget tree, the events, and the
//! [`StepExecutor`] seam. The real subagent roster ([S-174]) and the persisted
//! scratchpad/working memory ([S-175]) plug into this core next iteration.
//!
//! [S-173]: ../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [S-174]: ../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [chat-agent]: ../../docs/specs/architecture/components/chat-agent.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

pub mod budget;
pub mod event;
pub mod plan;
pub mod planner;
pub mod roster;
pub mod step;

pub use budget::{BudgetBound, BudgetTree};
pub use event::{CapturingSink, EventSink, FanOut, OrchestratorEvent};
pub use plan::{PlanStep, PlannerDecision, StepRole};
pub use planner::{Planner, DEFAULT_PLANNER_PREAMBLE};
pub use roster::{
    RoleModels, SubagentRoster, SynthesizerGrounding, GOVERNANCE_ANALYST_PREAMBLE,
    GRAPH_NAVIGATOR_PREAMBLE, SOURCE_READER_PREAMBLE, SYNTHESIZER_PREAMBLE,
};
pub use step::{AnswerSink, StepContext, StepError, StepExecutor, StepObservation};

use agent_core::rig::completion::CompletionModel;

/// Prefix of a degraded-step scratchpad observation ([CR-060] Layer 3): the
/// `[unavailable — the {role} step could not complete: …]` note the loop records
/// when it routes around a recoverable [`StepError::Unavailable`] roster fault.
///
/// Marking these notes lets the best-effort synthesis terminal
/// ([`Orchestrator::finalize_on_hard_halt`]) treat an all-`[unavailable]` scratchpad
/// like an empty one — an honest bare halt rather than a fabricated answer composed
/// over material that was never actually gathered ([FR-UI-28], [NFR-CC-04]).
///
/// [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
/// [FR-UI-28]: ../../docs/specs/requirements/FR-UI-28.md
const UNAVAILABLE_MARKER: &str = "[unavailable —";

/// Whether `scratchpad` holds at least one *usable* observation — one that is not a
/// degraded `[unavailable — …]` note. An empty scratchpad, or one holding **only**
/// degraded notes, has no grounded material to answer from, so a best-effort
/// synthesis terminal must fall back to an honest bare halt instead of composing an
/// answer over nothing ([CR-060] Layer 3, [NFR-CC-04]).
fn has_usable_observation(scratchpad: &[(PlanStep, StepObservation)]) -> bool {
    scratchpad
        .iter()
        .any(|(_, obs)| !obs.summary.trim_start().starts_with(UNAVAILABLE_MARKER))
}

/// Forwards the Synthesizer's streamed answer chunks to the orchestrator's event
/// sink as [`OrchestratorEvent::AnswerDelta`] ([FR-UI-19]). It borrows the turn's
/// `&impl EventSink`, so wiring it onto a Synthesizer step's [`StepContext`]
/// allocates nothing.
struct AnswerForwarder<'a, S: EventSink>(&'a S);

impl<S: EventSink> AnswerSink for AnswerForwarder<'_, S> {
    fn answer_delta(&self, delta: &str) {
        self.0.emit(OrchestratorEvent::AnswerDelta {
            delta: delta.to_string(),
        });
    }
}

/// How an orchestrated turn ended.
///
/// Both arms are *honest* terminal states ([NFR-CC-04]): the planner produced a
/// grounded answer, or a budget-tree bound stopped the turn — reported, never
/// papered over with a fabricated answer.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// The planner produced the final grounded answer.
    Answered(String),
    /// A budget-tree bound halted the turn; carries which bound was reached.
    Halted(BudgetBound),
}

/// A non-recoverable failure running an orchestrated turn.
///
/// Distinct from [`TurnOutcome::Halted`]: a budget halt is an expected, honest
/// outcome; these are genuine faults (a provider error, an unparseable plan, a
/// subagent failure). All are surfaced honestly — never a fabricated answer
/// ([NFR-CC-04]).
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// The planner's provider failed (e.g. the mock was exhausted, or a real
    /// provider errored). Carries the **classified** failure (transport vs
    /// HTTP-status vs auth) with its full source chain ([S-199], [FR-UI-24]) —
    /// never a flattened single line.
    #[error("the planner provider failed: {0}")]
    Planner(agent_core::ProviderFailure),

    /// The planner's reply could not be parsed into a [`PlannerDecision`].
    #[error("could not parse the planner's decision: {0}")]
    PlanParse(String),

    /// A subagent step failed for a non-budget reason. Carries the failing
    /// [`StepRole`] so the surface can name the **stage** (subagent vs synthesis,
    /// [S-199]); the `message` already carries the role-tagged, source-chained
    /// cause from the roster.
    #[error("{message}")]
    Step {
        /// The role whose step failed — distinguishes a tool-bearing subagent
        /// failure from a synthesis failure for stage naming.
        role: StepRole,
        /// The honest, source-chained failure message.
        message: String,
    },
}

impl OrchestratorError {
    /// The turn stage this failure occurred in — `"planner"`, `"subagent"`, or
    /// `"synthesis"` — so the Chat surface can name where the turn broke ([S-199],
    /// [FR-UI-24]). The honest error frame the SPA renders (and [S-200] consumes)
    /// leads with this.
    pub fn stage(&self) -> &'static str {
        match self {
            OrchestratorError::Planner(_) | OrchestratorError::PlanParse(_) => "planner",
            OrchestratorError::Step { role, .. } => match role {
                StepRole::Synthesizer => "synthesis",
                _ => "subagent",
            },
        }
    }
}

/// The orchestrator: a [`Planner`] over model `M`, a [`StepExecutor`] `E`, and
/// the [`BudgetTree`] bounding the turn ([ADR-41]).
pub struct Orchestrator<M, E> {
    planner: Planner<M>,
    executor: E,
    budget: BudgetTree,
}

impl<M, E> Orchestrator<M, E>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    E: StepExecutor,
{
    /// Build an orchestrator from a planner model, a step executor, and a budget
    /// tree.
    pub fn new(planner_model: M, executor: E, budget: BudgetTree) -> Self {
        Self {
            planner: Planner::new(planner_model),
            executor,
            budget,
        }
    }

    /// Build an orchestrator from a pre-configured [`Planner`] (e.g. with a custom
    /// preamble or a per-role model).
    pub fn with_planner(planner: Planner<M>, executor: E, budget: BudgetTree) -> Self {
        Self {
            planner,
            executor,
            budget,
        }
    }

    /// The budget tree bounding turns run by this orchestrator.
    pub fn budget(&self) -> &BudgetTree {
        &self.budget
    }

    /// Run one turn of `request` to a final answer or an honest budget halt,
    /// emitting every transition to `sink`.
    ///
    /// The plan→act→observe→replan loop:
    /// - the **initial** plan is planning round 0; each subsequent plan is a
    ///   replan. The planner may replan up to `max_replans` times — when it
    ///   requests a plan beyond that bound the turn halts with
    ///   [`BudgetBound::Replans`] (so `max_replans = 0` is a single plan pass).
    /// - charging a tool call against the global ceiling or a per-subagent cap
    ///   halts the turn with the corresponding bound — the **first** one reached.
    ///
    /// A budget halt is [`Ok`]`(`[`TurnOutcome::Halted`]`)`, naming the bound. A
    /// **recoverable** subagent fault ([`StepError::Unavailable`], [CR-060] Layer 3)
    /// does not end the turn: it degrades to a `[unavailable — …]` scratchpad
    /// observation and the loop routes around it, answering best-effort over what was
    /// gathered. Only a planner/parse fault and a **turn-fatal** subagent fault
    /// ([`StepError::Failed`] — structural or Synthesizer) are [`Err`]. Nothing
    /// fabricates an answer ([NFR-CC-04]).
    ///
    /// [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    pub async fn run(
        &self,
        request: &str,
        sink: &impl EventSink,
    ) -> Result<TurnOutcome, OrchestratorError> {
        let mut scratchpad: Vec<(PlanStep, StepObservation)> = Vec::new();
        // Plans fully executed so far: 0 while running the initial plan, ≥1 once
        // the planner is replanning. Bounded by `max_replans`.
        let mut plans_executed: u32 = 0;

        loop {
            let decision = self.planner.decide(request, &scratchpad).await?;
            let steps = match decision {
                PlannerDecision::Final { answer } => {
                    sink.emit(OrchestratorEvent::FinalAnswer {
                        answer: answer.clone(),
                    });
                    return Ok(TurnOutcome::Answered(answer));
                }
                PlannerDecision::Plan { steps } => steps,
            };

            // A plan beyond the initial one is a replan; once they exceed the
            // bound, this is a hard turn-halt. Rather than a bare halt, run a final
            // tool-free Synthesizer pass over the scratchpad and answer best-effort
            // when observations exist ([CR-048] A′, [NFR-CC-04]).
            if plans_executed > self.budget.max_replans() {
                let bound = BudgetBound::Replans {
                    limit: self.budget.max_replans(),
                };
                return self
                    .finalize_on_hard_halt(bound, plans_executed, &scratchpad, sink)
                    .await;
            }

            // The planner wants more tool work but the shared global ceiling is
            // spent: the other hard turn-halt. (The per-subagent cap is soft and
            // handled in the roster; only the global ceiling and max-replans reach
            // here.) A `Final` decision above is always honored first, so a turn
            // that can finish at the ceiling is never pre-empted. Same A′ path.
            if self.budget.global_remaining() == 0 {
                let bound = BudgetBound::GlobalToolCalls {
                    limit: self.budget.global_limit(),
                };
                return self
                    .finalize_on_hard_halt(bound, plans_executed, &scratchpad, sink)
                    .await;
            }

            sink.emit(OrchestratorEvent::Plan {
                round: plans_executed,
                steps: steps.clone(),
            });

            for (index, step) in steps.iter().enumerate() {
                sink.emit(OrchestratorEvent::StepStarted {
                    index,
                    role: step.role,
                    instruction: step.instruction.clone(),
                });
                // The tool-less Synthesizer's prose IS the user-facing answer, so
                // its step streams the answer token by token as AnswerDelta events
                // ([FR-UI-19]); every other role produces an intermediate
                // observation, surfaced as a StepObserved summary, never as answer
                // text. The forwarder borrows `sink`, so it costs nothing for the
                // non-Synthesizer steps that ignore it.
                let forwarder = AnswerForwarder(sink);
                let ctx = if step.role == StepRole::Synthesizer {
                    StepContext::with_answer_sink(&self.budget, &forwarder)
                } else {
                    StepContext::new(&self.budget)
                };
                match self.executor.execute(step, &ctx).await {
                    Ok(observation) => {
                        sink.emit(OrchestratorEvent::StepObserved {
                            index,
                            role: step.role,
                            summary: observation.summary.clone(),
                        });
                        scratchpad.push((step.clone(), observation));
                    }
                    // With the real roster the per-subagent cap is soft (closed out
                    // in `roster::run_tool_subagent`), so a raw budget bound reaching
                    // the loop is a hard halt (the global ceiling, or a test double
                    // surfacing a bound directly). Answer best-effort over the
                    // scratchpad when observations exist ([CR-048] A′, [NFR-CC-04]).
                    Err(StepError::Budget(bound)) => {
                        return self
                            .finalize_on_hard_halt(bound, plans_executed, &scratchpad, sink)
                            .await;
                    }
                    Err(StepError::Failed(message)) => {
                        return Err(OrchestratorError::Step {
                            role: step.role,
                            message,
                        });
                    }
                    // A RECOVERABLE fault ([CR-060] Layer 3): the step could not
                    // complete, but the turn routes AROUND it rather than aborting.
                    // Record an explicit `[unavailable — …]` observation to the
                    // scratchpad and continue to the plan's remaining steps; the
                    // tool-less Synthesizer then answers best-effort over whatever
                    // WAS gathered ([FR-UI-28], [NFR-CC-04]). It rides the existing
                    // `StepObserved` SSE event with no new wiring. The fault charges
                    // no budget, so `max_replans` remains the backstop that bounds a
                    // sustained per-role outage — the loop can never hang.
                    Err(StepError::Unavailable(message)) => {
                        // Built from `UNAVAILABLE_MARKER` so the prefix that
                        // `has_usable_observation` detects exists in exactly ONE
                        // place — construction and detection can never drift.
                        let summary = format!(
                            "{UNAVAILABLE_MARKER} the {:?} step could not complete: {message}]",
                            step.role
                        );
                        sink.emit(OrchestratorEvent::StepObserved {
                            index,
                            role: step.role,
                            summary: summary.clone(),
                        });
                        scratchpad.push((step.clone(), StepObservation::new(summary)));
                    }
                }
            }

            plans_executed += 1;
        }
    }

    /// Compose a best-effort grounded answer on a hard turn-halt, or an honest bare
    /// halt when nothing was gathered ([CR-048] Strategy A′, [NFR-CC-04]).
    ///
    /// Reached only for the two hard bounds — the global tool-call ceiling and
    /// max-replans (the per-subagent cap is soft and closed out in the roster). If
    /// the per-turn scratchpad holds any observations, one final **tool-free**
    /// [`Synthesizer`](StepRole::Synthesizer) pass runs over it (charging no
    /// budget, so it works even with the global ceiling spent) and the turn returns
    /// [`TurnOutcome::Answered`] with an explicit **bounded** marker; the answer is
    /// grounded in the recorded observations, never fabricated. A scratchpad with no
    /// **usable** observation — empty, or holding only degraded `[unavailable — …]`
    /// notes ([CR-060] Layer 3) — returns an honest bare [`TurnOutcome::Halted`]
    /// naming the bound, since there is no grounded material to answer from. If
    /// synthesis itself yields no text it falls back to the bare halt rather than
    /// inventing an answer.
    async fn finalize_on_hard_halt(
        &self,
        bound: BudgetBound,
        round: u32,
        scratchpad: &[(PlanStep, StepObservation)],
        sink: &impl EventSink,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // Nothing usable gathered → an honest bare halt, no fabricated answer. A
        // scratchpad that is empty OR holds only degraded `[unavailable — …]` notes
        // ([CR-060] Layer 3) has no grounded material to answer from, so composing a
        // "best-effort" answer over it would be a fabrication ([NFR-CC-04]).
        if !has_usable_observation(scratchpad) {
            sink.emit(OrchestratorEvent::Halted { round, bound });
            return Ok(TurnOutcome::Halted(bound));
        }

        // The bounded marker prefixes the terminal answer (the record of truth the
        // Chat view reconciles to) rather than a separate leading delta, so the
        // fallback path below stays a clean bare halt if synthesis produces nothing.
        let marker = format!(
            "[bounded — {bound}; this answer draws only on the observations gathered before \
             the turn was bounded and may be incomplete]"
        );

        // One tool-free Synthesizer pass over the scratchpad. In production the
        // roster injects the rendered scratchpad as the Synthesizer's grounding
        // (S-175); the step instruction only frames the bounded intent.
        let synth_step = PlanStep::new(
            StepRole::Synthesizer,
            "The turn was bounded by its budget before it could finish. Using only the \
             observations gathered so far, compose the best-effort grounded answer to the \
             user's question and make clear it may be incomplete. Ground every claim in those \
             observations; never invent facts.",
        );
        let forwarder = AnswerForwarder(sink);
        let ctx = StepContext::with_answer_sink(&self.budget, &forwarder);
        match self.executor.execute(&synth_step, &ctx).await {
            Ok(observation) => {
                let answer = format!("{marker}\n{}", observation.summary);
                sink.emit(OrchestratorEvent::FinalAnswer {
                    answer: answer.clone(),
                });
                Ok(TurnOutcome::Answered(answer))
            }
            // Synthesis could produce no grounded answer — report the honest halt
            // rather than fabricating one ([NFR-CC-04]).
            Err(_) => {
                sink.emit(OrchestratorEvent::Halted { round, bound });
                Ok(TurnOutcome::Halted(bound))
            }
        }
    }
}
