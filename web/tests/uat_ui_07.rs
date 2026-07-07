//! [UAT-UI-07] acceptance scenario — the agentic chat increment end-to-end over
//! the assembled `serve --ui` surface (S-172, [CR-045], [CR-046], [FR-UI-18],
//! [FR-UI-19], [FR-UI-20], [FR-CF-06], [NFR-SE-01], [NFR-SE-04], [NFR-SE-06],
//! [NFR-SE-07], [NFR-CC-04], [ADR-40], [ADR-41]).
//!
//! These drive the **real router** in-process exactly as `serve --ui` does
//! (`tower::ServiceExt::oneshot`, no socket bound), with a **mock-provider**
//! [`ChatService`](web::chat::ChatService) injected via [`web::router_with_chat`]
//! so the whole orchestrated flow runs **offline — zero real egress**. This is
//! the It7 regression lock the sprint risk table names: the carve-out's
//! byte-identical no-networking-crate fitness check ran in It1 (S-166) and is
//! proven again here over the *full orchestrated chat flow*.
//!
//! It is the cohesive scenario that ties together the per-story carve-out guards
//! built incrementally across the sprint — it does not re-prove their mechanics
//! (those stay in their own suites) but re-walks the [UAT-UI-07] steps as one
//! narrative, adding the integration coverage no single story test has: a
//! **compound, multi-subagent, multi-tool turn driven through the real SSE route,
//! proven zero-egress + loopback**.
//!
//! Mapping to the [UAT-UI-07] steps:
//! - step 1 — configure-first state with no provider
//!   (`uat_ui_07_step1_unconfigured_chat_is_configure_first`);
//! - steps 2/7 — the key writes the gitignored `secrets.toml` masked, and never
//!   surfaces on the Config page **or** the chat surface
//!   (`uat_ui_07_steps_2_7_key_writes_masked_and_never_echoed_on_any_surface`);
//! - steps 3/4/10 — a compound question plans, dispatches ≥2 specialized
//!   subagents each invoking ≥1 grounded tool, and streams a synthesized answer
//!   over SSE with **zero real egress** on a loopback-only listener
//!   (`uat_ui_07_steps_3_4_10_compound_turn_streams_two_subagents_grounded_zero_egress`);
//! - step 3 (the consent disclosure) names the configured endpoint before
//!   the composer (`uat_ui_07_step3_consent_disclosure_names_configured_endpoint`);
//! - step 5 — a Source-Reader sandbox-escape attempt is refused honestly
//!   end-to-end (`uat_ui_07_step5_source_sandbox_escape_is_refused_honestly`);
//! - step 6 — the budget tree halts at **each** of its three bounds, reporting
//!   which one, never fabricating an answer
//!   (`uat_ui_07_step6_budget_tree_halts_at_each_bound` +
//!   `uat_ui_07_step6_a_budget_halt_streams_an_honest_halted_event`);
//! - step 6b — per-thread memory survives a restart and Clear-history wipes it
//!   (`uat_ui_07_step6b_memory_persists_across_restart_then_clear_wipes_it`);
//! - step 8 — a non-chat `POST` is `405` and a forged/intent-less chat `POST` is
//!   `403` (`uat_ui_07_step8_non_chat_and_forged_chat_writes_are_rejected`);
//! - step 9 — Clear-history returns the surface to an empty state
//!   (`uat_ui_07_step9_clear_history_returns_an_empty_state`).
//!
//! The **structural** halves of step 10 — the default-feature no-HTTP-client scan
//! ([NFR-SE-01]) and the ui-vs-default `rig`/`reqwest` boundary — are guarded by
//! `logos-core/tests/no_network_deps.rs` and `agent-core/tests/carve_out.rs`
//! (unchanged, run as part of this story's verification); like `uat_ui_06.rs`'s
//! step-9 note, this scenario asserts the *behavioral* carve-out and defers the
//! dependency-tree scan to those canonical guards rather than duplicating it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use chat_agent::orchestrator::{
    BudgetBound, BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, PlanStep, RoleModels,
    StepContext, StepError, StepExecutor, StepObservation, SubagentRoster, TurnOutcome,
};
use chat_agent::{ChatRole, ChatStore, MemoryGrounding, MemoryStore};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::chat::{spawn_turn, ChatService, ChatStream};
use web::{
    router_with_chat, router_with_intent, IntentToken, CHAT_CLEAR_ROUTE, CHAT_POST_ROUTE,
    INTENT_HEADER,
};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
/// The synthesized final answer the compound mock planner returns — the sentinel
/// proving the streamed answer is the orchestrator's, not a fabricated pass-through.
const SYNTH_SENTINEL: &str = "UAT_UI_07_SYNTHESIZED_ANSWER";

// ── Mock-provider chat services (offline — the zero-egress substrate) ──────────

/// A [`ChatService`] backed by the **real** orchestrator + subagent roster over
/// the offline mock provider, scripted to run a **compound** turn: the planner
/// dispatches a graph step, a source step, and a synthesizer step, each routed to
/// its specialized subagent, then finalizes with [`SYNTH_SENTINEL`]. The
/// per-role models are held so the test can prove (via `request_count`) that each
/// subagent genuinely invoked its tool.
struct CompoundChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: PathBuf,
    planner: MockCompletionModel,
    graph: MockCompletionModel,
    source: MockCompletionModel,
    synthesizer: MockCompletionModel,
}

impl ChatService for CompoundChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("compound").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");
        let grounding = Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread, turn));

        let roster = SubagentRoster::with_models(
            Arc::clone(&self.engine),
            Arc::clone(&self.sandbox),
            RoleModels {
                graph_navigator: self.graph.clone(),
                governance_analyst: MockCompletionModel::new([]),
                source_reader: self.source.clone(),
                synthesizer: self.synthesizer.clone(),
            },
        )
        .with_synthesizer_grounding(grounding);
        let orchestrator = Orchestrator::new(self.planner.clone(), roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

/// A [`ChatService`] whose Source-Reader attempts to `read` **outside** the
/// project root — the sandbox refuses it, the step fails honestly, and the turn
/// surfaces an honest `error` event with no fabricated answer ([NFR-SE-04],
/// [NFR-CC-04]).
struct SandboxEscapeChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: PathBuf,
}

impl ChatService for SandboxEscapeChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = thread_id.unwrap_or_else(|| chat.create_thread("escape").expect("thread"));
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        let planner = MockCompletionModel::new([MockTurn::text(plan_json(
            "source_reader",
            "read a file outside the project root",
        ))]);
        // The Source-Reader is in its own domain (`read` is a source tool), but the
        // path escapes the root — the sandbox refuses it at call time.
        let source = MockCompletionModel::new([MockTurn::tool_call(
            "s1",
            "read",
            serde_json::json!({ "path": "../../etc/passwd" }),
        )]);
        let roster = SubagentRoster::with_models(
            Arc::clone(&self.engine),
            Arc::clone(&self.sandbox),
            RoleModels {
                graph_navigator: MockCompletionModel::new([]),
                governance_analyst: MockCompletionModel::new([]),
                source_reader: source,
                synthesizer: MockCompletionModel::new([]),
            },
        );
        let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

// ── Fixtures + request builders ───────────────────────────────────────────────

/// A writable fixture root with `.logos/` and a real `src/lib.rs` so the source
/// `read` tool returns grounded content and the graph tools have a runtime.
fn fixture_root() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn alpha() { beta(); }\npub fn beta() {}\n",
    )
    .expect("fixture src");
    dir
}

/// A started engine + sandbox over `dir` — the `serve --ui` shape the graph tools
/// need (a runtime to read on) and the source tools need (a root to confine to).
fn started_engine(dir: &Path) -> (Arc<Engine>, Arc<Sandbox>) {
    let engine = Arc::new(Engine::start(dir).expect("engine starts"));
    let sandbox = Arc::new(Sandbox::new(dir, std::iter::empty()).expect("sandbox"));
    (engine, sandbox)
}

/// A fixture root with a configured OpenAI-compatible provider (model + key) so
/// the Chat surface renders the consent banner + composer.
fn configured_root() -> TempDir {
    let dir = fixture_root();
    std::fs::write(
        dir.path().join(".logos/config.toml"),
        "[chat]\nprovider = \"openai\"\nmodel = \"openrouter/test-model\"\n\
         base_url = \"https://openrouter.ai/api/v1\"\n",
    )
    .expect("write config.toml");
    std::fs::write(
        dir.path().join(".logos/secrets.toml"),
        "[chat]\napi_key = \"sk-test-abcd1234\"\n",
    )
    .expect("write secrets.toml");
    dir
}

/// JSON the mock planner returns for a single-step `plan` decision.
fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

/// JSON the mock planner returns for a `final` decision.
fn final_json(answer: &str) -> String {
    format!(r#"{{"action":"final","answer":"{answer}"}}"#)
}

/// JSON the mock planner returns for a `plan` decision with **no** steps. Such a
/// plan records no observation, so a subsequent max-replans halt is reached with
/// an empty scratchpad — the honest bare-halt path ([CR-048]/[NFR-CC-04]), as
/// opposed to the best-effort A′ answer a populated scratchpad would yield.
fn empty_plan_json() -> String {
    r#"{"action":"plan","steps":[]}"#.to_string()
}

/// A `GET` request to a view path.
fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

/// A `POST /chat` form request, optionally carrying an `Origin`, the intent
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

/// A `POST` to an arbitrary route with an optional same-origin `Origin` and intent
/// token, carrying a urlencoded form body — for the guarded `/config/secret` write
/// and the `405`/`403` guard assertions.
fn form_post(path: &str, intent: Option<&str>, origin: Option<&str>, body: &'static str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::from(body)).unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ── Step 1: configure-first state ─────────────────────────────────────────────

/// Step 1: opening Chat with **no** provider configured renders a configure-first
/// state linking to Config — not an error or a crash ([FR-UI-18], [NFR-CC-04]).
#[tokio::test]
async fn uat_ui_07_step1_unconfigured_chat_is_configure_first() {
    let dir = fixture_root();
    let engine = Arc::new(Engine::open(dir.path()));
    let router = router_with_intent(engine, IntentToken::generate());

    // The SPA Chat view renders the configure-first state client-side from GET
    // /api/v1/config (S-192: the server-rendered /chat HTML is retired; visible
    // rendering is covered by the chat Vitest suite, S-190). The Rust-side
    // guarantee is that the read-model honestly reports no provider/key configured.
    let resp = router.oneshot(get("/api/v1/config")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the config read-model answers 200, not a 5xx");
    let body = body_string(resp).await;
    // Configure-first = no API key written and no user config on disk; the parsed
    // model carries a *default* provider/base_url, so the signal the SPA keys on is
    // the absent key, not the endpoint default.
    assert!(body.contains("\"present\":false"), "no chat key is configured: {body}");
    assert!(
        body.contains("\".logos/config.toml\",\"exists\":false"),
        "no user config.toml is written (configure-first data state): {body}",
    );
}

// ── Steps 2/7: masked key write, never echoed on any surface ───────────────────

/// Steps 2 + 7: the chat API key writes the **gitignored** `secrets.toml`, the
/// write response and a subsequent `GET /config` render only the masked presence
/// (last-4), and the raw key never appears on the Config page **or** the chat
/// surface ([FR-CF-06], [NFR-SE-07]). The per-guard `/config/secret` mechanics
/// (403s, at-rest persistence) live in `tests/carve_out.rs`; this asserts the
/// **never-on-the-chat-surface** angle the scenario adds.
#[tokio::test]
async fn uat_ui_07_steps_2_7_key_writes_masked_and_never_echoed_on_any_surface() {
    const RAW_KEY: &str = "sk-or-v1-uat07-secret-5150";
    let dir = fixture_root();
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());

    // Step 2: write the key through the guarded secret route.
    let resp = router
        .clone()
        .oneshot(form_post(
            "/config/secret",
            Some(intent.as_str()),
            Some(ORIGIN),
            "api_key=sk-or-v1-uat07-secret-5150",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a guarded secret write is accepted");
    let write_body = body_string(resp).await;
    assert!(!write_body.contains(RAW_KEY), "the write response never echoes the raw key");
    assert!(write_body.contains("5150"), "the masked response carries the last-4: {write_body}");

    // It persisted at rest in the gitignored secrets.toml (never config.toml).
    let secrets = std::fs::read_to_string(dir.path().join(".logos/secrets.toml")).unwrap();
    assert!(secrets.contains(RAW_KEY), "the key persists in the gitignored secrets.toml");

    // Step 7: the masked presence renders through the SPA's data source, GET
    // /api/v1/config; the raw key is nowhere (last-4 only, masked by construction).
    let page = body_string(router.clone().oneshot(get("/api/v1/config")).await.unwrap()).await;
    assert!(!page.contains(RAW_KEY), "the config read-model never contains the raw key");
    assert!(page.contains("5150"), "the config read-model shows the masked last-4");

    // The chat surface (the SPA shell served at /chat) likewise never carries the
    // raw key — the secret lives only in the masked read-model and secrets.toml.
    let chat_page = body_string(router.oneshot(get("/chat")).await.unwrap()).await;
    assert!(!chat_page.contains(RAW_KEY), "the Chat surface never contains the raw key");
}

// ── Steps 3/4/10: compound turn → plan + ≥2 subagents + synthesis, zero egress ─

/// Steps 3/4/10 (the centerpiece): a configured, guarded `POST /chat` with
/// `Accept: text/event-stream` runs a **compound** turn — the planner produces a
/// plan and dispatches **two specialized subagents each invoking a grounded tool**
/// (Graph-Navigator → `search`, Source-Reader → `read`) plus the tool-less
/// Synthesizer, then streams a synthesized answer — all over SSE under the
/// unchanged self-only CSP, with **zero real outbound connections** and a
/// loopback-only listener ([FR-UI-19], [NFR-SE-07], [NFR-SE-01], [UAT-UI-07]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_ui_07_steps_3_4_10_compound_turn_streams_two_subagents_grounded_zero_egress() {
    // A loopback tripwire counting any connection a real provider would have made.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let dir = fixture_root();
    let (engine, sandbox) = started_engine(dir.path());
    let intent = IntentToken::generate();

    // The compound plan: a graph step, a source step, a synthesizer step, then the
    // final synthesized answer — four planner turns over the mock.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "search for beta")),
        MockTurn::text(plan_json("source_reader", "read src/lib.rs")),
        MockTurn::text(plan_json("synthesizer", "compose the grounded answer")),
        MockTurn::text(final_json(SYNTH_SENTINEL)),
    ]);
    // Each subagent's model makes one tool round then a summary round; the held
    // clones let the test prove (via request_count) each subagent ran its tool.
    let graph = MockCompletionModel::new([
        MockTurn::tool_call("g1", "search", serde_json::json!({ "query": "beta" })),
        MockTurn::text("graph: searched for beta"),
    ]);
    let source = MockCompletionModel::new([
        MockTurn::tool_call("s1", "read", serde_json::json!({ "path": "src/lib.rs" })),
        MockTurn::text("source: read src/lib.rs"),
    ]);
    let synthesizer = MockCompletionModel::new([MockTurn::text("synthesized from the scratchpad")]);

    let service: Arc<dyn ChatService> = Arc::new(CompoundChatService {
        engine: Arc::clone(&engine),
        sandbox,
        root: dir.path().to_path_buf(),
        planner: planner.clone(),
        graph: graph.clone(),
        source: source.clone(),
        synthesizer: synthesizer.clone(),
    });
    let router = router_with_chat(engine, intent.clone(), service);

    let resp = router
        .oneshot(chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=what+is+riskiest+and+who+calls+it"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "the response is a Server-Sent Events stream: {content_type}",
    );
    assert!(
        resp.headers().contains_key(header::CONTENT_SECURITY_POLICY),
        "the streaming response carries the unchanged self-only CSP",
    );

    let body = body_string(resp).await;

    // The plan, the two specialized subagents (each observed), the synthesizer,
    // and the synthesized final answer all streamed, tagged, in order.
    for marker in ["event: plan", "event: step_started", "event: step_observed", "event: final_answer"] {
        assert!(body.contains(marker), "the stream carries `{marker}`: {body}");
    }
    // ≥2 distinct specialized subagents were dispatched, scoped to the
    // `step_observed` frames (their roles ride that event's JSON, snake_case-tagged)
    // so a wrong-role observation could not pass on a `step_started` role alone.
    let observed: String = body
        .split("\n\n")
        .filter(|frame| frame.contains("event: step_observed"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(observed.contains("\"role\":\"graph_navigator\""), "the Graph-Navigator step was observed: {body}");
    assert!(observed.contains("\"role\":\"source_reader\""), "the Source-Reader step was observed: {body}");
    assert!(observed.contains("\"role\":\"synthesizer\""), "the Synthesizer step was observed: {body}");
    let plan_at = body.find("event: plan").unwrap();
    let final_at = body.find("event: final_answer").unwrap();
    assert!(plan_at < final_at, "events stream in order (plan before answer)");
    assert!(body.contains(SYNTH_SENTINEL), "the streamed answer is the orchestrator's synthesis: {body}");

    // The Graph-Navigator and Source-Reader each satisfy "≥2 subagents, ≥1 tool
    // each": a tool round + a summary round ⇒ request_count == 2 (the mock is scripted
    // tool-call-then-summary, so 2 is only reachable via the tool cycle; that the tool
    // actually executed and charged the budget is asserted in
    // `chat-agent/tests/roster.rs`). The Synthesizer is a third, intentionally tool-less
    // subagent that ran exactly once.
    assert_eq!(graph.request_count(), 2, "the Graph-Navigator ran its tool then summarized");
    assert_eq!(source.request_count(), 2, "the Source-Reader ran its tool then summarized");
    assert_eq!(synthesizer.request_count(), 1, "the tool-less Synthesizer ran once");

    // Step 10: the behavioral zero-egress guarantee is that the turn ran on the mock
    // provider — no `reqwest` client is constructed on this path at all — and the
    // structural guarantee (no HTTP-client crate in the default tree) lives in
    // `logos-core/tests/no_network_deps.rs` + `agent-core/tests/carve_out.rs`. The
    // loopback tripwire is a belt-and-suspenders regression guard: it confirms nothing
    // dialed the monitoring socket across the whole compound streamed turn, and the
    // surface listener binds only loopback.
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "nothing connected to the monitoring loopback socket during the turn (UAT-UI-07)",
    );
    let bound = web::bind(0).expect("bind a loopback listener");
    assert!(
        bound.local_addr().unwrap().ip().is_loopback(),
        "the web surface binds only a loopback address",
    );
}

// ── Step 5: sandbox escape is refused honestly end-to-end ─────────────────────

/// Step 5: a Source-Reader attempt to `read` outside the project root is refused
/// by the sandbox; the turn surfaces an honest `error` event and **no** answer is
/// fabricated ([NFR-SE-04], [NFR-CC-04]). The per-path sandbox refusals (`..`,
/// absolute, `ignored_dirs`, symlink) are exhaustively covered in
/// `agent-core/tests/sandbox.rs`; this proves the refusal holds **through the
/// orchestrated chat flow**, not only at the tool seam.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_ui_07_step5_source_sandbox_escape_is_refused_honestly() {
    let dir = fixture_root();
    let (engine, sandbox) = started_engine(dir.path());
    let intent = IntentToken::generate();
    let service: Arc<dyn ChatService> = Arc::new(SandboxEscapeChatService {
        engine: Arc::clone(&engine),
        sandbox,
        root: dir.path().to_path_buf(),
    });
    let router = router_with_chat(engine, intent.clone(), service);

    let resp = router
        .oneshot(chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=read+outside+the+root"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    assert!(body.contains("event: error"), "the sandbox refusal surfaces as an honest error: {body}");
    assert!(
        body.contains("escapes the project root"),
        "the error names the sandbox refusal (NFR-SE-04): {body}",
    );
    assert!(!body.contains("event: final_answer"), "no answer is fabricated on a refused escape: {body}");
}

// ── Step 6: the budget tree halts at each of its three bounds ──────────────────

/// A scripted stand-in for the subagent roster that charges `calls_per_step` tool
/// calls through the [`StepContext`] (exercising the budget tree) — a budget
/// refusal propagates as [`StepError::Budget`], never a fabricated observation.
struct ChargingExecutor {
    calls_per_step: usize,
}

impl StepExecutor for ChargingExecutor {
    fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> impl std::future::Future<Output = Result<StepObservation, StepError>> + Send {
        let calls = self.calls_per_step;
        let role = step.role;
        async move {
            for _ in 0..calls {
                ctx.charge_tool_call()?;
            }
            Ok(StepObservation::new(format!("{role:?} ran {calls} tool call(s)")))
        }
    }
}

/// Step 6: the budget tree halts a long turn at the **first** bound reached and
/// reports **which** one — exercised across **all three** bounds (global
/// `max_tool_calls`, per-subagent `max_subagent_tool_calls`, `max_replans`) — with
/// no fabricated answer on any halt ([ADR-41], [NFR-CC-04]).
#[tokio::test]
async fn uat_ui_07_step6_budget_tree_halts_at_each_bound() {
    // Global ceiling = 1, the step charges 2 → the second call trips the GLOBAL bound.
    assert_halts_at(
        BudgetTree::new(1, 8, 3),
        ChargingExecutor { calls_per_step: 2 },
        &[plan_json("graph_navigator", "deep dive")],
        BudgetBound::GlobalToolCalls { limit: 1 },
    )
    .await;

    // Per-subagent cap = 1 (global high), the step charges 2 → the PER-SUBAGENT
    // bound binds before the global ceiling.
    assert_halts_at(
        BudgetTree::new(24, 1, 3),
        ChargingExecutor { calls_per_step: 2 },
        &[plan_json("source_reader", "read everything")],
        BudgetBound::SubagentToolCalls { limit: 1 },
    )
    .await;

    // max_replans = 1, a planner that never finalizes with empty (no-step) plans →
    // the third plan request trips the REPLANS bound. Empty plans keep the
    // scratchpad empty, so the hard halt is a bare, honest one naming the bound
    // (a populated scratchpad would instead yield a best-effort A′ answer, [CR-048]).
    assert_halts_at(
        BudgetTree::new(24, 8, 1),
        ChargingExecutor { calls_per_step: 0 },
        &[empty_plan_json(), empty_plan_json(), empty_plan_json()],
        BudgetBound::Replans { limit: 1 },
    )
    .await;
}

/// Run a turn whose planner returns `plans` (each a `plan` decision) under
/// `budget` + `executor`, and assert it halts at exactly `expected`, emitting an
/// honest `Halted` event with **no** fabricated `FinalAnswer`.
async fn assert_halts_at(
    budget: BudgetTree,
    executor: ChargingExecutor,
    plans: &[String],
    expected: BudgetBound,
) {
    let planner = MockCompletionModel::new(plans.iter().map(|p| MockTurn::text(p.clone())));
    let orchestrator = Orchestrator::new(planner, executor, budget);
    let sink = CapturingSink::new();
    let outcome = orchestrator.run("a long question", &sink).await.unwrap();

    assert_eq!(outcome, TurnOutcome::Halted(expected), "the turn halts at the first bound reached");
    let events = sink.events();
    assert!(
        events.iter().any(|e| matches!(e, OrchestratorEvent::Halted { bound, .. } if *bound == expected)),
        "an honest Halted event names the bound: {events:?}",
    );
    assert!(
        !events.iter().any(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. })),
        "no answer is fabricated on a halt: {events:?}",
    );
}

/// Step 6 (through the route): a budget halt streams an honest `halted` SSE event
/// naming the bound, and no `final_answer` — exercised for **all three** bounds
/// over the wire (global / per-subagent / max-replans), so the route-observable
/// halt is proven for each, not only at the orchestrator seam ([FR-UI-19],
/// [NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_ui_07_step6_a_budget_halt_streams_an_honest_halted_event() {
    // A halting service parametrized by the budget tree, the per-step tool charge,
    // and the planner's plan rounds — one harness drives each bound to a halt.
    struct HaltingChatService {
        root: PathBuf,
        budget: (usize, usize, u32),
        calls_per_step: usize,
        plans: Vec<String>,
    }
    impl ChatService for HaltingChatService {
        fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
            let mut chat = ChatStore::open(&self.root).expect("open chat store");
            let thread = thread_id.unwrap_or_else(|| chat.create_thread("halt").expect("thread"));
            let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
            let turn = memory.next_turn(thread).expect("turn");
            let planner =
                MockCompletionModel::new(self.plans.iter().map(|p| MockTurn::text(p.clone())));
            let (g, sub, replans) = self.budget;
            let orchestrator = Orchestrator::new(
                planner,
                ChargingExecutor { calls_per_step: self.calls_per_step },
                BudgetTree::new(g, sub, replans),
            );
            spawn_turn(orchestrator, question, memory, thread, turn)
        }
    }

    // One bound per case: the budget tree, the per-step tool charge, the planner's
    // plan rounds, and the serialized bound tag the SSE `halted` event must carry.
    struct HaltCase {
        budget: (usize, usize, u32),
        calls_per_step: usize,
        plans: Vec<String>,
        expected_bound: &'static str,
    }
    let cases = [
        // Global ceiling 1, a step charging 2 → the global bound.
        HaltCase {
            budget: (1, 8, 3),
            calls_per_step: 2,
            plans: vec![plan_json("graph_navigator", "deep dive")],
            expected_bound: "global_tool_calls",
        },
        // Per-subagent cap 1 (global high), a step charging 2 → the per-subagent bound.
        HaltCase {
            budget: (24, 1, 3),
            calls_per_step: 2,
            plans: vec![plan_json("source_reader", "read all")],
            expected_bound: "subagent_tool_calls",
        },
        // max_replans 1, a never-finalizing planner with empty (no-step) plans →
        // the replans bound, reached with an empty scratchpad so the halt is bare.
        HaltCase {
            budget: (24, 8, 1),
            calls_per_step: 0,
            plans: vec![empty_plan_json(), empty_plan_json(), empty_plan_json()],
            expected_bound: "replans",
        },
    ];

    for HaltCase { budget, calls_per_step, plans, expected_bound } in cases {
        let dir = fixture_root();
        let root = dir.path().to_path_buf();
        let intent = IntentToken::generate();
        let service: Arc<dyn ChatService> = Arc::new(HaltingChatService {
            root: root.clone(),
            budget,
            calls_per_step,
            plans,
        });
        let router = router_with_chat(Arc::new(Engine::open(&root)), intent.clone(), service);

        let resp = router
            .oneshot(chat_post(Some(intent.as_str()), Some(ORIGIN), true, "q=halt+me"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("event: halted"),
            "{expected_bound}: an honest halted event streams: {body}",
        );
        assert!(
            body.contains(&format!("\"bound\":\"{expected_bound}\"")),
            "{expected_bound}: the halted event names which bound was hit: {body}",
        );
        assert!(
            !body.contains("event: final_answer"),
            "{expected_bound}: no answer is fabricated on a halt: {body}",
        );
    }
}

// ── Step 6b: memory persists across a restart; Clear-history wipes it ──────────

/// Step 6b: per-thread working memory + the conversation survive a simulated
/// `serve --ui` restart (stores dropped and re-opened from the same files), so a
/// follow-up sees prior context; then Clear-history wipes both via the live FK
/// cascade ([FR-UI-20], [S-168], [S-175]).
#[test]
fn uat_ui_07_step6b_memory_persists_across_restart_then_clear_wipes_it() {
    let dir = fixture_root();
    let root = dir.path();

    let thread = {
        let mut chat = ChatStore::open(root).unwrap();
        let memory = MemoryStore::open(root).unwrap();
        let thread = chat.create_thread("about the binder").unwrap();
        chat.append_message(thread, ChatRole::User, "where is the binder?", &[]).unwrap();
        chat.append_message(thread, ChatRole::Assistant, "It lives in binder.rs.", &[]).unwrap();
        memory.set_working_memory(thread, "User asked where the binder is; answer: binder.rs.").unwrap();
        thread
        // Both stores drop here — connections (and WAL) close: the restart.
    };

    // Re-open from the same files: brand-new handles over the persisted data.
    let chat = ChatStore::open(root).unwrap();
    let memory = MemoryStore::open(root).unwrap();
    assert_eq!(
        memory.working_memory(thread).unwrap().as_deref(),
        Some("User asked where the binder is; answer: binder.rs."),
        "working memory survived the restart (a follow-up sees prior context)",
    );
    assert_eq!(chat.messages(thread).unwrap().len(), 2, "the prior transcript persisted too");

    // Clear-history (one DELETE FROM chat_threads) wipes the conversation and its
    // per-thread memory via the FK cascade.
    let mut chat = ChatStore::open(root).unwrap();
    assert_eq!(chat.clear_history().unwrap(), 1, "the one thread is removed");
    let memory = MemoryStore::open(root).unwrap();
    assert!(memory.working_memory(thread).unwrap().is_none(), "memory cascaded away");
    assert!(memory.is_empty().unwrap(), "no orphaned memory survives Clear-history");
    assert!(ChatStore::open(root).unwrap().is_empty().unwrap(), "the conversation is wiped");
}

// ── Step 8: non-chat and forged chat writes are rejected ──────────────────────

/// Step 8: the chat route does not widen the read-only posture — a `POST` to a
/// non-chat route is `405`, and a cross-origin or intent-less `POST /chat` is
/// `403` before any turn starts ([NFR-SE-06]).
#[tokio::test]
async fn uat_ui_07_step8_non_chat_and_forged_chat_writes_are_rejected() {
    let dir = configured_root();
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());

    // A POST to a non-chat route is 405 (the relaxation is bounded).
    let resp = router
        .clone()
        .oneshot(form_post("/graph", Some(intent.as_str()), Some(ORIGIN), "x=1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "a non-chat POST is 405");

    // A cross-origin chat POST (browser-set Origin is the attacker's) is 403.
    let resp = router
        .clone()
        .oneshot(chat_post(Some(intent.as_str()), Some("http://evil.example.com"), true, "q=hi"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a cross-origin chat POST is rejected");

    // A same-origin chat POST without the intent token is 403.
    let resp = router
        .oneshot(chat_post(None, Some(ORIGIN), true, "q=hi"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "an intent-less chat POST is rejected");
}

// ── Step 9: Clear-history returns an empty state ──────────────────────────────

/// Step 9: the guarded `POST /chat/clear` empties the conversation store and
/// returns the surface to an empty state ([FR-UI-18], [FR-UI-20]).
#[tokio::test]
async fn uat_ui_07_step9_clear_history_returns_an_empty_state() {
    let dir = configured_root();
    let root = dir.path().to_path_buf();

    // Seed a conversation.
    {
        let mut store = ChatStore::open(&root).expect("open chat store");
        let thread = store.create_thread("seeded").expect("thread");
        store.append_message(thread, ChatRole::User, "hello", &[]).expect("append");
        MemoryStore::open(&root)
            .expect("open memory")
            .set_working_memory(thread, "a prior-turn summary")
            .expect("seed working memory");
        assert!(!store.is_empty().expect("count"), "history is seeded");
    }

    let engine = Arc::new(Engine::open(&root));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri(CHAT_CLEAR_ROUTE)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "Clear-history succeeds");

    assert!(
        ChatStore::open(&root).expect("reopen").is_empty().expect("count"),
        "the conversation store is empty after Clear-history",
    );
    // The clear handler cascades to per-thread memory too (S-175 FK cascade),
    // proven here through the HTTP route, not only the store API (step 6b).
    assert!(
        MemoryStore::open(&root).expect("reopen memory").is_empty().expect("count"),
        "per-thread memory is wiped by the Clear-history route",
    );
}

// ── Step 3: first-use consent disclosure names the configured endpoint ───────

/// Step 3: returning to a configured Chat, a first-use consent disclosure naming
/// the configured endpoint precedes the composer — shown before any outbound call
/// ([NFR-SE-07], [FR-UI-18]). The streamed answer this gates is asserted by the
/// compound-turn test above; this backs the consent-disclosure half of step 3.
/// (Banner mechanics + ordering are covered by the chat Vitest suite, S-190;
/// this asserts the read-model carries the endpoint the banner names.)
#[tokio::test]
async fn uat_ui_07_step3_consent_disclosure_names_configured_endpoint() {
    let dir = configured_root();
    let engine = Arc::new(Engine::open(dir.path()));
    let router = router_with_intent(engine, IntentToken::generate());

    // The first-use consent banner is rendered client-side by the SPA Chat view,
    // which names the endpoint from GET /api/v1/config (S-192: the server-rendered
    // /chat HTML is retired; the banner's presence + ordering before the composer
    // are covered by the chat Vitest suite, S-190). The Rust-side guarantee is that
    // the read-model carries the configured endpoint host for the banner to name.
    let body = body_string(router.oneshot(get("/api/v1/config")).await.unwrap()).await;
    assert!(
        body.contains("openrouter.ai"),
        "the config read-model names the configured endpoint host: {body}",
    );
}
