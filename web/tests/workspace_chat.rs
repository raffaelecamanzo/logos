//! The workspace chat is its own service, route and store ([S-482], [FR-WS-34],
//! [FR-WS-30], [ADR-71], [ADR-67]) — over the **real** workspace router.
//!
//! The route contract shared with `/chat` (intent guard, host guard, SSE and the
//! buffered fallback) is pinned by running `chat_sse.rs`'s and
//! `chat_threads_api.rs`'s cases on both routes. This suite pins what is the
//! workspace chat's own:
//!
//! 1. a single-root serve has no workspace chat: all four routes are `404`;
//! 2. `?repo=` is not read — the turn and the threads answer for the workspace;
//! 3. the production service takes its `[chat]` from the workspace tier alone: a
//!    member's complete `[chat]` under an empty tier is configure-first naming the
//!    workspace root, and a complete tier reaches the provider (stopped,
//!    egress-free, by the pre-send preflight on the workspace's endpoint);
//! 4. a turn persists to `<workspace root>/.logos/chat.db`, leaves every member's
//!    `chat.db` exactly as it was, and leaves a tracked workspace root
//!    `git status`-clean under the generated ignore rules;
//! 5. the consent disclosure's facts — the endpoint and read roots — are the
//!    workspace tier's, on the workspace config read-model the view reads;
//! 6. a `/chat?repo=web` turn in a workspace serve is the member chat: it resolves
//!    the member's own policy and writes the member's store, never the
//!    workspace's.
//!
//! [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
//! [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
//! [FR-WS-30]: ../../docs/specs/requirements/FR-WS-30.md
//! [ADR-71]: ../../docs/specs/architecture/decisions/ADR-71.md
//! [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md

#![cfg(feature = "agents")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use chat_agent::{ChatRole, ChatStore};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::chat::ChatService;
use web::{
    IntentToken, CHAT_POST_ROUTE, CHAT_THREADS_ROUTE, INTENT_HEADER, WORKSPACE_CHAT_POST_ROUTE,
    WORKSPACE_CHAT_THREADS_ROUTE,
};

#[path = "support/workspace_chat.rs"]
mod workspace_chat;

use workspace_chat::{porcelain, sh_git, ScriptedWorkspaceChat};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
const ANSWER: &str = "WORKSPACE_ANSWER_SENTINEL";
const WS_KEY: &str = "sk-workspace-tier-ws42";
const MEMBER_KEY: &str = "sk-member-complete-mb77";

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

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

async fn send(router: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn write(root: &Path, file: &str, body: &str) {
    std::fs::create_dir_all(root.join(".logos")).unwrap();
    std::fs::write(root.join(".logos").join(file), body).unwrap();
}

/// A workspace router whose workspace chat is the scripted mock-provider one.
fn scripted(root: &Path, intent: &IntentToken) -> axum::Router {
    let service: Arc<dyn ChatService> =
        Arc::new(ScriptedWorkspaceChat::over(root, "two members.", ANSWER));
    web::workspace_router_with_chat(workspace_chat::registry(root), intent.clone(), service)
        .expect("the workspace router builds")
}

/// Every thread id in the store at `root`, or none when it has no store.
fn threads_at(root: &Path) -> Vec<i64> {
    if !root.join(".logos/chat.db").exists() {
        return Vec::new();
    }
    ChatStore::open(root).unwrap().list_threads().unwrap().into_iter().map(|t| t.id).collect()
}

// ── 1. Single-root: no workspace chat ───────────────────────────────────────

/// AC-1: a single-root serve answers each workspace chat route with the
/// workspace family's `404` — a fully guarded turn included — and starts nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_root_serve_answers_every_workspace_chat_route_404() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    let intent = IntentToken::generate();
    let router = web::router_with_intent(Arc::new(Engine::open(dir.path())), intent.clone());
    for req in [
        post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello"),
        get(WORKSPACE_CHAT_THREADS_ROUTE),
        get(&format!("{WORKSPACE_CHAT_THREADS_ROUTE}/1")),
        post(&format!("{WORKSPACE_CHAT_THREADS_ROUTE}/1/delete"), &intent, ""),
    ] {
        let uri = req.uri().to_string();
        let (status, body) = send(&router, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(body.contains("not a workspace"), "{uri}: {body}");
    }
    assert!(!dir.path().join(".logos/chat.db").exists(), "no store was opened");
}

/// The threads routes read the workspace root's store without creating it: on a
/// workspace nobody has chatted in, the list is empty, a thread is `404`, a
/// delete is `404` — and no `chat.db` appears at the root (the guard this suite
/// adds beside `workspace_api.rs`'s write-free loop, which cannot walk `:id`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_threads_routes_never_create_the_workspace_store() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);
    let (status, body) = send(&router, get(WORKSPACE_CHAT_THREADS_ROUTE)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "[]"));
    let (status, _) = send(&router, get(&format!("{WORKSPACE_CHAT_THREADS_ROUTE}/1"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) =
        send(&router, post(&format!("{WORKSPACE_CHAT_THREADS_ROUTE}/1/delete"), &intent, "")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!root.join(".logos/chat.db").exists(), "no store was created by a read");
}

// ── 2. `?repo=` is not read ─────────────────────────────────────────────────

/// AC-1: the workspace routes ignore `?repo=` — a member the workspace does not
/// have, which `/chat` refuses, changes nothing — and the turn lands in the
/// workspace root's store whichever member the query names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_workspace_routes_ignore_repo() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    let intent = IntentToken::generate();
    let router = scripted(root, &intent);

    let (status, _) = send(&router, post(&format!("{CHAT_POST_ROUTE}?repo=nosuch"), &intent, "q=hi")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the control: /chat resolves the member");

    let (status, body) = send(
        &router,
        post(&format!("{WORKSPACE_CHAT_POST_ROUTE}?repo=nosuch"), &intent, "q=which+members"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(ANSWER), "{body}");
    let (status, body) = send(
        &router,
        post(&format!("{WORKSPACE_CHAT_POST_ROUTE}?repo=web"), &intent, "q=and+web"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let threads = threads_at(root);
    assert_eq!(threads.len(), 2, "both turns are the workspace's");
    assert!(threads_at(&root.join("web")).is_empty(), "none is web's");
    for uri in [
        format!("{WORKSPACE_CHAT_THREADS_ROUTE}?repo=web"),
        format!("{WORKSPACE_CHAT_THREADS_ROUTE}?repo=nosuch"),
    ] {
        let (status, body) = send(&router, get(&uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.as_array().map(Vec::len), Some(2), "{uri}: {body}");
    }
}

// ── 3. The workspace tier alone ─────────────────────────────────────────────

/// AC-2: the default member declares a complete `[chat]`; the workspace tier is
/// empty. The workspace turn is the configure-first frame naming the workspace
/// root, both missing halves and Workspace Config — not a turn on the member's
/// model — and records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_complete_member_chat_does_not_configure_the_workspace_chat() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    let api = root.join("api");
    write(
        &api,
        "config.toml",
        "[chat]\nmodel = \"member/model\"\nbase_url = \"https://member.example/v1/chat/completions\"\n",
    );
    write(&api, "secrets.toml", &format!("[chat]\napi_key = \"{MEMBER_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("event: error"), "{body}");
    assert!(
        body.contains(&format!(
            "The workspace chat is not configured yet for the workspace root {} — neither a \
             provider model nor an API key is declared there.",
            root.canonicalize().unwrap().display()
        )),
        "{body}"
    );
    assert!(body.contains("Workspace Config"), "{body}");
    assert!(!body.contains("chat/completions"), "the member's endpoint was never reached: {body}");
    assert!(!body.contains(MEMBER_KEY), "{body}");
    assert!(threads_at(root).is_empty(), "a refused turn records no thread");

    // The control: the member's own chat IS configured by that declaration.
    let (_, body) = send(&router, post(CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert!(body.contains("chat/completions"), "the member chat reaches its preflight: {body}");
}

/// A complete workspace tier reaches the provider: the production turn gets past
/// configure-first and is stopped by the deterministic pre-send preflight on the
/// **workspace's** endpoint — an egress-free proof that the workspace's policy
/// and key were resolved — and it never echoes the key ([NFR-SE-07]).
///
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_complete_workspace_tier_reaches_the_preflight_on_its_own_endpoint() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    write(
        root,
        "config.toml",
        "[chat]\nmodel = \"workspace/model\"\n\
         base_url = \"https://workspace.example/v1/chat/completions\"\n",
    );
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("not configured"), "{body}");
    assert!(
        body.contains("event: error") && body.contains("workspace.example/v1/chat/completions"),
        "the workspace's endpoint reached the preflight: {body}"
    );
    assert!(!body.contains(WS_KEY) && !body.contains("ws42"), "{body}");
    assert_eq!(threads_at(root).len(), 1, "the question is recorded at the workspace root");
}

// ── 4. The store: the workspace root's, and git-clean ───────────────────────

/// `(len, mtime)` of every `chat.db*` file under each member's `.logos/`.
fn member_stores(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    for member in ["api", "web"] {
        let dir = root.join(member).join(".logos");
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("chat.db") {
                let meta = entry.metadata().unwrap();
                out.insert(entry.path(), (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out
}

/// AC-3: on a tracked workspace root enabled by `logos init --workspace`'s
/// enablement, a workspace turn — the scripted one (question, scratchpad and
/// answer all persisted) and the production one — writes the workspace root's
/// `.logos/chat.db`, leaves every member's `chat.db` byte-for-byte where it was
/// (one exists, one does not), and leaves `git status --porcelain` empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_stores_at_the_workspace_root_and_leaves_the_tracked_root_clean() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    // The workspace root is its own repository; the member clones are not part
    // of it, as on an estate whose root repo holds only the manifest.
    std::fs::write(root.join(".gitignore"), "/api/\n/web/\n").unwrap();
    sh_git(root, &["init", "-q", "-b", "main"]);
    let members = logos_core::federation::discover(root).unwrap().expect("a workspace").members;
    logos_core::federation::enable::enable(root, "shop", &members).expect("enables");
    write(root, "config.toml", "[chat]\nmodel = \"workspace/model\"\n");
    sh_git(root, &["add", "-A"]);
    sh_git(root, &["commit", "-q", "-m", "the workspace root, enabled"]);
    // The key is the user's own, and ignored like every credential.
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    assert_eq!(porcelain(root), "", "guard the guard: the tracked root starts clean");

    // `web` already holds a conversation of its own; `api` holds none.
    let mut web_store = ChatStore::open(&root.join("web")).unwrap();
    let web_thread = web_store.create_thread("web's own").unwrap();
    web_store.append_message(web_thread, ChatRole::User, "a web question", &[]).unwrap();
    drop(web_store);
    let before = member_stores(root);
    assert!(before.keys().any(|p| p.ends_with("web/.logos/chat.db")), "{before:?}");

    let intent = IntentToken::generate();
    let (status, body) =
        send(&scripted(root, &intent), post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=which+members")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(ANSWER), "{body}");
    let (_, body) = send(
        &workspace_chat::router(root, &intent),
        post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=again"),
    )
    .await;
    assert!(body.contains("event: error"), "the production turn stopped at the provider: {body}");

    let store = ChatStore::open(root).unwrap();
    let threads = store.list_threads().unwrap();
    assert_eq!(threads.len(), 2, "both turns are in the workspace root's store");
    // Most-recent-first: the production turn recorded its question, the scripted
    // turn its answer.
    let contents = |id: i64| {
        store.messages(id).unwrap().into_iter().map(|m| (m.role, m.content)).collect::<Vec<_>>()
    };
    assert_eq!(contents(threads[0].id), [(ChatRole::User, "again".to_string())]);
    assert_eq!(contents(threads[1].id), [(ChatRole::Assistant, ANSWER.to_string())]);
    drop(store);
    assert_eq!(member_stores(root), before, "no member's chat.db was created or modified");
    assert!(root.join(".logos/chat.db-wal").exists() || root.join(".logos/chat.db").exists());
    assert_eq!(porcelain(root), "", "a workspace turn leaves the tracked root clean");
}

// ── 5. The consent disclosure's facts ───────────────────────────────────────

/// [NFR-SE-07]: the workspace chat's consent disclosure names the endpoint it
/// will send excerpts to and the read roots its source tools may reach. Both are
/// the workspace tier's — on the workspace config read-model the view reads,
/// resolved at the workspace root with no tier above it — and the key is masked.
///
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_workspace_config_read_model_carries_the_disclosures_facts() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("shared-docs")).unwrap();
    write(
        root,
        "config.toml",
        "[chat]\nmodel = \"workspace/model\"\nbase_url = \"https://workspace.example/v1\"\n\
         read_roots = [\"shared-docs\"]\n",
    );
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    // A member's own, different endpoint is not the workspace chat's.
    write(&root.join("api"), "config.toml", "[chat]\nmodel = \"m\"\nbase_url = \"https://member.example/v1\"\n");
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, get("/api/v1/workspace/config")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let chat = &json["effective_chat"];
    assert_eq!(chat["policy"]["base_url"], "https://workspace.example/v1", "{body}");
    assert_eq!(chat["policy"]["read_roots"], serde_json::json!(["shared-docs"]), "{body}");
    assert_eq!(chat["credential_origin"], "member", "declared at this root: {body}");
    assert!(!body.contains(WS_KEY) && !body.contains("member.example"), "{body}");
}

// ── 6. The member chat stays the member's ───────────────────────────────────

/// AC-4: a `/chat?repo=web` turn in a workspace serve is the member chat of
/// `web` — it resolves `web`'s own policy (its preflight names `web`'s
/// endpoint) and records into `web`'s store — and the workspace root's store is
/// never created by it. Its roster is single-backing by construction: the
/// member service takes no query backing at all (pinned on the launch path by
/// `web/src/chat/configured.rs`'s
/// `a_workspace_member_turn_through_launch_has_no_xservice_tool`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_member_turn_is_the_members_own_chat() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    let web_root = root.join("web");
    write(
        &web_root,
        "config.toml",
        "[chat]\nmodel = \"web/model\"\nbase_url = \"https://web.example/v1/chat/completions\"\n",
    );
    write(&web_root, "secrets.toml", &format!("[chat]\napi_key = \"{MEMBER_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) =
        send(&router, post(&format!("{CHAT_POST_ROUTE}?repo=web"), &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("web.example/v1/chat/completions"), "web's own policy: {body}");
    assert_eq!(threads_at(&web_root).len(), 1, "recorded in web's store");
    assert!(!root.join(".logos/chat.db").exists(), "never in the workspace's");

    let (status, body) = send(&router, get(&format!("{CHAT_THREADS_ROUTE}?repo=web"))).await;
    assert_eq!(status, StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().map(Vec::len), Some(1), "{body}");
}
