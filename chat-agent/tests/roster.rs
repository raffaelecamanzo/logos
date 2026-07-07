//! The fixed subagent roster end-to-end ([S-174], [ADR-41], [NFR-SE-04],
//! [NFR-CC-04]).
//!
//! These drive the **public** orchestrator API with the offline mock
//! `CompletionModel` (the S-166 substrate) as both the planner and each
//! subagent's model, over real agent-core tools (S-167) on a fixture `Engine` +
//! `Sandbox`. They prove the three S-174 acceptance criteria:
//!
//! 1. each of the four subagents runs with **only** its domain's tools — an
//!    out-of-subset tool call is refused, charging nothing;
//! 2. the planner routes a graph/governance/source step to the matching
//!    subagent, and the tool-less Synthesizer makes **no** tool calls;
//! 3. a compound question dispatches to ≥2 subagents and yields a grounded
//!    synthesized answer.
//!
//! [S-174]: ../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use chat_agent::orchestrator::{
    BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, RoleModels,
    StepRole, SubagentRoster, TurnOutcome,
};
use logos_core::Engine;

/// A fixture project with an `alpha → beta → gamma` call chain so the graph tools
/// resolve real symbols/edges and the `read` source tool returns real content.
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

/// JSON the mock planner returns for a single-step `plan` decision.
fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

/// JSON the mock planner returns for a `final` decision.
fn final_json(answer: &str) -> String {
    format!(r#"{{"action":"final","answer":"{answer}"}}"#)
}

/// The distinct subagent roles observed during a turn, in order.
fn observed_roles(sink: &CapturingSink) -> Vec<StepRole> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::StepObserved { role, .. } => Some(role),
            _ => None,
        })
        .collect()
}

// ── Acceptance 1: least privilege — an out-of-subset call is refused ──────────
//
// The S-174 least-privilege partition still refuses an out-of-subset tool call
// **without charging** (asserted below). [S-241]/[CR-060] Layer 2 changes only its
// *disposition*: the refusal is now fed back to the model as a self-correcting
// observation (naming the subagent's available tools) instead of aborting the turn,
// so the subagent can reroute and the turn completes. The tool-listing content of
// that refusal and the streak-cap bound are covered in `tests/fault_resilience.rs`.

#[tokio::test]
async fn source_reader_cannot_call_a_graph_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // The planner routes one step to the Source-Reader, then finalizes.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "find where beta is defined")),
        MockTurn::text(final_json("beta is defined in src/lib.rs.")),
    ]);
    // …but the Source-Reader's model first tries `search`, a GRAPH tool outside its
    // sandboxed source domain. The misroute is refused without charge and fed back
    // as an observation, so the subagent adapts with a plain-text answer.
    let source = MockCompletionModel::new([
        MockTurn::tool_call("x1", "search", serde_json::json!({ "query": "beta" })),
        MockTurn::text("beta is defined in src/lib.rs."),
    ]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: source,
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("where is beta defined?", &sink)
        .await
        .expect("an out-of-domain misroute self-corrects — it never aborts the turn");

    assert!(
        matches!(outcome, TurnOutcome::Answered(_)),
        "the subagent adapts past the refused misroute and the turn answers: {outcome:?}"
    );
    // A refused misroute charges nothing on the global ceiling — the S-174
    // least-privilege guarantee is preserved ([NFR-CC-04]).
    assert_eq!(orchestrator.budget().global_used(), 0);
}

#[tokio::test]
async fn graph_navigator_cannot_call_a_source_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "read the source")),
        MockTurn::text(final_json("the graph shows alpha calls beta.")),
    ]);
    // The Graph-Navigator tries `read`, a SOURCE tool outside its graph domain; the
    // misroute is refused without charge, fed back, and the subagent then answers.
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g1", "read", serde_json::json!({ "path": "src/lib.rs" })),
        MockTurn::text("the graph shows alpha calls beta."),
    ]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph,
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("q", &sink)
        .await
        .expect("an out-of-domain misroute self-corrects — it never aborts the turn");
    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers: {outcome:?}");
    assert_eq!(orchestrator.budget().global_used(), 0);
}

// ── Acceptance 2 + 3: routing, no synthesizer tools, grounded compound answer ─

#[tokio::test]
async fn a_compound_question_routes_to_each_subagent_and_synthesizes_grounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // A compound plan: a graph step, then a source step, then synthesis —
    // exercising routing to three distinct subagents and the tool-less
    // Synthesizer, then a final grounded answer.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "find callers of beta")),
        MockTurn::text(plan_json("source_reader", "read src/lib.rs")),
        MockTurn::text(plan_json("synthesizer", "compose the final answer")),
        MockTurn::text(final_json(
            "beta is called by alpha; the source confirms the alpha->beta->gamma chain."
        )),
    ]);

    // Distinct models per role so request counts prove which subagent ran.
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g1", "callers", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("beta is called by alpha (from the graph)."),
    ]);
    let governance = MockCompletionModel::new([]); // never routed to in this turn
    let source = MockCompletionModel::new([
        MockTurn::tool_call("s1", "read", serde_json::json!({ "path": "src/lib.rs" })),
        MockTurn::text("source shows alpha calls beta calls gamma."),
    ]);
    let synthesizer = MockCompletionModel::new([MockTurn::text(
        "Grounded synthesis: alpha → beta → gamma, confirmed by graph and source.",
    )]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: governance.clone(),
            source_reader: source.clone(),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("Who calls beta and what does the source say?", &sink)
        .await
        .expect("the orchestrated compound turn completes");

    // A grounded synthesized final answer.
    assert_eq!(
        outcome,
        TurnOutcome::Answered(
            "beta is called by alpha; the source confirms the alpha->beta->gamma chain."
                .to_string()
        ),
    );

    // ≥2 distinct subagents were dispatched, in plan order.
    let roles = observed_roles(&sink);
    assert_eq!(
        roles,
        vec![
            StepRole::GraphNavigator,
            StepRole::SourceReader,
            StepRole::Synthesizer
        ],
    );

    // Each step reached the CORRECT subagent's model (routing): the graph and
    // source models each served a tool round + a summary round; the synthesizer
    // served one round; the unrouted governance model served none.
    assert_eq!(graph.request_count(), 2, "graph navigator ran (tool + summary)");
    assert_eq!(source.request_count(), 2, "source reader ran (tool + summary)");
    assert_eq!(synthesizer.request_count(), 1, "synthesizer ran once");
    assert_eq!(governance.request_count(), 0, "governance was not routed to");

    // Exactly the two real tool calls (callers + read) were charged; the
    // Synthesizer is tool-less and added nothing.
    assert_eq!(
        orchestrator.budget().global_used(),
        2,
        "graph `callers` + source `read`; the synthesizer makes no tool calls",
    );
}

/// The answer chunks streamed during a turn, in emission order ([FR-UI-19]).
fn answer_deltas(sink: &CapturingSink) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::AnswerDelta { delta } => Some(delta),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_synthesizer_streams_the_answer_token_by_token() {
    // FR-UI-19: the tool-less Synthesizer's prose is the user-facing answer, so it
    // streams token by token as `AnswerDelta` events. A multi-chunk synthesizer
    // turn must surface one delta per chunk, in order, concatenating to the full
    // answer — while the planner's terminal `FinalAnswer` still carries the whole
    // authoritative text.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("synthesizer", "compose the final answer")),
        MockTurn::text(final_json("Engine has three callers.")),
    ]);
    // The synthesizer streams its answer in four chunks (the token-by-token path).
    let chunks = ["Engine ", "has ", "three ", "callers."];
    let synthesizer = MockCompletionModel::new([MockTurn::text_chunks(chunks)]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("who calls Engine?", &sink).await.expect("completes");

    // One delta per streamed chunk, in order.
    assert_eq!(answer_deltas(&sink), chunks, "one AnswerDelta per streamed token, in order");
    // The deltas concatenate to the synthesizer's full prose (no line-joining).
    assert_eq!(answer_deltas(&sink).concat(), "Engine has three callers.");
    // The deltas all precede the terminal FinalAnswer (live preview → record).
    let events = sink.events();
    let last_delta = events
        .iter()
        .rposition(|e| matches!(e, OrchestratorEvent::AnswerDelta { .. }))
        .expect("at least one delta streamed");
    let final_at = events
        .iter()
        .position(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. }))
        .expect("a final answer is emitted");
    assert!(last_delta < final_at, "every token streams before the final answer: {events:?}");
    // The planner's terminal answer is the authoritative full text.
    assert_eq!(outcome, TurnOutcome::Answered("Engine has three callers.".to_string()));
    assert_eq!(synthesizer.request_count(), 1, "the synthesizer streamed once");
}

#[tokio::test]
async fn the_synthesizer_makes_no_tool_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // A turn whose single step is the tool-less Synthesizer.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("synthesizer", "answer from what you have")),
        MockTurn::text(final_json("a synthesized answer")),
    ]);
    let synthesizer = MockCompletionModel::new([MockTurn::text(
        "Synthesized from the (empty) scratchpad — nothing to ground yet.",
    )]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("q", &sink).await.expect("completes");

    assert_eq!(outcome, TurnOutcome::Answered("a synthesized answer".to_string()));
    assert_eq!(synthesizer.request_count(), 1, "the synthesizer ran");
    // Tool-less: it charged zero tool calls.
    assert_eq!(orchestrator.budget().global_used(), 0);
    assert!(observed_roles(&sink).contains(&StepRole::Synthesizer));
}

// ── S-182: budget-aware preambles reduce tool-call count on a broad step ──────

#[tokio::test]
async fn a_single_context_call_answers_breadth_for_fewer_charges_than_several_search_node_calls() {
    // [S-182]/[CR-048]: the Graph-Navigator's preamble now steers it to prefer one
    // breadth-efficient `context` call (a ranked multi-symbol bundle) over several
    // narrow `search`/`node` round-trips when a step is broad.
    //
    // What this test does NOT (and cannot) prove: the offline mock
    // `CompletionModel` never reads a request's preamble at all — both scripts
    // below run through the SAME current, budget-aware preamble; only the
    // hand-scripted tool-call sequence differs. So this is not a behavioral A/B
    // of "budget-aware preamble vs. un-hinted baseline" (no offline test can
    // drive that, since the mock is preamble-blind by construction). The
    // preamble TEXT actually gaining the context-preference wording over its
    // pre-S-182 form is instead asserted directly, in-crate, by
    // `budget_awareness_tests::graph_navigator_preamble_gained_the_context_preference_over_the_pre_s182_baseline`
    // (`chat-agent/src/orchestrator/roster.rs`).
    //
    // What this test DOES prove: the system-level efficiency property the new
    // wording leans on — answering the SAME broad step via `context` alone is
    // charged strictly fewer tool calls than answering it via several
    // `search`/`node` calls — with every tool still fully present and
    // dispatchable (nothing removed or weakened, [S-182] AC2).
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());
    let broad_question = "describe everything reachable from alpha";

    // The un-hinted-style baseline: three separate narrow lookups cover the breadth.
    let planner_narrow = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", broad_question)),
        MockTurn::text(final_json("alpha, beta, and gamma form a call chain.")),
    ]);
    let graph_narrow = MockCompletionModel::new([
        MockTurn::tool_call("n1", "search", serde_json::json!({ "query": "alpha" })),
        MockTurn::tool_call("n2", "node", serde_json::json!({ "symbol": "beta" })),
        MockTurn::tool_call("n3", "node", serde_json::json!({ "symbol": "gamma" })),
        MockTurn::text("alpha calls beta calls gamma (gathered via three separate lookups)."),
    ]);
    let roster_narrow = SubagentRoster::with_models(
        engine.clone(),
        sandbox.clone(),
        RoleModels {
            graph_navigator: graph_narrow.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator_narrow =
        Orchestrator::new(planner_narrow, roster_narrow, BudgetTree::new(24, 8, 3));
    let sink_narrow = CapturingSink::new();
    orchestrator_narrow
        .run(broad_question, &sink_narrow)
        .await
        .expect("the narrow-lookup baseline still completes — search/node are not weakened");
    let narrow_calls = orchestrator_narrow.budget().global_used();
    assert_eq!(narrow_calls, 3, "three narrow search/node calls were charged");

    // The budget-aware path: one `context` call covers the same breadth.
    let planner_context = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", broad_question)),
        MockTurn::text(final_json("alpha, beta, and gamma form a call chain.")),
    ]);
    let graph_context = MockCompletionModel::new([
        MockTurn::tool_call("c1", "context", serde_json::json!({ "task": broad_question })),
        MockTurn::text("alpha calls beta calls gamma (gathered via one ranked bundle)."),
    ]);
    let roster_context = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph_context.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator_context =
        Orchestrator::new(planner_context, roster_context, BudgetTree::new(24, 8, 3));
    let sink_context = CapturingSink::new();
    orchestrator_context
        .run(broad_question, &sink_context)
        .await
        .expect("the context-preferring run completes — context is dispatchable");
    let context_calls = orchestrator_context.budget().global_used();
    assert_eq!(context_calls, 1, "one breadth-efficient context call was charged");

    assert!(
        context_calls < narrow_calls,
        "preferring `context` for a broad question charges fewer tool calls \
         ({context_calls}) than several narrow `search`/`node` calls ({narrow_calls})"
    );
}

#[tokio::test]
async fn governance_analyst_runs_a_governance_tool() {
    // Routing to the Governance-Analyst with a real governance tool (scan).
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("governance_analyst", "scan the module")),
        MockTurn::text(final_json("the module scans clean")),
    ]);
    let governance = MockCompletionModel::new([
        MockTurn::tool_call("gov1", "scan", serde_json::json!({})),
        MockTurn::text("scan reports a grounded quality signal."),
    ]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: governance.clone(),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("is the module clean?", &sink).await.expect("completes");

    assert_eq!(outcome, TurnOutcome::Answered("the module scans clean".to_string()));
    assert_eq!(governance.request_count(), 2, "governance analyst ran the scan + summarized");
    assert_eq!(observed_roles(&sink), vec![StepRole::GovernanceAnalyst]);
    assert_eq!(orchestrator.budget().global_used(), 1, "one governance tool call");
}

#[tokio::test]
async fn a_subagent_reaching_its_cap_soft_closes_without_overcharging_global() {
    // The per-subagent cap is a SOFT bound ([CR-048]): reaching it does NOT halt
    // the turn. The subagent performs exactly-cap dispatches, closes its
    // conversation out well-formed, summarizes tool-free, and returns a marked
    // bounded observation the planner then finalizes from — never a
    // `SubagentToolCalls` turn halt. The charge ordering invariant still holds: the
    // refused over-cap call charges the shared global ceiling nothing extra.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // The planner routes one step to the Graph-Navigator, then finalizes once the
    // bounded observation comes back.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "exhaust your tool budget")),
        MockTurn::text(final_json("beta is called by alpha (bounded, from the graph).")),
    ]);
    // The Graph-Navigator asks for TWO in-domain tool calls; the per-subagent cap
    // is 1, so the second is refused — triggering the soft close-out. Its third
    // turn is the tool-free summarization the close-out re-prompts for.
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g1", "callers", serde_json::json!({ "symbol": "beta" })),
        MockTurn::tool_call("g2", "callees", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("From the one call I made: beta is called by alpha."),
    ]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    // Global ceiling high (24), per-subagent cap 1 → the per-subagent bound binds
    // first, but as a soft close, not a halt.
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 1, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("q", &sink)
        .await
        .expect("a soft-capped subagent yields a grounded turn, not an error");

    // The turn finalized from the bounded observation — NOT a per-subagent-cap halt.
    assert_eq!(
        outcome,
        TurnOutcome::Answered("beta is called by alpha (bounded, from the graph).".to_string()),
    );

    // Exactly-cap dispatches: the one allowed call ran (global +1); the refused
    // over-cap call charged the global ceiling nothing extra.
    assert_eq!(orchestrator.budget().global_used(), 1);
    // Two tool rounds + one tool-free summarization round = three model requests.
    assert_eq!(graph.request_count(), 3, "two tool rounds + one summarization");

    let events = sink.events();
    // The bounded observation surfaced as a StepObserved summary, explicitly marked
    // partial — the seam the planner and Synthesizer read.
    assert!(
        events.iter().any(|e| matches!(
            e,
            OrchestratorEvent::StepObserved { role: StepRole::GraphNavigator, summary, .. }
                if summary.starts_with("[bounded — reached the 1-tool-call subagent cap; this summary may be partial]")
        )),
        "the bounded observation is marked partial: {events:?}"
    );
    // No per-subagent-cap turn halt was emitted.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrchestratorEvent::Halted { .. })),
        "the soft cap does not halt the turn: {events:?}"
    );
}
