// CR-078/ADR-60: the chat conversation-history read/delete API is part of the
// LLM egress carve-out, so these route-fitness tests exercise the `agents`-gated
// surface and compile only under `--features agents`. A listen-only
// `--features ui` build mounts none of these routes, so the whole test crate is
// empty there.
#![cfg(feature = "agents")]
//! Chat thread read/delete API fitness tests (S-209, [FR-UI-26], [ADR-47],
//! [ADR-28], [ADR-31], [NFR-SE-06], [NFR-SE-07]).
//!
//! These drive the **real** router in-process (`tower::ServiceExt::oneshot`, no
//! socket) over the store-backed read/delete seam — no mock provider is needed
//! because the endpoints touch only `.logos/chat.db`, never the LLM. They prove:
//!
//! - `GET /api/v1/chat/threads` lists conversations **most-recent-first**, as a
//!   slim `{id,title,updated_at}` payload carrying no secret ([FR-UI-26]);
//! - `GET /api/v1/chat/threads/{id}` returns that thread's **ordered** messages,
//!   and `404`s an unknown thread;
//! - the reads are **GET-only** (a POST to the list route is `405`);
//! - `POST /api/v1/chat/threads/{id}/delete` is **intent-guarded** — a forged
//!   (cross-origin) or intent-less delete is `403` and mutates nothing, a valid
//!   one is `204` and removes exactly that thread; an unknown id is `404`;
//! - the retired global `POST /chat/clear` route is **gone** (`405`);
//! - the surface stays **loopback-only** (a non-loopback `Host` is `403`).
//!
//! Every case but the retired-route one runs **twice** ([S-482]): over the member
//! chat's `/api/v1/chat/threads` and over the workspace chat's
//! `/api/v1/workspace/chat/threads`, whose store is `<workspace root>/.logos/chat.db`
//! ([`over_both_trees!`] names them `<case>::member` and `<case>::workspace`).
//!
//! [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
//!
//! [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
//! [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
//! [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, Response, StatusCode};
use chat_agent::{ChatRole, ChatStore, ToolTrace};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::{
    router_with_intent, IntentToken, CHAT_THREADS_ROUTE, INTENT_HEADER,
    WORKSPACE_CHAT_THREADS_ROUTE,
};

#[path = "support/workspace_chat.rs"]
mod workspace_chat;

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// The two thread trees every case runs over ([S-482]).
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
#[derive(Clone, Copy)]
enum Tree {
    /// `/api/v1/chat/threads` over a single root's `.logos/chat.db`.
    Member,
    /// `/api/v1/workspace/chat/threads` over `<workspace root>/.logos/chat.db`.
    Workspace,
}

/// One case's world: the root whose store the tree reads, the real router, the
/// session's intent token and the tree's base route.
struct Fixture {
    _keep: TempDir,
    root: PathBuf,
    router: axum::Router,
    intent: IntentToken,
    base: &'static str,
}

/// The real router over a throwaway root, plus a fresh per-session intent token
/// — enough to reach the store-backed chat routes of `tree`.
fn fixture(tree: Tree) -> Fixture {
    let intent = IntentToken::generate();
    match tree {
        Tree::Member => {
            let dir = TempDir::new().expect("temp dir");
            std::fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");
            let engine = Arc::new(Engine::open(dir.path()));
            Fixture {
                root: dir.path().to_path_buf(),
                router: router_with_intent(engine, intent.clone()),
                _keep: dir,
                intent,
                base: CHAT_THREADS_ROUTE,
            }
        }
        Tree::Workspace => {
            let dir = workspace_chat::workspace();
            Fixture {
                root: dir.path().to_path_buf(),
                router: workspace_chat::router(dir.path(), &intent),
                _keep: dir,
                intent,
                base: WORKSPACE_CHAT_THREADS_ROUTE,
            }
        }
    }
}

/// Run the case `$case(Fixture)` once per tree, as `$case::member` and
/// `$case::workspace`.
macro_rules! over_both_trees {
    ($($case:ident),+ $(,)?) => {$(
        mod $case {
            #[tokio::test]
            async fn member() {
                super::$case(super::fixture(super::Tree::Member)).await;
            }
            #[tokio::test]
            async fn workspace() {
                super::$case(super::fixture(super::Tree::Workspace)).await;
            }
        }
    )+};
}

over_both_trees!(
    list_returns_threads_most_recent_first,
    reads_are_get_only_and_carry_no_secret,
    messages_returns_ordered_transcript,
    messages_for_empty_thread_is_empty_ok,
    messages_for_unknown_thread_is_404,
    delete_requires_a_valid_intent_token,
    delete_unknown_thread_is_404,
    non_loopback_host_is_rejected,
);

/// Seed `n` conversations, each with one user message, returning their ids in
/// creation order. Because `append_message` bumps `updated_at` and `list_threads`
/// orders by `updated_at DESC, id DESC`, the newest-created sorts first.
fn seed_threads(root: &std::path::Path, titles: &[&str]) -> Vec<i64> {
    let mut store = ChatStore::open(root).expect("open chat store");
    titles
        .iter()
        .map(|title| {
            let id = store.create_thread(title).expect("create thread");
            store
                .append_message(id, ChatRole::User, &format!("hello from {title}"), &[])
                .expect("append");
            id
        })
        .collect()
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

/// A mutating POST with optional intent token and origin — the shape the guards
/// see. `None` intent / a foreign origin models a forged or intent-less write.
fn post(path: &str, intent: Option<&str>, origin: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, HOST);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_text(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(resp: Response<Body>) -> serde_json::Value {
    serde_json::from_str(&body_text(resp).await).expect("response body is JSON")
}

// ── The list endpoint ─────────────────────────────────────────────────────────

/// `GET /api/v1/chat/threads` returns every conversation most-recent-first, as
/// the exact `{id,title,updated_at}` contract the rail reads — nothing more.
async fn list_returns_threads_most_recent_first(f: Fixture) {
    let Fixture { root, router, base, .. } = f;
    let ids = seed_threads(&root, &["first", "second", "third"]);
    // The store's own order is the contract the endpoint must faithfully surface.
    let expected: Vec<i64> = ChatStore::open(&root)
        .unwrap()
        .list_threads()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();

    let router = router.clone();
    let resp = router.oneshot(get(base)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let rows = json.as_array().expect("the list is a JSON array");

    let got: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(got, expected, "the endpoint preserves the store's most-recent-first order");
    // Newest-created (highest id) sorts first — the observable "most-recent-first".
    assert_eq!(got.first().copied(), ids.iter().copied().max(), "newest conversation is first");

    // The contract is exactly {id, title, updated_at} — no `created_at`, no other keys.
    let obj = rows[0].as_object().unwrap();
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    assert_eq!(keys, ["id", "title", "updated_at"], "the summary carries only the contract fields");
}

/// The list payload carries no secret: it is a GET (a POST is `405`), and its
/// serialized body names no key/secret field ([NFR-SE-07], [ADR-28]).
async fn reads_are_get_only_and_carry_no_secret(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let ids = seed_threads(&root, &["alpha", "beta"]);
    let router = router.clone();

    // GET-only: a POST to the list route is not an admitted mutating route → 405.
    let posted = router
        .clone()
        .oneshot(post(base, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::METHOD_NOT_ALLOWED, "the list route is GET-only");

    // The `{id}` messages route (no `/delete` suffix) is a read too: a well-formed
    // POST to it is not admitted by `is_thread_delete_route` → 405.
    let posted_msg = router
        .clone()
        .oneshot(post(
            &format!("{base}/{}", ids[0]),
            Some(intent.as_str()),
            Some(ORIGIN),
        ))
        .await
        .unwrap();
    assert_eq!(
        posted_msg.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "the per-thread messages route is GET-only (only `…/{{id}}/delete` is a POST)",
    );

    // No secret rides the read payload.
    let text = body_text(router.oneshot(get(base)).await.unwrap()).await;
    for marker in ["secret", "api_key", "apikey", "last4", "sk-"] {
        assert!(
            !text.to_ascii_lowercase().contains(marker),
            "the thread-list payload must carry no secret (found {marker:?})",
        );
    }
}

// ── The messages endpoint ───────────────────────────────────────────────────

/// `GET /api/v1/chat/threads/{id}` returns the thread's messages in stored order,
/// as the exact producer-contract shape (incl. `tool_traces`), and carries no
/// secret on the richer transcript payload either.
async fn messages_returns_ordered_transcript(f: Fixture) {
    let Fixture { root, router, base, .. } = f;
    let trace = ToolTrace {
        tool_name: "graph_search".to_string(),
        arguments: "{\"q\":\"binder\"}".to_string(),
        result: "found binder.rs".to_string(),
        is_error: false,
    };
    let thread = {
        let mut store = ChatStore::open(&root).unwrap();
        let thread = store.create_thread("ordered").unwrap();
        store.append_message(thread, ChatRole::User, "first question", &[]).unwrap();
        store
            .append_message(thread, ChatRole::Assistant, "first answer", std::slice::from_ref(&trace))
            .unwrap();
        store.append_message(thread, ChatRole::User, "second question", &[]).unwrap();
        thread
    };

    let router = router.clone();
    let resp = router.oneshot(get(&format!("{base}/{thread}"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    // The transcript is the richer payload (content + tool traces) — the more
    // likely secret-leak vector, so scan it too (NFR-SE-07).
    for marker in ["secret", "api_key", "apikey", "last4", "sk-"] {
        assert!(
            !text.to_ascii_lowercase().contains(marker),
            "the transcript payload must carry no secret (found {marker:?})",
        );
    }
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = json.as_array().expect("messages are a JSON array");

    let contents: Vec<&str> = rows.iter().map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(
        contents,
        ["first question", "first answer", "second question"],
        "messages come back in stored ordinal order",
    );
    let roles: Vec<&str> = rows.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user"], "each message's role is preserved");

    // The message object's exact serialized contract (what S-210/S-211 hydrate from).
    let mut keys: Vec<&String> = rows[0].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["content", "created_at", "id", "role", "tool_traces"],
        "each message carries exactly the producer-contract fields",
    );
    // The tool-trace sub-shape the assistant turn surfaces.
    let mut trace_keys: Vec<&String> =
        rows[1]["tool_traces"][0].as_object().unwrap().keys().collect();
    trace_keys.sort();
    assert_eq!(
        trace_keys,
        ["arguments", "is_error", "result", "tool_name"],
        "a tool trace carries exactly the contract fields",
    );
}

/// A real thread with no appended messages returns an honest empty `200` array —
/// the branch the `thread()` existence check exists to distinguish from a `404`.
async fn messages_for_empty_thread_is_empty_ok(f: Fixture) {
    let Fixture { root, router, base, .. } = f;
    let thread = ChatStore::open(&root).unwrap().create_thread("empty").unwrap();

    let router = router.clone();
    let resp = router.oneshot(get(&format!("{base}/{thread}"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a real-but-empty thread is 200, not 404");
    let json = body_json(resp).await;
    assert_eq!(json.as_array().map(|a| a.len()), Some(0), "the transcript is an empty array");
}

/// A request for a thread that does not exist is an honest `404`, never a
/// misleading empty `200`.
async fn messages_for_unknown_thread_is_404(f: Fixture) {
    let Fixture { router, base, .. } = f;
    let router = router.clone();
    let resp = router.oneshot(get(&format!("{base}/99999"))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "an unknown thread is 404");
}

// ── The intent-guarded delete ─────────────────────────────────────────────────

/// A forged (cross-origin) or intent-less delete is rejected `403` and mutates
/// nothing; a valid same-origin + intent-token delete is `204` and removes
/// exactly that thread ([ADR-31], [NFR-SE-06]).
async fn delete_requires_a_valid_intent_token(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let ids = seed_threads(&root, &["keep", "victim"]);
    let victim = ids[1];
    let router = router.clone();
    let route = format!("{base}/{victim}/delete");

    // Cross-origin (forged) — the browser-set Origin is the attacker's → 403.
    let forged = router
        .clone()
        .oneshot(post(&route, Some(intent.as_str()), Some("http://evil.example.com")))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN, "a cross-origin delete is rejected");

    // Same-origin but no intent token → 403.
    let tokenless = router.clone().oneshot(post(&route, None, Some(ORIGIN))).await.unwrap();
    assert_eq!(tokenless.status(), StatusCode::FORBIDDEN, "an intent-less delete is rejected");

    // Neither rejection touched the store.
    assert_eq!(
        ChatStore::open(&root).unwrap().list_threads().unwrap().len(),
        2,
        "a rejected delete mutates nothing",
    );

    // The valid delete removes exactly the victim, leaving the other thread.
    let ok = router.oneshot(post(&route, Some(intent.as_str()), Some(ORIGIN))).await.unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT, "a valid delete succeeds");
    let remaining: Vec<i64> = ChatStore::open(&root)
        .unwrap()
        .list_threads()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(remaining, vec![ids[0]], "only the targeted thread is gone");
}

/// Deleting a thread that does not exist is an idempotent `404`, never a silent
/// success (`delete_thread` returned `false`).
async fn delete_unknown_thread_is_404(f: Fixture) {
    let Fixture { router, intent, base, .. } = f;
    let router = router.clone();
    let resp = router
        .oneshot(post(
            &format!("{base}/4242/delete"),
            Some(intent.as_str()),
            Some(ORIGIN),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "deleting a missing thread is 404");
}

/// The retired global `POST /chat/clear` route is gone — a fully-valid guarded
/// POST to it is `405` (never a wipe), the old constant having been removed
/// ([ADR-47]).
#[tokio::test]
async fn global_chat_clear_route_is_gone() {
    let Fixture { root, router, intent, .. } = fixture(Tree::Member);
    seed_threads(&root, &["survives"]);
    let resp = router
        .oneshot(post("/chat/clear", Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "the global /chat/clear route no longer exists",
    );
    assert_eq!(
        ChatStore::open(&root).unwrap().list_threads().unwrap().len(),
        1,
        "nothing was wiped",
    );
}

// ── Carve-out invariants over the new surface ─────────────────────────────────

/// The new endpoints stay loopback-only: a non-loopback `Host` is `403` before
/// any handler runs ([FR-UI-01]), on both a read and the delete.
async fn non_loopback_host_is_rejected(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let ids = seed_threads(&root, &["local"]);
    let router = router.clone();

    let read = Request::builder()
        .method(Method::GET)
        .uri(base)
        .header(header::HOST, "evil.example.com")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(read).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a non-loopback Host is rejected on reads");

    let del = Request::builder()
        .method(Method::POST)
        .uri(format!("{base}/{}/delete", ids[0]))
        .header(header::HOST, "evil.example.com")
        .header(header::ORIGIN, "http://evil.example.com")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(del).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "a non-loopback Host is rejected on delete");
}
