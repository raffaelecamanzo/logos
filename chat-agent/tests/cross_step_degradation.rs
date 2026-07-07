//! Cross-step fault degradation in the orchestrator loop ([S-242], [CR-060]
//! Layer 3, [FR-UI-28], [ADR-41], [NFR-CC-04]).
//!
//! [S-241] made a *tool* fault a self-correcting observation bounded by a
//! soft-close cap; [S-240] added the provider retry seam so "a provider fault
//! surviving retries" is a real condition. This suite proves the third layer: a
//! **recoverable** roster fault becomes a [`StepError::Unavailable`] the loop
//! **routes around** — degrading the step to a `[unavailable — …]` scratchpad
//! observation and continuing — while **structural** and **Synthesizer** faults
//! stay honest turn-fatal [`StepError::Failed`]. The four S-242 acceptance
//! criteria:
//!
//! 1. a non-Synthesizer provider fault surviving retries degrades to a
//!    `[unavailable — …]` scratchpad observation and the turn **continues** to its
//!    remaining planned steps;
//! 2. a **best-effort** grounded answer is returned when any usable observation
//!    exists; an **empty or all-`[unavailable]`** scratchpad returns an honest bare
//!    halt (no fabrication, [NFR-CC-04]);
//! 3. a Synthesizer fault (stage `"synthesis"`) or a structural fault stays
//!    turn-fatal `Failed` with no fabricated answer;
//! 4. a sustained per-role outage across replans **terminates** — bounded by
//!    `max_replans` — and never hangs.
//!
//! Two layers of test double are used. The loop-arm behavior (1, 2, 3, 4) is
//! driven with a scripted [`StepExecutor`] over the mock `CompletionModel` planner,
//! for deterministic control of each step's outcome. The **reclassification**
//! itself — that the real roster now emits `Unavailable` at the three recoverable
//! sites and `Failed` at the Synthesizer site — is proven end-to-end by driving the
//! **real** [`SubagentRoster`] over a mock `CompletionModel` on a fixture engine.
//!
//! The [S-181]/[CR-048] soft-cap and [S-241] tool-error paths are proven unchanged
//! by the untouched `tests/soft_cap.rs` and `tests/fault_resilience.rs` suites.
//!
//! [S-242]: ../../docs/planning/journal.md#s-242-cross-step-fault-degradation-in-the-orchestrator-loop
//! [S-241]: ../../docs/planning/journal.md#s-241-tool-errors-become-self-correcting-observations-with-a-soft-close-cap
//! [S-240]: ../../docs/planning/journal.md#s-240-provider-call-retry-with-backoff-in-agent-core
//! [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
//! [FR-UI-28]: ../../docs/specs/requirements/FR-UI-28.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use chat_agent::orchestrator::{
    BudgetBound, BudgetTree, CapturingSink, Orchestrator, OrchestratorError, OrchestratorEvent,
    PlanStep, RoleModels, StepContext, StepError, StepExecutor, StepObservation, StepRole,
    SubagentRoster, TurnOutcome,
};
use logos_core::Engine;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// JSON a mock planner returns for a single-step `plan` decision.
fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

/// JSON a mock planner returns for a **two-step** `plan` decision — lets a test
/// prove the loop continues to the *next* step in the SAME plan after degrading
/// the first (S-242 AC1).
fn plan_json_two(role_a: &str, instr_a: &str, role_b: &str, instr_b: &str) -> String {
    format!(
        r#"{{"action":"plan","steps":[{{"role":"{role_a}","instruction":"{instr_a}"}},{{"role":"{role_b}","instruction":"{instr_b}"}}]}}"#
    )
}

/// JSON a mock planner returns for a `final` decision.
fn final_json(answer: &str) -> String {
    format!(r#"{{"action":"final","answer":"{answer}"}}"#)
}

/// The `StepObserved` summaries recorded during a turn, in order.
fn observed_summaries(sink: &CapturingSink) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::StepObserved { summary, .. } => Some(summary),
            _ => None,
        })
        .collect()
}

fn any_halted(sink: &CapturingSink) -> bool {
    sink.events()
        .iter()
        .any(|e| matches!(e, OrchestratorEvent::Halted { .. }))
}

fn any_final_answer(sink: &CapturingSink) -> bool {
    sink.events()
        .iter()
        .any(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. }))
}

// ── A scripted step executor for loop-arm behavior ────────────────────────────

/// One scripted step outcome the [`ScriptedExecutor`] returns per `execute` call.
#[derive(Clone)]
enum Outcome {
    /// A grounded observation summary.
    Observe(String),
    /// A recoverable fault → [`StepError::Unavailable`] (the loop should degrade).
    Unavailable(String),
    /// A turn-fatal fault → [`StepError::Failed`] (the loop should abort).
    Failed(String),
}

/// A scripted [`StepExecutor`] returning a queued [`Outcome`] per step — with an
/// optional `fallback` returned once the queue drains, so a **sustained** outage
/// (every replan's step failing) can be modeled without scripting an unbounded
/// queue. It charges no budget: these tests exercise the degradation arm, not the
/// budget tree (that is `tests/orchestrator.rs` / `tests/soft_cap.rs`).
struct ScriptedExecutor {
    outcomes: Mutex<VecDeque<Outcome>>,
    fallback: Option<Outcome>,
}

impl ScriptedExecutor {
    fn new(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            fallback: None,
        }
    }

    /// An executor that returns `outcome` for **every** step, forever — models a
    /// sustained per-role outage across replans (S-242 AC4).
    fn always(outcome: Outcome) -> Self {
        Self {
            outcomes: Mutex::new(VecDeque::new()),
            fallback: Some(outcome),
        }
    }
}

impl StepExecutor for ScriptedExecutor {
    async fn execute(
        &self,
        step: &PlanStep,
        _ctx: &StepContext<'_>,
    ) -> Result<StepObservation, StepError> {
        let next = {
            let mut queue = self.outcomes.lock().unwrap_or_else(|p| p.into_inner());
            queue.pop_front().or_else(|| self.fallback.clone())
        };
        match next {
            Some(Outcome::Observe(summary)) => Ok(StepObservation::new(summary)),
            Some(Outcome::Unavailable(message)) => Err(StepError::Unavailable(message)),
            Some(Outcome::Failed(message)) => Err(StepError::Failed(message)),
            None => panic!(
                "ScriptedExecutor exhausted with no fallback (role {:?})",
                step.role
            ),
        }
    }
}

// ── AC1: a recoverable fault degrades to `[unavailable — …]` and the turn continues

#[tokio::test]
async fn a_recoverable_fault_degrades_to_an_unavailable_observation_and_the_turn_continues() {
    // ONE plan with TWO steps: the first (Graph-Navigator) is unavailable, the
    // second (Source-Reader) succeeds — proving the loop routes AROUND the degraded
    // step and continues to the remaining planned step in the SAME plan, rather than
    // aborting the whole turn ([CR-060] Layer 3, [FR-UI-28]).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json_two(
            "graph_navigator",
            "map the module",
            "source_reader",
            "read the entrypoint",
        )),
        MockTurn::text(final_json("Answered from the source read despite the graph outage.")),
    ]);
    let executor = ScriptedExecutor::new([
        Outcome::Unavailable("the provider errored after retries".to_string()),
        Outcome::Observe("src/lib.rs defines the entrypoint".to_string()),
    ]);
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("what defines the module?", &sink)
        .await
        .expect("a recoverable fault degrades and the turn continues — it never aborts");

    assert_eq!(
        outcome,
        TurnOutcome::Answered(
            "Answered from the source read despite the graph outage.".to_string()
        ),
    );
    let summaries = observed_summaries(&sink);
    // The degraded step recorded an explicit, role-named `[unavailable — …]` note.
    assert!(
        summaries.iter().any(|s| s
            == "[unavailable — the GraphNavigator step could not complete: the provider errored after retries]"),
        "the recoverable fault degraded to a marked scratchpad observation: {summaries:?}"
    );
    // The loop continued to the second planned step, whose grounded observation was
    // recorded — proof the turn routed AROUND the degraded step.
    assert!(
        summaries
            .iter()
            .any(|s| s.contains("src/lib.rs defines the entrypoint")),
        "the loop continued to the remaining planned step: {summaries:?}"
    );
    assert!(!any_halted(&sink), "a degraded step is not a halt");
}

// ── AC2: best-effort answer when a usable observation exists (mixed scratchpad) ─

#[tokio::test]
async fn a_best_effort_answer_is_returned_when_a_usable_observation_survives_a_degraded_step() {
    // A degraded step then a real observation, then the planner over-runs
    // `max_replans` → the hard-halt terminal composes a best-effort answer because a
    // USABLE (non-`[unavailable]`) observation is present ([CR-048] A′, [CR-060]).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "round 0")),
        MockTurn::text(plan_json("source_reader", "round 1")),
        MockTurn::text(plan_json("graph_navigator", "round 2 — over budget")),
    ]);
    let executor = ScriptedExecutor::new([
        Outcome::Unavailable("graph provider down".to_string()),
        Outcome::Observe("the real, grounded facts about X".to_string()),
        // The final tool-free Synthesizer pass the hard-halt terminal runs.
        Outcome::Observe("a best-effort answer over what we gathered".to_string()),
    ]);
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 1));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("compound question", &sink)
        .await
        .expect("a usable observation yields a best-effort answer, not an error");

    match outcome {
        TurnOutcome::Answered(answer) => {
            assert!(
                answer.starts_with("[bounded —"),
                "the best-effort answer is honestly marked bounded: {answer}"
            );
            assert!(
                answer.contains("a best-effort answer over what we gathered"),
                "the answer is the grounded synthesis over the scratchpad: {answer}"
            );
        }
        other => panic!("expected a best-effort Answered, got {other:?}"),
    }
    // The degraded step is still on the record, alongside the usable observation.
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with("[unavailable —"))
            && summaries
                .iter()
                .any(|s| s.contains("the real, grounded facts about X")),
        "both the degraded and the usable observation were recorded: {summaries:?}"
    );
}

// ── AC2 + AC4: an all-`[unavailable]` scratchpad halts honestly (bounded by replans)

#[tokio::test]
async fn an_all_unavailable_scratchpad_returns_an_honest_bare_halt_bounded_by_replans() {
    // A sustained outage: EVERY step is unavailable, across the initial plan and its
    // replans. The scratchpad fills only with `[unavailable — …]` notes, so the
    // hard-halt terminal returns an honest BARE halt (no fabricated best-effort
    // answer, [NFR-CC-04]) — AND the turn TERMINATES, bounded by `max_replans`,
    // never hanging on the outage (S-242 AC2 + AC4).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "attempt 0")),
        MockTurn::text(plan_json("graph_navigator", "attempt 1")),
        MockTurn::text(plan_json("graph_navigator", "attempt 2 — over budget")),
    ]);
    let executor = ScriptedExecutor::always(Outcome::Unavailable("provider down".to_string()));
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 1));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("never satisfiable while the provider is down", &sink)
        .await
        .expect("a sustained outage terminates via max_replans, never hangs");

    // Honest bare halt naming the bound — NOT a fabricated answer over the
    // all-`[unavailable]` scratchpad.
    assert_eq!(outcome, TurnOutcome::Halted(BudgetBound::Replans { limit: 1 }));
    assert!(any_halted(&sink), "the turn halted honestly");
    assert!(
        !any_final_answer(&sink),
        "no answer is fabricated over an all-[unavailable] scratchpad"
    );
    // The outage WAS recorded (steps ran and degraded), proving the halt is not a
    // pre-empted empty run — it is the all-`[unavailable]` bare-halt branch.
    let summaries = observed_summaries(&sink);
    assert!(
        !summaries.is_empty() && summaries.iter().all(|s| s.starts_with("[unavailable —")),
        "the scratchpad held only degraded notes: {summaries:?}"
    );
}

#[tokio::test]
async fn an_all_unavailable_scratchpad_bare_halts_even_when_synthesis_would_have_answered() {
    // Non-vacuous guard for the `has_usable_observation` gate itself: unlike the
    // `always(Unavailable)` test above (where the terminal Synthesizer pass would
    // ALSO fail, so both a correct and a removed gate halt), here the queued outcome
    // for the terminal synthesis pass is a SUCCESSFUL observation. So the ONLY thing
    // that keeps the turn from fabricating a `[bounded — …]` answer over the
    // all-`[unavailable]` scratchpad is the gate. If it regressed, this turn would
    // answer instead of halt ([CR-060] Layer 3, [NFR-CC-04]).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "attempt 0")),
        MockTurn::text(plan_json("graph_navigator", "attempt 1")),
        MockTurn::text(plan_json("graph_navigator", "attempt 2 — over budget")),
    ]);
    let executor = ScriptedExecutor::new([
        Outcome::Unavailable("provider down".to_string()), // round 0 step
        Outcome::Unavailable("provider down".to_string()), // round 1 step
        // Would be consumed by the terminal synthesis pass IFF the gate let it run —
        // a would-be fabricated answer the gate must prevent.
        Outcome::Observe("a fabricated answer over nothing".to_string()),
    ]);
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 1));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("never satisfiable while the provider is down", &sink)
        .await
        .expect("the turn terminates");

    assert_eq!(
        outcome,
        TurnOutcome::Halted(BudgetBound::Replans { limit: 1 }),
        "an all-[unavailable] scratchpad bare-halts even though a synthesis pass could have answered"
    );
    // The would-be synthesis observation was queued but the gate halted before the
    // terminal synthesis pass ran — so no answer (fabricated or otherwise) streamed.
    assert!(
        !any_final_answer(&sink),
        "the gate prevented a fabricated best-effort answer over an all-[unavailable] scratchpad"
    );
}

// ── AC3: a Synthesizer fault, and a structural fault, stay turn-fatal `Failed` ──

#[tokio::test]
async fn a_synthesizer_fault_stays_turn_fatal_with_the_synthesis_stage() {
    // A `Failed` from the Synthesizer step is turn-fatal and named at the
    // `"synthesis"` stage — degradation does NOT apply to the answer composer
    // ([CR-060], [NFR-CC-04]).
    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("synthesizer", "compose the answer"))]);
    let executor = ScriptedExecutor::new([Outcome::Failed("the synthesizer stream failed".to_string())]);
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("a Synthesizer fault is turn-fatal, never degraded");

    assert_eq!(err.stage(), "synthesis", "the fault is named at the synthesis stage: {err:?}");
    assert!(
        matches!(err, OrchestratorError::Step { role, .. } if role == StepRole::Synthesizer),
        "a Synthesizer fault stays a turn-fatal Step error: {err:?}"
    );
    assert!(!any_halted(&sink) && !any_final_answer(&sink), "no halt, no fabricated answer");
}

#[tokio::test]
async fn a_structural_fault_stays_turn_fatal_and_does_not_degrade() {
    // A `Failed` from a tool-bearing role (e.g. a structural fault such as the
    // toolset failing to load) stays turn-fatal — it is NOT reclassified to a
    // recoverable degradation ([CR-060]).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "do it")),
        // A second turn the planner never reaches — the structural fault aborts first.
        MockTurn::text(final_json("this must never be returned")),
    ]);
    let executor =
        ScriptedExecutor::new([Outcome::Failed("could not load the subagent's tools".to_string())]);
    let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("a structural fault is turn-fatal, never degraded");

    assert_eq!(
        err.stage(),
        "subagent",
        "a non-Synthesizer fault is named at the subagent stage: {err:?}"
    );
    assert!(
        matches!(err, OrchestratorError::Step { role, .. } if role == StepRole::GraphNavigator),
        "a structural subagent fault surfaces as a turn-fatal Step error: {err:?}"
    );
    // The turn aborted at the fault: no observation was recorded, no answer forged.
    assert!(
        observed_summaries(&sink).is_empty(),
        "the turn aborted before recording any observation"
    );
    assert!(!any_final_answer(&sink), "no answer is fabricated on a structural fault");
}

// ── Roster reclassification (real roster over a mock provider) ─────────────────

/// A fixture project so the real graph/source tools resolve against real files.
fn fixture(dir: &std::path::Path) -> (Arc<Engine>, Arc<Sandbox>) {
    std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn alpha() { beta(); }\n\
         pub fn beta() { gamma(); }\n\
         pub fn gamma() {}\n",
    )
    .expect("write fixture");
    let engine = Arc::new(Engine::start(dir).expect("engine start"));
    let sandbox = Arc::new(Sandbox::new(dir, std::iter::empty()).expect("sandbox"));
    (engine, sandbox)
}

/// Build a roster whose named `role` is backed by `model` and whose other roles are
/// idle (an empty mock never invoked). The planner is a separate model passed to
/// the orchestrator, so the roles never share a scripted turn queue.
fn roster_with(
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    role: &str,
    model: MockCompletionModel,
) -> SubagentRoster<MockCompletionModel> {
    let mut models = RoleModels {
        graph_navigator: MockCompletionModel::new([]),
        governance_analyst: MockCompletionModel::new([]),
        source_reader: MockCompletionModel::new([]),
        synthesizer: MockCompletionModel::new([]),
    };
    match role {
        "graph_navigator" => models.graph_navigator = model,
        "governance_analyst" => models.governance_analyst = model,
        "source_reader" => models.source_reader = model,
        "synthesizer" => models.synthesizer = model,
        other => panic!("unexpected role {other}"),
    }
    SubagentRoster::with_models(engine, sandbox, models)
}

#[tokio::test]
async fn the_roster_reclassifies_a_provider_fault_surviving_retries_to_unavailable() {
    // Site (a): the tool-bearing subagent's main completion faults after the retry
    // seam. Modeled by an EXHAUSTED mock (its first completion is a provider error).
    // The real roster must now emit `Unavailable`, so the loop DEGRADES and the turn
    // continues to a final answer — rather than aborting with `OrchestratorError::Step`.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "read the sources")),
        MockTurn::text(final_json("Answered best-effort despite the provider outage.")),
    ]);
    let roster = roster_with(engine, sandbox, "source_reader", MockCompletionModel::new([]));
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("what defines the call chain?", &sink)
        .await
        .expect("a provider fault degrades and the turn continues — it is not turn-fatal");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answered: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "[unavailable — the SourceReader step could not complete:"
        ) && s.contains("provider failed")),
        "the provider fault was reclassified to a degraded observation: {summaries:?}"
    );
    // A provider fault never dispatched a tool — nothing charged the global ceiling.
    assert_eq!(orchestrator.budget().global_used(), 0);
}

#[tokio::test]
async fn the_roster_reclassifies_a_neither_tool_nor_text_reply_to_unavailable() {
    // Site (b): the model returns neither a tool call nor text (empty reply). The
    // real roster must emit `Unavailable`, so the turn degrades and continues.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "map beta")),
        MockTurn::text(final_json("Answered despite the empty subagent reply.")),
    ]);
    // A single empty-text turn: no tool call, no prose.
    let graph = MockCompletionModel::new([MockTurn::text("")]);
    let roster = roster_with(engine, sandbox, "graph_navigator", graph);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("map beta", &sink)
        .await
        .expect("an empty reply degrades and the turn continues — it is not turn-fatal");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answered: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "[unavailable — the GraphNavigator step could not complete:"
        ) && s.contains("neither a tool call nor an answer")),
        "the empty reply was reclassified to a degraded observation: {summaries:?}"
    );
}

#[tokio::test]
async fn the_roster_reclassifies_a_bounded_summarization_provider_fault_to_unavailable() {
    // Site (c): a soft-close's tool-free summarization completion itself faults. With
    // a per-subagent cap of 1, the first `glob` dispatches and the second trips the
    // cap → `close_and_summarize` runs its closing completion, which the exhausted
    // mock fails. The real roster must emit `Unavailable` (not `Failed`), so the turn
    // degrades and continues.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "survey the sources")),
        MockTurn::text(final_json("Answered despite the bounded-summarization outage.")),
    ]);
    // Two `glob` calls: the first dispatches (cap = 1), the second trips the cap and
    // enters close_and_summarize; its tool-free closing completion is the 3rd call —
    // the mock is exhausted by then, so it faults → the bounded-summarization site.
    let source = MockCompletionModel::new([
        MockTurn::tool_call("g1", "glob", serde_json::json!({ "pattern": "src/**/*.rs" })),
        MockTurn::tool_call("g2", "glob", serde_json::json!({ "pattern": "src/**/*.rs" })),
    ]);
    let roster = roster_with(engine, sandbox, "source_reader", source);
    // Global ceiling generous (24) so the per-subagent cap (1) is what binds first.
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 1, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("survey the sources", &sink)
        .await
        .expect("a bounded-summarization fault degrades and the turn continues");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answered: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "[unavailable — the SourceReader step could not complete:"
        ) && s.contains("bounded summarization failed")),
        "the bounded-summarization fault was reclassified to a degraded observation: {summaries:?}"
    );
}

#[tokio::test]
async fn the_roster_keeps_a_synthesizer_fault_turn_fatal() {
    // The Synthesizer runs through `run_synthesizer`, whose faults stay `Failed`.
    // An exhausted mock makes its stream fault → a turn-fatal `OrchestratorError::Step`
    // at the `"synthesis"` stage, never a degraded observation ([CR-060], [NFR-CC-04]).
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("synthesizer", "compose the answer"))]);
    let roster = roster_with(engine, sandbox, "synthesizer", MockCompletionModel::new([]));
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("q", &sink)
        .await
        .expect_err("a Synthesizer provider fault is turn-fatal, never degraded");

    assert_eq!(err.stage(), "synthesis", "the fault is named at the synthesis stage: {err:?}");
    assert!(
        matches!(err, OrchestratorError::Step { role, .. } if role == StepRole::Synthesizer),
        "a Synthesizer fault stays a turn-fatal Step error: {err:?}"
    );
    assert!(!any_final_answer(&sink), "no answer is fabricated on a Synthesizer fault");
}
