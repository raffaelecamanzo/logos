//! The chat surface seam: stream an orchestrated turn to the browser over SSE
//! while persisting its scratchpad, holding **no** agent logic ([ADR-01],
//! [ADR-40], [S-170], [FR-UI-19]).
//!
//! The thin web adapter forwards a turn to the [`chat-agent`] orchestrator and
//! streams its [`OrchestratorEvent`]s back. The composition this module owns
//! (the cross-story S-170 job carried from the It4 review):
//!
//! - a [`FanOut`] sink fans every event to the streaming [`SseSink`] (→ the
//!   client), the [`ScratchpadSink`] (→ `.logos/chat.db` memory) and the
//!   [`TitleSink`] (→ the title a first turn names), in one pass, with **no
//!   change** to [`Orchestrator::run`](chat_agent::Orchestrator::run);
//! - the tool-less Synthesizer is grounded on that persisted memory via
//!   [`MemoryGrounding`] — so [S-175]'s "the Synthesizer uses the scratchpad in
//!   the final answer" holds in production, not just as an available seam;
//! - a turn that genuinely [`Answered`](chat_agent::TurnOutcome::Answered) also
//!   appends that answer to the conversation's durable `chat_messages` transcript,
//!   beside the user's question ([FR-UI-26] AC-2) — the scratchpad is per-turn and
//!   invisible to the rail, so without this a restored conversation replayed the
//!   questions and none of the answers;
//! - the turn that opened its conversation stores the Synthesizer's title
//!   ([FR-UI-48]) and only then streams `thread_title`, after `final_answer`.
//!
//! [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
//!
//! # Why SSE rides the intent-guarded `POST`, not a `GET` `EventSource`
//! A streaming chat turn **mutates** (consent-gated outbound egress + scratchpad
//! persistence), so [NFR-SE-06] requires it carry the same-origin **+ intent
//! token** proof every mutating route does. A `GET` `EventSource` cannot set the
//! custom [`INTENT_HEADER`](web::INTENT_HEADER); consuming SSE over the
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
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md
//! [S-170]: ../../docs/planning/journal.md#s-170-sse-streaming-and-intent-guarded-chat-post-routes
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
//! [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
//! [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [`chat-agent`]: ../../docs/specs/architecture/components/chat-agent.md

mod configured;
mod workspace;

pub(crate) use configured::ConfiguredChatService;
pub(crate) use workspace::WorkspaceChatService;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::response::sse;
use chat_agent::{
    title_from_first_message, ChatRole, ChatStore, EventSink, FanOut, MemoryStore, Orchestrator,
    OrchestratorError, OrchestratorEvent, Preset, ScratchpadSink, StepExecutor, TurnOutcome,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

use agent_core::rig::completion::CompletionModel;
use logos_core::config::{ChatOrigin, ChatResolution};

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

/// The provider model and raw key a turn dials with, taken from a usable
/// [`ChatResolution`] — the only way the turn path reaches either value.
pub(crate) struct TurnProvider {
    /// The effective `[chat] model` (non-blank by the seam's invariant).
    pub(crate) model_id: String,
    /// The effective raw API key — egress only, never rendered ([NFR-SE-07]).
    ///
    /// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
    pub(crate) api_key: String,
}

/// The turn's readiness verdict over the one resolution seam ([ADR-67] §6,
/// [FR-UI-18]): the provider to dial, or the **configure-first** message a refused
/// turn streams as its single frame ([NFR-CC-04]).
///
/// The verdict is read off the two **origins** — exactly the facts the config
/// read-model serializes for the tab — and never off the file contents, so the
/// tab and the turn cannot disagree: a half is usable exactly when its origin is
/// not [`ChatOrigin::Unset`]. There is no second model-or-key check here; the
/// seam already decided that a blank `model` or a blank key is undeclared.
///
/// A refusal names the facts the tab's state names: the `member_root` it
/// inspected (and the workspace root, when one was consulted), the half or halves
/// that are absent, and where any present half came from.
///
/// [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
/// [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn turn_provider(
    member_root: &Path,
    workspace_root: Option<&Path>,
    resolution: &ChatResolution,
) -> Result<TurnProvider, String> {
    usable_provider(resolution)
        .ok_or_else(|| configure_first_message(member_root, workspace_root, resolution))
}

/// The provider a resolution can dial, or `None` for the configure-first state —
/// the verdict read off the two origins alone. Shared by the member chat's
/// [`turn_provider`] and the workspace chat ([`workspace`]), which differ only in
/// how they word a refusal.
pub(crate) fn usable_provider(resolution: &ChatResolution) -> Option<TurnProvider> {
    let policy_set = resolution.policy_origin != ChatOrigin::Unset;
    let credential_set = resolution.credential_origin != ChatOrigin::Unset;
    match (resolution.policy.model.as_deref(), resolution.api_key()) {
        (Some(model_id), Some(api_key)) if policy_set && credential_set => Some(TurnProvider {
            model_id: model_id.to_string(),
            api_key: api_key.to_string(),
        }),
        _ => None,
    }
}

/// The configure-first frame text: the root inspected, the absent half (or both),
/// the origin of a present half, a member key withheld from the inherited
/// workspace endpoint ([ADR-67] §2), and the Config-tab action that fixes it
/// ([FR-UI-18], [NFR-CC-04]).
fn configure_first_message(
    member_root: &Path,
    workspace_root: Option<&Path>,
    resolution: &ChatResolution,
) -> String {
    let where_looked = match workspace_root {
        Some(ws) => format!(" by this member or by the workspace root {}", ws.display()),
        None => String::new(),
    };
    let present = |half: &str, origin: ChatOrigin| match (origin, workspace_root) {
        (ChatOrigin::Workspace, Some(ws)) => {
            format!(
                "; the {half} is inherited from the workspace root {}",
                ws.display()
            )
        }
        _ => format!("; the {half} is declared by {}", member_root.display()),
    };
    let (absent, present, action) = match (resolution.policy_origin, resolution.credential_origin) {
        (ChatOrigin::Unset, ChatOrigin::Unset) => (
            format!("neither a provider model nor an API key is declared{where_looked}"),
            String::new(),
            "Choose a provider model and add an API key",
        ),
        (ChatOrigin::Unset, origin) => (
            format!("no provider model is declared{where_looked}"),
            present("API key", origin),
            "Choose a provider model",
        ),
        // The policy is inherited, so its endpoint takes the workspace root's key
        // alone ([ADR-67] §2): only the workspace root can supply the absent key,
        // and a member key — declared (withheld) or yet to be added — is used only
        // once the member owns its policy.
        (ChatOrigin::Workspace, _) => (
            match workspace_root {
                Some(ws) => format!("no API key is declared by the workspace root {}", ws.display()),
                None => "no API key is declared".to_string(),
            },
            format!(
                "{}; {}",
                present("provider model", ChatOrigin::Workspace),
                if resolution.member_key_withheld {
                    "this member's own API key is not used with the inherited workspace \
                     endpoint — setting a [chat] model on this member makes it use its own key"
                } else {
                    "an API key added to this member is not used with the inherited workspace \
                     endpoint — it is used once this member declares its own [chat] model"
                }
            ),
            if resolution.member_key_withheld {
                "Choose a provider model"
            } else {
                "Choose a provider model and add an API key"
            },
        ),
        (origin, _) => (
            format!("no API key is declared{where_looked}"),
            present("provider model", origin),
            "Add an API key",
        ),
    };
    format!(
        "Chat is not configured yet for {} — {absent}{present}. {action} in the Config tab \
         before starting a turn.",
        member_root.display()
    )
}

/// The honest setup-fault text for a chat-resolution read failure — the one
/// spelling the web adapter keeps for every agent that reads the `resolve_chat`
/// seam (the wiki generator too), so the two cannot word a fault differently.
pub(crate) use web::config_resolution_fault as resolution_fault;

/// Starts a chat turn and returns its live event stream — the seam that lets the
/// SSE route be driven by the config-resolved real provider in production
/// ([`ConfiguredChatService`]) and by the offline mock `CompletionModel` in tests
/// ([UAT-UI-07] zero real egress), with identical streaming/teardown machinery.
pub trait ChatService: Send + Sync + 'static {
    /// Start `question` on `thread_id` (a new thread when `None`) under `preset`,
    /// which steers the planner and Synthesizer preambles and nothing else
    /// ([FR-UI-49]). The turn runs in a spawned task; dropping the returned
    /// [`ChatStream`] cancels it ([FR-UI-19] client-disconnect → in-flight cancel).
    /// A configure-first or setup fault yields a single honest
    /// [`ChatFrame::Error`] and no spawned turn.
    ///
    /// [FR-UI-49]: ../../docs/specs/requirements/FR-UI-49.md
    fn start_turn(&self, question: String, thread_id: Option<i64>, preset: Preset) -> ChatStream;
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
        // The conversation title is streamed by `run_orchestrated` once it is
        // stored ([FR-UI-48]: strip, store, then emit), never straight from the run.
        //
        // [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
        if matches!(event, OrchestratorEvent::ThreadTitle { .. }) {
            return;
        }
        let _ = self.tx.send(ChatFrame::Event(event));
    }
}

/// Where one turn's memory and durable transcript are written: the project root
/// holding `.logos/chat.db`, the conversation the turn appends to, and its
/// per-thread turn ordinal (the scratchpad's scope).
///
/// Carried as one value so the turn machinery keeps a readable signature and the
/// two write destinations — the per-turn scratchpad and the conversation's
/// `chat_messages` transcript — can never be addressed with mismatched ids.
#[derive(Debug, Clone)]
pub struct TurnTarget {
    /// The project root whose `.logos/chat.db` holds the conversation store.
    pub root: PathBuf,
    /// The conversation the turn appends to.
    pub thread_id: i64,
    /// The per-thread turn ordinal scoping this turn's scratchpad.
    pub turn: i64,
    /// Whether this turn created `thread_id` — its first turn, the only one whose
    /// Synthesizer titles the conversation ([FR-UI-48]). Set by
    /// [`opening_thread`](Self::opening_thread); a later turn never retitles.
    ///
    /// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
    pub opens_thread: bool,
}

impl TurnTarget {
    /// The target for `turn` of `thread_id` under `root`.
    #[must_use]
    pub fn new(root: PathBuf, thread_id: i64, turn: i64) -> Self {
        Self {
            root,
            thread_id,
            turn,
            opens_thread: false,
        }
    }

    /// Mark the target as the conversation this turn just created, so the turn
    /// asks the Synthesizer for its title ([FR-UI-48]).
    ///
    /// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
    #[must_use]
    pub fn opening_thread(mut self) -> Self {
        self.opens_thread = true;
        self
    }
}

/// Captures the [`OrchestratorEvent::ThreadTitle`] a turn emits, so the turn can
/// write it to `chat_threads.title` once the run ends ([FR-UI-48]). The SSE sink
/// beside it has already streamed the same event to the client.
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
#[derive(Default)]
struct TitleSink(std::sync::Mutex<Option<String>>);

impl TitleSink {
    /// The title the turn named, if it named one.
    fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

impl EventSink for TitleSink {
    fn emit(&self, event: OrchestratorEvent) {
        if let OrchestratorEvent::ThreadTitle { title, .. } = event {
            *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(title);
        }
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
    target: TurnTarget,
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
    let handle = tokio::spawn(run_orchestrated(orchestrator, question, memory, target, tx));
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
/// [`SseSink`], the persisting [`ScratchpadSink`] (the S-170 composition) and the
/// [`TitleSink`], then appending a genuine final answer to the conversation's
/// durable transcript, storing a first turn's title (and only then streaming
/// `thread_title`, [FR-UI-48]), and surfacing an honest fault or a
/// failed-to-persist note ([NFR-CC-04]). The
/// shared task body for both [`spawn_turn`] (tests) and the production
/// [`ConfiguredChatService`], which run it after their own (possibly blocking) setup.
///
/// The durable append lives **here**, not in either caller, so the mock-provider
/// path and the real provider path persist a turn through the *same* code — what
/// makes the `chat_messages` round-trip an honest regression gate rather than a
/// test fixture ([FR-UI-26] AC-2).
pub(crate) async fn run_orchestrated<M, E>(
    orchestrator: Orchestrator<M, E>,
    question: String,
    memory: Arc<MemoryStore>,
    target: TurnTarget,
    tx: mpsc::UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
    E: StepExecutor + 'static,
{
    // The turn that created its conversation asks the Synthesizer to title it
    // ([FR-UI-48]) — here, once, so the member and workspace chats and the mock
    // path cannot differ on which turn that is.
    let orchestrator = if target.opens_thread {
        orchestrator.titling(target.thread_id)
    } else {
        orchestrator
    };
    // The scratchpad sink persists the turn's events to chat.db as they stream
    // (S-175); the SSE sink relays them to the client; the title sink keeps the
    // conversation title the turn named. The fan-out drives all three without the
    // orchestrator loop knowing any of them exists.
    let scratchpad = ScratchpadSink::new(&memory, target.thread_id, target.turn);
    let sse = SseSink::new(tx.clone());
    let titles = TitleSink::default();
    let fan = FanOut::new(vec![&sse, &scratchpad, &titles]);

    let outcome = orchestrator.run(&question, &fan).await;
    if let Err(err) = &outcome {
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
    // The turn's ANSWER joins the user's question in the conversation's durable
    // transcript ([FR-UI-26] AC-2) — but ONLY for a turn that genuinely produced
    // one. `Answered` is exactly the outcome that emitted the terminal
    // `FinalAnswer` (its `String` is that event's authoritative full text, so the
    // S-297 streaming Synthesizer needs no delta accumulation here); a bare
    // `Halted` and an `Err` fault carry no answer and persist nothing rather than
    // leave a fabricated assistant row behind ([NFR-CC-04]). A failed append is
    // reported for the same reason the scratchpad's is — the client is told its
    // answer did not reach the transcript.
    if let Some(answer) = durable_answer(outcome) {
        let append = write_conversation(&target, move |store, thread_id| {
            store.append_message(thread_id, ChatRole::Assistant, &answer, &[]).map(drop)
        });
        if let Err(err) = append.await {
            let _ = tx.send(ChatFrame::Error(format!(
                "the turn answered but the answer failed to persist to the conversation: {err}"
            )));
        }
    }
    // The Synthesizer's title replaces the first-message title ([FR-UI-48]), and
    // only once it is stored does `thread_title` stream — after `final_answer`, as
    // [FR-UI-19] places it, and before the stream closes, so the rail's re-read
    // after the turn already shows it.
    //
    // It replaces only the title the thread was created with: a rename the user
    // made while this turn ran is theirs, and is kept (no event then either).
    if let Some(title) = titles.take() {
        let stored = title.clone();
        let fallback = title_from_first_message(&question);
        let retitle = write_conversation(&target, move |store, thread_id| {
            store.replace_title(thread_id, &fallback, &stored)
        });
        match retitle.await {
            Ok(true) => {
                let _ = tx.send(ChatFrame::Event(OrchestratorEvent::ThreadTitle {
                    thread_id: target.thread_id,
                    title,
                }));
            }
            Ok(false) => {}
            // A title that cannot be stored degrades to the deterministic one the
            // thread already carries ([FR-UI-48] → [NFR-CC-04]). The turn answered:
            // an `error` frame would mark the answer failed (and replace it on the
            // buffered path), so the fault is logged and no title is streamed.
            //
            // [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
            Err(err) => {
                tracing::warn!(
                    target: "logos::web",
                    "chat thread {}: the Synthesizer's title was not stored, the fallback title stays: {err:#}",
                    target.thread_id
                );
            }
        }
    }
    // `tx` (and the clone held by `sse`) drop here → the receiver ends → the SSE
    // stream closes cleanly on completion / honest halt.
}

/// The answer a turn's outcome makes durable, or `None` when there is nothing
/// honest to record ([NFR-CC-04]).
///
/// Only [`TurnOutcome::Answered`] yields one. A whitespace-only answer is treated as
/// no answer: an empty assistant row is not a transcript entry, it is a blank turn a
/// restore would replay. That last case is **defense in depth** rather than a live
/// path — `run_synthesizer` already refuses a blank synthesis upstream ("the
/// synthesizer produced no answer"), so a blank `Answered` cannot currently reach
/// here; the guard keeps transcript honesty a local property of this function instead
/// of an inherited invariant from another crate. It is covered by the unit test below
/// (no orchestrator can produce the input).
fn durable_answer(outcome: Result<TurnOutcome, OrchestratorError>) -> Option<String> {
    match outcome {
        Ok(TurnOutcome::Answered(answer)) if !answer.trim().is_empty() => Some(answer),
        Ok(TurnOutcome::Answered(_) | TurnOutcome::Halted(_)) | Err(_) => None,
    }
}

/// Run one durable write against `target`'s conversation — the answer the turn
/// appends beside the user question its setup recorded ([FR-UI-26] AC-2), or the
/// title its Synthesizer named ([FR-UI-48]).
///
/// Opening `chat.db` and committing the write are synchronous SQLite work, so they
/// run on the blocking pool like every other engine touch on this surface
/// ([ADR-03]) rather than on the async I/O thread. A fresh short-lived handle (WAL
/// admits it) keeps the store's `&mut self` write contract without holding a second
/// connection open for the turn's lifetime.
async fn write_conversation<F, T>(target: &TurnTarget, write: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut ChatStore, i64) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let root = target.root.clone();
    let thread_id = target.thread_id;
    tokio::task::spawn_blocking(move || write(&mut ChatStore::open(&root)?, thread_id))
        .await
        .map_err(|_join| anyhow::anyhow!("the conversation-persistence task failed unexpectedly"))?
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
        // Additive ([FR-UI-48]): the [FR-UI-19] events above are unchanged.
        OrchestratorEvent::ThreadTitle { .. } => "thread_title",
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

#[cfg(test)]
mod tests {
    use super::*;
    use chat_agent::{BudgetBound, StepRole};

    /// The title event is the additive `thread_title` on the wire, its JSON carrying
    /// the conversation and the title ([FR-UI-48]); the [FR-UI-19] events keep their
    /// names.
    ///
    /// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
    /// [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
    #[test]
    fn the_title_event_streams_as_thread_title() {
        let event = OrchestratorEvent::ThreadTitle {
            thread_id: 7,
            title: "Risk in alpha".to_string(),
        };
        assert_eq!(event_name(&event), "thread_title");
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({"event": "thread_title", "thread_id": 7, "title": "Risk in alpha"})
        );
        let delta = OrchestratorEvent::AnswerDelta { delta: "x".to_string() };
        assert_eq!(event_name(&delta), "answer_delta");
    }

    /// `durable_answer` records a turn's answer only when the turn genuinely produced
    /// one ([FR-UI-26] AC-2, [NFR-CC-04]). The blank case is unreachable through the
    /// orchestrator (`run_synthesizer` rejects a blank synthesis first), so this is the
    /// only place that guard can be exercised — without it, a future upstream change
    /// could put an empty assistant row in a conversation with nothing to catch it.
    #[test]
    fn durable_answer_records_only_a_genuine_answer() {
        assert_eq!(
            durable_answer(Ok(TurnOutcome::Answered("the answer".to_string()))),
            Some("the answer".to_string()),
            "a genuine answer is made durable",
        );
        assert_eq!(
            durable_answer(Ok(TurnOutcome::Answered("   \n\t ".to_string()))),
            None,
            "a whitespace-only answer is not a transcript entry",
        );
        assert_eq!(
            durable_answer(Ok(TurnOutcome::Halted(BudgetBound::GlobalToolCalls {
                limit: 8
            }))),
            None,
            "an honest budget halt persists no answer",
        );
        assert_eq!(
            durable_answer(Err(OrchestratorError::Step {
                role: StepRole::Synthesizer,
                message: "the synthesizer provider failed".to_string(),
            })),
            None,
            "a faulted turn persists no answer — nothing is fabricated",
        );
    }
}
