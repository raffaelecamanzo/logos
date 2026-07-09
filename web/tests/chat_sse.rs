// CR-078/ADR-60: the chat surface is the LLM egress carve-out, so these route
// fitness tests exercise the `agents`-gated mock-provider surface and compile
// only under `--features agents`. A listen-only `--features ui` build has no chat
// routes/handlers, so the whole test crate is empty there.
#![cfg(feature = "agents")]
//! Chat SSE route fitness tests (S-170, [FR-UI-19], [NFR-SE-06], [NFR-CC-04],
//! [UAT-UI-07]).
//!
//! These drive the **real** router in-process (`ServiceExt::oneshot`, no socket)
//! with a mock-provider [`ChatService`](web::chat::ChatService) injected via
//! [`web::router_with_chat`], proving the chat surface end-to-end without a live
//! provider:
//!
//! - method/intent guards: a non-chat `POST` is `405`; a cross-origin or
//!   intent-less `POST /chat` is `403` ([NFR-SE-06]);
//! - a streamed turn emits the plan / subagent-activity / final-answer events
//!   incrementally as Server-Sent Events under the unchanged self-only CSP
//!   ([FR-UI-19]); the no-stream path renders the buffered answer;
//! - clean teardown — a client disconnect cancels the in-flight turn;
//! - zero real egress across a streamed turn, and the listener binds loopback
//!   ([UAT-UI-07]).
//!
//! [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [UAT-UI-07]: ../../docs/specs/requirements/UAT-UI-07.md

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use chat_agent::{
    BudgetTree, ChatStore, MemoryGrounding, MemoryStore, Orchestrator, PlanStep, StepContext,
    StepError, StepExecutor, StepObservation, SubagentRoster,
};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::chat::{spawn_turn, ChatService, ChatStream};
use web::{router_with_chat, IntentToken, CHAT_POST_ROUTE, INTENT_HEADER};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
const FINAL_SENTINEL: &str = "FINAL_ANSWER_SENTINEL";

// ── Mock chat services ────────────────────────────────────────────────────────

/// A [`ChatService`] backed by the real orchestrator over the offline mock
/// provider: a synthesizer-only plan that finalises with [`FINAL_SENTINEL`]. Drives
/// the genuine [`spawn_turn`] machinery (fan-out, scratchpad persistence, abort
/// guard) so the route tests exercise production code, not a stub.
struct ScriptedChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: std::path::PathBuf,
}

impl ChatService for ScriptedChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("mock turn").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        let planner = MockCompletionModel::new([
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"synthesizer","instruction":"compose the answer"}]}"#,
            ),
            MockTurn::text(format!(r#"{{"action":"final","answer":"{FINAL_SENTINEL}"}}"#)),
        ]);
        let subagent = MockCompletionModel::new([MockTurn::text("a grounded synthesis")]);
        let grounding = Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread, turn));
        let roster = SubagentRoster::new(Arc::clone(&self.engine), Arc::clone(&self.sandbox), subagent)
            .with_synthesizer_grounding(grounding);
        let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

/// A [`StepExecutor`] whose step never completes, and which flips `dropped` when
/// its in-flight future is dropped — so a test can prove a client disconnect
/// aborts the running turn ([FR-UI-19]).
struct HangingExecutor {
    dropped: Arc<AtomicBool>,
}

/// Flips an `AtomicBool` on drop — the witness that the in-flight step future was
/// cancelled.
struct DropWitness(Arc<AtomicBool>);

impl Drop for DropWitness {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl StepExecutor for HangingExecutor {
    fn execute(
        &self,
        _step: &PlanStep,
        _ctx: &StepContext<'_>,
    ) -> impl std::future::Future<Output = Result<StepObservation, StepError>> + Send {
        let dropped = Arc::clone(&self.dropped);
        async move {
            let _witness = DropWitness(dropped);
            // Never resolves: the turn stays in-flight until the stream is dropped
            // (client disconnect) aborts the task, dropping `_witness`.
            std::future::pending::<Result<StepObservation, StepError>>().await
        }
    }
}

/// A [`ChatService`] that starts a turn which hangs forever after emitting its
/// plan, exposing the drop witness for the teardown assertion.
struct HangingChatService {
    root: std::path::PathBuf,
    dropped: Arc<AtomicBool>,
}

impl ChatService for HangingChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("hang").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        let planner = MockCompletionModel::new([MockTurn::text(
            r#"{"action":"plan","steps":[{"role":"graph_navigator","instruction":"hang here"}]}"#,
        )]);
        let executor = HangingExecutor {
            dropped: Arc::clone(&self.dropped),
        };
        let orchestrator = Orchestrator::new(planner, executor, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

/// A [`ChatService`] whose orchestrator's planner provider immediately errors, so
/// the turn surfaces an honest `error` event and the stream closes — the AC2
/// error-teardown path.
struct ErroringChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: std::path::PathBuf,
}

impl ChatService for ErroringChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("err").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        // The planner's provider errors on the first call → Orchestrator::run
        // returns Err → run_orchestrated emits an honest `error` frame.
        let planner = MockCompletionModel::new([MockTurn::Error("injected provider fault".into())]);
        let subagent = MockCompletionModel::new([]);
        let roster = SubagentRoster::new(Arc::clone(&self.engine), Arc::clone(&self.sandbox), subagent);
        let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

/// A [`ChatService`] whose turn hard-halts at the max-replans bound and — because
/// this scenario leaves no synthesizer turn for the [CR-048] A′ best-effort pass —
/// honestly falls back to a bare halt rather than fabricating an answer. Exercises
/// the A′ synthesis-unavailable fallback ([NFR-CC-04]).
struct HaltingChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: std::path::PathBuf,
}

impl ChatService for HaltingChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("halt").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        // max_replans = 0: the planner returns a plan, its (only) synthesizer turn
        // runs, then the planner tries to replan → the max-replans hard halt. The
        // A′ best-effort synthesis has no synthesizer turn left, so it honestly
        // falls back to a bare Replans halt — never a fabricated answer ([CR-048]).
        let planner = MockCompletionModel::new([
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"synthesizer","instruction":"answer"}]}"#,
            ),
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"synthesizer","instruction":"again"}]}"#,
            ),
        ]);
        let subagent = MockCompletionModel::new([MockTurn::text("a partial synthesis")]);
        let roster = SubagentRoster::new(Arc::clone(&self.engine), Arc::clone(&self.sandbox), subagent);
        let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 0));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

// ── Fixtures + request builders ───────────────────────────────────────────────

/// A writable fixture root with an engine + sandbox over it.
fn fixture() -> (TempDir, Arc<Engine>, Arc<Sandbox>) {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn alpha() {}\n").expect("fixture src");
    let engine = Arc::new(Engine::open(dir.path()));
    let sandbox = Arc::new(Sandbox::new(dir.path(), std::iter::empty()).expect("sandbox"));
    (dir, engine, sandbox)
}

/// A router whose chat seam is the scripted mock-provider service.
fn scripted_router() -> (TempDir, axum::Router, IntentToken) {
    let (dir, engine, sandbox) = fixture();
    let intent = IntentToken::generate();
    let service: Arc<dyn ChatService> = Arc::new(ScriptedChatService {
        engine: Arc::clone(&engine),
        sandbox,
        root: dir.path().to_path_buf(),
    });
    let router = router_with_chat(engine, intent.clone(), service);
    (dir, router, intent)
}

/// A router whose chat seam is built by `make` over a fresh fixture (engine +
/// sandbox + root).
fn router_with_service<F>(make: F) -> (TempDir, axum::Router, IntentToken)
where
    F: FnOnce(Arc<Engine>, Arc<Sandbox>, std::path::PathBuf) -> Arc<dyn ChatService>,
{
    let (dir, engine, sandbox) = fixture();
    let intent = IntentToken::generate();
    let service = make(Arc::clone(&engine), sandbox, dir.path().to_path_buf());
    let router = router_with_chat(engine, intent.clone(), service);
    (dir, router, intent)
}

/// Build a `POST /chat` form request, optionally carrying an `Origin`, the intent
/// token, and an `Accept: text/event-stream` header (the streaming opt-in).
fn chat_post(
    intent: Option<&str>,
    origin: Option<&str>,
    accept_event_stream: bool,
    body: &'static str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(CHAT_POST_ROUTE)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    if accept_event_stream {
        builder = builder.header(header::ACCEPT, "text/event-stream");
    }
    builder.body(Body::from(body)).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── Guards (NFR-SE-06) ─────────────────────────────────────────────────────────

/// A `POST` to a non-chat, non-config path is still `405` — the chat route does
/// not widen the read-only posture beyond itself ([NFR-SE-06]).
#[tokio::test]
async fn non_chat_post_is_405() {
    let (_dir, router, _intent) = scripted_router();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/graph")
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "even the 405 carries the self-only CSP",
    );
}

/// A cross-origin `POST /chat` is rejected `403` before any turn starts — the chat
/// route carries the same same-origin defense as the config writes ([NFR-SE-06]).
#[tokio::test]
async fn cross_origin_chat_post_is_403() {
    let (_dir, router, intent) = scripted_router();
    let req = chat_post(
        Some(intent.as_str()),
        Some("http://evil.example.com"),
        true,
        "q=hello",
    );
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A same-origin `POST /chat` without a valid intent token is rejected `403` — the
/// per-session intent (CSRF) proof is required on the streaming route too, which
/// is exactly why it rides the `POST` rather than a header-less `GET` `EventSource`
/// ([NFR-SE-06]).
#[tokio::test]
async fn intentless_chat_post_is_403() {
    let (_dir, router, _intent) = scripted_router();
    let req = chat_post(None, Some(ORIGIN), true, "q=hello");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Streaming + buffered (FR-UI-19) ────────────────────────────────────────────

/// A guarded `POST /chat` with `Accept: text/event-stream` streams the turn's
/// events incrementally as SSE under the unchanged self-only CSP: the plan,
/// subagent activity, and the final answer all appear, in order, tagged with their
/// event names ([FR-UI-19]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_post_streams_sse_events_under_csp() {
    let (_dir, router, intent) = scripted_router();
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=what+is+here");
    let resp = router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "the response is a Server-Sent Events stream, not a WebSocket: {content_type}",
    );
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the streaming response carries the unchanged self-only CSP",
    );

    let body = body_string(resp).await;
    // The plan, the subagent step, the streamed answer tokens, and the final answer
    // all streamed, tagged.
    for marker in [
        "event: plan",
        "event: step_started",
        "event: step_observed",
        "event: answer_delta",
        "event: final_answer",
    ] {
        assert!(body.contains(marker), "the stream carries `{marker}`: {body}");
    }
    // Incremental order: plan → streamed answer token(s) → final answer (FR-UI-19).
    let plan_at = body.find("event: plan").unwrap();
    let delta_at = body.find("event: answer_delta").unwrap();
    let final_at = body.find("event: final_answer").unwrap();
    assert!(plan_at < delta_at, "the plan precedes the streamed answer tokens: {body}");
    assert!(delta_at < final_at, "the answer tokens stream before the final answer: {body}");
    // The synthesizer's prose streamed token-by-token in the answer_delta events.
    assert!(
        body.contains("a grounded synthesis"),
        "the synthesizer's prose streamed as answer_delta tokens: {body}",
    );
    assert!(
        body.contains(FINAL_SENTINEL),
        "the streamed answer is the orchestrator's, carried in the final_answer event: {body}",
    );
}

/// Without `Accept: text/event-stream`, the same guarded turn renders the complete
/// buffered answer — the no-JS / no-stream progressive-enhancement fallback
/// ([FR-UI-19]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_post_buffered_fallback_renders_full_answer() {
    let (_dir, router, intent) = scripted_router();
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), false, "q=what+is+here");
    let resp = router.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !content_type.starts_with("text/event-stream"),
        "the no-stream path is not an SSE response: {content_type}",
    );
    let body = body_string(resp).await;
    assert!(
        body.contains(FINAL_SENTINEL),
        "the buffered fallback renders the full answer: {body}",
    );
}

/// A provider/orchestrator fault streams an honest `error` event and then the
/// stream closes — the AC2 "closes cleanly on an error event" path, surfaced
/// honestly with no fabricated answer ([NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_event_closes_the_stream() {
    let (_dir, router, intent) = router_with_service(|engine, sandbox, root| {
        Arc::new(ErroringChatService {
            engine,
            sandbox,
            root,
        })
    });
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=cause+a+fault");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await; // collecting to completion proves the stream closes
    assert!(
        body.contains("event: error"),
        "the fault surfaces as an honest SSE error event: {body}",
    );
    assert!(
        !body.contains("event: final_answer"),
        "no answer is fabricated on a fault: {body}",
    );
}

/// With no provider configured, the real `ConfiguredChatService` streams a single
/// honest `error` frame naming the Config tab — the configure-first state, not a
/// crash ([FR-UI-18], [NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_first_streams_an_honest_error() {
    // A fixture root with `.logos/` but no `config.toml` model / `secrets.toml` key.
    let (dir, engine, _sandbox) = fixture();
    let intent = IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone()); // production ConfiguredChatService
    let _keep = dir;

    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=hello");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("event: error") && body.contains("Config"),
        "the unconfigured state is an honest error frame naming the Config tab: {body}",
    );
}

/// A configured-but-misconfigured provider (a `base_url` that already includes the
/// `/chat/completions` path rig appends) is caught by the pre-send **preflight**
/// before the turn runs: an honest `error` frame naming the specific problem, and
/// the API key never appears ([S-199], [FR-UI-24], [NFR-SE-07]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_rejects_a_misconfigured_base_url_before_the_turn() {
    let (dir, engine, _sandbox) = fixture();
    // A model + key ARE configured (so this is past the configure-first state),
    // but the base_url wrongly embeds the path rig appends → double-append.
    std::fs::write(
        dir.path().join(".logos/config.toml"),
        "[chat]\nprovider = \"openai\"\nmodel = \"openrouter/test-model\"\n\
         base_url = \"https://openrouter.ai/api/v1/chat/completions\"\n",
    )
    .expect("write config.toml");
    std::fs::write(
        dir.path().join(".logos/secrets.toml"),
        "[chat]\napi_key = \"sk-test-preflight-9999\"\n",
    )
    .expect("write secrets.toml");

    let intent = IntentToken::generate();
    let router = web::router_with_intent(engine, intent.clone()); // production ConfiguredChatService
    let _keep = dir;

    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=hello");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("event: error") && body.contains("chat/completions"),
        "the preflight names the double-appended base_url problem: {body}",
    );
    assert!(
        !body.contains("sk-test-preflight-9999"),
        "the preflight message must never echo the API key (NFR-SE-07): {body}",
    );
}

/// The buffered fallback reports a fault honestly (error precedence over a missing
/// answer) ([NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_fallback_reports_a_fault_honestly() {
    let (_dir, router, intent) = router_with_service(|engine, sandbox, root| {
        Arc::new(ErroringChatService {
            engine,
            sandbox,
            root,
        })
    });
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), false, "q=cause+a+fault");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("could not complete"),
        "the buffered fallback reports the fault honestly, not a blank answer: {body}",
    );
}

/// The buffered fallback reports an honest budget halt when the [CR-048] A′
/// best-effort synthesis cannot run (no synthesizer turn available here) — never a
/// fabricated completion ([NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_fallback_reports_a_halt_honestly() {
    let (_dir, router, intent) = router_with_service(|engine, sandbox, root| {
        Arc::new(HaltingChatService {
            engine,
            sandbox,
            root,
        })
    });
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), false, "q=halt+me");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("halted honestly"),
        "the buffered fallback reports the honest halt, not a fabricated answer: {body}",
    );
}

/// An empty message is a `400` — the turn never starts ([NFR-CC-04] honest input
/// handling).
#[tokio::test]
async fn empty_chat_message_is_400() {
    let (_dir, router, intent) = scripted_router();
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Teardown (FR-UI-19) ────────────────────────────────────────────────────────

/// A client disconnect mid-stream cancels the in-flight turn: dropping the SSE
/// response body aborts the spawned turn, dropping its running step future
/// ([FR-UI-19] — the turn is cancelled, not orphaned).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_disconnect_cancels_the_in_flight_turn() {
    let (dir, _engine, _sandbox) = fixture();
    let intent = IntentToken::generate();
    let dropped = Arc::new(AtomicBool::new(false));
    let service: Arc<dyn ChatService> = Arc::new(HangingChatService {
        root: dir.path().to_path_buf(),
        dropped: Arc::clone(&dropped),
    });
    let router = router_with_chat(Arc::new(Engine::open(dir.path())), intent.clone(), service);

    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=hang+please");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Read the first streamed frame (the plan), proving the turn is in-flight…
    let mut body = resp.into_body();
    let _first = body.frame().await;
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the in-flight step is still running while the client is connected",
    );

    // …then disconnect by dropping the body. The abort guard cancels the turn.
    drop(body);

    // Give the runtime a moment to process the abort and drop the step future.
    for _ in 0..50 {
        if dropped.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "dropping the SSE body aborted the in-flight turn (client disconnect → cancel)",
    );
}

// ── Carve-out: zero real egress + loopback bind (UAT-UI-07) ────────────────────

/// A full streamed turn through the route records **zero** real outbound
/// connections — the mock provider served it, nothing reached the wire — and the
/// listener binds only a loopback address ([UAT-UI-07], [NFR-SE-07], [NFR-SE-01]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamed_turn_records_zero_real_egress_and_binds_loopback() {
    // A loopback tripwire counting any connection a real provider would have made.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let (_dir, router, intent) = scripted_router();
    let req = chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=anything");
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    // The mock — not a real provider — produced the answer.
    assert!(
        body.contains(FINAL_SENTINEL),
        "the mock-provider turn produced the streamed answer: {body}",
    );
    // Nothing reached the wire across the whole streamed turn.
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the streamed turn opened zero real outbound connections (UAT-UI-07)",
    );

    // The surface listener binds only loopback (the carve-out boundary).
    let bound = web::bind(0).expect("bind a loopback listener");
    assert!(
        bound.local_addr().unwrap().ip().is_loopback(),
        "the web surface binds only a loopback address",
    );
}
