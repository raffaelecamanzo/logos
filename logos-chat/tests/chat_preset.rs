// The chat routes are the `agents`-gated egress carve-out (CR-078/ADR-60): a
// listen-only build has no turn route, so this crate is empty there.
//! The turn POST validates its `preset` ([S-625], [FR-UI-49], [FR-UI-18]).
//!
//! Both turn routes — the member chat's `POST /chat` and the workspace chat's
//! `POST /workspace/chat`, which share `serve_turn` — are driven in-process
//! through the real router, with a probe [`ChatService`] that records the preset
//! each turn was started under and runs that turn over an offline mock provider:
//!
//! - a value that names none of the four presets — a near miss included — is a
//!   `400` naming the accepted values, the service never starts a turn and the
//!   mock provider is never called;
//! - each of the four reaches the service as itself, and an absent field (the
//!   no-JS form) as Default.
//!
//! [S-625]: ../../docs/planning/journal.md#s-625-a-chat-turn-runs-under-a-prompt-only-preset-chosen-in-the-composer
//! [FR-UI-49]: ../../docs/specs/requirements/FR-UI-49.md
//! [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md

use std::sync::{Arc, Mutex};

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use chat_agent::{
    BudgetTree, ChatStore, MemoryGrounding, MemoryStore, Orchestrator, Planner, Preset,
    SubagentRoster,
};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use logos_chat::{spawn_turn, ChatService, ChatStream, TurnTarget};
use logos_chat::{router_with_chat, CHAT_POST_ROUTE, WORKSPACE_CHAT_POST_ROUTE};
use web::{IntentToken, INTENT_HEADER};

#[path = "support/workspace_chat.rs"]
mod workspace_chat;

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";

/// A [`ChatService`] that records every turn's preset and runs it — a
/// conversational answer from the member roster under that preset — over one
/// shared mock provider whose request count is the egress the route caused.
struct PresetProbe {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: std::path::PathBuf,
    started: Mutex<Vec<Preset>>,
    provider: MockCompletionModel,
}

impl ChatService for PresetProbe {
    fn start_turn(&self, question: String, _thread_id: Option<i64>, preset: Preset) -> ChatStream {
        self.started.lock().unwrap().push(preset);
        let thread = ChatStore::open(&self.root).unwrap().create_thread("probe").unwrap();
        let memory = Arc::new(MemoryStore::open(&self.root).unwrap());
        let turn = memory.next_turn(thread).unwrap();
        let grounding = Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread, turn));
        let roster = SubagentRoster::new(
            Arc::clone(&self.engine),
            Arc::clone(&self.sandbox),
            self.provider.clone(),
        )
        .with_synthesizer_grounding(grounding)
        .with_preset(preset);
        let planner = Planner::with_preamble(self.provider.clone(), roster.planner_preamble());
        let orchestrator = Orchestrator::with_planner(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, TurnTarget::new(self.root.clone(), thread, turn))
    }
}

/// A router on `route` whose chat seam is a fresh [`PresetProbe`] scripted for
/// `turns` conversational answers.
struct Probed {
    _dir: TempDir,
    _workspace: Option<TempDir>,
    router: axum::Router,
    intent: IntentToken,
    uri: &'static str,
    probe: Arc<PresetProbe>,
}

fn probed(workspace: bool, turns: usize) -> Probed {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let engine = Arc::new(Engine::open(dir.path()));
    let script = (0..turns).flat_map(|_| {
        [MockTurn::text(r#"{"action":"final","grounded":false}"#), MockTurn::text("hello")]
    });
    let probe = Arc::new(PresetProbe {
        engine: Arc::clone(&engine),
        sandbox: Arc::new(Sandbox::new(dir.path(), std::iter::empty()).unwrap()),
        root: dir.path().to_path_buf(),
        started: Mutex::new(Vec::new()),
        provider: MockCompletionModel::new(script),
    });
    let intent = IntentToken::generate();
    let service: Arc<dyn ChatService> = probe.clone();
    if workspace {
        let ws = workspace_chat::workspace();
        let router = logos_chat::workspace_router_with_chat(workspace_chat::registry(ws.path()), intent.clone(), service)
            .expect("the workspace router builds");
        Probed { _dir: dir, _workspace: Some(ws), router, intent, uri: WORKSPACE_CHAT_POST_ROUTE, probe }
    } else {
        let router = router_with_chat(engine, intent.clone(), service);
        Probed { _dir: dir, _workspace: None, router, intent, uri: CHAT_POST_ROUTE, probe }
    }
}

/// A same-origin, intent-carrying, buffered turn POST of the form `body`.
fn turn_post(p: &Probed, body: String) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(p.uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(INTENT_HEADER, p.intent.as_str())
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

/// A value no accepted preset contains, so a `400` that reflected the sent value
/// would carry it — the probe for "the refusal never echoes its input".
const ECHO_PROBE: &str = "zz_echo_probe";

/// Values that name no preset: an invented one, the echo probe, the empty value, and
/// near misses one case, one separator or one space from a real one — form-encoded as
/// sent.
const UNKNOWN: [&str; 8] = [
    "custom",
    ECHO_PROBE,
    "",
    "Default",
    "impact-analysis",
    "impact_analysis%20",
    "%20onboarding",
    "troubleshoot",
];

async fn an_unknown_preset_is_400_and_calls_no_provider(workspace: bool) {
    for value in UNKNOWN {
        let p = probed(workspace, 1);
        let resp = p
            .router
            .clone()
            .oneshot(turn_post(&p, format!("q=hello&preset={value}")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "preset={value:?} on {}", p.uri);
        let body = body_string(resp).await;
        assert!(
            body.contains("default, impact_analysis, onboarding, troubleshooting"),
            "the refusal names the accepted values: {body}"
        );
        assert!(!body.contains(ECHO_PROBE), "the refusal never echoes the value sent: {body}");
        assert!(p.probe.started.lock().unwrap().is_empty(), "preset={value:?}: no turn was started");
        assert_eq!(p.probe.provider.request_count(), 0, "preset={value:?}: no provider call");
    }
}

async fn each_preset_reaches_the_service_and_an_absent_one_is_default(workspace: bool) {
    let p = probed(workspace, Preset::ALL.len() + 1);
    let mut sent = Vec::new();
    for preset in Preset::ALL {
        sent.push(format!("q=hello&preset={}", preset.wire_name()));
    }
    sent.push("q=hello".to_string());
    for body in sent {
        let resp = p.router.clone().oneshot(turn_post(&p, body.clone())).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{body} on {}", p.uri);
        assert!(body_string(resp).await.contains("hello"), "{body}: the turn answered");
    }
    let mut expected = Preset::ALL.to_vec();
    expected.push(Preset::Default);
    assert_eq!(*p.probe.started.lock().unwrap(), expected);
    // The service ran each turn under its preset: the planner's system prompt is
    // that preset's (every other request is a Synthesizer's).
    let planner_prompts: Vec<String> = p
        .probe
        .provider
        .system_prompts()
        .into_iter()
        .step_by(2)
        .map(Option::unwrap_or_default)
        .collect();
    for (prompt, preset) in planner_prompts.iter().zip(&expected) {
        match preset.planner_clause() {
            Some(clause) => assert!(prompt.ends_with(clause), "{preset:?}: {prompt}"),
            None => assert_eq!(prompt, chat_agent::orchestrator::DEFAULT_PLANNER_PREAMBLE),
        }
    }
}

mod member {
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unknown_preset_is_400_and_calls_no_provider() {
        super::an_unknown_preset_is_400_and_calls_no_provider(false).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn each_preset_reaches_the_service_and_an_absent_one_is_default() {
        super::each_preset_reaches_the_service_and_an_absent_one_is_default(false).await;
    }
}

mod workspace {
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unknown_preset_is_400_and_calls_no_provider() {
        super::an_unknown_preset_is_400_and_calls_no_provider(true).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn each_preset_reaches_the_service_and_an_absent_one_is_default() {
        super::each_preset_reaches_the_service_and_an_absent_one_is_default(true).await;
    }
}
