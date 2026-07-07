//! The chat surface seam: stream an orchestrated turn to the browser over SSE
//! while persisting its scratchpad, holding **no** agent logic ([ADR-01],
//! [ADR-40], [S-170], [FR-UI-19]).
//!
//! The thin web adapter forwards a turn to the [`chat-agent`] orchestrator and
//! streams its [`OrchestratorEvent`]s back. The composition this module owns
//! (the cross-story S-170 job carried from the It4 review):
//!
//! - a [`FanOut`] sink fans every event to **both** the streaming [`SseSink`]
//!   (→ the client) **and** the [`ScratchpadSink`] (→ `.logos/chat.db` memory),
//!   in one pass, with **no change** to
//!   [`Orchestrator::run`](chat_agent::Orchestrator::run);
//! - the tool-less Synthesizer is grounded on that persisted memory via
//!   [`MemoryGrounding`] — so [S-175]'s "the Synthesizer uses the scratchpad in
//!   the final answer" holds in production, not just as an available seam.
//!
//! # Why SSE rides the intent-guarded `POST`, not a `GET` `EventSource`
//! A streaming chat turn **mutates** (consent-gated outbound egress + scratchpad
//! persistence), so [NFR-SE-06] requires it carry the same-origin **+ intent
//! token** proof every mutating route does. A `GET` `EventSource` cannot set the
//! custom [`INTENT_HEADER`](crate::INTENT_HEADER); consuming SSE over the
//! intent-guarded `POST` (via `fetch`) keeps that guard intact — the strongest
//! CSRF posture — while staying Server-Sent Events (text/event-stream),
//! same-origin, under the **unchanged** self-only CSP, with **no** WebSocket
//! ([FR-UI-19]). The architecture's "SSE GET" interface is met in substance and
//! strengthened on CSRF.
//!
//! # Honest teardown ([NFR-CC-04], [FR-UI-19])
//! The turn runs in a spawned task feeding an mpsc channel. The SSE body owns an
//! [`AbortOnDrop`] guard, so a client disconnect drops the body → aborts the task
//! → **cancels the in-flight turn**. Completion and an honest budget halt end the
//! channel cleanly; a provider/tool/persistence fault is surfaced as an honest
//! `error` event — never a fabricated answer.
//!
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
//! [S-170]: ../../../docs/planning/journal.md#s-170-sse-streaming-and-intent-guarded-chat-post-routes
//! [S-175]: ../../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
//! [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
//! [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`chat-agent`]: ../../../docs/specs/architecture/components/chat-agent.md

mod configured;

pub(crate) use configured::ConfiguredChatService;

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::response::sse;
use chat_agent::{
    EventSink, FanOut, MemoryStore, Orchestrator, OrchestratorEvent, ScratchpadSink, StepExecutor,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

use agent_core::rig::completion::CompletionModel;

/// One frame in a chat turn's stream: an orchestrator transition to relay, or an
/// honest terminal fault ([NFR-CC-04]) — never a fabricated answer.
#[derive(Debug, Clone)]
pub enum ChatFrame {
    /// An [`OrchestratorEvent`] to stream verbatim (plan / subagent activity /
    /// halt / final answer).
    Event(OrchestratorEvent),
    /// An honest fault (a provider/tool error, or a memory-persistence failure)
    /// surfaced to the client mid- or end-of-stream.
    Error(String),
}

/// Starts a chat turn and returns its live event stream — the seam that lets the
/// SSE route be driven by the config-resolved real provider in production
/// ([`ConfiguredChatService`]) and by the offline mock `CompletionModel` in tests
/// ([UAT-UI-07] zero real egress), with identical streaming/teardown machinery.
pub trait ChatService: Send + Sync + 'static {
    /// Start `question` on `thread_id` (a new thread when `None`). The turn runs
    /// in a spawned task; dropping the returned [`ChatStream`] cancels it
    /// ([FR-UI-19] client-disconnect → in-flight cancel). A configure-first or
    /// setup fault yields a single honest [`ChatFrame::Error`] and no spawned turn.
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream;
}

/// The live stream of a chat turn's [`ChatFrame`]s, backed by the spawned turn's
/// mpsc channel. Owns the [`AbortOnDrop`] guard, so dropping the stream (the SSE
/// body, on client disconnect) cancels the in-flight turn ([FR-UI-19]).
pub struct ChatStream {
    inner: UnboundedReceiverStream<ChatFrame>,
    /// Aborts the spawned turn on drop — ties the turn's lifetime to the stream's
    /// (client disconnect → in-flight cancel, [FR-UI-19]).
    _guard: AbortOnDrop,
}

impl ChatStream {
    /// Wrap a spawned turn's receiver and join handle (the abort guard ties the
    /// turn's lifetime to the stream's — disconnect → cancel, [FR-UI-19]).
    pub(crate) fn from_spawn(rx: mpsc::UnboundedReceiver<ChatFrame>, handle: JoinHandle<()>) -> Self {
        Self {
            inner: UnboundedReceiverStream::new(rx),
            _guard: AbortOnDrop(handle),
        }
    }

    /// Drain the whole turn into the buffered answer the no-JS / no-stream path
    /// renders ([FR-UI-19] progressive-enhancement fallback). Honest precedence:
    /// a fault or an honest budget halt is reported rather than a (missing or
    /// fabricated) answer ([NFR-CC-04]).
    pub(crate) async fn into_buffered(mut self) -> String {
        let mut answer: Option<String> = None;
        let mut halt: Option<String> = None;
        let mut error: Option<String> = None;
        while let Some(frame) = self.inner.next().await {
            match frame {
                ChatFrame::Event(OrchestratorEvent::FinalAnswer { answer: a }) => answer = Some(a),
                ChatFrame::Event(OrchestratorEvent::Halted { bound, .. }) => {
                    halt.get_or_insert_with(|| bound.to_string());
                }
                ChatFrame::Error(message) => {
                    error.get_or_insert(message);
                }
                ChatFrame::Event(_) => {}
            }
        }
        if let Some(error) = error {
            return format!("The chat turn could not complete: {error}");
        }
        if let Some(halt) = halt {
            return format!("The chat turn halted honestly: {halt}");
        }
        answer.unwrap_or_else(|| "The chat turn produced no answer.".to_string())
    }
}

impl Stream for ChatStream {
    type Item = ChatFrame;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// Aborts the spawned chat turn when dropped — the disconnect → in-flight-cancel
/// teardown ([FR-UI-19]). Aborting an already-finished turn is a harmless no-op,
/// so completion and disconnect share one path.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// An [`EventSink`] that forwards every orchestrator event into the turn's stream
/// channel ([S-170]). Send is non-blocking and infallible from `emit`'s view: once
/// the client disconnects the receiver is gone and sends are dropped, while the
/// [`AbortOnDrop`] guard is what actually cancels the turn.
pub(crate) struct SseSink {
    tx: mpsc::UnboundedSender<ChatFrame>,
}

impl SseSink {
    fn new(tx: mpsc::UnboundedSender<ChatFrame>) -> Self {
        Self { tx }
    }
}

impl EventSink for SseSink {
    fn emit(&self, event: OrchestratorEvent) {
        let _ = self.tx.send(ChatFrame::Event(event));
    }
}

/// Spawn an orchestrated turn and return its live [`ChatStream`] — the shared
/// machinery both [`ChatService`] impls drive ([S-170]).
///
/// The turn runs in a spawned task that fans every event to the streaming
/// [`SseSink`] **and** the [`ScratchpadSink`] persisting `(thread_id, turn)`'s
/// scratchpad to `memory` (the S-170 composition), then surfaces an honest fault
/// or a failed-to-persist note ([NFR-CC-04]). Dropping the returned stream aborts
/// the task ([FR-UI-19] client-disconnect → cancel).
pub fn spawn_turn<M, E>(
    orchestrator: Orchestrator<M, E>,
    question: String,
    memory: Arc<MemoryStore>,
    thread_id: i64,
    turn: i64,
) -> ChatStream
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    E: StepExecutor + 'static,
{
    // The orchestrator is already built, so there is no blocking setup to offload:
    // spawn the run directly. (The production path does blocking config/store setup
    // off the executor thread before building the orchestrator — see
    // `ConfiguredChatService`.)
    let (tx, rx) = unbounded_chat_channel();
    let handle = tokio::spawn(run_orchestrated(
        orchestrator,
        question,
        memory,
        thread_id,
        turn,
        tx,
    ));
    ChatStream::from_spawn(rx, handle)
}

/// The chat turn's event channel. **Unbounded by design:** [`EventSink::emit`] is
/// synchronous (no `await`), so a bounded sender's async back-pressure cannot be
/// applied from inside the orchestrator loop without restructuring the sink
/// contract. The accepted bound is the turn itself — the budget tree caps the
/// event count per turn, the surface is a single-user loopback listener, and a
/// client disconnect drops the receiver and aborts the turn ([`AbortOnDrop`]), so
/// events cannot accumulate unboundedly across turns or after a disconnect.
pub(crate) fn unbounded_chat_channel() -> (
    mpsc::UnboundedSender<ChatFrame>,
    mpsc::UnboundedReceiver<ChatFrame>,
) {
    mpsc::unbounded_channel::<ChatFrame>()
}

/// Drive one orchestrated turn to completion, fanning every event to the streaming
/// [`SseSink`] **and** the persisting [`ScratchpadSink`] (the S-170 composition),
/// then surfacing an honest fault or a failed-to-persist note ([NFR-CC-04]). The
/// shared task body for both [`spawn_turn`] (tests) and the production
/// [`ConfiguredChatService`], which run it after their own (possibly blocking)
/// setup.
pub(crate) async fn run_orchestrated<M, E>(
    orchestrator: Orchestrator<M, E>,
    question: String,
    memory: Arc<MemoryStore>,
    thread_id: i64,
    turn: i64,
    tx: mpsc::UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
    E: StepExecutor + 'static,
{
    // The scratchpad sink persists the turn's events to chat.db as they stream
    // (S-175); the SSE sink relays them to the client. The fan-out drives both
    // without the orchestrator loop knowing either exists.
    let scratchpad = ScratchpadSink::new(&memory, thread_id, turn);
    let sse = SseSink::new(tx.clone());
    let fan = FanOut::new(vec![&sse, &scratchpad]);

    if let Err(err) = orchestrator.run(&question, &fan).await {
        // A provider/parse/subagent fault — honest, never a fabricated answer. The
        // frame names the turn STAGE (planner / subagent / synthesis) and carries
        // the classified, source-chained cause from the orchestrator ([S-199],
        // [FR-UI-24]); this is the honest error contract S-200's surface renders.
        let _ = tx.send(ChatFrame::Error(format!(
            "Chat failed during the {} stage: {err}",
            err.stage()
        )));
    }
    // A memory-write failure is reported, never silently swallowed (NFR-CC-04).
    if let Some(err) = scratchpad.first_error() {
        let _ = tx.send(ChatFrame::Error(format!(
            "the turn streamed but its scratchpad failed to persist: {err}"
        )));
    }
    // `tx` (and the clone held by `sse`) drop here → the receiver ends → the SSE
    // stream closes cleanly on completion / honest halt.
}

/// The SSE `event:` name for an [`OrchestratorEvent`] — mirrors its serde tag
/// (`#[serde(tag = "event", rename_all = "snake_case")]`) so the browser can
/// `addEventListener` on the same discriminant the JSON payload carries.
fn event_name(event: &OrchestratorEvent) -> &'static str {
    match event {
        OrchestratorEvent::Plan { .. } => "plan",
        OrchestratorEvent::StepStarted { .. } => "step_started",
        OrchestratorEvent::StepObserved { .. } => "step_observed",
        OrchestratorEvent::Halted { .. } => "halted",
        OrchestratorEvent::AnswerDelta { .. } => "answer_delta",
        OrchestratorEvent::FinalAnswer { .. } => "final_answer",
    }
}

/// Render one [`ChatFrame`] as an SSE event: the orchestrator event tagged with
/// its discriminant and carrying its JSON body, or an honest `error` event.
pub(crate) fn frame_to_event(frame: ChatFrame) -> sse::Event {
    match frame {
        ChatFrame::Event(event) => {
            let name = event_name(&event);
            sse::Event::default()
                .event(name)
                .json_data(&event)
                // The orchestrator events are plain serde structs; a serialization
                // failure is not expected, but degrade honestly rather than panic.
                .unwrap_or_else(|_| {
                    sse::Event::default()
                        .event("error")
                        .data("event serialization failed")
                })
        }
        ChatFrame::Error(message) => sse::Event::default().event("error").data(message),
    }
}

/// Adapt a [`ChatStream`] into the `Result`-yielding stream `axum::response::Sse`
/// consumes, keeping the [`AbortOnDrop`] guard alive for the body's lifetime so a
/// client disconnect still cancels the turn ([FR-UI-19]).
pub(crate) fn sse_body(
    stream: ChatStream,
) -> impl Stream<Item = Result<sse::Event, std::convert::Infallible>> {
    stream.map(|frame| Ok(frame_to_event(frame)))
}
