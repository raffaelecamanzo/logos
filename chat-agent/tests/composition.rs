//! The S-170 cross-story composition over the merged orchestrator stack
//! ([S-170], [S-174], [S-175], [FR-UI-20], [NFR-CC-04]).
//!
//! Two properties the SSE seam (S-170) must enforce in production, proven here at
//! the chat-agent layer with the offline mock substrate so they hold independent
//! of the web adapter:
//!
//! 1. **Fan-out** — one [`FanOut`] sink delivers every orchestrator event to
//!    several sinks at once (the streaming sink AND the persisting
//!    [`ScratchpadSink`]), in emission order, with no change to the loop.
//! 2. **Grounded synthesis** — with a [`MemoryGrounding`] wired onto the roster,
//!    the tool-less Synthesizer is prompted with the **persisted** per-turn
//!    scratchpad (the plan + prior subagent observations), so [S-175]'s "the
//!    Synthesizer uses the scratchpad in the final answer" is enforced, not just
//!    available as a seam.
//!
//! [S-170]: ../../docs/planning/journal.md#s-170-sse-streaming-and-intent-guarded-chat-post-routes
//! [S-174]: ../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::sync::{Arc, Mutex};

use agent_core::rig::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
};
use agent_core::rig::streaming::{
    RawStreamingChoice, StreamingCompletionResponse, StreamingResult,
};
use agent_core::rig::OneOrMany;
use agent_core::{MockCompletionModel, MockRawResponse, MockTurn, Sandbox};
use chat_agent::{
    BudgetTree, CapturingSink, EventSink, FanOut, MemoryGrounding, MemoryStore, Orchestrator,
    OrchestratorEvent, RoleModels, ScratchpadEntry, ScratchpadSink, StepRole, SubagentRoster,
    TurnOutcome,
};
use logos_core::Engine;
use tempfile::TempDir;

// ── A prompt-capturing CompletionModel ────────────────────────────────────────

/// A `rig` [`CompletionModel`] that **records each request** (serialized, so the
/// assertion is independent of `rig`'s message internals) and returns a fixed
/// reply. Lets a test inspect exactly what prompt a subagent received — here, that
/// the Synthesizer's prompt carries the persisted scratchpad's observations.
#[derive(Clone)]
struct CapturingModel {
    reply: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl CapturingModel {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The serialized requests this model has served, in order.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl CompletionModel for CapturingModel {
    type Response = MockRawResponse;
    type StreamingResponse = MockRawResponse;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::new("")
    }

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        // `CompletionRequest: Serialize`, so the whole prompt (preamble + chat
        // history, including the injected scratchpad) is captured verbatim.
        let dump = serde_json::to_string(&request).unwrap_or_default();
        self.requests.lock().unwrap().push(dump);
        Ok(CompletionResponse {
            choice: OneOrMany::one(AssistantContent::text(self.reply.clone())),
            usage: Usage::new(),
            raw_response: MockRawResponse::default(),
            message_id: None,
        })
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        // The tool-less Synthesizer drives its model through `stream` (FR-UI-19), so
        // capture the request here too — same verbatim dump as `completion` — and
        // stream the fixed reply as one chunk + the final-response marker.
        let dump = serde_json::to_string(&request).unwrap_or_default();
        self.requests.lock().unwrap().push(dump);
        let chunks: Vec<Result<RawStreamingChoice<MockRawResponse>, CompletionError>> = vec![
            Ok(RawStreamingChoice::Message(self.reply.clone())),
            Ok(RawStreamingChoice::FinalResponse(MockRawResponse::default())),
        ];
        let stream: StreamingResult<MockRawResponse> =
            Box::pin(futures::stream::iter(chunks));
        Ok(StreamingCompletionResponse::stream(stream))
    }
}

// ── 1. FanOut ─────────────────────────────────────────────────────────────────

/// A [`FanOut`] delivers every event to each member sink, in emission order — the
/// composition the SSE route uses to stream a turn AND persist its scratchpad in
/// one pass (S-170).
#[test]
fn fan_out_delivers_every_event_to_each_sink_in_order() {
    let a = CapturingSink::new();
    let b = CapturingSink::new();
    {
        let fan = FanOut::new(vec![&a as &dyn EventSink, &b as &dyn EventSink]);
        fan.emit(OrchestratorEvent::Plan {
            round: 0,
            steps: Vec::new(),
        });
        fan.emit(OrchestratorEvent::FinalAnswer {
            answer: "done".to_string(),
        });
    }
    // Both sinks observed the identical sequence — none was starved.
    assert_eq!(a.events(), b.events());
    assert_eq!(a.events().len(), 2);
    assert!(matches!(a.events()[0], OrchestratorEvent::Plan { .. }));
    assert!(matches!(a.events()[1], OrchestratorEvent::FinalAnswer { .. }));
}

/// `FanOut` handles the degenerate sink counts: a single sink receives the event
/// (no clone needed), and an empty fan-out is a no-op that does not panic.
#[test]
fn fan_out_handles_single_and_empty_sink_counts() {
    // Single sink: delivered.
    let only = CapturingSink::new();
    FanOut::new(vec![&only as &dyn EventSink]).emit(OrchestratorEvent::FinalAnswer {
        answer: "solo".to_string(),
    });
    assert_eq!(only.events().len(), 1);

    // Empty fan-out: emitting is a harmless no-op (no panic, nothing recorded).
    let empty = FanOut::new(Vec::new());
    empty.emit(OrchestratorEvent::FinalAnswer {
        answer: "dropped".to_string(),
    });
}

// ── 2. Grounded synthesis ───────────────────────────────────────────────────

/// A fixture project so the roster's tools construct over a real `Engine` (no tool
/// is actually called in this turn — the subagents reply with text directly).
fn fixture(dir: &std::path::Path) -> (Arc<Engine>, Arc<Sandbox>) {
    std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
    std::fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").expect("write fixture");
    let engine = Arc::new(Engine::open(dir));
    let sandbox = Arc::new(Sandbox::new(dir, std::iter::empty()).expect("sandbox"));
    (engine, sandbox)
}

fn two_step_plan() -> String {
    r#"{"action":"plan","steps":[
        {"role":"graph_navigator","instruction":"gather the facts"},
        {"role":"synthesizer","instruction":"compose the final answer"}
    ]}"#
    .to_string()
}

/// With a [`MemoryGrounding`] wired onto the roster, the Synthesizer's prompt
/// carries the **persisted** scratchpad — the plan and the prior Graph-Navigator
/// observation — so its answer is grounded on memory ([S-175] AC1), and the
/// [`FanOut`] both streamed (to the capturing sink) and persisted (via the
/// [`ScratchpadSink`]) the same turn.
#[tokio::test]
async fn synthesizer_is_grounded_on_the_persisted_scratchpad() {
    let dir = TempDir::new().unwrap();
    let (engine, sandbox) = fixture(dir.path());

    // A real thread + memory to hang the turn's scratchpad off (FK parent).
    let mut chat = chat_agent::ChatStore::open(dir.path()).unwrap();
    let thread = chat.create_thread("grounding test").unwrap();
    let memory = Arc::new(MemoryStore::open(dir.path()).unwrap());
    let turn = memory.next_turn(thread).unwrap();

    // The planner lays out a graph step then a synthesis step, then finalises.
    let planner = MockCompletionModel::new([
        MockTurn::text(two_step_plan()),
        MockTurn::text(r#"{"action":"final","answer":"grounded answer"}"#),
    ]);

    // The Graph-Navigator replies with a distinctive grounded observation (no tool
    // call); the Synthesizer captures the prompt it is given.
    let graph = CapturingModel::new("GRAPH_SENTINEL_OBS: alpha is the entry point");
    let synthesizer = CapturingModel::new("the synthesized answer");
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: graph,
            governance_analyst: CapturingModel::new(""),
            source_reader: CapturingModel::new(""),
            synthesizer: synthesizer.clone(),
        },
    )
    // The production grounding seam: the Synthesizer is prompted with this turn's
    // persisted scratchpad ([S-175] AC1).
    .with_synthesizer_grounding(Arc::new(MemoryGrounding::new(
        Arc::clone(&memory),
        thread,
        turn,
    )));

    let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));

    // The fan-out streams to `events` AND persists to the scratchpad in one pass —
    // exactly the S-170 composition, with no change to `Orchestrator::run`.
    let events = CapturingSink::new();
    let scratchpad = ScratchpadSink::new(&memory, thread, turn);
    let outcome = {
        let fan = FanOut::new(vec![&events as &dyn EventSink, &scratchpad as &dyn EventSink]);
        orchestrator
            .run("what is the entry point?", &fan)
            .await
            .expect("the grounded turn completes")
    };
    assert_eq!(
        outcome,
        TurnOutcome::Answered("grounded answer".to_string())
    );
    assert!(scratchpad.first_error().is_none(), "no scratchpad write failed");

    // Fan-out reached the streaming sink: it saw the plan, both step observations,
    // and the final answer.
    let observed_roles: Vec<StepRole> = events
        .events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::StepObserved { role, .. } => Some(role),
            _ => None,
        })
        .collect();
    assert_eq!(
        observed_roles,
        vec![StepRole::GraphNavigator, StepRole::Synthesizer],
        "the streaming sink observed both steps",
    );

    // Fan-out reached the persisting sink: the graph observation is in chat.db.
    let entries = memory.scratchpad(thread, turn).unwrap();
    assert!(
        entries.iter().any(|e| matches!(
            e,
            ScratchpadEntry::Observation { summary, .. } if summary.contains("GRAPH_SENTINEL_OBS")
        )),
        "the graph observation was persisted: {entries:?}",
    );

    // The crux: the Synthesizer's prompt carried the persisted scratchpad — both
    // the compose header and the prior observation — so its answer is grounded on
    // memory ([S-175] AC1), not just on the bare planner instruction.
    let synth_prompts = synthesizer.requests();
    assert_eq!(synth_prompts.len(), 1, "the synthesizer ran exactly once");
    let prompt = &synth_prompts[0];
    assert!(
        prompt.contains("GRAPH_SENTINEL_OBS"),
        "the Synthesizer was grounded on the persisted graph observation: {prompt}",
    );
    assert!(
        prompt.contains("Scratchpad"),
        "the grounding block was injected into the Synthesizer prompt: {prompt}",
    );
}
