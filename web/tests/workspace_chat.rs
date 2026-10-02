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
//!    workspace's;
//! 7. every read root the turn's source tools may reach is checked before the
//!    turn touches a store: a missing one — the workspace tier's, or a member's
//!    effective one — fails the turn by name and records no thread
//!    ([NFR-SE-04], sprint-84 HF-1).
//!
//! [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
//! [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
//! [FR-WS-30]: ../../docs/specs/requirements/FR-WS-30.md
//! [ADR-71]: ../../docs/specs/architecture/decisions/ADR-71.md
//! [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
//! [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md

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
/// A complete workspace tier whose endpoint already ends in the
/// `/chat/completions` path rig appends, so a production turn gets past
/// configure-first and is stopped by the deterministic pre-send preflight — an
/// egress-free production turn ([NFR-SE-07]). A tier without a `base_url` would
/// dial the default OpenRouter endpoint for real.
///
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
const PREFLIGHT_STOPPED_TIER: &str = "[chat]\nmodel = \"workspace/model\"\n\
     base_url = \"https://workspace.example/v1/chat/completions\"\n";

/// The production turn reached the provider seam and was stopped there by the
/// preflight on the workspace's endpoint — never dialled, never configure-first.
fn assert_stopped_at_the_preflight(body: &str) {
    assert!(
        body.contains("event: error") && body.contains("workspace.example/v1/chat/completions"),
        "the production turn stops at the preflight on the workspace endpoint: {body}"
    );
    assert!(!body.contains("not configured"), "{body}");
    assert!(!body.contains(WS_KEY), "{body}");
}

fn post(uri: &str, intent: &IntentToken, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "text/event-stream")
        .header(INTENT_HEADER, intent.as_str())
        .body(body.into())
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
    // A guarded turn whose body is not a form at all: the 404 is the extractor's,
    // decided from the request head before the body is read — never the form
    // extractor's `415` for a request that has no workspace chat to reach.
    let mut unformed = post(WORKSPACE_CHAT_POST_ROUTE, &intent, "{\"q\":\"hello\"}");
    unformed.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    for req in [
        unformed,
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
    write(root, "config.toml", PREFLIGHT_STOPPED_TIER);
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert_stopped_at_the_preflight(&body);
    assert!(!body.contains("ws42"), "{body}");
    assert_eq!(threads_at(root).len(), 1, "the question is recorded at the workspace root");
}

/// A follow-up turn on the production route appends to the thread it names
/// ([S-483]): `thread=` reaches the workspace setup, so turn 2 lands in turn 1's
/// conversation — the thread the prior-turn window is read from — rather than
/// opening a fresh one. Both turns stop at the preflight, after the question is
/// recorded, so nothing dials.
///
/// [S-483]: ../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_turn_appends_to_the_thread_it_names() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    write(root, "config.toml", PREFLIGHT_STOPPED_TIER);
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (_, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=first+question")).await;
    assert_stopped_at_the_preflight(&body);
    let threads = threads_at(root);
    assert_eq!(threads.len(), 1, "turn 1 opened one thread");
    let thread = threads[0];

    let follow_up = format!("q=second+question&thread={thread}");
    let (_, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, follow_up)).await;
    assert_stopped_at_the_preflight(&body);
    assert_eq!(threads_at(root), vec![thread], "turn 2 opened no second thread");
    let messages = ChatStore::open(root).unwrap().messages(thread).unwrap();
    assert_eq!(
        messages.into_iter().map(|m| (m.role, m.content)).collect::<Vec<_>>(),
        [
            (ChatRole::User, "first question".to_string()),
            (ChatRole::User, "second question".to_string()),
        ],
    );
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
    // The endpoint carries rig's own `/chat/completions` suffix, so the production
    // turn below stops at the pre-send preflight: nothing leaves loopback.
    write(root, "config.toml", PREFLIGHT_STOPPED_TIER);
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
    assert_stopped_at_the_preflight(&body);

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

// ── 7. Read roots are checked up front (sprint-84 HF-1) ─────────────────────

/// A workspace `[chat]` with `model` and a key whose `read_roots` names a
/// directory that does not exist: the turn fails up front with the member
/// chat's wording shape — which root declared it, which entry, why — and
/// records no thread at `<root>/.logos/chat.db`. Once the directory exists the
/// same tier runs its turn to the preflight, so the refusal is the entry's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_workspace_read_root_fails_the_turn_up_front_by_name() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    write(root, "config.toml", &format!("{PREFLIGHT_STOPPED_TIER}read_roots = [\"no-such-docs\"]\n"));
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    let canonical = root.canonicalize().unwrap();
    assert!(
        body.contains(&format!(
            "event: error\ndata: could not open the source sandbox of the workspace root {}: \
             [chat] read_roots entry \"no-such-docs\" (resolved to \"{}\") does not exist",
            canonical.display(),
            canonical.join("no-such-docs").display()
        )),
        "{body}"
    );
    assert!(!body.contains("chat/completions"), "the provider seam was never reached: {body}");
    assert!(!body.contains(WS_KEY), "{body}");
    assert!(!root.join(".logos/chat.db").exists(), "the refused turn opened no store");

    std::fs::create_dir_all(root.join("no-such-docs")).unwrap();
    let (_, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_stopped_at_the_preflight(&body);
    assert_eq!(threads_at(root).len(), 1, "the control turn is recorded");
}

/// A member's **effective** read roots are checked, as its addressed sandbox
/// resolves them. `web` declaring `read_roots` but no `model` inherits the
/// workspace table whole — its own entry goes with the rest of its table — so
/// the turn runs. Once `web` owns its policy the same missing entry fails the
/// turn up front, naming `web` and the entry, and records no second thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_members_missing_read_root_fails_the_turn_up_front_naming_the_member() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    let web_root = root.join("web");
    write(root, "config.toml", PREFLIGHT_STOPPED_TIER);
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    write(&web_root, "config.toml", "[chat]\nread_roots = [\"../no-such-member-docs\"]\n");
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (_, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_stopped_at_the_preflight(&body);
    assert_eq!(threads_at(root).len(), 1, "an inherited table drops web's own read roots");

    write(
        &web_root,
        "config.toml",
        "[chat]\nmodel = \"web/model\"\nread_roots = [\"../no-such-member-docs\"]\n",
    );
    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=again")).await;
    assert_eq!(status, StatusCode::OK);
    let web_canonical = web_root.canonicalize().unwrap();
    assert!(
        body.contains(&format!(
            "event: error\ndata: could not open the source sandbox of the workspace member web: \
             [chat] read_roots entry \"../no-such-member-docs\" (resolved to \"{}\") does not exist",
            web_canonical.join("../no-such-member-docs").display()
        )),
        "{body}"
    );
    assert!(!body.contains("chat/completions"), "the provider seam was never reached: {body}");
    assert_eq!(threads_at(root).len(), 1, "the refused turn recorded no thread");
}

/// The sprint-84 review's reproduction (Agent 3): a workspace tier declaring
/// `read_roots = ["nope"]` over members that declare no `[chat]`. Each member
/// inherits the workspace table, so the sandbox its addressed source tools read
/// through refuses with `BadReadRoot` — the per-call `DispatchError::Tool` the
/// review saw, reproduced here on the very resolution the tools perform. The
/// turn now fails before any tool is offered, naming the workspace root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reviewers_repro_now_fails_the_turn_instead_of_the_call() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path().canonicalize().unwrap();
    write(&root, "config.toml", &format!("{PREFLIGHT_STOPPED_TIER}read_roots = [\"nope\"]\n"));
    write(&root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));

    // The precondition: the fault the addressed call raised is still there.
    let api = root.join("api");
    let resolution = logos_core::config::resolve_chat(&api, Some(&root)).unwrap();
    assert_eq!(resolution.policy_origin, logos_core::config::ChatOrigin::Workspace);
    let per_call = agent_core::Sandbox::from_root(&api)
        .unwrap()
        .with_chat_read_roots(&api, Some(&root), &resolution)
        .expect_err("the inheriting member's sandbox refuses the entry");
    assert!(matches!(per_call, agent_core::SandboxError::BadReadRoot { .. }), "{per_call}");

    let intent = IntentToken::generate();
    let router = workspace_chat::router(&root, &intent);
    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(
            "event: error\ndata: could not open the source sandbox of the workspace root {}: {per_call}",
            root.display()
        )),
        "the turn fails with the call's own fault, named against the workspace root: {body}"
    );
    assert!(threads_at(&root).is_empty(), "no thread is recorded");
}

/// A member whose `[chat]` cannot be read has read roots nobody can establish,
/// and its addressed sandbox would refuse every call for that reason: the turn
/// fails up front naming the member and the file, never echoing the file's
/// content, and records no thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_chat_cannot_be_read_fails_the_turn_up_front_naming_it() {
    let tmp = workspace_chat::workspace();
    let root = tmp.path();
    write(root, "config.toml", PREFLIGHT_STOPPED_TIER);
    write(root, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
    write(&root.join("web"), "secrets.toml", "[chat]\napi_key = \"sk-unterminated-mb77\n");
    let intent = IntentToken::generate();
    let router = workspace_chat::router(root, &intent);

    let (status, body) = send(&router, post(WORKSPACE_CHAT_POST_ROUTE, &intent, "q=hello")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("event: error\ndata: could not read the workspace member web's chat secret — check that"),
        "{body}"
    );
    assert!(body.contains("web/.logos/secrets.toml is valid TOML"), "{body}");
    assert!(!body.contains("mb77") && !body.contains(WS_KEY), "no secret is echoed: {body}");
    assert!(threads_at(root).is_empty(), "the refused turn recorded no thread");
}
