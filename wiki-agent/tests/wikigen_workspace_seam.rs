//! The wiki-generation pass reads its chat halves through the one resolution seam,
//! over the **real** workspace router ([S-449], [ADR-67], [ADR-42]).
//!
//! The unit tests in `src/web.rs` (the configured run service) pin the verdict and the
//! resolution; this pins the **wiring** — that the router hands the federation's
//! workspace root to the wiki-generation service, for the default member and for a
//! `?repo=`-scoped one. The chat turn's twin of this suite moved with chat to the
//! `chat` extension (`logos-chat/tests/chat_workspace_seam.rs`, S-643). Nothing here dials: the workspace declares a
//! `base_url` that already carries rig's appended `/chat/completions` path, so a
//! turn that gets past configure-first is stopped by the deterministic pre-send
//! preflight ([FR-UI-24]) with a frame naming that path — an egress-free proof
//! that the workspace's policy and key were the ones resolved.
//!
//! [S-449]: ../../docs/planning/journal.md#s-449-the-chat-turn-path-reads-the-same-resolution-the-gate-reads
//! [ADR-42]: ../../docs/specs/architecture/decisions/ADR-42.md
//! [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
//! [FR-UI-24]: ../../docs/specs/requirements/FR-UI-24.md


use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::federation::{discover, EngineRegistry};
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use logos_wiki::web::WIKI_GENERATE_ROUTE;
use web::{IntentToken, INTENT_HEADER};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
const WS_KEY: &str = "sk-workspace-seam-ws42";
/// The configure-first prefix both agents' refusals start with.
const NOT_CONFIGURED: &str = "not configured";

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

/// A committed git repo — `discover` keeps only members that are git roots.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// A two-member workspace whose members declare **neither** chat half, and whose
/// root declares both — the estate-wide single declaration [FR-WS-30] enables.
fn inheriting_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"));
    init_repo(&root.join("web"));
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".logos")).unwrap();
    std::fs::write(
        root.join(".logos/config.toml"),
        "[chat]\nprovider = \"openai\"\nmodel = \"workspace/model\"\n\
         base_url = \"https://workspace.example/v1/chat/completions\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".logos/secrets.toml"),
        format!("[chat]\napi_key = \"{WS_KEY}\"\n"),
    )
    .unwrap();
    tmp
}

fn ws_router(tmp: &TempDir) -> (axum::Router, IntentToken) {
    let federation = discover(tmp.path())
        .expect("discovery succeeds")
        .expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    let intent = IntentToken::generate();
    let router = logos_wiki::web::workspace_router(registry, intent.clone(), Some(std::sync::Arc::new(wiki_agent::ConfiguredGenerator)))
        .expect("the workspace router builds");
    (router, intent)
}

fn post(uri: &str, intent: &IntentToken, body: &'static str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "text/event-stream")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from(body))
        .unwrap()
}

async fn send(router: axum::Router, req: Request<Body>) -> String {
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The frame proves the turn resolved the workspace's policy and key: it got past
/// configure-first and was stopped by the preflight on the workspace's endpoint.
fn assert_resolved_from_the_workspace(body: &str, what: &str) {
    assert!(
        !body.contains(NOT_CONFIGURED),
        "{what}: an inheriting member is not configure-first: {body}"
    );
    assert!(
        body.contains("event: error") && body.contains("chat/completions"),
        "{what}: the workspace's endpoint reached the preflight: {body}",
    );
    assert!(
        !body.contains(WS_KEY),
        "{what}: the key is never echoed (NFR-SE-07): {body}"
    );
}

/// The scope extension ([ADR-42]): wiki generation inherits the chat provider and
/// key, so an inheriting member gets wiki generation too — for the default member
/// and for a scoped one.
///
/// [ADR-42]: ../../docs/specs/architecture/decisions/ADR-42.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_generation_resolves_the_workspace_halves() {
    let tmp = inheriting_workspace();
    for uri in [
        WIKI_GENERATE_ROUTE.to_string(),
        format!("{WIKI_GENERATE_ROUTE}?repo=web"),
    ] {
        let (router, intent) = ws_router(&tmp);
        let body = send(router, post(&uri, &intent, "")).await;
        assert_resolved_from_the_workspace(&body, &uri);
    }
}
