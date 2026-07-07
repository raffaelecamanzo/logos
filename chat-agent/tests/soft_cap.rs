//! Soft per-subagent budget cap — graceful summarization and answer-on-halt
//! ([S-181], [CR-048], [ADR-41], [NFR-CC-04]).
//!
//! These drive the **public** orchestrator API with the offline mock
//! `CompletionModel` over the real subagent roster (S-174) on a fixture `Engine`
//! + `Sandbox`, proving the four S-181 acceptance criteria:
//!
//! 1. a broad single-step question whose subagent reaches the per-subagent cap
//!    returns a grounded answer built from a bounded summary marked partial —
//!    never a bare halt;
//! 2. a capped subagent performs **exactly-cap** tool dispatches and leaves a
//!    **well-formed** conversation (every `tool_use` answered) before its tool-free
//!    summarization turn, and the orchestrator finalizes rather than returning a
//!    per-subagent-cap halt;
//! 3. a hard halt (global ceiling / max-replans) with a non-empty scratchpad
//!    returns a best-effort grounded answer marked bounded; an empty scratchpad
//!    returns an honest bare halt;
//! 4. termination is preserved — three consecutive bounded steps drain the global
//!    ceiling and the turn hard-halts, with no unbounded loop and no fabricated
//!    tool result or answer.
//!
//! [S-181]: ../../docs/planning/journal.md#s-181-soft-per-subagent-budget-cap-with-graceful-summarization-and-answer-on-halt
//! [CR-048]: ../../docs/requests/CR-048-soft-per-subagent-budget-cap.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_core::rig::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    GetTokenUsage, Usage,
};
use agent_core::rig::message::{Message, ToolCall, ToolFunction, UserContent};
use agent_core::rig::streaming::StreamingCompletionResponse;
use agent_core::rig::OneOrMany;
use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use chat_agent::orchestrator::{
    BudgetBound, BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, RoleModels,
    SubagentRoster, TurnOutcome,
};
use logos_core::Engine;
use serde::{Deserialize, Serialize};

/// A fixture project with an `alpha → beta → gamma` call chain so the graph tools
/// resolve real symbols/edges.
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

fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

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

// ── A well-formedness-checking model, standing in for a real provider ─────────

/// The mock's raw-response type (zero usage — no real call is made).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Raw {
    usage: Usage,
}
impl GetTokenUsage for Raw {
    fn token_usage(&self) -> Usage {
        self.usage
    }
}

/// A scripted [`CompletionModel`] that behaves like a real provider with respect
/// to **conversation well-formedness**: on the tool-free closing request (the one
/// the soft-cap close-out issues, detected by an empty tool set) it verifies that
/// **every** assistant `tool_use` in the history has a matching `tool_result` and
/// returns a provider error if not — exactly how the Anthropic / OpenAI-compatible
/// providers reject a dangling `tool_use` ([CR-048] risk table). It records the
/// verdict so the test can assert the close-out was well-formed.
///
/// Tool rounds (a non-empty tool set) return the next scripted tool call.
struct WellFormedModel {
    /// Scripted tool calls returned on successive tool rounds: `(id, name, args)`.
    tool_calls: Mutex<VecDeque<(String, String, serde_json::Value)>>,
    /// The summary text returned on the (well-formed) closing round. An empty
    /// string exercises the honest "no summary produced" branch.
    summary: String,
    /// A stray tool call the model emits on the tool-free closing round despite the
    /// "no tools" directive — lets a test prove the orchestrator ignores it (never
    /// dispatches it, never charges budget). `(id, name, args)`.
    closing_stray_tool: Option<(String, String, serde_json::Value)>,
    /// Whether the closing request's conversation was well-formed (set once the
    /// tool-free closing round is served).
    closing_well_formed: Arc<AtomicBool>,
    /// Whether a tool-free closing round was ever served.
    saw_closing_round: Arc<AtomicBool>,
    /// Completion requests served (proves the mock — not a real provider — ran).
    requests: AtomicUsize,
}

impl WellFormedModel {
    fn new(
        tool_calls: impl IntoIterator<Item = (String, String, serde_json::Value)>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            tool_calls: Mutex::new(tool_calls.into_iter().collect()),
            summary: summary.into(),
            closing_stray_tool: None,
            closing_well_formed: Arc::new(AtomicBool::new(false)),
            saw_closing_round: Arc::new(AtomicBool::new(false)),
            requests: AtomicUsize::new(0),
        }
    }

    /// Emit a stray tool call on the tool-free closing round (a model ignoring the
    /// "no tools" directive), so a test can prove it is ignored, not dispatched.
    fn with_closing_stray_tool(
        mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        self.closing_stray_tool = Some((id.into(), name.into(), args));
        self
    }
}

impl Clone for WellFormedModel {
    fn clone(&self) -> Self {
        Self {
            tool_calls: Mutex::new(
                self.tool_calls
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            ),
            summary: self.summary.clone(),
            closing_stray_tool: self.closing_stray_tool.clone(),
            closing_well_formed: self.closing_well_formed.clone(),
            saw_closing_round: self.saw_closing_round.clone(),
            requests: AtomicUsize::new(self.requests.load(Ordering::SeqCst)),
        }
    }
}

/// Every assistant `tool_use` id in `history` has a matching `tool_result` id.
fn conversation_is_well_formed(history: &OneOrMany<Message>) -> bool {
    let mut tool_use_ids: Vec<&str> = Vec::new();
    let mut tool_result_ids: Vec<&str> = Vec::new();
    for message in history.iter() {
        match message {
            Message::Assistant { content, .. } => {
                for item in content.iter() {
                    if let AssistantContent::ToolCall(tc) = item {
                        tool_use_ids.push(tc.id.as_str());
                    }
                }
            }
            Message::User { content } => {
                for item in content.iter() {
                    if let UserContent::ToolResult(tr) = item {
                        tool_result_ids.push(tr.id.as_str());
                    }
                }
            }
            Message::System { .. } => {}
        }
    }
    tool_use_ids
        .iter()
        .all(|id| tool_result_ids.contains(id))
}

impl CompletionModel for WellFormedModel {
    type Response = Raw;
    type StreamingResponse = Raw;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::new([], String::new())
    }

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        // The tool-free closing round: behave like a real provider and reject a
        // malformed (dangling `tool_use`) conversation.
        if request.tools.is_empty() {
            self.saw_closing_round.store(true, Ordering::SeqCst);
            let ok = conversation_is_well_formed(&request.chat_history);
            self.closing_well_formed.store(ok, Ordering::SeqCst);
            if !ok {
                return Err(CompletionError::ProviderError(
                    "malformed conversation: an assistant tool_use has no matching tool_result"
                        .to_string(),
                ));
            }
            // A well-behaved closing reply is text-only; an ill-behaved one emits a
            // stray tool call (which the orchestrator must ignore, never dispatch).
            let mut contents = Vec::new();
            if let Some((id, name, args)) = &self.closing_stray_tool {
                contents.push(AssistantContent::ToolCall(ToolCall::new(
                    id.clone(),
                    ToolFunction::new(name.clone(), args.clone()),
                )));
            }
            if !self.summary.is_empty() {
                contents.push(AssistantContent::text(self.summary.clone()));
            }
            let choice = match OneOrMany::many(contents) {
                Ok(choice) => choice,
                // Empty summary + no stray tool → a single empty-text turn, which
                // drives the honest "no summary produced" branch.
                Err(_) => OneOrMany::one(AssistantContent::text(String::new())),
            };
            return Ok(CompletionResponse {
                choice,
                usage: Usage::new(),
                raw_response: Raw::default(),
                message_id: None,
            });
        }
        // A tool round: emit the next scripted tool call.
        let (id, name, args) = self
            .tool_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .ok_or_else(|| {
                CompletionError::ProviderError("well-formed mock: no scripted tool call".to_string())
            })?;
        Ok(CompletionResponse {
            choice: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                id,
                ToolFunction::new(name, args),
            ))),
            usage: Usage::new(),
            raw_response: Raw::default(),
            message_id: None,
        })
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        // The tool-bearing subagent loop never streams (only the Synthesizer does),
        // and this model backs the Graph-Navigator — so a stream call is a test bug.
        unreachable!("WellFormedModel backs a tool-bearing subagent and is never streamed")
    }
}

// ── Criterion 1 + 2: soft close, well-formed, exactly-cap, bounded answer ─────

#[tokio::test]
async fn a_broad_step_reaching_its_cap_soft_closes_well_formed_and_the_turn_answers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // The planner routes one broad graph step, then finalizes from the bounded
    // observation the soft-capped subagent returns.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "map the whole call graph")),
        MockTurn::text(final_json(
            "The system centers on the alpha->beta->gamma chain (bounded, partial).",
        )),
    ]);

    // The Graph-Navigator eagerly fans out: it requests FOUR in-domain tool calls,
    // but the per-subagent cap is 3, so the fourth is refused and the step
    // soft-closes. The first three must be valid graph calls that actually run.
    let graph = WellFormedModel::new(
        [
            (
                "g1".to_string(),
                "callers".to_string(),
                serde_json::json!({ "symbol": "beta" }),
            ),
            (
                "g2".to_string(),
                "callees".to_string(),
                serde_json::json!({ "symbol": "beta" }),
            ),
            (
                "g3".to_string(),
                "callers".to_string(),
                serde_json::json!({ "symbol": "alpha" }),
            ),
            (
                "g4".to_string(),
                "callees".to_string(),
                serde_json::json!({ "symbol": "alpha" }),
            ),
        ],
        "From the three calls I made: beta is called by alpha; alpha is a root.",
    );
    let closing_well_formed = graph.closing_well_formed.clone();
    let saw_closing_round = graph.saw_closing_round.clone();

    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        // `RoleModels<M>` is homogeneous; the unused roles get an empty
        // never-called `WellFormedModel`. The planner is a separate model type.
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: WellFormedModel::new([], String::new()),
            source_reader: WellFormedModel::new([], String::new()),
            synthesizer: WellFormedModel::new([], String::new()),
        },
    );
    // Global ceiling ample (24), per-subagent cap 3 → the cap binds first, softly.
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 3, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("what is the purpose of this system?", &sink)
        .await
        .expect("a soft-capped broad step yields a grounded turn, not an error");

    // Criterion 1: a grounded answer, not a bare halt.
    assert_eq!(
        outcome,
        TurnOutcome::Answered(
            "The system centers on the alpha->beta->gamma chain (bounded, partial).".to_string()
        ),
    );
    assert!(!any_halted(&sink), "the per-subagent cap does not halt the turn");

    // Criterion 2: exactly-cap tool dispatches — three ran (global += 3), the
    // fourth was refused and charged nothing extra.
    assert_eq!(orchestrator.budget().global_used(), 3, "exactly-cap dispatches");

    // Criterion 2: the close-out left a well-formed conversation — the tool-free
    // closing round happened and its history answered every tool_use, so the
    // provider-shaped model accepted it rather than erroring.
    assert!(
        saw_closing_round.load(Ordering::SeqCst),
        "a tool-free summarization round ran after the cap"
    );
    assert!(
        closing_well_formed.load(Ordering::SeqCst),
        "every dangling tool_use was answered before summarization"
    );

    // The bounded observation is explicitly marked partial.
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "[bounded — reached the 3-tool-call subagent cap; this summary may be partial]"
        ) && s.contains("beta is called by alpha")),
        "the bounded observation is marked partial and carries the grounded summary: {summaries:?}"
    );
}

// ── Criterion 2 honesty edges: empty close-out + stray closing tool call ──────

#[tokio::test]
async fn a_capped_step_with_an_empty_closing_summary_reports_no_summary_produced() {
    // NFR-CC-04 honesty edge: a soft-capped subagent whose tool-free closing round
    // yields no text returns an explicit "no summary produced" observation — never
    // a fabricated finding and never an aborting failure; the turn still finalizes.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "explore")),
        MockTurn::text(final_json("done, though the step summarized nothing")),
    ]);
    // Cap 1: one dispatch, the second refused → soft close; the closing round
    // returns empty text (summary = "").
    let graph = WellFormedModel::new(
        [
            ("g1".to_string(), "callers".to_string(), serde_json::json!({ "symbol": "beta" })),
            ("g2".to_string(), "callees".to_string(), serde_json::json!({ "symbol": "beta" })),
        ],
        "",
    );
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph,
            governance_analyst: WellFormedModel::new([], String::new()),
            source_reader: WellFormedModel::new([], String::new()),
            synthesizer: WellFormedModel::new([], String::new()),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 1, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("explore broadly", &sink)
        .await
        .expect("an empty close-out is an observation, not an aborting failure");

    assert!(
        matches!(outcome, TurnOutcome::Answered(_)),
        "an empty bounded summary does not abort the turn: {outcome:?}"
    );
    let summaries = observed_summaries(&sink);
    assert!(
        summaries
            .iter()
            .any(|s| s == "[bounded — reached the 1-tool-call subagent cap; no summary produced]"),
        "the empty close-out is an honest 'no summary produced' note, never fabricated: {summaries:?}"
    );
    assert!(!any_halted(&sink));
}

#[tokio::test]
async fn a_stray_tool_call_on_the_closing_round_is_ignored_and_uncharged() {
    // CR-048 risk mitigation: the tool-free closing request offers no tools, so a
    // model that ignores the "no tools" directive and emits a tool call anyway has
    // it IGNORED — never dispatched, never budget-charged ([NFR-CC-04]). Only the
    // text is taken as the bounded summary.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "explore")),
        MockTurn::text(final_json("done")),
    ]);
    // Cap 1: one real dispatch, the second refused → soft close. The closing round
    // emits BOTH a stray tool call and text; the tool call must be ignored.
    let graph = WellFormedModel::new(
        [
            ("g1".to_string(), "callers".to_string(), serde_json::json!({ "symbol": "beta" })),
            ("g2".to_string(), "callees".to_string(), serde_json::json!({ "symbol": "beta" })),
        ],
        "Despite asking for another tool, here is what I found: beta is called by alpha.",
    )
    .with_closing_stray_tool("stray1", "impact", serde_json::json!({ "symbol": "beta" }));
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph,
            governance_analyst: WellFormedModel::new([], String::new()),
            source_reader: WellFormedModel::new([], String::new()),
            synthesizer: WellFormedModel::new([], String::new()),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 1, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("explore", &sink)
        .await
        .expect("a stray closing tool call is ignored, the turn completes");

    assert!(matches!(outcome, TurnOutcome::Answered(_)));
    // The stray closing tool call was NOT dispatched: exactly the one real in-loop
    // dispatch charged the global ceiling (cap = 1).
    assert_eq!(
        orchestrator.budget().global_used(),
        1,
        "the stray closing tool call is never dispatched or charged"
    );
    // Only the closing text became the bounded observation; the tool call left no trace.
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with(
            "[bounded — reached the 1-tool-call subagent cap; this summary may be partial]"
        ) && s.contains("beta is called by alpha")),
        "only the closing text is taken as the bounded summary: {summaries:?}"
    );
}

#[test]
fn the_well_formedness_check_rejects_a_dangling_tool_use() {
    // Guard the guard: the criterion-2 test asserts only the positive direction, so
    // prove `conversation_is_well_formed` actually REJECTS a dangling `tool_use` —
    // otherwise that provider-shaped assertion would be vacuous.
    let tool_call = |id: &str| Message::Assistant {
        id: None,
        content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
            id.to_string(),
            ToolFunction::new("callers".to_string(), serde_json::json!({ "symbol": "beta" })),
        ))),
    };

    // A dangling tool_use (no matching tool_result) is malformed.
    let dangling = OneOrMany::many([Message::user("find callers of beta"), tool_call("t1")])
        .expect("non-empty");
    assert!(
        !conversation_is_well_formed(&dangling),
        "an unanswered tool_use is rejected"
    );

    // The same conversation with the tool_use answered is well-formed.
    let answered = OneOrMany::many([
        Message::user("find callers of beta"),
        tool_call("t1"),
        Message::tool_result("t1", "beta is called by alpha"),
    ])
    .expect("non-empty");
    assert!(
        conversation_is_well_formed(&answered),
        "an answered tool_use is well-formed"
    );
}

// ── Criterion 3: hard halt → best-effort answer (non-empty) or bare (empty) ───

#[tokio::test]
async fn a_hard_global_halt_with_observations_answers_best_effort_marked_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // Global ceiling = 1. The first graph step spends it (one real call) and
    // summarizes normally; the planner then wants MORE tool work, but the global
    // ceiling is spent → hard halt → best-effort synthesis over the scratchpad.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "who calls beta?")),
        MockTurn::text(plan_json("graph_navigator", "now map everything else")),
    ]);
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g1", "callers", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("beta is called by alpha (from the graph)."),
    ]);
    let synthesizer = MockCompletionModel::new([MockTurn::text(
        "Best-effort grounded answer: beta is called by alpha.",
    )]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(1, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("who calls beta, and everything else?", &sink)
        .await
        .expect("a hard halt with observations answers best-effort, not an error");

    match outcome {
        TurnOutcome::Answered(answer) => {
            assert!(
                answer.starts_with(
                    "[bounded — global per-turn tool-call ceiling reached (1 calls);"
                ),
                "the best-effort answer is explicitly marked bounded: {answer}"
            );
            assert!(
                answer.contains("Best-effort grounded answer: beta is called by alpha."),
                "the answer is grounded in the tool-free synthesis: {answer}"
            );
        }
        other => panic!("expected a best-effort bounded answer, got {other:?}"),
    }
    assert_eq!(orchestrator.budget().global_used(), 1, "the one call the ceiling allowed");
    assert_eq!(synthesizer.request_count(), 1, "the final tool-free synthesis ran once");
    assert!(any_final_answer(&sink), "the best-effort answer rides the FinalAnswer event");
    assert!(!any_halted(&sink), "a best-effort answer is not a bare halt");
}

#[tokio::test]
async fn a_hard_global_halt_with_an_empty_scratchpad_is_an_honest_bare_halt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // Global ceiling = 0: the planner wants a tool step but no tool work is
    // possible and nothing has been gathered → an honest bare halt, never a
    // fabricated answer ([NFR-CC-04]).
    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("graph_navigator", "do anything"))]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([]),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(0, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator.run("q", &sink).await.expect("an honest halt is Ok");

    assert_eq!(
        outcome,
        TurnOutcome::Halted(BudgetBound::GlobalToolCalls { limit: 0 }),
    );
    assert!(any_halted(&sink), "a Halted event names the bound");
    assert!(!any_final_answer(&sink), "no answer is fabricated on a bare halt");
    assert!(
        !sink
            .events()
            .iter()
            .any(|e| matches!(e, OrchestratorEvent::AnswerDelta { .. })),
        "no bounded-answer preamble streams when nothing was gathered"
    );
}

#[tokio::test]
async fn a_hard_replans_halt_with_observations_answers_best_effort_marked_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // max_replans = 1 and a planner that never finalizes: after the initial plan
    // and one replan, a third plan trips the replan bound. Both executed steps
    // recorded observations, so the hard halt answers best-effort ([CR-048] A′).
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "round 0")),
        MockTurn::text(plan_json("graph_navigator", "round 1")),
        MockTurn::text(plan_json("graph_navigator", "round 2")),
    ]);
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g0", "callers", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("round 0: beta is called by alpha."),
        MockTurn::tool_call("g1", "callees", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("round 1: beta calls gamma."),
    ]);
    let synthesizer = MockCompletionModel::new([MockTurn::text(
        "Best-effort: alpha -> beta -> gamma.",
    )]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 1));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("map the graph", &sink)
        .await
        .expect("a replan-bound halt with observations answers best-effort");

    match outcome {
        TurnOutcome::Answered(answer) => {
            assert!(
                answer.starts_with("[bounded — max replans reached (1 replans);"),
                "the best-effort answer is marked bounded by the replan bound: {answer}"
            );
            assert!(answer.contains("Best-effort: alpha -> beta -> gamma."));
        }
        other => panic!("expected a best-effort bounded answer, got {other:?}"),
    }
    assert_eq!(synthesizer.request_count(), 1, "the final tool-free synthesis ran once");
    assert!(!any_halted(&sink), "a best-effort answer is not a bare halt");
}

// ── Criterion 4: termination — bounded steps drain the ceiling, then hard-halt ─

#[tokio::test]
async fn three_consecutive_bounded_steps_drain_the_global_ceiling_and_hard_halt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // Global ceiling 3, per-subagent cap 1 (the 24:8 = 3:1 structure, scaled so the
    // mock script stays short): each step makes exactly ONE call then soft-closes,
    // so three consecutive bounded steps drain the global ceiling. max_replans is
    // high, so the GLOBAL ceiling — not the replan bound — is what hard-halts.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "step 0")),
        MockTurn::text(plan_json("graph_navigator", "step 1")),
        MockTurn::text(plan_json("graph_navigator", "step 2")),
        // The planner wants a fourth tool step, but the ceiling is spent → hard halt.
        MockTurn::text(plan_json("graph_navigator", "step 3")),
    ]);
    // Per bounded step: one dispatched call, one refused (cap 1) → soft close, then
    // one tool-free summary. Three steps = three (tool, refused, summary) triples.
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("a1", "callers", serde_json::json!({ "symbol": "beta" })),
        MockTurn::tool_call("a2", "callees", serde_json::json!({ "symbol": "beta" })),
        MockTurn::text("step 0 summary"),
        MockTurn::tool_call("b1", "callers", serde_json::json!({ "symbol": "alpha" })),
        MockTurn::tool_call("b2", "callees", serde_json::json!({ "symbol": "alpha" })),
        MockTurn::text("step 1 summary"),
        MockTurn::tool_call("c1", "callers", serde_json::json!({ "symbol": "gamma" })),
        MockTurn::tool_call("c2", "callees", serde_json::json!({ "symbol": "gamma" })),
        MockTurn::text("step 2 summary"),
    ]);
    let synthesizer =
        MockCompletionModel::new([MockTurn::text("Best-effort from three bounded steps.")]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph.clone(),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner.clone(), roster, BudgetTree::new(3, 1, 10));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("map everything, exhaustively", &sink)
        .await
        .expect("the bounded turn terminates and answers best-effort");

    // Terminated with a best-effort answer marked bounded by the GLOBAL ceiling.
    match outcome {
        TurnOutcome::Answered(answer) => assert!(
            answer.starts_with("[bounded — global per-turn tool-call ceiling reached (3 calls);"),
            "the turn hard-halted on the drained global ceiling: {answer}"
        ),
        other => panic!("expected a bounded best-effort answer, got {other:?}"),
    }

    // Exactly three bounded steps ran, each marked partial — no more, no fewer.
    // The first two soft-close on the per-subagent cap (the global ceiling still
    // has room); the third's second call finds the ceiling exactly drained, so it
    // soft-closes on the global ceiling — still a bounded, marked observation.
    let summaries = observed_summaries(&sink);
    let bounded: Vec<&String> = summaries
        .iter()
        .filter(|s| s.starts_with("[bounded — "))
        .collect();
    assert_eq!(bounded.len(), 3, "exactly three consecutive bounded steps: {summaries:?}");
    assert_eq!(
        summaries
            .iter()
            .filter(|s| s.starts_with("[bounded — reached the 1-tool-call subagent cap"))
            .count(),
        2,
        "the first two steps soft-close on the per-subagent cap: {summaries:?}"
    );
    assert_eq!(
        summaries
            .iter()
            .filter(|s| s.starts_with("[bounded — reached the turn's 3-tool-call ceiling"))
            .count(),
        1,
        "the third step soft-closes on the drained global ceiling, marked accordingly: {summaries:?}"
    );

    // The global ceiling drained to exactly its limit — no fabricated/over-charged
    // tool call, and the loop did not run unbounded.
    assert_eq!(orchestrator.budget().global_used(), 3, "the ceiling drained exactly");
    // Termination: the planner ran a bounded number of rounds (four decisions:
    // three plans executed + the fourth that tripped the hard halt), never looping.
    assert_eq!(planner.request_count(), 4, "no unbounded planning loop");
    assert_eq!(synthesizer.request_count(), 1, "one final tool-free synthesis");
    assert!(any_final_answer(&sink));
}
