//! End-to-end orchestrator-core tests ([S-173], [ADR-41], [NFR-CC-04]).
//!
//! These drive the **public** orchestrator API with the offline mock
//! `CompletionModel` (the S-166 substrate) as the planner and a scripted
//! [`StepExecutor`] standing in for the S-174 subagent roster. They prove the
//! three acceptance criteria:
//!
//! 1. the planner turns a compound request into an explicit step plan and runs
//!    plan→act→observe→replan to a final answer;
//! 2. the **hard** budget bounds (global ceiling / max-replans) stop a long turn
//!    and report which one, with no fabricated tool result or answer; when the
//!    scratchpad is empty the halt is bare, when it holds observations the turn
//!    answers best-effort ([CR-048] A′). The **per-subagent cap** is a soft bound
//!    handled in the roster (see `tests/soft_cap.rs`), so it never halts here;
//! 3. plan and step-transition events are emitted for the SSE stream.
//!
//! [S-173]: ../../docs/planning/journal.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use agent_core::{MockCompletionModel, MockTurn};
use chat_agent::orchestrator::{
    BudgetBound, BudgetTree, CapturingSink, Orchestrator, OrchestratorError, OrchestratorEvent,
    PlanStep, Planner, StepContext, StepError, StepExecutor, StepObservation, StepRole, TurnOutcome,
};

/// A scripted stand-in for the S-174 subagent roster. Each step charges
/// `calls_per_step` tool calls through the [`StepContext`] (exercising the budget
/// tree) and returns a summary; a budget refusal propagates as
/// [`StepError::Budget`] — never a fabricated observation ([NFR-CC-04]).
struct ScriptedExecutor {
    calls_per_step: usize,
}

impl StepExecutor for ScriptedExecutor {
    fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> impl std::future::Future<Output = Result<StepObservation, StepError>> + Send {
        let calls = self.calls_per_step;
        let role = step.role;
        async move {
            for _ in 0..calls {
                // Honest charge: a refused call halts the step at the bound, the
                // tool is never invoked, no result is fabricated.
                ctx.charge_tool_call()?;
            }
            Ok(StepObservation::new(format!(
                "{role:?} ran {calls} grounded tool call(s)"
            )))
        }
    }
}

/// JSON the mock planner returns for a `plan` decision over one step.
fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

/// JSON the mock planner returns for a `final` decision.
fn final_json(answer: &str) -> String {
    format!(r#"{{"action":"final","answer":"{answer}"}}"#)
}

/// JSON the mock planner returns for a `plan` decision with **no** steps — a plan
/// that records no observation, so a subsequent replan-bound halt is reached with
/// an empty scratchpad (the honest bare-halt path, [CR-048]/[NFR-CC-04]).
fn empty_plan_json() -> String {
    r#"{"action":"plan","steps":[]}"#.to_string()
}

#[tokio::test]
async fn plan_act_observe_replan_runs_to_a_final_answer() {
    // A compound request: plan a graph step, replan a governance step, then
    // synthesize the final answer — three planner turns over the mock.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "find callers of Engine")),
        MockTurn::text(plan_json("governance_analyst", "scan the Engine module")),
        MockTurn::text(final_json("Engine has 3 callers and a clean scan.")),
    ]);
    let orchestrator = Orchestrator::new(
        planner.clone(),
        ScriptedExecutor { calls_per_step: 1 },
        BudgetTree::new(24, 8, 3),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("Who calls Engine and is the module clean?", &sink)
        .await
        .expect("the orchestrated turn completes");

    assert_eq!(
        outcome,
        TurnOutcome::Answered("Engine has 3 callers and a clean scan.".to_string()),
    );
    // The mock — not a real provider — served all three planner turns.
    assert!(planner.request_count() >= 3, "the mock planned each round");

    // The loop genuinely replanned: an initial plan (round 0) and one replan
    // (round 1) were emitted.
    let events = sink.events();
    let plan_rounds: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            OrchestratorEvent::Plan { round, .. } => Some(*round),
            _ => None,
        })
        .collect();
    assert_eq!(plan_rounds, vec![0, 1], "an initial plan and one replan");
}

#[tokio::test]
async fn global_ceiling_halts_first_and_reports_the_bound() {
    // Global ceiling = 1, per-subagent high: the second tool call in the first
    // step trips the GLOBAL bound.
    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("graph_navigator", "deep dive"))]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 2 },
        BudgetTree::new(1, 8, 3),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("a long question", &sink).await.unwrap();

    assert_eq!(outcome, TurnOutcome::Halted(BudgetBound::GlobalToolCalls { limit: 1 }));
    // Tripped inside the initial plan's step, so round 0.
    assert_honest_halt(&sink, 0, BudgetBound::GlobalToolCalls { limit: 1 });
}

#[tokio::test]
async fn per_subagent_cap_halts_first_and_reports_the_bound() {
    // The budget tree still reports the per-subagent bound BEFORE the global one
    // when a step's own cap binds first (the "charge the discarded slot first"
    // ordering). With the real roster this bound is now SOFT — closed out in
    // `run_tool_subagent`, never reaching the loop (see `tests/soft_cap.rs`); here
    // the scripted executor surfaces the raw bound directly, so this pins the
    // loop's honest fallback: with an empty scratchpad, the hard-halt path returns
    // a bare halt naming the bound, never a fabricated answer ([CR-048]).
    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("source_reader", "read everything"))]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 2 },
        BudgetTree::new(24, 1, 3),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("a long question", &sink).await.unwrap();

    assert_eq!(
        outcome,
        TurnOutcome::Halted(BudgetBound::SubagentToolCalls { limit: 1 })
    );
    // Tripped inside the initial plan's step, so round 0.
    assert_honest_halt(&sink, 0, BudgetBound::SubagentToolCalls { limit: 1 });
}

#[tokio::test]
async fn max_replans_halts_first_and_reports_the_bound() {
    // max_replans = 1 and a planner that never finishes: after the initial plan
    // and one replan, a third plan request trips the REPLANS bound. The plans have
    // no steps, so nothing is recorded to the scratchpad and the hard halt is a
    // bare, honest halt naming the bound ([CR-048] — A′ answers only when the
    // scratchpad holds observations).
    let planner = MockCompletionModel::new([
        MockTurn::text(empty_plan_json()),
        MockTurn::text(empty_plan_json()),
        MockTurn::text(empty_plan_json()),
    ]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 0 },
        BudgetTree::new(24, 8, 1),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("never satisfiable", &sink).await.unwrap();

    assert_eq!(outcome, TurnOutcome::Halted(BudgetBound::Replans { limit: 1 }));
    // Plans 0 and 1 executed; the refused round is 2 (one beyond the last emitted
    // Plan, which carried round 1).
    assert_honest_halt(&sink, 2, BudgetBound::Replans { limit: 1 });
    // The refused over-budget plan is never emitted: no Plan event at round 2.
    assert!(
        !sink.events().iter().any(|e| matches!(
            e,
            OrchestratorEvent::Plan { round: 2, .. }
        )),
        "the over-budget plan is refused, never emitted"
    );
}

#[tokio::test]
async fn max_replans_zero_is_a_single_plan_pass() {
    // max_replans = 0: the initial plan runs; a second plan request halts. Empty
    // plans keep the scratchpad empty, so the halt is a bare, honest one naming
    // the bound ([CR-048]).
    let planner = MockCompletionModel::new([
        MockTurn::text(empty_plan_json()),
        MockTurn::text(empty_plan_json()),
    ]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 0 },
        BudgetTree::new(24, 8, 0),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("q", &sink).await.unwrap();
    assert_eq!(outcome, TurnOutcome::Halted(BudgetBound::Replans { limit: 0 }));
}

#[tokio::test]
async fn a_planner_provider_error_surfaces_honestly() {
    // An exhausted mock (no scripted turns) is an honest provider failure, not a
    // fabricated plan (NFR-CC-04).
    let planner = MockCompletionModel::new([]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 0 },
        BudgetTree::new(24, 8, 3),
    );

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("an exhausted planner is an error, never a fabricated answer");
    assert!(
        matches!(err, chat_agent::orchestrator::OrchestratorError::Planner(_)),
        "exhausted provider surfaces as a planner error: {err:?}"
    );
    // No answer was fabricated.
    assert!(!sink
        .events()
        .iter()
        .any(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. })));
}

#[tokio::test]
async fn emits_plan_and_step_transition_events_in_order() {
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "find Engine")),
        MockTurn::text(final_json("done")),
    ]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 1 },
        BudgetTree::new(24, 8, 3),
    );

    let sink = CapturingSink::new();
    orchestrator.run("compound question", &sink).await.unwrap();

    let events = sink.events();
    // The exact transition sequence for a one-step plan that then finalizes.
    assert_eq!(events.len(), 4, "plan, step-started, step-observed, final: {events:?}");
    assert!(matches!(
        events[0],
        OrchestratorEvent::Plan { round: 0, .. }
    ));
    assert!(matches!(
        events[1],
        OrchestratorEvent::StepStarted {
            index: 0,
            role: StepRole::GraphNavigator,
            ..
        }
    ));
    assert!(matches!(
        events[2],
        OrchestratorEvent::StepObserved {
            index: 0,
            role: StepRole::GraphNavigator,
            ..
        }
    ));
    assert!(matches!(
        &events[3],
        OrchestratorEvent::FinalAnswer { answer } if answer == "done"
    ));
}

/// A step executor that always fails for a non-budget reason — exercises the
/// honest `StepError::Failed` → `OrchestratorError::Step` path (NFR-CC-04).
struct FailingExecutor;

impl StepExecutor for FailingExecutor {
    async fn execute(
        &self,
        _step: &PlanStep,
        _ctx: &StepContext<'_>,
    ) -> Result<StepObservation, StepError> {
        Err(StepError::Failed("tool exploded".to_string()))
    }
}

#[tokio::test]
async fn a_step_failure_surfaces_as_an_orchestrator_error() {
    let planner = MockCompletionModel::new([MockTurn::text(plan_json("graph_navigator", "do it"))]);
    let orchestrator = Orchestrator::new(planner, FailingExecutor, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("a non-budget step failure is an error, never a fabricated answer");
    assert!(
        matches!(err, OrchestratorError::Step { .. }),
        "a non-budget step failure surfaces as OrchestratorError::Step: {err:?}"
    );
    // A genuine fault is neither a budget halt nor a fabricated answer.
    assert!(!sink.events().iter().any(|e| matches!(
        e,
        OrchestratorEvent::Halted { .. } | OrchestratorEvent::FinalAnswer { .. }
    )));
}

#[tokio::test]
async fn an_unparseable_planner_reply_surfaces_as_a_plan_parse_error() {
    // The planner returns prose, not the JSON contract: an honest PlanParse error,
    // never a guessed plan (NFR-CC-04).
    let planner = MockCompletionModel::new([MockTurn::text("I'm not sure how to answer that.")]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 0 },
        BudgetTree::new(24, 8, 3),
    );

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("an unparseable reply is an error, never a fabricated plan");
    assert!(
        matches!(err, OrchestratorError::PlanParse(_)),
        "a garbage reply surfaces as PlanParse: {err:?}"
    );
    assert!(
        sink.events().is_empty(),
        "the parse failure precedes any plan/step event"
    );
}

#[tokio::test]
async fn with_planner_runs_a_custom_preamble_planner() {
    // The with_planner / with_preamble construction path (the seam S-174 uses to
    // inject a per-role-configured planner) runs a turn end-to-end.
    let model = MockCompletionModel::new([MockTurn::text(final_json("custom-preamble answer"))]);
    let planner = Planner::with_preamble(model, "You are a bespoke Logos planner.");
    let orchestrator = Orchestrator::with_planner(
        planner,
        ScriptedExecutor { calls_per_step: 0 },
        BudgetTree::new(24, 8, 3),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("q", &sink).await.unwrap();
    assert_eq!(
        outcome,
        TurnOutcome::Answered("custom-preamble answer".to_string())
    );
}

#[tokio::test]
async fn max_replans_zero_still_answers_when_the_first_plan_finalizes() {
    // max_replans = 0 bounds *replans*, not finalization: after the initial plan
    // executes, the planner may still produce the final answer — the property the
    // replan check is designed to preserve (a pre-check halt would break it).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "round 0")),
        MockTurn::text(final_json("answered within a single plan pass")),
    ]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor { calls_per_step: 1 },
        BudgetTree::new(24, 8, 0),
    );

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("q", &sink).await.unwrap();
    assert_eq!(
        outcome,
        TurnOutcome::Answered("answered within a single plan pass".to_string())
    );
}

/// A budget halt is honest ([NFR-CC-04]): the stream carries a `Halted` event
/// naming the exact bound **at the expected round**, and **no** `FinalAnswer` is
/// emitted (no fabricated answer on a halt).
fn assert_honest_halt(sink: &CapturingSink, expected_round: u32, expected: BudgetBound) {
    let events = sink.events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            OrchestratorEvent::Halted { round, bound }
                if *round == expected_round && *bound == expected
        )),
        "a Halted event naming the bound at round {expected_round} was emitted: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. })),
        "no answer is fabricated on a halt: {events:?}"
    );
}

/// The honest error frame the SPA renders (and S-200 consumes) leads with the
/// turn **stage** ([S-199], [FR-UI-24]): planner faults are `"planner"`, a
/// tool-bearing subagent failure is `"subagent"`, and a synthesis failure is
/// `"synthesis"` — so a reader can see *where* the turn broke.
#[test]
fn orchestrator_error_names_the_failing_stage() {
    let planner_failure = OrchestratorError::Planner(agent_core::ProviderFailure {
        kind: agent_core::ProviderErrorKind::Transport,
        detail: "connection refused".to_string(),
        status: None,
        body: None,
    });
    assert_eq!(planner_failure.stage(), "planner");

    let parse_failure = OrchestratorError::PlanParse("not json".to_string());
    assert_eq!(parse_failure.stage(), "planner");

    let subagent_failure = OrchestratorError::Step {
        role: StepRole::GraphNavigator,
        message: "the GraphNavigator subagent provider failed".to_string(),
    };
    assert_eq!(subagent_failure.stage(), "subagent");

    let synthesis_failure = OrchestratorError::Step {
        role: StepRole::Synthesizer,
        message: "the synthesizer provider failed".to_string(),
    };
    assert_eq!(synthesis_failure.stage(), "synthesis");
}
