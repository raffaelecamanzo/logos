// CR-078/ADR-60: the conversation-history surface lives inside the LLM egress
// carve-out, so this end-to-end acceptance scenario (mock-provider, zero real
// egress) compiles only under `--features agents`. A listen-only `--features ui`
// build mounts none of these routes, so the whole test crate is empty there.
#![cfg(feature = "agents")]
//! [UAT-UI-08] acceptance scenario — the chat **conversation-history** surface
//! end-to-end over the assembled `serve --ui` surface (S-211, [CR-053],
//! [FR-UI-26], [FR-UI-20], [FR-UI-18], [NFR-SE-06], [NFR-SE-07], [NFR-CC-04],
//! [ADR-47], [ADR-28], [ADR-31]).
//!
//! These drive the **real router** in-process exactly as `serve --ui` does
//! (`tower::ServiceExt::oneshot`, no socket bound). Where a step needs a turn, a
//! **mock-provider** [`ChatService`](web::chat::ChatService) is injected via
//! [`web::router_with_chat`], so the whole flow runs **offline — zero real
//! egress**.
//!
//! It is the deletion-model successor to [UAT-UI-07]: that scenario's global
//! Clear-history is retired here in favour of **per-conversation delete**
//! ([ADR-47]). This file walks the [UAT-UI-08] steps as one narrative:
//!
//! - step 1 — the rail lists conversations most-recent-first, auto-titled from the
//!   first user message (`uat_ui_08_step1_rail_lists_conversations_most_recent_first`);
//! - step 2 — selecting a conversation restores its ordered transcript
//!   (`uat_ui_08_step2_select_restores_the_ordered_transcript`);
//! - step 3 — "+ New chat" creates no row until the first send, after which the
//!   conversation appears auto-titled at the top
//!   (`uat_ui_08_step3_a_conversation_is_persisted_only_on_its_first_send`);
//! - steps 4/5 — delete is **confirm-gated** in the rail, cancelling deletes
//!   nothing, confirming removes the conversation **and** its per-thread memory,
//!   and there is **no global clear-all** anywhere
//!   (`uat_ui_08_steps_4_5_delete_cascades_and_no_global_clear_survives` +
//!   `uat_ui_08_steps_4_5_the_rail_gates_delete_behind_a_confirm_step`);
//! - step 6 — conversations and the remembered selection survive a `serve --ui`
//!   restart (`uat_ui_08_step6_conversations_persist_across_a_restart`);
//! - step 7 — the reads are GET-only and **no** thread payload carries the key
//!   (`uat_ui_08_step7_reads_are_get_only_and_carry_no_key`);
//! - step 8 — a forged / intent-less delete is rejected and mutates nothing
//!   (`uat_ui_08_step8_a_forged_or_intentless_delete_is_rejected`);
//! - step 9 — the rail collapses behind a toggle below ~1023px
//!   (`uat_ui_08_step9_the_rail_collapses_below_the_tablet_breakpoint`);
//! - step 10 — the self-only CSP is **byte-identical** on every conversation-history
//!   response (`uat_ui_08_step10_csp_is_byte_identical_on_every_history_response`).
//!
//! Two halves are deliberately NOT re-proven here, exactly as [UAT-UI-07] defers
//! its dependency scan: the **built-bundle** CSP cleanliness (no inline
//! `<script>`/`<style>`) is `tests/spa_bundle.rs`, and the **offline no-network**
//! dependency scan is `logos-core/tests/no_network_deps.rs` +
//! `agent-core/tests/carve_out.rs`. Both run as part of this story's verification.
//! The rail's *rendered* interaction (the confirm click, the cancel, the collapse
//! toggle) is covered by the chat Vitest suite (`web/ui/src/views/chat/`); the
//! Rust-side guarantee asserted here is over the **authored SPA source** — that the
//! confirm step and the breakpoint exist and that no clear-all path survives.
//!
//! [UAT-UI-08]: ../../docs/specs/requirements/UAT-UI-08.md
//! [UAT-UI-07]: ../../docs/specs/requirements/UAT-UI-07.md
//! [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
//! [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    response::Response,
};
use chat_agent::orchestrator::{BudgetTree, Orchestrator, RoleModels, SubagentRoster};
use chat_agent::{ChatRole, ChatStore, MemoryStore};
use http_body_util::BodyExt;
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::chat::{spawn_turn, ChatService, ChatStream};
use web::{
    router_with_chat, router_with_intent, IntentToken, CHAT_POST_ROUTE, CHAT_THREADS_ROUTE,
    INTENT_HEADER,
};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";
/// The answer the mock Synthesizer composes — the sentinel proving the streamed
/// answer came from the orchestrator, not a fabricated pass-through.
const SYNTH_SENTINEL: &str = "UAT_UI_08_SYNTHESIZED_ANSWER";

/// The exact self-only CSP the surface stamps on every response ([NFR-SE-06],
/// [FR-UI-02]) — pinned byte-for-byte (mirroring `api_v1.rs`'s `EXPECTED_CSP`) so
/// the conversation-history routes proving "the CSP is byte-identical" is a real
/// regression gate on **every** directive, not a `default-src` substring check.
const EXPECTED_CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'none'; \
                   frame-ancestors 'none'; object-src 'none'";

// ── The mock-provider chat service (the zero-egress substrate) ────────────────

/// A [`ChatService`] that mirrors the PRODUCTION setup path
/// (`web::chat::configured`) — a `thread`-less turn creates its conversation
/// **auto-titled from the first user message** and records that message — then
/// runs the real orchestrator over the offline mock provider. Mirroring the setup
/// is what makes step 3 ("persisted on its first send, auto-titled") an honest
/// assertion rather than a fixture artefact.
struct HistoryChatService {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    root: PathBuf,
}

impl ChatService for HistoryChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut chat = ChatStore::open(&self.root).expect("open chat store");
        let thread = match thread_id {
            Some(id) => id,
            None => chat
                .create_thread_from_message(&question)
                .expect("auto-title a new conversation"),
        };
        chat.append_message(thread, ChatRole::User, &question, &[])
            .expect("record the user message");
        drop(chat);

        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        // One grounding step then a grounded finalize — the terminal answer is
        // composed by the tool-less Synthesizer ([CR-086]), never planner prose.
        let planner = MockCompletionModel::new([
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"graph_navigator","instruction":"look around"}]}"#,
            ),
            MockTurn::text(r#"{"action":"final","grounded":true}"#),
        ]);
        let roster = SubagentRoster::with_models(
            Arc::clone(&self.engine),
            Arc::clone(&self.sandbox),
            RoleModels {
                graph_navigator: MockCompletionModel::new([MockTurn::text("graph: looked around")]),
                governance_analyst: MockCompletionModel::new([]),
                source_reader: MockCompletionModel::new([]),
                synthesizer: MockCompletionModel::new([MockTurn::text(SYNTH_SENTINEL)]),
            },
        );
        let orchestrator = Orchestrator::new(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, thread, turn)
    }
}

// ── Fixtures + request builders ───────────────────────────────────────────────

/// A writable fixture root with `.logos/` and a real `src/lib.rs`, so the graph
/// tools have something to read and the sandbox has a root to confine to.
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

/// A fixture root with a configured OpenAI-compatible provider + key, so the chat
/// surface is past its configure-first state.
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
        "[chat]\napi_key = \"sk-uat08-secret-9271\"\n",
    )
    .expect("write secrets.toml");
    dir
}

/// Seed `n` conversations, each auto-titled from its own first user message —
/// the shape the rail reads. `append_message` bumps `updated_at`, and
/// `list_threads` orders by `updated_at DESC, id DESC`, so the last seeded sorts
/// first.
fn seed_conversations(root: &Path, first_messages: &[&str]) -> Vec<i64> {
    let mut store = ChatStore::open(root).expect("open chat store");
    first_messages
        .iter()
        .map(|msg| {
            let id = store.create_thread_from_message(msg).expect("create thread");
            store
                .append_message(id, ChatRole::User, msg, &[])
                .expect("append user message");
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

/// A mutating `POST` with optional intent token and `Origin` — the shape the
/// guards see. A missing token / foreign origin models a forged write.
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

/// A guarded streaming `POST /chat` turn, optionally continuing `thread`.
fn chat_turn(intent: &str, question: &str, thread: Option<i64>) -> Request<Body> {
    let body = match thread {
        Some(id) => format!("q={question}&thread={id}"),
        None => format!("q={question}"),
    };
    Request::builder()
        .method(Method::POST)
        .uri(CHAT_POST_ROUTE)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "text/event-stream")
        .header(INTENT_HEADER, intent)
        .body(Body::from(body))
        .unwrap()
}

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_json(resp: Response) -> serde_json::Value {
    serde_json::from_str(&body_string(resp).await).expect("response body is JSON")
}

/// The exact self-only CSP, asserted byte-for-byte ([NFR-SE-06]).
fn assert_byte_identical_csp(resp: &Response, what: &str) {
    let csp = resp
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap_or_else(|| panic!("{what} carries a CSP header"))
        .to_str()
        .unwrap();
    assert_eq!(csp, EXPECTED_CSP, "{what}: the self-only CSP is byte-identical");
}

/// `<root>/web/ui` — the authored SPA project (CARGO_MANIFEST_DIR is `<root>/web`).
fn ui_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn read_ui(rel: &str) -> String {
    let path = ui_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ── Step 1: the rail lists conversations most-recent-first, auto-titled ────────

/// Step 1: with conversations present, `GET /api/v1/chat/threads` — the rail's one
/// data source — returns them **most-recent-first**, each **auto-titled from its
/// first user message** ([FR-UI-26]).
#[tokio::test]
async fn uat_ui_08_step1_rail_lists_conversations_most_recent_first() {
    let dir = configured_root();
    let ids = seed_conversations(dir.path(), &["where is the binder?", "what is risky here?"]);
    let engine = Arc::new(Engine::open(dir.path()));
    let router = router_with_intent(engine, IntentToken::generate());

    let resp = router.oneshot(get(CHAT_THREADS_ROUTE)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    let rows = list.as_array().expect("the thread list is a JSON array");
    assert_eq!(rows.len(), 2, "both conversations are listed: {list}");

    // Most-recent-first: the last seeded conversation leads the rail.
    assert_eq!(rows[0]["id"].as_i64(), Some(ids[1]), "the newest conversation is first");
    assert_eq!(rows[1]["id"].as_i64(), Some(ids[0]));
    // Each carries the auto-title derived from its own first user message (S-208),
    // not a generic placeholder.
    assert_eq!(rows[0]["title"].as_str(), Some("what is risky here?"));
    assert_eq!(rows[1]["title"].as_str(), Some("where is the binder?"));
}

// ── Step 2: selecting a conversation restores its ordered transcript ──────────

/// Step 2: selecting an earlier conversation restores its **full, ordered**
/// transcript from `GET /api/v1/chat/threads/{id}` — the messages the rail
/// hydrates — and that thread is then the one a turn appends to ([FR-UI-26]).
///
/// The transcript is seeded through the store rather than by a live turn, and
/// deliberately so: the current turn path (`web::chat::configured`) records the
/// **user** message but never appends the assistant's final answer to
/// `chat_messages` — the answer lands in the per-turn `chat_scratchpad` instead. So
/// this asserts the READ contract the rail depends on (faithful, ordered, both
/// roles) over a transcript that has both. The producer-side gap — a restored
/// conversation currently replays only the questions — is recorded in this task's
/// implementation notes for the sprint review; it lives in the merged S-208/S-209
/// layer, not in this story's SPA surface.
#[tokio::test]
async fn uat_ui_08_step2_select_restores_the_ordered_transcript() {
    let dir = configured_root();
    let ids = seed_conversations(dir.path(), &["first conversation", "second conversation"]);
    let earlier = ids[0];
    {
        let mut store = ChatStore::open(dir.path()).expect("open chat store");
        store
            .append_message(earlier, ChatRole::Assistant, "the earlier answer", &[])
            .expect("append assistant message");
        store
            .append_message(earlier, ChatRole::User, "a follow-up", &[])
            .expect("append follow-up");
    }
    let engine = Arc::new(Engine::open(dir.path()));
    let router = router_with_intent(engine, IntentToken::generate());

    let resp = router
        .oneshot(get(&format!("{CHAT_THREADS_ROUTE}/{earlier}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let messages = body_json(resp).await;
    let rows = messages.as_array().expect("the transcript is a JSON array");
    // Stored ordinal order, verbatim — the restore is faithful, not re-sorted.
    let ordered: Vec<(&str, &str)> = rows
        .iter()
        .map(|m| {
            (
                m["role"].as_str().expect("role"),
                m["content"].as_str().expect("content"),
            )
        })
        .collect();
    assert_eq!(
        ordered,
        vec![
            ("user", "first conversation"),
            ("assistant", "the earlier answer"),
            ("user", "a follow-up"),
        ],
        "the earlier conversation restores its full history in order",
    );
}

// ── Step 3: "+ New chat" persists only on the first send ──────────────────────

/// Step 3: "+ New chat" creates **no** conversation — the SPA simply sends the
/// next turn with no `thread` ([FR-UI-26] "no empty rows"). That first send is
/// what persists the conversation, **auto-titled** from the message, and it then
/// leads the most-recent-first rail; the follow-up turn continues the SAME
/// conversation rather than forking a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uat_ui_08_step3_a_conversation_is_persisted_only_on_its_first_send() {
    let dir = configured_root();
    let root = dir.path().to_path_buf();
    let engine = Arc::new(Engine::start(&root).expect("engine starts"));
    let sandbox = Arc::new(Sandbox::new(&root, std::iter::empty()).expect("sandbox"));
    let intent = IntentToken::generate();
    let service: Arc<dyn ChatService> = Arc::new(HistoryChatService {
        engine: Arc::clone(&engine),
        sandbox,
        root: root.clone(),
    });
    let router = router_with_chat(Arc::clone(&engine), intent.clone(), service);

    // "+ New chat" is a client-side reset: before the first send the store holds no
    // conversation at all (no empty row was created).
    assert!(
        ChatStore::open(&root).unwrap().is_empty().unwrap(),
        "a fresh composer creates no conversation until it is sent",
    );

    // The first send persists it, auto-titled from the message.
    let resp = router
        .clone()
        .oneshot(chat_turn(intent.as_str(), "what+is+risky", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let stream = body_string(resp).await;
    assert!(
        stream.contains(SYNTH_SENTINEL),
        "the turn streamed the synthesized answer offline: {stream}",
    );

    let threads = ChatStore::open(&root).unwrap().list_threads().unwrap();
    assert_eq!(threads.len(), 1, "exactly one conversation now exists");
    assert_eq!(
        threads[0].title, "what is risky",
        "the new conversation is auto-titled from its first user message",
    );
    let created = threads[0].id;

    // A follow-up carrying that id continues the SAME conversation (the rail's
    // adopt-then-continue contract), never forking a second row.
    let resp = router
        .oneshot(chat_turn(intent.as_str(), "and+who+calls+it", Some(created)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = body_string(resp).await;
    let threads = ChatStore::open(&root).unwrap().list_threads().unwrap();
    assert_eq!(threads.len(), 1, "the follow-up did not fork a second conversation");
    assert_eq!(
        threads[0].title, "what is risky",
        "the auto-title is set once and never rewritten by a later turn",
    );
}

// ── Steps 4/5: confirm-gated delete, cascade, and no global clear ─────────────

/// Steps 4/5 (server half): the guarded per-thread delete removes **exactly** that
/// conversation and its per-thread memory by cascade, leaving the others intact —
/// and the global clear-all is **gone**: `POST /chat/clear` is `405`, and repeating
/// the delete is an idempotent `404` rather than a wider wipe ([FR-UI-26],
/// [FR-UI-20], [ADR-47]).
#[tokio::test]
async fn uat_ui_08_steps_4_5_delete_cascades_and_no_global_clear_survives() {
    let dir = configured_root();
    let root = dir.path().to_path_buf();
    let ids = seed_conversations(&root, &["keep me", "delete me"]);
    let (keep, victim) = (ids[0], ids[1]);
    {
        let memory = MemoryStore::open(&root).expect("open memory");
        memory.set_working_memory(keep, "the kept summary").expect("seed kept memory");
        memory
            .set_working_memory(victim, "the doomed summary")
            .expect("seed victim memory");
    }
    let engine = Arc::new(Engine::open(&root));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());
    let route = format!("{CHAT_THREADS_ROUTE}/{victim}/delete");

    // The retired global clear cannot wipe anything — the route no longer exists.
    let stale = router
        .clone()
        .oneshot(post("/chat/clear", Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(
        stale.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "the global /chat/clear route is gone (405, never a wipe)",
    );

    // The confirmed delete removes exactly the one conversation.
    let resp = router
        .clone()
        .oneshot(post(&route, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "the confirmed delete succeeds");

    let remaining = ChatStore::open(&root).unwrap().list_threads().unwrap();
    assert_eq!(remaining.len(), 1, "only the deleted conversation is gone");
    assert_eq!(remaining[0].id, keep, "the other conversation is untouched");

    // The cascade wiped the victim's per-thread memory and left the keeper's — the
    // "no orphaned memory" half of [FR-UI-20], proven through the HTTP route.
    let memory = MemoryStore::open(&root).expect("reopen memory");
    assert!(
        memory.working_memory(victim).unwrap().is_none(),
        "the deleted conversation's per-thread memory cascaded away",
    );
    assert_eq!(
        memory.working_memory(keep).unwrap().as_deref(),
        Some("the kept summary"),
        "a per-conversation delete never touches another conversation's memory",
    );

    // Repeating it is an idempotent 404 — never a broader wipe.
    let again = router
        .oneshot(post(&route, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND, "a repeated delete is an honest 404");
    assert_eq!(
        ChatStore::open(&root).unwrap().list_threads().unwrap().len(),
        1,
        "the repeat mutated nothing",
    );
}

/// Steps 4/5 (surface half): the authored SPA gates the delete behind a
/// **confirmation** step in the rail, and **no** global clear-all path survives
/// anywhere in the SPA source — not a control, not a client helper, not the retired
/// route constant ([FR-UI-26], [ADR-47]). The *rendered* confirm/cancel interaction
/// is covered by the chat Vitest suite; this is the source-level regression lock
/// that a future edit cannot quietly re-add a one-click delete or a clear-all.
#[test]
fn uat_ui_08_steps_4_5_the_rail_gates_delete_behind_a_confirm_step() {
    let rail = read_ui("src/views/chat/ThreadList.tsx");
    // The delete affordance arms a confirmation; `onDelete` fires only from the
    // confirming branch, so a single click can never delete.
    assert!(
        rail.contains("confirmingId"),
        "the rail tracks which row is awaiting confirmation",
    );
    assert!(
        rail.contains("onDelete(t.id)"),
        "the rail dispatches the delete for a specific conversation",
    );
    let confirm_at = rail.find("threadConfirm").expect("the rail renders a confirm panel");
    let dispatch_at = rail.find("onDelete(t.id)").expect("the rail dispatches a delete");
    assert!(
        confirm_at < dispatch_at,
        "the delete dispatch lives inside the confirm panel, never on the row button",
    );

    // No clear-all survives: not the control, not the helper, not the route.
    for (file, source) in [
        ("ChatView.tsx", read_ui("src/views/chat/ChatView.tsx")),
        ("chatRuntime.tsx", read_ui("src/views/chat/chatRuntime.tsx")),
        ("chatClient.ts", read_ui("src/api/chatClient.ts")),
    ] {
        for retired in ["Clear history", "clearChatHistory", "CHAT_CLEAR_ROUTE", "/chat/clear"] {
            assert!(
                !source.contains(retired),
                "{file} must not carry the retired global clear ({retired})",
            );
        }
    }
}

// ── Step 6: conversations + the selection persist across a restart ────────────

/// Step 6: the conversations — and the SPA's memory of which one was open —
/// survive a `serve --ui` restart. The store half is proven by dropping every
/// handle and re-opening from the same files; the selection half is the SPA's
/// `localStorage` key, asserted over the authored source (its restore behaviour is
/// covered by the chat Vitest suite) ([FR-UI-26], [FR-UI-20]).
#[test]
fn uat_ui_08_step6_conversations_persist_across_a_restart() {
    let dir = configured_root();
    let root = dir.path().to_path_buf();

    let (kept, selected) = {
        let ids = seed_conversations(&root, &["an older chat", "the open chat"]);
        let memory = MemoryStore::open(&root).expect("open memory");
        memory
            .set_working_memory(ids[1], "what the open chat was about")
            .expect("seed memory");
        (ids[0], ids[1])
        // Every handle drops here — connections (and WAL) close: the restart.
    };

    // Re-open from the same files: brand-new handles over the persisted data.
    let store = ChatStore::open(&root).expect("reopen chat store");
    let threads = store.list_threads().expect("list");
    assert_eq!(threads.len(), 2, "both conversations survived the restart");
    assert_eq!(threads[0].id, selected, "the most-recent-first order survived too");
    assert_eq!(threads[1].id, kept);
    assert_eq!(
        store.messages(selected).expect("messages").len(),
        1,
        "the open conversation's transcript survived",
    );
    assert_eq!(
        MemoryStore::open(&root)
            .expect("reopen memory")
            .working_memory(selected)
            .unwrap()
            .as_deref(),
        Some("what the open chat was about"),
        "its per-thread memory survived, so a follow-up still sees prior context",
    );

    // The SPA remembers WHICH conversation was open across the same restart.
    let runtime = read_ui("src/views/chat/chatRuntime.tsx");
    assert!(
        runtime.contains(r#"ACTIVE_THREAD_KEY = "logos.chat.activeThread""#),
        "the SPA persists the open conversation under a stable storage key",
    );
}

// ── Step 7: GET-only reads carrying no key ────────────────────────────────────

/// Step 7: the thread-list and thread-messages reads are **GET** ([ADR-28]) — a
/// `POST` to either is `405` before any handler runs — and the masked write-only
/// chat key never appears on **any** thread payload ([NFR-SE-07]).
#[tokio::test]
async fn uat_ui_08_step7_reads_are_get_only_and_carry_no_key() {
    const RAW_KEY: &str = "sk-uat08-secret-9271";
    let dir = configured_root();
    let ids = seed_conversations(dir.path(), &["a question about the key"]);
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());
    let thread_route = format!("{CHAT_THREADS_ROUTE}/{}", ids[0]);

    for path in [CHAT_THREADS_ROUTE, thread_route.as_str()] {
        // The read answers a GET…
        let resp = router.clone().oneshot(get(path)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path} is a GET read");
        let body = body_string(resp).await;
        // …carrying no secret: neither the raw key nor its masked last-4, and no
        // key-shaped field at all.
        assert!(!body.contains(RAW_KEY), "{path} never carries the raw key");
        assert!(!body.contains("9271"), "{path} never carries even the masked last-4");
        assert!(!body.contains("api_key"), "{path} carries no key field: {body}");

        // …and rejects a fully-guarded POST: the reads are not a write seam.
        let written = router
            .clone()
            .oneshot(post(path, Some(intent.as_str()), Some(ORIGIN)))
            .await
            .unwrap();
        assert_eq!(
            written.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{path} is GET-only — a POST is 405 before any handler runs",
        );
    }
}

// ── Step 8: a forged or intent-less delete is rejected ────────────────────────

/// Step 8: the delete is intent-guarded ([ADR-31], [NFR-SE-06]) — a cross-origin
/// (forged) delete and a same-origin delete without the per-session intent token
/// are each `403`, and **neither mutates the store**.
#[tokio::test]
async fn uat_ui_08_step8_a_forged_or_intentless_delete_is_rejected() {
    let dir = configured_root();
    let root = dir.path().to_path_buf();
    let ids = seed_conversations(&root, &["keep me", "target me"]);
    let target = ids[1];
    let engine = Arc::new(Engine::open(&root));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());
    let route = format!("{CHAT_THREADS_ROUTE}/{target}/delete");

    // Cross-origin: the browser-set Origin is the attacker's page.
    let forged = router
        .clone()
        .oneshot(post(&route, Some(intent.as_str()), Some("http://evil.example.com")))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN, "a cross-origin delete is rejected");

    // Same-origin but token-less: a cross-origin page cannot read the token.
    let tokenless = router
        .clone()
        .oneshot(post(&route, None, Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(tokenless.status(), StatusCode::FORBIDDEN, "an intent-less delete is rejected");

    assert_eq!(
        ChatStore::open(&root).unwrap().list_threads().unwrap().len(),
        2,
        "a rejected delete mutates nothing",
    );

    // The genuine, intentional delete still works — the guard is a filter, not a wall.
    let ok = router
        .oneshot(post(&route, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT, "the guarded delete succeeds");
}

// ── Step 9: the rail collapses below the tablet breakpoint ────────────────────

/// Step 9: below the ~1023px tablet breakpoint the rail collapses behind a toggle
/// and the conversation keeps full width (frontend-design §4.13, [FR-UI-26]). The
/// rendered disclosure is covered by the chat Vitest suite; the authored-source
/// guarantee asserted here is that the toggle and the breakpoint exist.
#[test]
fn uat_ui_08_step9_the_rail_collapses_below_the_tablet_breakpoint() {
    let css = read_ui("src/views/chat/Chat.module.css");
    let narrow = css
        .split("@media (max-width: 1023px)")
        .nth(1)
        .expect("the chat layout has a ≤1023px breakpoint");
    assert!(
        narrow.contains(".railToggle") && narrow.contains(".railPane"),
        "below the breakpoint the toggle appears and the rail hides behind it",
    );
    assert!(
        narrow.contains("grid-template-columns: 1fr"),
        "the layout stacks to a single column so the conversation keeps full width",
    );

    let view = read_ui("src/views/chat/ChatView.tsx");
    assert!(
        view.contains(r#"aria-controls="chat-rail""#) && view.contains("aria-expanded={railOpen}"),
        "the toggle is an accessible disclosure over the rail region",
    );
}

// ── Step 10: the CSP is byte-identical on every history response ──────────────

/// Step 10 (behavioural half): every conversation-history response — the list GET,
/// the messages GET, the `204` delete, and the `404` miss — carries the
/// **byte-identical** self-only CSP ([NFR-SE-06]). The conversation-history surface
/// widened the mutating allow-list by exactly one route and must not have relaxed a
/// single directive.
///
/// The *built-bundle* half of step 10 (no inline `<script>`/`<style>`) is
/// `tests/spa_bundle.rs`; the offline no-network dependency scan is
/// `logos-core/tests/no_network_deps.rs` + `agent-core/tests/carve_out.rs`.
#[tokio::test]
async fn uat_ui_08_step10_csp_is_byte_identical_on_every_history_response() {
    let dir = configured_root();
    let ids = seed_conversations(dir.path(), &["a conversation"]);
    let engine = Arc::new(Engine::open(dir.path()));
    let intent = IntentToken::generate();
    let router = router_with_intent(engine, intent.clone());
    let thread_route = format!("{CHAT_THREADS_ROUTE}/{}", ids[0]);
    let delete_route = format!("{CHAT_THREADS_ROUTE}/{}/delete", ids[0]);

    let list = router.clone().oneshot(get(CHAT_THREADS_ROUTE)).await.unwrap();
    assert_byte_identical_csp(&list, "the thread-list read");

    let messages = router.clone().oneshot(get(&thread_route)).await.unwrap();
    assert_byte_identical_csp(&messages, "the thread-messages read");

    let deleted = router
        .clone()
        .oneshot(post(&delete_route, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_byte_identical_csp(&deleted, "the per-thread delete");

    // Error responses carry it too — the header is stamped outermost, so no path
    // (not even a miss) escapes the policy.
    let missing = router
        .oneshot(post(&delete_route, Some(intent.as_str()), Some(ORIGIN)))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_byte_identical_csp(&missing, "a delete miss");
}
