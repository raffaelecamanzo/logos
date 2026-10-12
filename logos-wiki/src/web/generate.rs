//! The wiki-generation surface seam: trigger a **connection-independent**,
//! single-run wiki-agent pass on Wiki-tab open, own its lifetime in application
//! state, and stream its per-page [`WikiProgress`] to the browser over SSE — holding
//! **no** generation logic ([ADR-01], [ADR-42], [CR-056], [FR-WK-18], [FR-UI-19],
//! [S-178], [S-222]).
//!
//! The thin web adapter forwards a view-open trigger to the [`wiki-agent`] runner
//! (`wiki_agent::run_configured`, through the run service the composing binary
//! hands the extension — [`WikiGenerator`]) and streams its [`WikiProgress`]
//! events back. The composition this module owns:
//!
//! - a **single-run lock + run registry** ([`WikiRunState`]) held in the router
//!   state so a Wiki-tab open (or a re-open while a pass is in flight) starts
//!   **exactly one** background run — a second trigger sees a run in flight and
//!   streams a single honest `busy` frame, starting no second pass ([FR-WK-18]
//!   acceptance);
//! - a [`WikiProgress`]→SSE mapping mirroring the chat extension's turn route (`logos-chat`),
//!   so the browser `addEventListener`s on the same kebab-case discriminant the
//!   JSON payload carries;
//! - a buffered fallback ([`WikiRunStream::into_buffered`]) for the no-JS /
//!   no-stream progressive-enhancement path ([FR-UI-19]).
//!
//! # Why the trigger rides the intent-guarded `POST`, not a `GET` `EventSource`
//! Starting a run **mutates** (consent-gated outbound egress + `wiki.db` writes via
//! the unchanged `wiki write` contract), so [NFR-SE-06] requires it carry the same
//! same-origin **+ intent token** proof every mutating route does — exactly the
//! reasoning the chat extension's SSE route follows (`logos-chat`). Consuming SSE over the
//! intent-guarded `POST` (via `fetch`) keeps that guard intact while staying
//! Server-Sent Events (`text/event-stream`), same-origin, under the **unchanged**
//! self-only CSP, with **no** WebSocket ([FR-UI-19]). The architecture's "view-open
//! event + SSE GET" interface is met in substance and strengthened on CSRF.
//!
//! # Connection-independent run lifetime ([CR-056], [S-222], [ADR-42])
//! The run's lifetime is owned by **application state ([`WikiRunState`]), not the SSE
//! response body**. When a trigger acquires the single-run lock the run task is
//! spawned and holds the run's [`WikiRunGuard`]; its per-page [`WikiProgress`] is
//! fan-out over a [`tokio::sync::broadcast`] channel whose sender lives in the run
//! registry. The SSE body is only a **subscriber** to that broadcast — dropping it
//! (navigating away from the Wiki tab) unsubscribes but does **not** abort the run,
//! so the pass **completes server-side** and the pages it writes stay fresh. The
//! run releases the single-run lock when its task finishes (the guard's `Drop` clears
//! the registry slot) — completion, an honest ceiling/provider halt, and a setup
//! fault all end the run and free the next trigger. Because the broadcast sender
//! outlives any one subscriber, a later trigger can **re-attach**
//! ([`WikiRunState::subscribe`]) to the live run's progress: the Wiki-tab generation
//! trigger route re-attaches instead of answering a reopen with `busy` when the
//! client accepts a streamed response ([S-223], [FR-WK-18] as amended by [CR-056]).
//! A re-attach replays the run's retained frame history before continuing with the
//! live tail, so the reopened tab's "N of M" is the run's true cumulative progress
//! from its very first render, never a fresh "page 1 of N" ([FR-UI-19]). The replay
//! carries every per-page denominator verbatim, so when a chunk re-read grew the
//! run's scope ([CR-093]) a re-attached observer folds the same grown denominator
//! as one present from the first frame; this relay neither computes nor rewrites it.
//!
//! # Honesty ([NFR-CC-04])
//! A setup/provider fault is surfaced as an honest `error` event and a missing
//! provider as a `configure-first` event — never a fabricated page.
//!
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
//! [CR-056]: ../../../docs/requests/CR-056-wiki-generation-usability.md
//! [CR-093]: ../../../docs/requests/CR-093-wiki-progress-denominator-honesty.md
//! [S-178]: ../../../docs/planning/journal.md#s-178-wiki-tab-trigger-background-generation-sse-streaming-and-first-use-consent
//! [S-222]: ../../../docs/planning/journal.md#s-222-connection-resilient-auto-continuing-background-generation-run
//! [S-223]: ../../../docs/planning/journal.md#s-223-wiki-tab-re-attach-to-the-in-flight-run-and-cumulative-progress
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
//! [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`wiki-agent`]: ../../../docs/specs/architecture/components/wiki-agent.md

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{FromRef, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{self, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::web::{WebContext, WebRoute};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use web::{wants_event_stream, workspace_root_of, MemberEngine};

use crate::WikiProgress;

/// The broadcast ring-buffer capacity for a run's live [`WikiFrame`] fan-out.
///
/// Sized generously so a promptly-polling SSE subscriber never lags on a realistic
/// run (post-[S-221] pruning the cold-start work-list is dozens of pages ≈ a couple
/// hundred frames). If a slow/absent subscriber does lag past this, the broadcast
/// drops the oldest frames for *that subscriber only* — the run and the `wiki.db`
/// store are unaffected ([NFR-RA-06]). This bounds only the **live** tail; a
/// re-attach ([S-223]) replays the run's retained [`ActiveRun::history`] instead of
/// relying on the ring, so a re-attaching subscriber's cumulative "N of M" is exact
/// regardless of how long the run has been running. A dropped progress frame is
/// never a fabricated page ([NFR-CC-04]).
const RUN_FRAME_CAPACITY: usize = 1024;

/// One frame in a wiki-generation run's stream: a per-page [`WikiProgress`]
/// transition to relay, an honest configure-first state, an honest terminal fault,
/// or the single-run-lock `busy` signal — never a fabricated page ([NFR-CC-04]).
#[derive(Debug, Clone)]
pub enum WikiFrame {
    /// A [`WikiProgress`] event to stream verbatim (run/page lifecycle).
    Progress(WikiProgress),
    /// The honest configure-first state ([FR-UI-18], [NFR-CC-04]): no wiki/chat
    /// model or no API key is set, so no run started and no outbound call was made.
    /// Carries the message the surface shows.
    ConfigureFirst(String),
    /// An honest fault (a config-read/setup failure or a provider/infrastructure
    /// error) surfaced to the client, never a fabricated page.
    Error(String),
    /// A run was already in flight when this trigger arrived, so the single-run lock
    /// ([`WikiRunState`]) admitted no second pass ([FR-WK-18] "exactly one run").
    Busy,
}

/// The in-flight run held in the registry: the broadcast sender that fans a run's
/// live [`WikiFrame`]s out to every subscriber (the initiating SSE body and any
/// later re-attach, [S-223]), plus the **retained frame history** every emitted
/// frame is appended to before it is broadcast. A re-attach ([`WikiRunState::subscribe`])
/// replays this history so a late observer's cumulative "N of M" ([FR-UI-19]) is
/// exact from its very first render, not just once the next live frame arrives.
/// Its presence **is** the single-run lock — a slot that is `Some` means a pass is
/// in flight.
struct ActiveRun {
    frames: broadcast::Sender<WikiFrame>,
    /// Every [`WikiFrame`] emitted so far, in order. [`WikiSink`] appends to this
    /// under the same lock it holds while broadcasting, so a concurrent
    /// [`WikiRunState::subscribe`] can never observe a frame twice (once in the
    /// replayed history, once again on the live tail) nor miss one — see
    /// [`WikiSink::emit`].
    history: Arc<Mutex<Vec<WikiFrame>>>,
}

/// The single-run lock **and** run registry for the Wiki-tab generation trigger
/// ([FR-WK-18], [CR-056], [S-222]).
///
/// Lives in the router state (cheap to clone — an `Arc<Mutex<…>>`), so the run's
/// lifetime is owned by **application state, not any SSE response body**. Opening
/// the Wiki tab (or re-opening it while a pass runs) calls
/// [`begin`](Self::begin): when no run is in flight it installs a fresh broadcast
/// channel, hands back a [`WikiRunGuard`] (which the run task holds, clearing the
/// slot when the task finishes — completion **or** honest halt) plus a [`WikiSink`]
/// to drive and the initiating subscriber [`WikiRunStream`]. A `None` return means a
/// pass is already in flight, so the trigger streams a single `busy` frame and
/// starts nothing — "exactly one background run". A concurrent re-open re-attaches
/// to the live run via [`subscribe`](Self::subscribe) without starting a second one.
#[derive(Clone, Default)]
pub struct WikiRunState {
    slot: Arc<Mutex<Option<ActiveRun>>>,
}

impl WikiRunState {
    /// A fresh registry with no run in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin the single background run, or decline if one is already in flight.
    ///
    /// `Some((guard, sink, stream))` when no run was in flight: `guard` rides the run
    /// task and releases the lock on completion; `sink` drives the run's frames; and
    /// `stream` is the initiating client's subscriber, taken **before** the task
    /// starts so it misses no frame. `None` when a pass is already in flight —
    /// exactly one run ([FR-WK-18]). The mutex makes concurrent triggers race-safe:
    /// exactly one installs the slot.
    pub(crate) fn begin(&self) -> Option<(WikiRunGuard, WikiSink, WikiRunStream)> {
        let mut slot = self.slot.lock().expect("wiki run registry mutex is not poisoned");
        if slot.is_some() {
            return None;
        }
        let (tx, rx) = broadcast::channel(RUN_FRAME_CAPACITY);
        let history: Arc<Mutex<Vec<WikiFrame>>> = Arc::new(Mutex::new(Vec::new()));
        // Store the sender BEFORE releasing the lock and BEFORE spawning the task, so
        // a guard drop (which clears the slot) can only ever happen after this
        // install — no torn state where the task finishes before the slot exists.
        *slot = Some(ActiveRun {
            frames: tx.clone(),
            history: Arc::clone(&history),
        });
        let guard = WikiRunGuard {
            slot: Arc::clone(&self.slot),
        };
        let sink = WikiSink { tx, history };
        Some((guard, sink, WikiRunStream::subscribe(rx)))
    }

    /// Re-attach to the live run's progress, if one is in flight ([S-223]).
    ///
    /// `Some(stream)` gives a **new** observer the run's retained frame history
    /// **replayed first**, then the live tail — so the reopened Wiki tab renders the
    /// SAME run's cumulative "N of M" ([FR-UI-19]) from its very first frame, never a
    /// fresh "page 1 of N" ([NFR-CC-04]). The run is untouched: a re-attach never
    /// starts a second run. `None` when no run is in flight, so the caller falls back
    /// to the work-list/status read (a completed run reads "up to date").
    ///
    /// The history snapshot and the live subscription are taken under the SAME
    /// [`ActiveRun::history`] lock [`WikiSink::emit`] holds while broadcasting, so a
    /// frame is delivered to a re-attaching observer exactly once — either it is
    /// already in the replayed snapshot (if `emit` won the race) or it arrives on the
    /// live tail (if `subscribe` won it) — never both, never neither.
    pub fn subscribe(&self) -> Option<WikiRunStream> {
        let slot = self.slot.lock().expect("wiki run registry mutex is not poisoned");
        slot.as_ref().map(|run| {
            let history = run.history.lock().expect("wiki run history mutex is not poisoned");
            let replay = history.clone();
            let live = run.frames.subscribe();
            drop(history);
            WikiRunStream::reattach(replay, live)
        })
    }
}

/// The RAII holder of the single-run lock for one run. Its `Drop` clears the
/// registry slot, releasing the lock when the run's task finishes — completion or an
/// honest ceiling/provider halt. Because the guard rides the **spawned run task**
/// (not the SSE body), a client disconnect no longer releases it: the run keeps the
/// lock until it genuinely completes server-side ([CR-056], [S-222]).
pub struct WikiRunGuard {
    slot: Arc<Mutex<Option<ActiveRun>>>,
}

impl Drop for WikiRunGuard {
    fn drop(&mut self) {
        // Clear the in-flight run, freeing the next trigger. Dropping the stored
        // broadcast sender here closes the fan-out to any lingering subscriber.
        if let Ok(mut slot) = self.slot.lock() {
            *slot = None;
        }
    }
}

/// Drives a background wiki-generation run to completion, emitting its
/// [`WikiFrame`]s into the [`WikiSink`] the run registry supplies — the seam that
/// lets the run be driven by the config-resolved real provider in production
/// (the `wiki-agent`'s configured service, [`WikiGenerator`]) and by the offline mock `CompletionModel` in
/// tests, with identical streaming machinery ([FR-WK-18], [ADR-42], [CR-056]).
pub trait WikiRunService: Send + Sync + 'static {
    /// Start the run that `guard` authorizes (the single-run lock is already held by
    /// the registry). The run executes in a **detached** task that holds `guard`, so
    /// its lifetime is owned by application state, **not** any SSE body — a client
    /// disconnect no longer aborts it and the pass completes server-side ([CR-056],
    /// [S-222]). Frames are emitted through `sink`; when the task finishes, `guard`
    /// drops and releases the lock. An empty work-list starts no model call
    /// ([NFR-CC-04]); a configure-first or setup fault yields a single honest frame
    /// and no outbound call.
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink);
}

/// The live stream of a wiki-generation run's [`WikiFrame`]s — a **subscriber** to
/// the run's [`tokio::sync::broadcast`] fan-out. Dropping the stream (the SSE body,
/// on client disconnect) only unsubscribes; it does **not** abort the run, which is
/// owned by [`WikiRunState`] and completes server-side ([CR-056], [S-222]).
pub struct WikiRunStream {
    inner: Pin<Box<dyn Stream<Item = WikiFrame> + Send>>,
}

impl WikiRunStream {
    /// Subscribe to a run's live broadcast — the initiating client's stream, taken by
    /// [`WikiRunState::begin`] before the run task starts (so it misses no frame).
    ///
    /// A subscriber that falls behind the [`RUN_FRAME_CAPACITY`] ring gets
    /// `Err(Lagged)` from the broadcast; we **skip** the missed frames (`.ok()`) and
    /// keep streaming — the run and the deterministic store are untouched
    /// ([NFR-RA-06], [NFR-CC-04]).
    pub(crate) fn subscribe(rx: broadcast::Receiver<WikiFrame>) -> Self {
        let inner = BroadcastStream::new(rx).filter_map(|frame| frame.ok());
        Self {
            inner: Box::pin(inner),
        }
    }

    /// Re-attach to a run already in flight ([`WikiRunState::subscribe`], [S-223]):
    /// `replay` (the run's retained history, taken atomically with `live`'s
    /// subscription) plays back first, so the reopened tab renders the whole run's
    /// cumulative progress immediately; `live` (the broadcast tail) then continues it
    /// with frames emitted from this point on. Transparent to the consumer — it is
    /// the exact same [`WikiFrame`] sequence a subscriber present from the start
    /// would have seen, just delivered late ([FR-UI-19], [NFR-CC-04]).
    pub(crate) fn reattach(replay: Vec<WikiFrame>, live: broadcast::Receiver<WikiFrame>) -> Self {
        let tail = BroadcastStream::new(live).filter_map(|frame| frame.ok());
        let inner = tokio_stream::iter(replay).chain(tail);
        Self {
            inner: Box::pin(inner),
        }
    }

    /// The pre-filled "already running" stream: one `busy` frame, then close. Used
    /// when a run is already in flight, so a concurrent Wiki-tab open starts no
    /// second pass ([FR-WK-18]). Holds no lock and touches no run.
    pub(crate) fn busy() -> Self {
        let inner = tokio_stream::iter(std::iter::once(WikiFrame::Busy));
        Self {
            inner: Box::pin(inner),
        }
    }

    /// Drain the whole run into the buffered summary the no-JS / no-stream path
    /// renders ([FR-UI-19] progressive-enhancement fallback). Honest by
    /// construction ([NFR-CC-04]): configure-first, busy, and fault take precedence
    /// over a page count; an all-clean work-list reports "nothing to generate".
    pub(crate) async fn into_buffered(mut self) -> String {
        let mut started = false;
        let mut written = 0usize;
        let mut failed = 0usize;
        let mut halted: Option<String> = None;
        let mut configure: Option<String> = None;
        let mut error: Option<String> = None;
        let mut busy = false;
        while let Some(frame) = self.inner.next().await {
            match frame {
                WikiFrame::Progress(WikiProgress::Started { .. }) => started = true,
                WikiFrame::Progress(WikiProgress::Completed {
                    pages_written,
                    pages_failed,
                }) => {
                    written = pages_written;
                    failed = pages_failed;
                }
                WikiFrame::Progress(WikiProgress::Halted { reason }) => {
                    halted.get_or_insert(reason);
                }
                WikiFrame::Progress(_) => {}
                WikiFrame::ConfigureFirst(message) => configure = Some(message),
                WikiFrame::Error(message) => {
                    error.get_or_insert(message);
                }
                WikiFrame::Busy => busy = true,
            }
        }
        if let Some(message) = configure {
            return message;
        }
        if busy {
            return "A wiki generation run is already in progress.".to_string();
        }
        if let Some(error) = error {
            return format!("Wiki generation could not complete: {error}");
        }
        if !started {
            return "The wiki is up to date — nothing to generate.".to_string();
        }
        let mut summary = format!("Wiki generation finished: {written} page(s) written");
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        if let Some(halted) = halted {
            summary.push_str(&format!("; halted: {halted}"));
        }
        summary.push('.');
        summary
    }
}

impl Stream for WikiRunStream {
    type Item = WikiFrame;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// The run's frame sink — the seam a [`WikiRunService::start_run`] body drives to
/// emit a run's [`WikiFrame`]s. Forwards per-page [`WikiProgress`] and the honest
/// terminal frames (configure-first / fault) onto the run's
/// [`tokio::sync::broadcast`] channel, whose sender the run registry
/// ([`WikiRunState`]) also holds so later observers can re-attach ([S-223]) — and
/// records every frame into the shared [`ActiveRun::history`] first, so a re-attach
/// can replay the whole run's cumulative progress, not just what happens to still be
/// in the broadcast ring.
///
/// Every send is best-effort from the runner's view: broadcast `send` returns an
/// error only when **no** subscriber is currently attached (the SSE body dropped and
/// no re-attach yet), which is ignored — the run keeps writing pages regardless of
/// who is watching ([CR-056]). The **live** broadcast tail is bounded by
/// [`RUN_FRAME_CAPACITY`] per subscriber (a lagging subscriber drops its oldest live
/// frames, not the run's); the retained history a re-attach replays is unaffected by
/// that ring.
#[derive(Clone)]
pub struct WikiSink {
    tx: broadcast::Sender<WikiFrame>,
    history: Arc<Mutex<Vec<WikiFrame>>>,
}

impl WikiSink {
    /// Record `frame` into the run's retained history, then broadcast it — in that
    /// order, under the same [`ActiveRun::history`] lock [`WikiRunState::subscribe`]
    /// takes to snapshot the history and open its live subscription. That shared lock
    /// is what makes delivery to a re-attaching observer exactly-once: `subscribe`
    /// either runs entirely before this `emit` (so its replay omits `frame`, but its
    /// live subscription — opened before this `emit`'s broadcast — receives it) or
    /// entirely after (so its replay already includes `frame`, sent before the
    /// subscription existed). Interleaving the two steps is what the lock forbids.
    fn emit(&self, frame: WikiFrame) {
        let mut history = self.history.lock().expect("wiki run history mutex is not poisoned");
        history.push(frame.clone());
        let _ = self.tx.send(frame);
    }

    /// Emit one per-page [`WikiProgress`] transition.
    pub fn progress(&self, event: WikiProgress) {
        self.emit(WikiFrame::Progress(event));
    }

    /// Emit the honest **configure-first** frame ([FR-UI-18], [NFR-CC-04]) — no
    /// model/key resolved, so no run started.
    pub fn configure_first(&self, message: impl Into<String>) {
        self.emit(WikiFrame::ConfigureFirst(message.into()));
    }

    /// Emit an honest fault frame ([NFR-CC-04]) — a setup/provider/infrastructure
    /// error, never a fabricated page.
    pub fn error(&self, message: impl Into<String>) {
        self.emit(WikiFrame::Error(message.into()));
    }

    /// An owned `Fn(WikiProgress)` adapter for the runner
    /// (`wiki_agent::run_configured` / `WikiAgent::run`), which
    /// take `impl Fn(WikiProgress)`. Cloning `self` (cheap — an `Arc`-backed sender and
    /// history handle) detaches it from this [`WikiSink`]'s borrow, so the closure can
    /// be held across the run's awaits, while still recording into and broadcasting
    /// through the exact same run history and channel [`progress`](Self::progress)
    /// does — through the same [`emit`](Self::emit), not a second copy of its logic.
    pub fn as_progress_fn(&self) -> impl Fn(WikiProgress) + Clone + Send + Sync + 'static {
        let sink = self.clone();
        move |event| sink.progress(event)
    }
}

/// Spawn the background wiki-generation run driven by `run` — the shared machinery
/// both the production configured service (the `wiki-agent`'s) and the carve-out test's mock
/// service drive their run through (mirrors the chat extension's `spawn_turn`).
///
/// The task is **detached**: its lifetime is owned by application state (the run's
/// [`WikiRunGuard`] rides it and the registry holds the broadcast sender), **not**
/// any SSE body. Dropping the returned nothing — there is no handle to abort — so a
/// client disconnect cannot cancel the run; it completes server-side ([CR-056],
/// [S-222]). The `run` closure receives the [`WikiSink`] and drives the pass to
/// completion, emitting per-page progress and any terminal configure-first / fault
/// frame; when its future ends, `guard` drops and releases the single-run lock
/// ([FR-WK-18]).
pub fn spawn_run<F, Fut>(guard: WikiRunGuard, sink: WikiSink, run: F)
where
    F: FnOnce(WikiSink) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    tokio::spawn(async move {
        // The guard rides the task: dropping it on completion (or an honest halt)
        // releases the single-run lock ([FR-WK-18], [CR-056]). The task is detached,
        // so a client disconnect never reaches it — the run finishes server-side.
        let _guard = guard;
        run(sink).await;
    });
}

/// The SSE `event:` name for a [`WikiProgress`] — mirrors its serde tag
/// (`#[serde(tag = "event", rename_all = "kebab-case")]`) so the browser can
/// `addEventListener` on the same discriminant the JSON payload carries.
fn progress_event_name(event: &WikiProgress) -> &'static str {
    match event {
        WikiProgress::Started { .. } => "started",
        WikiProgress::PageStarted { .. } => "page-started",
        WikiProgress::PageWritten { .. } => "page-written",
        WikiProgress::PageFailed { .. } => "page-failed",
        WikiProgress::Halted { .. } => "halted",
        WikiProgress::Completed { .. } => "completed",
    }
}

/// Render one [`WikiFrame`] as an SSE event: a progress event tagged with its
/// kebab-case discriminant and carrying its JSON body, or an honest
/// `configure-first` / `error` / `busy` event with a plain-text payload.
pub(crate) fn frame_to_event(frame: WikiFrame) -> sse::Event {
    match frame {
        WikiFrame::Progress(event) => {
            let name = progress_event_name(&event);
            sse::Event::default()
                .event(name)
                .json_data(&event)
                // The progress events are plain serde structs; a serialization
                // failure is not expected, but degrade honestly rather than panic.
                .unwrap_or_else(|_| {
                    sse::Event::default()
                        .event("error")
                        .data("event serialization failed")
                })
        }
        WikiFrame::ConfigureFirst(message) => {
            sse::Event::default().event("configure-first").data(message)
        }
        WikiFrame::Error(message) => sse::Event::default().event("error").data(message),
        WikiFrame::Busy => sse::Event::default()
            .event("busy")
            .data("a wiki generation run is already in progress"),
    }
}

/// Adapt a [`WikiRunStream`] into the `Result`-yielding stream `axum::response::Sse`
/// consumes. The stream is a broadcast **subscriber**, so a client disconnect drops
/// only the subscription — the run keeps completing server-side, owned by
/// [`WikiRunState`] ([CR-056], [S-222]).
pub(crate) fn sse_body(
    stream: WikiRunStream,
) -> impl Stream<Item = Result<sse::Event, std::convert::Infallible>> {
    stream.map(|frame| Ok(frame_to_event(frame)))
}

// ── The generation trigger route ([FR-WK-18], [FR-UI-19]) ──────────────────────

/// What starts a wiki generation pass: a run service bound to one member of the
/// serve ([`WikiRunService`]).
///
/// The `wiki` extension owns the generation route, its single-run lock and its
/// SSE stream ([`generation_routes`]); the pass itself — the `wiki-agent` over the
/// configured provider — depends on this crate, so it cannot be named here. The
/// composing binary hands the extension a generator instead
/// ([`crate::WikiExtension::with_generator`]): under `agents` the `wiki-agent`'s
/// configured one, in the route tests a mock. Without one the route is not
/// mounted, which is the listen-only build's wiki ([ADR-60]).
///
/// [ADR-60]: ../../../docs/specs/architecture/decisions/ADR-60.md
pub trait WikiGenerator: Send + Sync + 'static {
    /// The run service for `engine` — the serve's default member, or the member a
    /// `?repo=`-scoped request names — inside the workspace rooted at
    /// `workspace_root`, if the serve is one.
    fn service(&self, engine: Arc<Engine>, workspace_root: Option<PathBuf>) -> Arc<dyn WikiRunService>;
}

/// The generation route's state: the serve's default engine and backing (what
/// [`MemberEngine`] resolves a request against), the generator, the default
/// member's run service, and the single-run lock.
#[derive(Clone)]
struct GenerationState {
    engine: Arc<Engine>,
    backing: Arc<Backing<Engine>>,
    generator: Arc<dyn WikiGenerator>,
    default_service: Arc<dyn WikiRunService>,
    runs: WikiRunState,
}

impl FromRef<GenerationState> for Arc<Engine> {
    fn from_ref(state: &GenerationState) -> Self {
        Arc::clone(&state.engine)
    }
}

impl FromRef<GenerationState> for Arc<Backing<Engine>> {
    fn from_ref(state: &GenerationState) -> Self {
        Arc::clone(&state.backing)
    }
}

/// The enumerated wiki-generation trigger `POST` route (S-178, [FR-WK-18],
/// [FR-UI-19], [NFR-SE-06]): the Wiki tab posts here on open to launch a
/// background, single-run generation pass and stream its per-page
/// [`WikiProgress`] back as Server-Sent Events. Starting a run mutates (consent-gated
/// egress and `wiki.db` writes), so it is a `POST` behind the intent guard, like
/// every mutating route; a `GET` here is a browser navigation to the Wiki view,
/// answered with the SPA shell ([`WebRoute::shell`]).
///
/// [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
pub const WIKI_GENERATE_ROUTE: &str = "/wiki/generate";

/// The generation routes over `context`, run by `generator`: the trigger `POST`
/// and its SPA shell. Built once per serve, so the default member's service and
/// the single-run lock are shared by every request.
pub(super) fn generation_routes(context: &WebContext, generator: Arc<dyn WikiGenerator>) -> Vec<WebRoute> {
    let default_service =
        generator.service(Arc::clone(&context.engine), workspace_root_of(&context.backing));
    let state = GenerationState {
        engine: Arc::clone(&context.engine),
        backing: Arc::clone(&context.backing),
        generator,
        default_service,
        runs: WikiRunState::new(),
    };
    vec![
        // The trigger (S-178, FR-WK-18, FR-UI-19, ADR-42): under the single-run lock it
        // launches a background pass and, with `Accept: text/event-stream`, streams the
        // per-page progress as SSE under the unchanged self-only CSP; otherwise it
        // renders the buffered summary. A GET is a browser navigation to the Wiki
        // view, so it serves the SPA shell.
        WebRoute::shell(WIKI_GENERATE_ROUTE),
        WebRoute::post(WIKI_GENERATE_ROUTE, wiki_generate, state),
    ]
}

/// The wiki-generation service for this request's member scope (S-250, [FR-UI-29]).
///
/// The load-bearing case is a **write**: the Wiki tab's read-models are member-scoped, so
/// a generation pass launched from it must write into the member whose pages it is
/// showing — not into the workspace default's `wiki.db`. Same engine ⇒ the default
/// member's service, which keeps the S-178 mock seam intact; a different engine ⇒ a
/// service the generator binds to that member (the rule the `chat` extension's
/// member chat follows too).
///
/// [FR-UI-29]: ../../../docs/specs/requirements/FR-UI-29.md
fn wiki_for(state: &GenerationState, scoped: Arc<Engine>) -> Arc<dyn WikiRunService> {
    if Arc::ptr_eq(&state.engine, &scoped) {
        Arc::clone(&state.default_service)
    } else {
        state.generator.service(scoped, workspace_root_of(&state.backing))
    }
}

/// The wiki-generation trigger handler (S-178, [FR-WK-18], [FR-UI-19], [ADR-42]):
/// gate the Wiki-tab open through the single-run lock, then start a background
/// generation pass on the [`WikiRunService`] and either **stream** its per-page
/// [`WikiProgress`] as Server-Sent Events or render the **buffered** summary, per
/// the client's `Accept` — the surface holds no generation logic ([ADR-01]).
///
/// - lock **acquired** + `Accept: text/event-stream` → an SSE response streaming the
///   run/page-lifecycle events incrementally under the **unchanged** self-only CSP
///   (the web surface's outer CSP layer stamps the streaming response's headers
///   before its body flows); the body is only a **subscriber** to the run's
///   broadcast, so a client disconnect does **not** abort the run — it completes
///   server-side, owned by [`WikiRunState`] ([CR-056], [S-222], [FR-UI-19]);
/// - run **already in flight** + `Accept: text/event-stream` → the Wiki tab has
///   **reopened** mid-run: **re-attach** ([`WikiRunState::subscribe`]) to the SAME
///   run's progress rather than starting a second one or reporting `busy` — the
///   reattached stream replays the run's cumulative history first, then continues
///   live, so the reopened tab's "N of M" is exact from its first render, never a
///   fresh "page 1 of N" ([S-223], [FR-WK-18] as amended by [CR-056], [FR-UI-19]);
/// - run already in flight, **no** SSE opt-in → the buffered no-JS fallback cannot
///   usefully wait out an unknown-length reattach, so it keeps reporting the
///   in-flight run honestly as `busy` rather than blocking the response until the
///   run drains;
/// - lock acquired, no SSE opt-in → the buffered summary (the non-streaming
///   fallback, [FR-UI-19]), honest on a configure-first / halt / fault
///   ([NFR-CC-04]).
///
/// The route is already same-origin + intent-token gated by the web surface's
/// intent guard ([NFR-SE-06]) — the streaming request rides this `POST` so it keeps
/// that proof. The Wiki tab sends no body; a work-list check + configure-first are
/// decided by the run service, not here.
///
/// [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [CR-056]: ../../../docs/requests/CR-056-wiki-generation-usability.md
/// [S-222]: ../../../docs/planning/journal.md#s-222-connection-resilient-auto-continuing-background-generation-run
/// [S-223]: ../../../docs/planning/journal.md#s-223-wiki-tab-re-attach-to-the-in-flight-run-and-cumulative-progress
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
async fn wiki_generate(
    State(state): State<GenerationState>,
    MemberEngine(engine): MemberEngine,
    headers: HeaderMap,
) -> Response {
    // Generate INTO the member whose pages the tab is showing — never the default's
    // wiki.db (S-250; the Wiki read-models are member-scoped) — inheriting its chat
    // provider and key through the same seam the turn reads ([ADR-67], [ADR-42]).
    let wiki = wiki_for(&state, engine);
    let streaming = wants_event_stream(&headers);
    // The single-run lock ([FR-WK-18]): begin → own the one connection-independent
    // background run. The run's lifetime is owned by the run state, not this response
    // body ([CR-056], [S-222]).
    //
    // Already in flight: a **streaming** reopen re-attaches to the SAME run instead
    // of starting a second one ([S-223]) — this is what the Wiki tab actually sends
    // (it always requests `text/event-stream`, [FR-UI-19]). The **buffered** no-JS
    // fallback keeps the simpler honest `busy` report: it cannot progressively render
    // a reattach, and blocking the whole response on an unknown-length live run would
    // be a worse fallback than today's instant honest answer.
    let stream = match state.runs.begin() {
        Some((guard, sink, stream)) => {
            wiki.start_run(guard, sink);
            stream
        }
        None if streaming => state.runs.subscribe().unwrap_or_else(WikiRunStream::busy),
        None => WikiRunStream::busy(),
    };

    if streaming {
        // Stream the run. KeepAlive comments keep intermediaries from idling the
        // connection between sparse per-page events.
        Sse::new(sse_body(stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response()
    } else {
        // Progressive-enhancement fallback: drain the run server-side and render
        // the honest summary (FR-UI-19).
        let summary = stream.into_buffered().await;
        (StatusCode::OK, summary).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run service that runs nothing — only its identity matters below.
    struct Idle;

    impl WikiRunService for Idle {
        fn start_run(&self, _guard: WikiRunGuard, _sink: WikiSink) {}
    }

    /// A generator that binds a fresh service to every member and records which
    /// engine each was bound to.
    #[derive(Default)]
    struct Recording(Mutex<Vec<Arc<Engine>>>);

    impl WikiGenerator for Recording {
        fn service(&self, engine: Arc<Engine>, _workspace_root: Option<PathBuf>) -> Arc<dyn WikiRunService> {
            self.0.lock().expect("not poisoned").push(engine);
            Arc::new(Idle)
        }
    }

    /// The wiki-generation service resolves against the request's member (S-250).
    ///
    /// The rule has two halves and both matter: the **default** engine keeps the
    /// service built once for it (the seam the route tests inject a mock through),
    /// while a **scoped** engine gets a service the generator binds to THAT member —
    /// otherwise the Wiki tab, whose read-models are member-scoped, would launch a
    /// pass that writes into the workspace default's `wiki.db` (a cross-member write,
    /// [NFR-RA-05]). Moved here from `web` with the route (S-644).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[test]
    fn the_wiki_generation_service_follows_the_member_scope() {
        let default_dir = tempfile::TempDir::new().unwrap();
        let other_dir = tempfile::TempDir::new().unwrap();
        let default = Arc::new(Engine::start(default_dir.path()).expect("default engine"));
        let other = Arc::new(Engine::start(other_dir.path()).expect("member engine"));
        let generator = Arc::new(Recording::default());
        let default_service: Arc<dyn WikiRunService> = Arc::new(Idle);
        let state = GenerationState {
            engine: Arc::clone(&default),
            backing: Arc::new(Backing::Single(Arc::clone(&default))),
            generator: Arc::clone(&generator) as Arc<dyn WikiGenerator>,
            default_service: Arc::clone(&default_service),
            runs: WikiRunState::new(),
        };

        // Unscoped (or single-root): the default member's service, verbatim.
        assert!(Arc::ptr_eq(&wiki_for(&state, Arc::clone(&default)), &default_service));
        assert!(generator.0.lock().unwrap().is_empty(), "no service is bound for the default member");

        // Scoped to another member: a different service, bound to that member's engine.
        assert!(!Arc::ptr_eq(&wiki_for(&state, Arc::clone(&other)), &default_service));
        let bound = generator.0.lock().unwrap();
        assert_eq!(bound.len(), 1);
        assert!(Arc::ptr_eq(&bound[0], &other), "bound to the member on screen");
    }

    /// The [`WikiRunState`] run-state contract [S-223] re-attaches over ([CR-056]):
    /// the single-run lock admits exactly one run; while it is in flight
    /// [`subscribe`](WikiRunState::subscribe) attaches a **new** observer to the
    /// **same** run (so a re-open re-attaches rather than starting a second run) and
    /// a frame reaches both the initiating stream and the re-attached one; and once
    /// the run ends (the guard drops) `subscribe` is `None` again so the tab falls
    /// back to the work-list read.
    #[tokio::test]
    async fn subscribe_reattaches_to_the_in_flight_run_and_is_none_when_idle() {
        let state = WikiRunState::new();
        assert!(
            state.subscribe().is_none(),
            "no run in flight → subscribe is None (S-223 falls back to the work-list read)",
        );

        let (guard, sink, mut initiating) = state.begin().expect("the first begin starts the run");
        assert!(
            state.begin().is_none(),
            "single-run lock: a second begin is refused while a run is in flight (FR-WK-18)",
        );

        // Re-attach a second observer to the SAME run — no second run is started.
        let mut reattached = state
            .subscribe()
            .expect("a run is in flight → subscribe attaches a new observer");

        // A frame emitted now reaches BOTH the initiating stream and the re-attach.
        sink.progress(WikiProgress::Started {
            total: 7,
            synthesis_timeout_secs: 180,
        });
        for (label, stream) in [
            ("initiating", &mut initiating),
            ("re-attached", &mut reattached),
        ] {
            match stream.next().await {
                Some(WikiFrame::Progress(WikiProgress::Started { total, .. })) => {
                    assert_eq!(total, 7, "{label} observer received the cumulative total");
                }
                other => panic!("{label} observer did not receive the Started frame: {other:?}"),
            }
        }

        // The run ends: dropping the guard releases the single-run lock.
        drop(guard);
        assert!(
            state.subscribe().is_none(),
            "after the run ends (guard dropped), subscribe is None again",
        );
        assert!(
            state.begin().is_some(),
            "the lock is released, so a fresh run can begin",
        );
    }

    /// The per-page denominator of a run whose scope grew ([CR-093], [S-310]) reaches
    /// a mid-run re-attach exactly as it reached the observer present from the first
    /// frame: the retained history replays every `page-started` total verbatim, in
    /// order, with the single `started` frame first and never repeated — so a surface
    /// folding the denominator as a monotonic maximum converges on the same value
    /// whichever way it joined.
    #[tokio::test]
    async fn a_mid_run_reattach_replays_the_grown_per_page_denominator() {
        fn page(index: usize, total: usize) -> WikiProgress {
            WikiProgress::PageStarted {
                slug: format!("p{index}"),
                title: format!("P{index}"),
                index,
                total,
            }
        }
        /// The per-page denominators a stream carries, in order, and its `started`
        /// count — read off exactly the frames this observer received. Each read is
        /// bounded: the run's sender stays alive for the whole test, so a frame the
        /// stream never delivers would otherwise park `next()` forever instead of
        /// failing.
        async fn fold(stream: &mut WikiRunStream, frames: usize) -> (Vec<usize>, usize) {
            let (mut totals, mut started) = (Vec::new(), 0);
            for i in 0..frames {
                let next = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
                    .await
                    .unwrap_or_else(|_| panic!("frame {} of {frames} was never delivered", i + 1));
                match next.expect("a frame was emitted") {
                    WikiFrame::Progress(WikiProgress::Started { .. }) => started += 1,
                    WikiFrame::Progress(WikiProgress::PageStarted { total, .. }) => totals.push(total),
                    other => panic!("unexpected frame {other:?}"),
                }
            }
            (totals, started)
        }

        let state = WikiRunState::new();
        let (_guard, sink, mut initiating) = state.begin().expect("the run starts");
        sink.progress(WikiProgress::Started {
            total: 2,
            synthesis_timeout_secs: 180,
        });
        sink.progress(page(1, 2));
        // A chunk re-read surfaced one more item: the next page carries the grown scope.
        sink.progress(page(2, 3));

        let mut reattached = state.subscribe().expect("a run is in flight");
        sink.progress(page(3, 3));

        let (live_totals, live_started) = fold(&mut initiating, 4).await;
        let (replayed_totals, replayed_started) = fold(&mut reattached, 4).await;
        assert_eq!(live_totals, vec![2, 3, 3], "the initiating observer saw the grown scope");
        assert_eq!(
            replayed_totals, live_totals,
            "the re-attach replays every per-page denominator verbatim, then the live tail",
        );
        assert_eq!(
            (live_started, replayed_started),
            (1, 1),
            "each observer receives exactly one started frame — it is never re-emitted",
        );
    }
}
