// CR-078/ADR-60: the chat conversation-history read/delete API is part of the
// LLM egress carve-out, so these route-fitness tests exercise the `agents`-gated
// surface and compile only under `--features agents`. A listen-only
// `--features ui` build mounts none of these routes, so the whole test crate is
// empty there.
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
//! - `POST /api/v1/chat/threads/{id}/rename` (S-623, [FR-UI-48]) has the delete's
//!   shape: a forged or intent-less rename is `403`, a blank or missing title is
//!   `400`, and neither changes the title; a valid one is `204` and survives a
//!   restart; an unknown id is `404`; only the enumerated verbs are admitted;
//! - `POST /api/v1/chat/threads/{id}/truncate` (S-624, [FR-UI-46]) has the same
//!   shape: a forged or intent-less truncate is `403`, a missing or malformed
//!   `user_turn` is `400`, and neither changes the thread; a valid one cuts back to
//!   before the named question — messages and the memory written after the cut —
//!   and the cut survives a restart; an unknown id is `404`;
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
//! [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
//! [FR-UI-46]: ../../docs/specs/requirements/FR-UI-46.md
//! [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
//! [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, Response, StatusCode};
use chat_agent::{ChatRole, ChatStore, MemoryStore, ScratchpadEntry, ToolTrace};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use logos_chat::{router_with_intent, CHAT_THREADS_ROUTE, WORKSPACE_CHAT_THREADS_ROUTE};
use web::{IntentToken, INTENT_HEADER};

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
    rename_requires_a_valid_intent_token,
    rename_refuses_a_blank_or_missing_title,
    rename_persists_across_a_restart,
    rename_unknown_thread_is_404,
    truncate_requires_a_valid_intent_token,
    truncate_cuts_the_messages_and_the_memory_after_the_cut,
    truncate_refuses_a_missing_or_malformed_user_turn,
    truncate_past_the_stored_questions_changes_nothing,
    truncate_unknown_thread_is_404,
    only_the_enumerated_thread_verbs_are_admitted,
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

/// A guarded, form-encoded POST — a rename as the SPA sends it.
fn post_form(path: &str, intent: Option<&str>, origin: Option<&str>, body: &str) -> Request<Body> {
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
    builder.body(Body::from(body.to_string())).unwrap()
}

/// The stored title of thread `id` under `root`.
fn title_of(root: &std::path::Path, id: i64) -> String {
    ChatStore::open(root).unwrap().thread(id).unwrap().expect("the thread exists").title
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

    // The `{id}` messages route (no verb suffix) is a read too: a well-formed POST
    // to it is not admitted by `is_thread_write_route` → 405.
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
        "the per-thread messages route is GET-only (only the `…/{{id}}/{{verb}}` writes are POSTs)",
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

// ── The rename endpoint (S-623, FR-UI-48) ──────────────────────────────────────

/// A rename is intent-guarded exactly like the delete: a forged (cross-origin) or
/// intent-less rename is `403` and leaves the title as it was; a valid one is
/// `204` and renames exactly that thread.
async fn rename_requires_a_valid_intent_token(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let ids = seed_threads(&root, &["keep", "target"]);
    let route = format!("{base}/{}/rename", ids[1]);
    let body = "title=Binder%20map";

    let forged = router
        .clone()
        .oneshot(post_form(&route, Some(intent.as_str()), Some("http://evil.example.com"), body))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN, "a cross-origin rename is rejected");
    let tokenless = router.clone().oneshot(post_form(&route, None, Some(ORIGIN), body)).await.unwrap();
    assert_eq!(tokenless.status(), StatusCode::FORBIDDEN, "an intent-less rename is rejected");
    assert_eq!(title_of(&root, ids[1]), "target", "a rejected rename changes nothing");

    let ok = router
        .oneshot(post_form(&route, Some(intent.as_str()), Some(ORIGIN), body))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT, "a valid rename succeeds");
    assert_eq!(title_of(&root, ids[1]), "Binder map");
    assert_eq!(title_of(&root, ids[0]), "keep", "only the targeted thread is renamed");
}

/// A blank, whitespace-only, missing or over-long title is a `400` — and the
/// title is unchanged.
async fn rename_refuses_a_blank_or_missing_title(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let id = seed_threads(&root, &["original"])[0];
    let route = format!("{base}/{id}/rename");
    let long = format!("title={}", "x".repeat(chat_agent::THREAD_TITLE_MAX + 1));
    for body in ["title=", "title=%20%20%0A", "", "name=Elsewhere", long.as_str()] {
        let resp = router
            .clone()
            .oneshot(post_form(&route, Some(intent.as_str()), Some(ORIGIN), body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body:?} is refused");
        assert_eq!(title_of(&root, id), "original", "{body:?} leaves the title unchanged");
    }
}

/// A rename persists across a `serve --ui` restart: a fresh router over the same
/// root lists the new title.
async fn rename_persists_across_a_restart(f: Fixture) {
    let Fixture { root, router, intent, base, _keep } = f;
    let id = seed_threads(&root, &["before"])[0];
    let resp = router
        .oneshot(post_form(
            &format!("{base}/{id}/rename"),
            Some(intent.as_str()),
            Some(ORIGIN),
            "title=After%20the%20restart",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The restart: a new router (and, for the member tree, a new engine) over the
    // same on-disk store, with a new session's intent token.
    let restarted = if base == CHAT_THREADS_ROUTE {
        router_with_intent(Arc::new(Engine::open(&root)), IntentToken::generate())
    } else {
        workspace_chat::router(&root, &IntentToken::generate())
    };
    let rows = body_json(restarted.oneshot(get(base)).await.unwrap()).await;
    assert_eq!(rows[0]["id"].as_i64(), Some(id));
    assert_eq!(rows[0]["title"].as_str(), Some("After the restart"));
}

/// Renaming a thread that does not exist is a `404`.
async fn rename_unknown_thread_is_404(f: Fixture) {
    let Fixture { router, intent, base, .. } = f;
    let resp = router
        .oneshot(post_form(
            &format!("{base}/4242/rename"),
            Some(intent.as_str()),
            Some(ORIGIN),
            "title=Ghost",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "renaming a missing thread is 404");
}

// ── The truncate endpoint (S-624, FR-UI-46) ────────────────────────────────────

/// Seed one conversation of `turns` answered questions as the chat surface writes
/// them — the question, the turn's scratchpad, then the answer — plus a
/// working-memory summary written after the last turn. Returns the thread id.
fn seed_conversation(root: &std::path::Path, turns: usize) -> i64 {
    let mut store = ChatStore::open(root).expect("open chat store");
    let memory = MemoryStore::open(root).expect("open chat memory");
    let id = store.create_thread("conversation").expect("create thread");
    for i in 0..turns {
        store.append_message(id, ChatRole::User, &format!("question {i}"), &[]).unwrap();
        let turn = memory.next_turn(id).unwrap();
        memory.record(id, turn, &ScratchpadEntry::Note { note: format!("step {i}") }).unwrap();
        store.append_message(id, ChatRole::Assistant, &format!("answer {i}"), &[]).unwrap();
    }
    memory.set_working_memory(id, "the whole conversation").unwrap();
    id
}

/// The stored message texts of thread `id` under `root`, in order.
fn contents_of(root: &std::path::Path, id: i64) -> Vec<String> {
    ChatStore::open(root).unwrap().messages(id).unwrap().into_iter().map(|m| m.content).collect()
}

/// The memory rows (scratchpad + working memory) thread `id` holds under `root`.
fn memory_rows_of(root: &std::path::Path, id: i64) -> i64 {
    rusqlite::Connection::open(chat_agent::db_path(root))
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM chat_scratchpad WHERE thread_id = ?1)
                  + (SELECT COUNT(*) FROM chat_working_memory WHERE thread_id = ?1)",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

/// A truncate is intent-guarded exactly like the rename: a forged (cross-origin) or
/// intent-less truncate is `403` and leaves the conversation whole; a valid one is
/// `204`.
async fn truncate_requires_a_valid_intent_token(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let id = seed_conversation(&root, 2);
    let route = format!("{base}/{id}/truncate");

    let forged = router
        .clone()
        .oneshot(post_form(&route, Some(intent.as_str()), Some("http://evil.example.com"), "user_turn=0"))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN, "a cross-origin truncate is rejected");
    let tokenless =
        router.clone().oneshot(post_form(&route, None, Some(ORIGIN), "user_turn=0")).await.unwrap();
    assert_eq!(tokenless.status(), StatusCode::FORBIDDEN, "an intent-less truncate is rejected");
    assert_eq!(contents_of(&root, id).len(), 4, "a rejected truncate deletes nothing");
    assert_eq!(memory_rows_of(&root, id), 3, "nor any memory");

    let ok = router
        .oneshot(post_form(&route, Some(intent.as_str()), Some(ORIGIN), "user_turn=1"))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT, "a valid truncate succeeds");
    assert_eq!(contents_of(&root, id), ["question 0", "answer 0"]);
}

/// AC: cutting back to before the second of four questions leaves the first two
/// messages; after a restart the thread reads exactly those, and only the first
/// turn's memory remains — the later scratchpads and the summary written after the
/// cut are gone (counted).
async fn truncate_cuts_the_messages_and_the_memory_after_the_cut(f: Fixture) {
    let Fixture { root, router, intent, base, _keep } = f;
    let id = seed_conversation(&root, 4);
    let other = seed_conversation(&root, 1);
    assert_eq!(memory_rows_of(&root, id), 5, "four scratchpads and a summary");

    let resp = router
        .oneshot(post_form(&format!("{base}/{id}/truncate"), Some(intent.as_str()), Some(ORIGIN), "user_turn=1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The restart: a new router over the same on-disk store.
    let restarted = if base == CHAT_THREADS_ROUTE {
        router_with_intent(Arc::new(Engine::open(&root)), IntentToken::generate())
    } else {
        workspace_chat::router(&root, &IntentToken::generate())
    };
    let rows = body_json(restarted.oneshot(get(&format!("{base}/{id}"))).await.unwrap()).await;
    let read: Vec<&str> = rows.as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(read, ["question 0", "answer 0"], "the thread reads exactly the kept messages");
    assert_eq!(memory_rows_of(&root, id), 1, "only the first turn's scratchpad remains");
    assert_eq!(contents_of(&root, other).len(), 2, "another conversation is untouched");
    assert_eq!(memory_rows_of(&root, other), 2);
}

/// A missing, blank, negative or non-numeric `user_turn` — or another field — is a
/// `400`, and the conversation is unchanged.
async fn truncate_refuses_a_missing_or_malformed_user_turn(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let id = seed_conversation(&root, 2);
    let route = format!("{base}/{id}/truncate");
    for body in ["", "user_turn=", "user_turn=-1", "user_turn=one", "user_turn=1.5", "turn=1"] {
        let resp = router
            .clone()
            .oneshot(post_form(&route, Some(intent.as_str()), Some(ORIGIN), body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body:?} is refused");
        assert_eq!(contents_of(&root, id).len(), 4, "{body:?} deletes nothing");
    }
    assert_eq!(memory_rows_of(&root, id), 3);
}

/// A question past the stored ones leaves nothing to cut: `204`, nothing deleted —
/// the case of a re-run whose question never reached the store.
async fn truncate_past_the_stored_questions_changes_nothing(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let id = seed_conversation(&root, 2);
    let resp = router
        .oneshot(post_form(&format!("{base}/{id}/truncate"), Some(intent.as_str()), Some(ORIGIN), "user_turn=2"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(contents_of(&root, id).len(), 4);
    assert_eq!(memory_rows_of(&root, id), 3);
}

/// Truncating a thread that does not exist is a `404`.
async fn truncate_unknown_thread_is_404(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    seed_conversation(&root, 1);
    let resp = router
        .oneshot(post_form(&format!("{base}/4242/truncate"), Some(intent.as_str()), Some(ORIGIN), "user_turn=0"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "truncating a missing thread is 404");
}

/// The allow-list admits exactly the enumerated per-thread verbs: a near-miss
/// verb, a non-integer id or an extra segment is `405` before any handler runs.
async fn only_the_enumerated_thread_verbs_are_admitted(f: Fixture) {
    let Fixture { root, router, intent, base, .. } = f;
    let id = seed_threads(&root, &["untouched"])[0];
    for path in [
        format!("{base}/{id}/renamex"),
        format!("{base}/{id}/rename/extra"),
        format!("{base}/x{id}/rename"),
        format!("{base}/{id}/clear"),
        format!("{base}/{id}/truncat"),
        format!("{base}/{id}/truncate/0"),
        format!("{base}/x{id}/truncate"),
    ] {
        let resp = router
            .clone()
            .oneshot(post_form(&path, Some(intent.as_str()), Some(ORIGIN), "title=Nope&user_turn=0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "{path} is not admitted");
    }
    assert_eq!(title_of(&root, id), "untouched");
    assert_eq!(contents_of(&root, id).len(), 1, "nothing was truncated");
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

/// The rename field's limit in the SPA is the store's cap (S-623): a drift would let
/// the field accept names the route refuses `400`, or refuse names it accepts.
#[test]
fn the_spa_title_cap_is_the_stores() {
    let model = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/ui/src/views/chat/chatModel.ts"),
    )
    .expect("read chatModel.ts");
    let declared = format!("export const THREAD_TITLE_MAX = {};", chat_agent::THREAD_TITLE_MAX);
    assert!(model.contains(&declared), "chatModel.ts must declare `{declared}`");
}
