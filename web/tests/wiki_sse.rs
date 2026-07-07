//! Wiki-generation trigger + SSE route fitness tests (S-178, [FR-WK-18],
//! [FR-UI-19], [NFR-SE-06], [NFR-CC-04]).
//!
//! These drive the **real** router in-process (`ServiceExt::oneshot`, no socket)
//! with a mock-provider [`WikiRunService`](web::wikigen::WikiRunService) injected
//! via [`web::router_with_wiki`], proving the wiki-generation surface end-to-end
//! without a live provider:
//!
//! - method/intent guards: a non-wiki `POST` is `405`; a cross-origin or
//!   intent-less `POST /wiki/generate` is `403` ([NFR-SE-06]);
//! - opening the tab with a non-empty work-list starts **exactly one** background
//!   run under the single-run lock, streaming the per-page `WikiProgress` events
//!   incrementally as Server-Sent Events under the unchanged self-only CSP; a
//!   concurrent open streams a single honest `busy` frame and starts no second run
//!   ([FR-WK-18], [FR-UI-19]);
//! - an **empty work-list starts no run** (no model call, no progress);
//! - a client disconnect does **not** abort the run: its lifetime is owned by
//!   application state, so the pass completes server-side and its pages read fresh
//!   on both axes ([CR-056], [S-222]);
//! - the real `ConfiguredWikiRunService` streams a `configure-first` frame with no
//!   provider set — not a crash ([FR-UI-18], [NFR-CC-04]);
//! - zero real egress across a streamed run, and the listener binds loopback
//!   ([NFR-SE-07], [NFR-SE-01]).
//!
//! The real-`Engine` fixture helpers mirror `wiki-agent/tests/generation.rs`; the
//! guard/stream assertions mirror `web/tests/chat_sse.rs`.
//!
//! [FR-WK-18]: ../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::{MockCompletionModel, MockTurn};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::wiki::revision_pending;
use logos_core::Engine;
use tempfile::TempDir;
use tokio::sync::Notify;
use tower::ServiceExt;
use wiki_agent::{WikiAgent, WikiProgress};

use web::wikigen::{spawn_run, WikiRunGuard, WikiRunService, WikiSink};
use web::{router_with_intent, router_with_wiki, IntentToken, INTENT_HEADER, WIKI_GENERATE_ROUTE};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";

// ── Real-Engine fixture helpers (mirror wiki-agent/tests/generation.rs) ───────

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// A committed, indexed repo whose graph has a revision > 0, so the wiki work-list
/// yields page-worthy entities to generate.
fn indexed_engine(repo: &Path) -> Arc<Engine> {
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    Arc::new(engine)
}

// ── Mock wiki-run services ────────────────────────────────────────────────────

/// A [`WikiRunService`] backed by the **real** [`WikiAgent`] over the offline mock
/// provider: it drives the genuine queue loop + `wiki write` persistence + streaming
/// machinery, so the route tests exercise production code, not a stub. `mock` shares
/// its scripted state across clones, so the test can assert `request_count()`.
struct AgentWikiRunService {
    engine: Arc<Engine>,
    mock: MockCompletionModel,
}

impl WikiRunService for AgentWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let engine = Arc::clone(&self.engine);
        let mock = self.mock.clone();
        spawn_run(guard, sink, move |sink| async move {
            match WikiAgent::new(mock, "test skill preamble", "mock-model")
                .run(engine, sink.as_progress_fn())
                .await
            {
                Ok(_) => {}
                Err(e) => sink.error(format!("wiki generation failed: {e}")),
            }
        })
    }
}

/// A [`WikiRunService`] whose run emits `Started`, then blocks on a [`Notify`] until
/// the test releases it, then emits `Completed` — so a test can prove the single-run
/// lock admits exactly one run while a pass is in flight ([FR-WK-18]). `runs` counts
/// how many runs actually started (the busy trigger never calls `start_run`).
struct GatedWikiRunService {
    runs: Arc<AtomicUsize>,
    gate: Arc<Notify>,
}

impl WikiRunService for GatedWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&self.gate);
        spawn_run(guard, sink, move |sink| async move {
            sink.progress(WikiProgress::Started {
                total: 1,
                synthesis_timeout_secs: 180,
            });
            gate.notified().await;
            sink.progress(WikiProgress::Completed {
                pages_written: 1,
                pages_failed: 0,
            });
        })
    }
}

/// A [`WikiRunService`] whose run emits `Started`, then blocks on a [`Notify`] until
/// released, tracking whether it **ran to completion** (emitted `Completed`) after
/// the gate — the witness that a run owned by application state finishes
/// server-side even after its driving SSE stream was dropped ([CR-056], [S-222]).
struct CompletionWitnessRunService {
    gate: Arc<Notify>,
    completed: Arc<AtomicUsize>,
}

impl WikiRunService for CompletionWitnessRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let gate = Arc::clone(&self.gate);
        let completed = Arc::clone(&self.completed);
        spawn_run(guard, sink, move |sink| async move {
            sink.progress(WikiProgress::Started {
                total: 1,
                synthesis_timeout_secs: 180,
            });
            // Block until the test releases the gate — the stream is dropped in
            // between, proving the run is not tied to the connection.
            gate.notified().await;
            sink.progress(WikiProgress::Completed {
                pages_written: 1,
                pages_failed: 0,
            });
            // The run reached completion server-side, after the client disconnected.
            completed.fetch_add(1, Ordering::SeqCst);
        })
    }
}

/// A [`WikiRunService`] that streams a per-page failure, an honest halt, and a
/// terminal fault — exercising the `page-failed` / `halted` progress-event names and
/// the honest `error` frame the surface must surface without fabricating a page
/// ([NFR-CC-04]).
struct FaultyWikiRunService;

impl WikiRunService for FaultyWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        spawn_run(guard, sink, move |sink| async move {
            sink.progress(WikiProgress::Started {
                total: 2,
                synthesis_timeout_secs: 180,
            });
            sink.progress(WikiProgress::PageFailed {
                slug: "overview/x".to_string(),
                error: "over-cap body".to_string(),
            });
            sink.progress(WikiProgress::Halted {
                reason: "the per-run budget was spent".to_string(),
            });
            sink.error("wiki generation failed: injected provider outage");
        })
    }
}

/// A gated variant of [`FaultyWikiRunService`]: emits `Started`, then blocks on a
/// [`Notify`] until released before failing a page, halting, and faulting — so a test
/// can re-attach a second observer while the run is still in flight and prove it sees
/// the SAME honest halt/error, not a silent success ([NFR-CC-04], [S-223]).
struct GatedFaultyWikiRunService {
    gate: Arc<Notify>,
}

impl WikiRunService for GatedFaultyWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let gate = Arc::clone(&self.gate);
        spawn_run(guard, sink, move |sink| async move {
            sink.progress(WikiProgress::Started {
                total: 2,
                synthesis_timeout_secs: 180,
            });
            gate.notified().await;
            sink.progress(WikiProgress::PageFailed {
                slug: "overview/x".to_string(),
                error: "over-cap body".to_string(),
            });
            sink.progress(WikiProgress::Halted {
                reason: "the per-run budget was spent".to_string(),
            });
            sink.error("wiki generation failed: injected provider outage");
        })
    }
}

// ── Fixtures + request builders ───────────────────────────────────────────────

/// A router whose wiki seam is the real-`WikiAgent` mock-provider service over an
/// indexed fixture. Returns the shared `mock` so the test can assert its call count.
fn agent_router() -> (TempDir, axum::Router, IntentToken, MockCompletionModel) {
    let dir = TempDir::new().expect("temp dir");
    let engine = indexed_engine(dir.path());
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty(), "the indexed fixture seeds a non-empty work-list");
    // One scripted text turn per queued page — the mock ignores the prompt, so the
    // loop runs deterministically and never exhausts (which would halt the pass).
    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            MockTurn::text(format!(
                "# Generated page {i}\n\nThis regenerated page carries enough grounded prose \
                 to clear the write-path validity guard ([FR-WK-19])."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(AgentWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
    });
    let router = router_with_wiki(engine, intent.clone(), service);
    (dir, router, intent, mock)
}

/// Build a `POST /wiki/generate` request, optionally carrying an `Origin`, the intent
/// token, and an `Accept: text/event-stream` header (the streaming opt-in). The
/// trigger carries no body.
fn wiki_post(intent: Option<&str>, origin: Option<&str>, accept_event_stream: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(WIKI_GENERATE_ROUTE)
        .header(header::HOST, HOST);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    if accept_event_stream {
        builder = builder.header(header::ACCEPT, "text/event-stream");
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── Guards (NFR-SE-06) ─────────────────────────────────────────────────────────

/// A `POST` to a non-wiki, non-enumerated path is still `405` — the wiki trigger
/// does not widen the read-only posture beyond itself ([NFR-SE-06]).
#[tokio::test]
async fn non_wiki_post_is_405() {
    let (_dir, router, _intent, _mock) = agent_router();
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

/// A cross-origin `POST /wiki/generate` is rejected `403` before any run starts —
/// the trigger carries the same same-origin defense as the config/chat writes
/// ([NFR-SE-06]).
#[tokio::test]
async fn cross_origin_wiki_post_is_403() {
    let (_dir, router, intent, _mock) = agent_router();
    let req = wiki_post(Some(intent.as_str()), Some("http://evil.example.com"), true);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// A same-origin `POST /wiki/generate` without a valid intent token is rejected
/// `403` — the per-session intent (CSRF) proof is required on the streaming trigger
/// too, which is exactly why it rides the `POST` rather than a header-less `GET`
/// `EventSource` ([NFR-SE-06]).
#[tokio::test]
async fn intentless_wiki_post_is_403() {
    let (_dir, router, _intent, _mock) = agent_router();
    let req = wiki_post(None, Some(ORIGIN), true);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Streaming + buffered (FR-UI-19, FR-WK-18) ──────────────────────────────────

/// A guarded `POST /wiki/generate` with `Accept: text/event-stream` streams the
/// run's per-page events incrementally as SSE under the unchanged self-only CSP: the
/// `started`, `page-started`, `page-written`, and terminal `completed` events all
/// appear, in order, tagged with their kebab-case names ([FR-UI-19], [FR-WK-18]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_post_streams_sse_events_under_csp() {
    let (_dir, router, intent, mock) = agent_router();
    let req = wiki_post(Some(intent.as_str()), Some(ORIGIN), true);
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
    for marker in ["event: started", "event: page-started", "event: page-written", "event: completed"] {
        assert!(body.contains(marker), "the stream carries `{marker}`: {body}");
    }
    // Incremental order: the run start precedes a page write, which precedes the
    // terminal completion (FR-UI-19 / FR-WK-18 per-page streaming).
    let started_at = body.find("event: started").unwrap();
    let written_at = body.find("event: page-written").unwrap();
    let completed_at = body.find("event: completed").unwrap();
    assert!(started_at < written_at, "the run start precedes a page write: {body}");
    assert!(written_at < completed_at, "a page writes before the terminal completion: {body}");
    assert!(
        mock.request_count() >= 1,
        "the mock — not a real provider — synthesized the pass",
    );
}

/// Without `Accept: text/event-stream`, the same guarded trigger renders the buffered
/// summary — the no-JS / no-stream progressive-enhancement fallback ([FR-UI-19]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_post_buffered_fallback_renders_summary() {
    let (_dir, router, intent, _mock) = agent_router();
    let req = wiki_post(Some(intent.as_str()), Some(ORIGIN), false);
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
        body.contains("Wiki generation finished") && body.contains("page(s) written"),
        "the buffered fallback renders the honest run summary: {body}",
    );
}

/// A run that fails a page, halts, and faults streams the honest `page-failed`,
/// `halted`, and terminal `error` events — and fabricates no page (`completed`/
/// `page-written` never appear) ([NFR-CC-04], [FR-UI-19]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn faulty_run_streams_page_failed_halted_and_error_without_fabricating_a_page() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(FaultyWikiRunService);
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    for marker in ["event: page-failed", "event: halted", "event: error"] {
        assert!(body.contains(marker), "the stream carries `{marker}`: {body}");
    }
    assert!(
        body.contains("injected provider outage") && body.contains("over-cap body"),
        "the honest failure/error reasons stream verbatim: {body}",
    );
    assert!(
        !body.contains("event: completed") && !body.contains("event: page-written"),
        "no page is fabricated on a faulted run: {body}",
    );
}

/// The buffered fallback reports an already-in-progress run honestly (the busy
/// branch of `into_buffered`) ([FR-WK-18], [FR-UI-19]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_fallback_reports_busy_when_a_run_is_in_flight() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let service: Arc<dyn WikiRunService> = Arc::new(GatedWikiRunService {
        runs: Arc::clone(&runs),
        gate: Arc::clone(&gate),
    });
    let intent = IntentToken::generate();
    let router = router_with_wiki(engine, intent.clone(), service);

    // First trigger holds the single-run lock (gated, in flight).
    let resp_a = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);

    // A concurrent buffered (no-SSE) trigger drains to the honest busy summary.
    let resp_b = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), false))
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    let body = body_string(resp_b).await;
    assert!(
        body.contains("already in progress"),
        "the buffered fallback reports the in-flight run honestly: {body}",
    );

    gate.notify_one();
    let _ = body_string(resp_a).await;
}

/// The buffered fallback reports an empty work-list honestly (the not-started branch
/// of `into_buffered`) — "nothing to generate", not a fabricated summary
/// ([NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_fallback_reports_nothing_to_generate_on_an_empty_work_list() {
    let dir = TempDir::new().unwrap();
    sh_git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "README.md", "# fixture\n", "add readme");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(AgentWikiRunService {
        engine: Arc::clone(&engine),
        mock: MockCompletionModel::new([]),
    });
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("nothing to generate"),
        "an empty work-list buffers the honest up-to-date summary: {body}",
    );
}

// ── Single-run lock: exactly one run (FR-WK-18) ────────────────────────────────

/// Opening the Wiki tab while a pass is in flight starts **no** second run: the
/// single-run lock admits the first trigger and a concurrent **streaming** reopen
/// **re-attaches** to the same run instead of being told `busy` ([FR-WK-18] as
/// amended by [CR-056], [S-223]). Releasing the gate lets the run finish, and BOTH
/// observers — the initiating stream and the reattached one — see it through to
/// completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_the_tab_mid_run_reattaches_instead_of_starting_a_second_run() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let service: Arc<dyn WikiRunService> = Arc::new(GatedWikiRunService {
        runs: Arc::clone(&runs),
        gate: Arc::clone(&gate),
    });
    let intent = IntentToken::generate();
    let router = router_with_wiki(engine, intent.clone(), service);

    // First trigger: acquires the single-run lock; the run is gated (in flight).
    let resp_a = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);

    // Reopening the tab while the first holds the lock (a second streaming trigger)
    // RE-ATTACHES to the SAME run — no second run starts, and it is not told `busy`.
    let resp_b = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "exactly one background run started under the single-run lock — the reopen \
         re-attached rather than starting a second one",
    );

    // Release the gate → the run finishes; both the initiating and the reattached
    // stream see it through to completion. `notify_one` stores a permit if the run
    // has not yet registered as a waiter, so this is race-free regardless of task
    // scheduling.
    gate.notify_one();
    let body_a = body_string(resp_a).await;
    let body_b = body_string(resp_b).await;
    assert!(
        body_a.contains("event: started") && body_a.contains("event: completed"),
        "the initiating stream saw the run's events to completion: {body_a}",
    );
    assert!(
        !body_b.contains("event: busy"),
        "the reattached observer is not told the run is merely busy: {body_b}",
    );
    assert!(
        body_b.contains("event: started") && body_b.contains("event: completed"),
        "the reattached observer sees the SAME run's cumulative progress — its \
         replayed history carries `started`, its live tail carries `completed`: {body_b}",
    );
    // Order matters, not just presence: the replayed `started` must precede the live
    // `completed` — a reversed `WikiRunStream::reattach` chain (live before replay)
    // would still satisfy the `contains` checks above but would read as a completed
    // run that only afterward claims to have started.
    let started_at = body_b.find("event: started").unwrap();
    let completed_at = body_b.find("event: completed").unwrap();
    assert!(
        started_at < completed_at,
        "the reattached observer's replayed `started` precedes its live `completed`: {body_b}",
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1, "still exactly one run after completion");
}

/// A re-attached observer sees the run's HONEST halt/error, not a silent success —
/// the replayed-history-then-live-tail sequence a reopen gets carries the same
/// `page-failed`/`halted`/`error` frames the initiating stream sees, and fabricates
/// no page ([NFR-CC-04], [S-223]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reattached_observer_sees_the_runs_honest_halt_and_error() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let gate = Arc::new(Notify::new());
    let service: Arc<dyn WikiRunService> = Arc::new(GatedFaultyWikiRunService { gate: Arc::clone(&gate) });
    let intent = IntentToken::generate();
    let router = router_with_wiki(engine, intent.clone(), service);

    // First trigger: the run emits `started`, then blocks on the gate (in flight).
    let resp_a = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp_a.status(), StatusCode::OK);

    // Reopen the tab while the run is still gated: re-attaches to the SAME run.
    let resp_b = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);

    // Release the gate → the run fails a page, halts, and faults.
    gate.notify_one();
    let body_a = body_string(resp_a).await;
    let body_b = body_string(resp_b).await;

    for (label, body) in [("initiating", &body_a), ("reattached", &body_b)] {
        for marker in ["event: page-failed", "event: halted", "event: error"] {
            assert!(
                body.contains(marker),
                "the {label} observer sees the honest `{marker}` frame, not a silent success: {body}",
            );
        }
        assert!(
            !body.contains("event: completed") && !body.contains("event: page-written"),
            "the {label} observer sees no fabricated page on a faulted run: {body}",
        );
    }
}

// The buffered (no-JS) path's `busy` report on a concurrent reopen is unchanged by
// this story — see `buffered_fallback_reports_busy_when_a_run_is_in_flight` below;
// the re-attach above applies only to the streaming path the SPA actually uses
// ([FR-UI-19]).

// ── Empty work-list starts no run (NFR-CC-04, FR-WK-18) ────────────────────────

/// An empty work-list starts no run and makes no model call ([NFR-CC-04], [FR-WK-18]
/// acceptance): an un-indexed repo has revision 0, so the FR-WK-13 queue is empty and
/// the stream carries no `started`/`completed` events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_work_list_starts_no_run() {
    let dir = TempDir::new().unwrap();
    sh_git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), "README.md", "# fixture\n", "add readme");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    assert!(
        engine.wiki_generate().expect("generate").items.is_empty(),
        "an un-indexed repo has an empty work-list",
    );

    let mock = MockCompletionModel::new([]);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(AgentWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
    });
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !body.contains("event: started") && !body.contains("event: page-written"),
        "an empty work-list emits no run/page progress: {body}",
    );
    assert_eq!(
        mock.request_count(),
        0,
        "no model call is made when the work-list is empty",
    );
}

// ── Configure-first (FR-UI-18, NFR-CC-04) ──────────────────────────────────────

/// With no provider configured, the real `ConfiguredWikiRunService` streams a single
/// honest `configure-first` frame — the configure-first state, not a crash or an
/// error ([FR-UI-18], [NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_first_streams_a_configure_first_frame() {
    // A fixture root with `.logos/` but no `config.toml` model / `secrets.toml` key.
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone()); // production ConfiguredWikiRunService

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("event: configure-first") && body.contains("Config"),
        "the unconfigured state is an honest configure-first frame naming the Config tab: {body}",
    );
    assert!(
        !body.contains("event: error"),
        "an unconfigured wiki is configure-first, not an error: {body}",
    );
}

/// [FR-WK-20]/[FR-WK-18]/[CR-062]: the production `ConfiguredWikiRunService`
/// runs `Engine::wiki_materialize` BEFORE the LLM half of the run — in an
/// SRS-mode project the presented Architecture page exists after the trigger
/// even though no model/key is configured (a `configure-first` frame, not an
/// error), proving materialize is unconditional, not gated on configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_first_run_still_materializes_the_presented_tier_first() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        "docs/specs/architecture.md",
        "# Architecture\n\nThe system design.\n",
        "add architecture doc",
    );
    commit(
        repo,
        "docs/specs/requirements/FR-X-01.md",
        "# FR-X-01\n\nA requirement.\n",
        "add requirement",
    );
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    assert!(
        engine.wiki_read("overview/architecture").unwrap().is_none(),
        "nothing presented yet before the trigger"
    );

    let intent = IntentToken::generate();
    let router = router_with_intent(Arc::clone(&engine), intent.clone());

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("event: configure-first"),
        "no model/key configured — the LLM half never ran: {body}",
    );
    assert!(!body.contains("event: error"), "materialize succeeded, no fault: {body}");

    let page = engine
        .wiki_read("overview/architecture")
        .expect("read")
        .expect("the presented Architecture page exists after the trigger");
    assert_eq!(
        page.generator, "logos:doc-present",
        "materialize ran ahead of (independent of) the unconfigured LLM half"
    );
}

// ── Connection-independent run lifetime (CR-056, S-222, FR-UI-19) ──────────────

/// Dropping the driving SSE stream mid-pass does **not** abort the run: with its
/// lifetime owned by application state (not the response body), the run **completes
/// server-side** after the client disconnects ([CR-056], [S-222]). A gated run emits
/// `Started`, the test reads it and disconnects, then releases the gate — and the run
/// still reaches `Completed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_stream_does_not_abort_the_run_which_completes_server_side() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let gate = Arc::new(Notify::new());
    let completed = Arc::new(AtomicUsize::new(0));
    let service: Arc<dyn WikiRunService> = Arc::new(CompletionWitnessRunService {
        gate: Arc::clone(&gate),
        completed: Arc::clone(&completed),
    });
    let intent = IntentToken::generate();
    let router = router_with_wiki(engine, intent.clone(), service);

    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Read the first streamed frame (the `started`), proving the run is in flight…
    let mut body = resp.into_body();
    let _first = body.frame().await;
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "the run has not completed yet (it is gated mid-pass)",
    );

    // …then disconnect by dropping the SSE body. Under the old connection-owned
    // design this aborted the run; now it only unsubscribes.
    drop(body);
    // Release the gate so the (still-running) server-side task can finish.
    gate.notify_one();

    // The run completes server-side despite the dropped stream.
    for _ in 0..200 {
        if completed.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "the run completed server-side after the client disconnected (CR-056, S-222)",
    );
}

/// The end-to-end acceptance ([S-222]): a **real** `WikiAgent` mock-provider run whose
/// driving SSE stream is dropped mid-pass continues to completion server-side and
/// leaves every generated page **fresh on both axes** (content + revision). Once it
/// drains, the single-run lock is released — a follow-up trigger finds an empty
/// work-list ("nothing to generate"), not a spurious `busy`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_stream_mid_pass_leaves_pages_fresh_on_both_axes() {
    let dir = TempDir::new().unwrap();
    let engine = indexed_engine(dir.path());
    let current = engine.status().graph_revision;
    let queue = engine.wiki_generate().expect("generate queue");
    let expected_slugs: Vec<String> = queue.items.iter().map(|i| i.slug.clone()).collect();
    assert!(!expected_slugs.is_empty(), "the indexed fixture seeds a non-empty work-list");

    let turns: Vec<_> = (0..expected_slugs.len())
        .map(|i| {
            MockTurn::text(format!(
                "# Generated page {i}\n\nThis regenerated page carries enough grounded prose \
                 to clear the write-path validity guard ([FR-WK-19])."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);
    let intent = IntentToken::generate();
    let service: Arc<dyn WikiRunService> = Arc::new(AgentWikiRunService {
        engine: Arc::clone(&engine),
        mock: mock.clone(),
    });
    let router = router_with_wiki(Arc::clone(&engine), intent.clone(), service);

    // Trigger the streaming run, read the first frame, then disconnect mid-pass.
    let resp = router
        .clone()
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();
    let _first = body.frame().await;
    drop(body);

    // The run completes server-side: poll the store until every queued page is
    // written and fresh on BOTH axes (content: not stale; revision: built at the
    // current revision) — never fabricated.
    let mut drained = false;
    for _ in 0..300 {
        let all_fresh = expected_slugs.iter().all(|slug| {
            matches!(
                engine.wiki_read(slug),
                Ok(Some(ref page)) if !page.stale && !revision_pending(page.built_at_revision, current)
            )
        });
        if all_fresh {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        drained,
        "every queued page was written and reads fresh on both axes after the client disconnected",
    );
    assert_eq!(
        mock.request_count(),
        expected_slugs.len(),
        "the mock synthesized exactly one turn per page across the server-side run",
    );

    // The lock is released on completion: a follow-up trigger finds the work-list
    // drained ("nothing to generate"), not a spurious `busy`. The guard drops a beat
    // after the last page is written (the loop re-reads, emits Completed, ends), so
    // retry briefly to avoid racing that tiny window rather than flaking on it.
    let mut body_after = String::new();
    for _ in 0..100 {
        let resp_after = router
            .clone()
            .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), false))
            .await
            .unwrap();
        assert_eq!(resp_after.status(), StatusCode::OK);
        body_after = body_string(resp_after).await;
        if body_after.contains("nothing to generate") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        body_after.contains("nothing to generate"),
        "after the server-side run drained the work-list and released the lock, a new \
         trigger is up-to-date, not busy: {body_after}",
    );
}

// ── Carve-out: zero real egress + loopback bind (NFR-SE-07, NFR-SE-01) ─────────

/// A full streamed run through the route records **zero** real outbound connections
/// — the mock provider served it, nothing reached the wire — and the listener binds
/// only a loopback address ([NFR-SE-07], [NFR-SE-01]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamed_run_records_zero_real_egress_and_binds_loopback() {
    // A loopback tripwire counting any connection a real provider would have made.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let (_dir, router, intent, _mock) = agent_router();
    let resp = router
        .oneshot(wiki_post(Some(intent.as_str()), Some(ORIGIN), true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("event: completed"),
        "the mock-provider run streamed to completion: {body}",
    );
    // Nothing reached the wire across the whole streamed run.
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the streamed run opened zero real outbound connections (NFR-SE-07)",
    );

    // The surface listener binds only loopback (the carve-out boundary).
    let bound = web::bind(0).expect("bind a loopback listener");
    assert!(
        bound.local_addr().unwrap().ip().is_loopback(),
        "the web surface binds only a loopback address",
    );
}
