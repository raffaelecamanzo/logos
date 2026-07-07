//! Tool errors become self-correcting observations, bounded by a soft-close cap
//! ([S-241], [CR-060] Layer 2, [FR-UI-28], [ADR-41], [NFR-CC-04]).
//!
//! These drive the **public** orchestrator API with an offline scripted
//! `CompletionModel` over the real subagent roster (S-174) on a fixture `Engine`
//! + `Sandbox`, proving the four S-241 acceptance criteria:
//!
//! 1. a tool error (`read` of a missing path → `DispatchError::Tool`) is fed back
//!    to the model as a `tool_result` observation and the subagent **adapts** on
//!    the next round instead of aborting the turn;
//! 2. a successful dispatch **resets** the consecutive-error streak, so
//!    non-consecutive errors never trip the cap;
//! 3. an out-of-domain request (`DispatchError::ToolNotFound`) is refused as a
//!    model-visible observation that **lists the subagent's available tools**, not
//!    a turn-fatal error;
//! 4. exceeding the consecutive-error cap **soft-closes** the step with a
//!    `[bounded — …consecutive tool errors…]` summary over a **well-formed**
//!    conversation — and a pure `ToolNotFound` loop that charges **no budget**
//!    still terminates via the cap.
//!
//! The [S-181]/[CR-048] budget-cap and global-ceiling soft-close paths are proven
//! unchanged by the untouched `tests/soft_cap.rs` suite; nothing is fabricated
//! ([NFR-CC-04]).
//!
//! [S-241]: ../../docs/planning/journal.md#s-241-tool-errors-become-self-correcting-observations-with-a-soft-close-cap
//! [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
//! [FR-UI-28]: ../../docs/specs/requirements/FR-UI-28.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [S-181]: ../../docs/planning/journal.md#s-181-soft-per-subagent-budget-cap-with-graceful-summarization-and-answer-on-halt
//! [CR-048]: ../../docs/requests/CR-048-soft-per-subagent-budget-cap.md
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
    BudgetTree, CapturingSink, Orchestrator, OrchestratorError, OrchestratorEvent, RoleModels,
    SubagentRoster, TurnOutcome,
};
use logos_core::Engine;
use serde::{Deserialize, Serialize};

/// A fixture project with an `alpha → beta → gamma` call chain so the graph tools
/// resolve real symbols/edges and the `read`/`glob` source tools see real files.
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

// ── A scripted, well-formedness-checking, tool-result-recording model ─────────

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

/// One scripted turn a [`FaultModel`] returns on a **tool round** (a request that
/// still offers tools).
#[derive(Clone)]
enum Turn {
    /// A tool call `(id, name, args)` — dispatched by the real bounded dispatcher.
    Call(String, String, serde_json::Value),
    /// Several tool calls in ONE assistant turn — `[(id, name, args), …]`. Lets a
    /// test leave a genuine **dangling** `tool_use` sibling (a call the loop never
    /// reaches because an earlier one trips the cap) for the soft-close to answer.
    Calls(Vec<(String, String, serde_json::Value)>),
    /// A plain-text reply — the subagent's grounded (self-corrected) observation.
    Text(String),
}

/// A scripted [`CompletionModel`] for the fault-resilience suite that, like a real
/// provider, **rejects a malformed conversation** (a dangling `tool_use`) on the
/// tool-free closing round, and additionally **records every `tool_result` text**
/// it is shown so a test can assert what the model actually saw (e.g. the
/// available-tools list in an out-of-domain refusal).
struct FaultModel {
    /// Scripted turns for successive tool rounds, consumed in order.
    turns: Mutex<VecDeque<Turn>>,
    /// Text returned on the tool-free closing round (empty → "no summary produced").
    closing_summary: String,
    /// Set true once a tool-free closing round is served.
    saw_closing_round: Arc<AtomicBool>,
    /// Whether the closing round's conversation was well-formed (every `tool_use`
    /// answered) — how a real provider decides to accept the summarization request.
    closing_well_formed: Arc<AtomicBool>,
    /// Every `tool_result` text the model has been shown, in arrival order — lets a
    /// test inspect the observations fed back to the model.
    seen_tool_results: Arc<Mutex<Vec<String>>>,
    /// Completion requests served (proves the mock — not a real provider — ran).
    /// `Arc`-shared so a clone taken before the model moves into the roster still
    /// observes the roster instance's running count.
    requests: Arc<AtomicUsize>,
}

impl FaultModel {
    fn new(turns: impl IntoIterator<Item = Turn>, closing_summary: impl Into<String>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            closing_summary: closing_summary.into(),
            saw_closing_round: Arc::new(AtomicBool::new(false)),
            closing_well_formed: Arc::new(AtomicBool::new(false)),
            seen_tool_results: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// An empty, never-called model for the roles a turn does not route to.
    fn idle() -> Self {
        Self::new([], String::new())
    }

    fn record_tool_results(&self, request: &CompletionRequest) {
        let mut seen = self.seen_tool_results.lock().unwrap_or_else(|p| p.into_inner());
        for message in request.chat_history.iter() {
            if let Message::User { content } = message {
                for item in content.iter() {
                    if let UserContent::ToolResult(tr) = item {
                        for part in tr.content.iter() {
                            if let agent_core::rig::message::ToolResultContent::Text(t) = part {
                                seen.push(t.text.clone());
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Clone for FaultModel {
    fn clone(&self) -> Self {
        Self {
            turns: Mutex::new(self.turns.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            closing_summary: self.closing_summary.clone(),
            saw_closing_round: self.saw_closing_round.clone(),
            closing_well_formed: self.closing_well_formed.clone(),
            seen_tool_results: self.seen_tool_results.clone(),
            requests: self.requests.clone(),
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

impl CompletionModel for FaultModel {
    type Response = Raw;
    type StreamingResponse = Raw;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::idle()
    }

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.record_tool_results(&request);

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
            let choice = if self.closing_summary.is_empty() {
                OneOrMany::one(AssistantContent::text(String::new()))
            } else {
                OneOrMany::one(AssistantContent::text(self.closing_summary.clone()))
            };
            return Ok(CompletionResponse {
                choice,
                usage: Usage::new(),
                raw_response: Raw::default(),
                message_id: None,
            });
        }

        // A tool round: emit the next scripted turn.
        let turn = self
            .turns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .ok_or_else(|| {
                CompletionError::ProviderError("fault mock: no scripted turn remaining".to_string())
            })?;
        let choice = match turn {
            Turn::Call(id, name, args) => OneOrMany::one(AssistantContent::ToolCall(
                ToolCall::new(id, ToolFunction::new(name, args)),
            )),
            Turn::Calls(calls) => {
                let items: Vec<AssistantContent> = calls
                    .into_iter()
                    .map(|(id, name, args)| {
                        AssistantContent::ToolCall(ToolCall::new(id, ToolFunction::new(name, args)))
                    })
                    .collect();
                OneOrMany::many(items).expect("Turn::Calls must be non-empty")
            }
            Turn::Text(text) => OneOrMany::one(AssistantContent::text(text)),
        };
        Ok(CompletionResponse {
            choice,
            usage: Usage::new(),
            raw_response: Raw::default(),
            message_id: None,
        })
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        // Only the tool-less Synthesizer streams; this model backs tool-bearing
        // subagents, so a stream call is a test bug.
        unreachable!("FaultModel backs a tool-bearing subagent and is never streamed")
    }
}

/// Build a roster whose tool-bearing roles are backed by `model` and whose other
/// roles are idle — the planner is a separate model passed to the orchestrator.
fn roster_with(
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    role: &str,
    model: FaultModel,
) -> SubagentRoster<FaultModel> {
    let mut models = RoleModels {
        graph_navigator: FaultModel::idle(),
        governance_analyst: FaultModel::idle(),
        source_reader: FaultModel::idle(),
        synthesizer: FaultModel::idle(),
    };
    match role {
        "graph_navigator" => models.graph_navigator = model,
        "source_reader" => models.source_reader = model,
        "governance_analyst" => models.governance_analyst = model,
        other => panic!("unexpected role {other}"),
    }
    SubagentRoster::with_models(engine, sandbox, models)
}

// ── Criterion 1: a tool error is an observation the subagent adapts from ──────

#[tokio::test]
async fn a_tool_error_becomes_an_observation_and_the_subagent_adapts_next_round() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // The planner routes one Source-Reader step, then finalizes from its grounded
    // (self-corrected) observation.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "what files define the call chain?")),
        MockTurn::text(final_json("The chain lives in src/lib.rs.")),
    ]);
    // The Source-Reader first reads a MISSING path (a real `DispatchError::Tool`),
    // then adapts — globs for the real files — then answers.
    let source = FaultModel::new(
        [
            Turn::Call("s1".into(), "read".into(), serde_json::json!({ "path": "README.md" })),
            Turn::Call("s2".into(), "glob".into(), serde_json::json!({ "pattern": "src/**/*.rs" })),
            Turn::Text("The call chain is defined in src/lib.rs.".into()),
        ],
        "",
    );
    let source_seen = source.seen_tool_results.clone();
    let roster = roster_with(engine, sandbox, "source_reader", source);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("what defines the call chain?", &sink)
        .await
        .expect("a tool error self-corrects — it never aborts the turn");

    // The turn answered, and the subagent's grounded observation is not bounded:
    // it adapted past the error rather than soft-closing.
    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.contains("src/lib.rs")),
        "the subagent's adapted, grounded observation is recorded: {summaries:?}"
    );
    assert!(
        summaries.iter().all(|s| !s.starts_with("[bounded —")),
        "a single, recovered-from tool error does not soft-close the step: {summaries:?}"
    );
    assert!(!any_halted(&sink));

    // The read error was fed back to the model as an observation it could adapt to
    // ([CR-060] Layer 2) — never silently swallowed.
    let seen = source_seen.lock().unwrap();
    assert!(
        seen.iter().any(|t| t.starts_with("error: tool `read` failed")),
        "the tool error was fed back as a self-correcting observation: {seen:?}"
    );
    // The successful `glob` charged exactly one global tool call; the failed `read`
    // never charged the global ceiling.
    assert_eq!(orchestrator.budget().global_used(), 1, "only the successful dispatch charges global");
}

// ── Exception: a sandbox containment refusal is turn-fatal, not routed around ──

#[tokio::test]
async fn a_source_sandbox_escape_is_turn_fatal_not_a_recoverable_observation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    // The planner routes a single Source-Reader step.
    let planner = MockCompletionModel::new([MockTurn::text(plan_json(
        "source_reader",
        "read a file outside the project root",
    ))]);
    // The Source-Reader attempts to `read` OUTSIDE the project root — a sandbox
    // containment refusal (CR-063), not a benign miss like `README.md`.
    let source = FaultModel::new(
        [Turn::Call(
            "s1".into(),
            "read".into(),
            serde_json::json!({ "path": "../../etc/passwd" }),
        )],
        "",
    );
    let roster = roster_with(engine, sandbox, "source_reader", source);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let err = orchestrator
        .run("read outside the root", &sink)
        .await
        .expect_err("a sandbox escape is turn-fatal — the turn aborts, it is not routed around");

    // Honest turn-fatal failure that NAMES the refusal (NFR-SE-04/NFR-CC-04, CR-063).
    match err {
        OrchestratorError::Step { message, .. } => assert!(
            message.contains("escapes the project root"),
            "the honest error names the sandbox refusal: {message}"
        ),
        other => panic!("expected a turn-fatal step failure, got {other:?}"),
    }
    // No answer is fabricated over the refused escape, and the step is NOT recorded
    // as a recoverable `[unavailable — …]`/soft-close observation.
    assert!(!any_final_answer(&sink), "no answer is fabricated on a refused escape");
    assert!(!any_halted(&sink), "a containment refusal is a hard failure, not a budget halt");
    assert!(
        observed_summaries(&sink)
            .iter()
            .all(|s| !s.starts_with("[unavailable —") && !s.starts_with("[bounded —")),
        "the escape is not degraded into a recoverable route-around observation",
    );
}

// ── Criterion 2: a successful dispatch resets the consecutive-error streak ─────

#[tokio::test]
async fn a_successful_dispatch_resets_the_consecutive_error_streak() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "survey the sources")),
        MockTurn::text(final_json("surveyed, with some read failures along the way")),
    ]);
    // FOUR tool errors, but never THREE in a row: a successful `glob` sits between
    // two error pairs. With the cap at 3 consecutive, the reset means the streak
    // peaks at 2 and the step NEVER soft-closes — it ends on a normal grounded
    // answer. Were the streak counting total (not consecutive) errors, the 3rd
    // error would have soft-closed the step instead.
    let source = FaultModel::new(
        [
            Turn::Call("e1".into(), "read".into(), serde_json::json!({ "path": "missing-1.md" })),
            Turn::Call("e2".into(), "read".into(), serde_json::json!({ "path": "missing-2.md" })),
            Turn::Call("ok".into(), "glob".into(), serde_json::json!({ "pattern": "src/**/*.rs" })),
            Turn::Call("e3".into(), "read".into(), serde_json::json!({ "path": "missing-3.md" })),
            Turn::Call("e4".into(), "read".into(), serde_json::json!({ "path": "missing-4.md" })),
            Turn::Text("Despite four missing reads, src/lib.rs holds the chain.".into()),
        ],
        "",
    );
    let roster = roster_with(engine, sandbox, "source_reader", source);
    // Per-subagent cap 8: a `Tool` error charges the per-subagent budget, so 4
    // errors + 1 success = 5 charges must stay under the cap (else the budget cap,
    // not the streak, would drive the close-out — defeating the test's purpose).
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("survey the sources", &sink)
        .await
        .expect("interleaved errors never trip the consecutive-error cap");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.contains("src/lib.rs")) && summaries.iter().all(|s| !s.starts_with("[bounded —")),
        "the reset kept the step from soft-closing despite four total errors: {summaries:?}"
    );
    assert!(!any_halted(&sink));
    assert_eq!(orchestrator.budget().global_used(), 1, "only the one successful glob charged global");
}

// ── Criterion 3: an out-of-domain request refusal lists the available tools ────

#[tokio::test]
async fn an_out_of_domain_request_is_refused_as_an_observation_listing_the_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "map beta")),
        MockTurn::text(final_json("beta sits mid-chain.")),
    ]);
    // The Graph-Navigator misroutes to `read` (a SOURCE tool outside its domain);
    // the refusal is fed back as an observation, then it adapts with a text answer.
    let graph = FaultModel::new(
        [
            Turn::Call("g1".into(), "read".into(), serde_json::json!({ "path": "src/lib.rs" })),
            Turn::Text("From the graph, beta is called by alpha and calls gamma.".into()),
        ],
        "",
    );
    let graph_seen = graph.seen_tool_results.clone();
    let roster = roster_with(engine, sandbox, "graph_navigator", graph);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("map beta", &sink)
        .await
        .expect("an out-of-domain request is an observation, never a turn-fatal error");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers: {outcome:?}");
    // The refusal was a model-visible observation that NAMES the misrouted tool and
    // LISTS the subagent's actual domain (the graph tools), so it can reroute.
    let seen = graph_seen.lock().unwrap();
    let refusal = seen
        .iter()
        .find(|t| t.contains("not one of your available tools"))
        .expect("the out-of-domain refusal was fed back as an observation");
    assert!(refusal.contains("read"), "names the misrouted tool: {refusal}");
    for tool in ["search", "context", "node", "callers", "callees", "impact", "explore", "affected"] {
        assert!(refusal.contains(tool), "lists the graph tool `{tool}`: {refusal}");
    }
    // A refused misroute charges nothing (S-174 least privilege preserved).
    assert_eq!(orchestrator.budget().global_used(), 0);
}

// ── Criterion 4: the cap soft-closes well-formed; a no-budget loop terminates ──

#[tokio::test]
async fn exceeding_the_consecutive_error_cap_soft_closes_well_formed_and_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "read the docs")),
        MockTurn::text(final_json("Bounded: the docs could not be read.")),
    ]);
    // Three consecutive `read` failures (missing paths) hit the cap → soft close.
    let source = FaultModel::new(
        [
            Turn::Call("r1".into(), "read".into(), serde_json::json!({ "path": "doc-1.md" })),
            Turn::Call("r2".into(), "read".into(), serde_json::json!({ "path": "doc-2.md" })),
            Turn::Call("r3".into(), "read".into(), serde_json::json!({ "path": "doc-3.md" })),
        ],
        "I could not read any of the requested docs; nothing usable was gathered.",
    );
    let saw_closing = source.saw_closing_round.clone();
    let well_formed = source.closing_well_formed.clone();
    let roster = roster_with(engine, sandbox, "source_reader", source);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("read the docs", &sink)
        .await
        .expect("a capped error streak soft-closes into a bounded observation, not a fatal error");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers best-effort: {outcome:?}");
    assert!(!any_halted(&sink), "the streak cap does not hard-halt the turn");

    // The soft-close ran a tool-free summarization over a WELL-FORMED conversation
    // (every erroring tool_use answered by its error tool_result).
    assert!(saw_closing.load(Ordering::SeqCst), "a tool-free summarization round ran after the cap");
    assert!(
        well_formed.load(Ordering::SeqCst),
        "every dangling tool_use was answered before the bounded summarization"
    );

    // The bounded observation names the consecutive-tool-error bound and carries the
    // grounded summary — never a fabricated finding ([NFR-CC-04]).
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with("[bounded — hit 3 consecutive tool errors")
            && s.contains("could not read any of the requested docs")),
        "a bounded 'consecutive tool errors' summary is recorded: {summaries:?}"
    );
}

#[tokio::test]
async fn a_pure_tool_not_found_loop_charges_no_budget_yet_still_terminates_via_the_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "explore beta")),
        MockTurn::text(final_json("Bounded: the navigator kept misrouting.")),
    ]);
    // The Graph-Navigator relentlessly requests `read` — a SOURCE tool outside its
    // domain. Each is refused with NO budget charge, so the budget bounds can never
    // bite: only the consecutive-error streak cap terminates the step.
    let graph = FaultModel::new(
        [
            Turn::Call("m1".into(), "read".into(), serde_json::json!({ "path": "a" })),
            Turn::Call("m2".into(), "read".into(), serde_json::json!({ "path": "b" })),
            Turn::Call("m3".into(), "read".into(), serde_json::json!({ "path": "c" })),
        ],
        "I kept requesting a tool outside my domain and gathered nothing.",
    );
    let graph_requests_model = graph.clone();
    let saw_closing = graph.saw_closing_round.clone();
    let well_formed = graph.closing_well_formed.clone();
    let roster = roster_with(engine, sandbox, "graph_navigator", graph);
    // A GENEROUS global ceiling and per-subagent cap: if termination depended on a
    // budget bound this loop would run to the mock's exhaustion and error — it must
    // instead terminate purely on the streak cap.
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(64, 64, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("explore beta", &sink)
        .await
        .expect("a no-budget ToolNotFound loop terminates via the streak cap");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers best-effort: {outcome:?}");
    assert!(!any_halted(&sink));
    // The sole termination guarantee did its job with ZERO budget charged.
    assert_eq!(
        orchestrator.budget().global_used(),
        0,
        "a pure ToolNotFound loop charges no budget — the streak cap is what terminates it"
    );
    // It terminated at the cap: exactly three misroute rounds then the closing
    // round — not run to the mock's exhaustion.
    assert_eq!(
        graph_requests_model.requests.load(Ordering::SeqCst),
        4,
        "three misroute rounds + one tool-free closing round, bounded by the cap"
    );
    assert!(saw_closing.load(Ordering::SeqCst), "the soft-close summarization round ran");
    assert!(well_formed.load(Ordering::SeqCst), "the refused tool_uses were all answered");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with("[bounded — hit 3 consecutive tool errors")),
        "the loop soft-closed on the consecutive-error cap: {summaries:?}"
    );
}

#[tokio::test]
async fn the_error_cap_soft_close_answers_a_dangling_sibling_tool_use_from_the_same_round() {
    // When the cap trips on the FIRST tool call of a multi-call assistant round, the
    // remaining call(s) in that round are genuine dangling `tool_use`s the soft-close
    // must answer via `close_and_summarize` (the `&tool_calls[idx + 1..]` slice is
    // non-empty). This makes the well-formedness assertion non-vacuous: were the
    // dangling-answering broken, the sibling would be unanswered, the provider-shaped
    // mock would reject the closing round, and the turn would fail instead of answer.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "explore beta")),
        MockTurn::text(final_json("Bounded: the navigator kept misrouting.")),
    ]);
    // Two single misroutes drive the streak to 2, then a round emitting TWO
    // out-of-domain `read` calls: the first trips the cap at 3 → the second is left
    // dangling for the soft-close to answer.
    let graph = FaultModel::new(
        [
            Turn::Call("m1".into(), "read".into(), serde_json::json!({ "path": "a" })),
            Turn::Call("m2".into(), "read".into(), serde_json::json!({ "path": "b" })),
            Turn::Calls(vec![
                ("m3a".into(), "read".into(), serde_json::json!({ "path": "c" })),
                ("m3b".into(), "read".into(), serde_json::json!({ "path": "d" })),
            ]),
        ],
        "I kept requesting a tool outside my domain and gathered nothing usable.",
    );
    let saw_closing = graph.saw_closing_round.clone();
    let well_formed = graph.closing_well_formed.clone();
    let roster = roster_with(engine, sandbox, "graph_navigator", graph);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(64, 64, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("explore beta", &sink)
        .await
        .expect("the soft-close answers the dangling sibling and the turn completes");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn answers best-effort: {outcome:?}");
    assert!(saw_closing.load(Ordering::SeqCst), "a tool-free summarization round ran");
    // Non-vacuous: the dangling sibling `m3b` (never dispatched) was answered ONLY by
    // `close_and_summarize`'s synthetic tool_result — otherwise this would be false.
    assert!(
        well_formed.load(Ordering::SeqCst),
        "the soft-close answered the dangling sibling tool_use, leaving a well-formed conversation"
    );
    assert_eq!(orchestrator.budget().global_used(), 0, "all calls were out-of-domain — nothing charged");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries.iter().any(|s| s.starts_with("[bounded — hit 3 consecutive tool errors")),
        "the step soft-closed on the consecutive-error cap: {summaries:?}"
    );
}

#[tokio::test]
async fn an_empty_error_cap_close_out_reports_no_summary_produced_never_fabricated() {
    // NFR-CC-04 honesty edge for the NEW close reason: a `CloseReason::ToolErrors`
    // soft-close whose tool-free summarization yields no text returns an explicit
    // "no summary produced" observation — never a fabricated finding — mirroring the
    // same edge already covered for the budget-cap reason in `soft_cap.rs`.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());

    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("source_reader", "read the docs")),
        MockTurn::text(final_json("done, though the step summarized nothing")),
    ]);
    // Three consecutive `read` failures trip the cap; the closing round returns
    // empty text (summary = ""), driving the honest "no summary produced" branch.
    let source = FaultModel::new(
        [
            Turn::Call("r1".into(), "read".into(), serde_json::json!({ "path": "doc-1.md" })),
            Turn::Call("r2".into(), "read".into(), serde_json::json!({ "path": "doc-2.md" })),
            Turn::Call("r3".into(), "read".into(), serde_json::json!({ "path": "doc-3.md" })),
        ],
        "",
    );
    let roster = roster_with(engine, sandbox, "source_reader", source);
    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("read the docs", &sink)
        .await
        .expect("an empty error-cap close-out is an honest observation, not an aborting failure");

    assert!(matches!(outcome, TurnOutcome::Answered(_)), "the turn still finalizes: {outcome:?}");
    let summaries = observed_summaries(&sink);
    assert!(
        summaries
            .iter()
            .any(|s| s == "[bounded — hit 3 consecutive tool errors; no summary produced]"),
        "the empty close-out is an honest 'no summary produced' note, never fabricated: {summaries:?}"
    );
    assert!(!any_halted(&sink));
}
