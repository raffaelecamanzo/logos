//! The chat extension's web routes ([S-643]): the turn, the conversation-history
//! API, and the workspace chat's twins of both — moved out of the web adapter
//! with their contract unchanged.
//!
//! Every route sits behind the web surface's guard stack (loopback host, CSP, the
//! method guard and the intent guard), because the host mounts it there. Each
//! `POST` here is declared with [`WebRoute::post`], which is this extension's
//! entry in the method guard's `POST` allow-list: the guard admits a `POST` the
//! router matched to one of these patterns, and every one of them is
//! additionally same-origin + intent-token gated ([NFR-SE-06]).
//!
//! Handlers hold no agent logic ([ADR-01]): a turn is forwarded to a
//! [`ChatService`] and streamed back; a thread read or write is one
//! [`ChatStore`] call on the blocking pool, through the web adapter's own
//! [`web::bridge`] / [`web::run_blocking`] hop.
//!
//! [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::{FromRef, FromRequestParts, Path as UrlPath, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use chat_agent::{ChatMessage, ChatStore, ChatThread, Preset};
use logos_core::federation::{Backing, BuildDependencies, ContractBridge};
use logos_core::observability::Surface;
use logos_core::Engine;
use logos_ext::web::{WebContext, WebRoute};
use web::{bridge, run_blocking, wants_event_stream, workspace_root_of, MemberEngine, WorkspaceRoot};

use crate::service::{self, ChatService, ConfiguredChatService, WorkspaceChatService};

/// The chat turn `POST` route (S-170, [FR-UI-19], [NFR-SE-06]). It carries a chat
/// turn and streams the orchestrator's events back as Server-Sent Events
/// (text/event-stream) under the unchanged self-only CSP, or — without
/// `Accept: text/event-stream` — renders the buffered answer (the [FR-UI-19]
/// progressive-enhancement fallback). Intent-guarded like every `POST`; the
/// streaming request rides the `POST` precisely so it keeps that proof, which a
/// `GET` `EventSource` cannot carry ([NFR-SE-06]). A `GET` here is a browser
/// navigation to the SPA's Chat view, answered with the SPA shell
/// ([`WebRoute::shell`]).
///
/// [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
pub const CHAT_POST_ROUTE: &str = "/chat";

/// The conversation-history list route (S-209, [FR-UI-26], [ADR-47], [ADR-28]):
/// every persisted conversation (`id`, `title`, `updated_at`), most-recent-first.
/// A pure loopback, same-origin GET carrying no secret ([NFR-SE-07]); the
/// per-thread messages read hangs off the `{id}` sub-path, and the per-thread
/// writes are the only mutating verbs under this tree — a `POST` to
/// `…/{id}/delete`, `…/{id}/rename` or `…/{id}/truncate`.
///
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
pub const CHAT_THREADS_ROUTE: &str = "/api/v1/chat/threads";

/// The workspace chat's turn `POST` route ([S-482], [FR-WS-34], [ADR-71]): the
/// workspace roster's turn, served by its own service rather than a member's.
/// [`CHAT_POST_ROUTE`]'s contract unchanged, but answering for the **workspace**:
/// it reads no `?repo=`, takes its `[chat]` from the workspace tier alone, and
/// persists to `<workspace root>/.logos/chat.db`. Under a single-root serve it
/// answers `404` (not a workspace, [ADR-52]).
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
/// [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
/// [ADR-71]: ../../docs/specs/architecture/decisions/ADR-71.md
/// [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
pub const WORKSPACE_CHAT_POST_ROUTE: &str = "/workspace/chat";

/// The workspace chat's conversation-history routes ([S-482], [FR-UI-26]): the
/// [`CHAT_THREADS_ROUTE`] list / messages / per-thread delete, rename and
/// truncate, over `<workspace root>/.logos/chat.db` instead of a member's.
/// `?repo=` is not read; a single-root serve answers `404`.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
pub const WORKSPACE_CHAT_THREADS_ROUTE: &str = "/api/v1/workspace/chat/threads";

/// How often an open chat turn's SSE stream writes its `: keepalive` comment
/// ([FR-UI-50]). The client's silence threshold (45 s, `STALL_THRESHOLD_SECONDS` in
/// `web/ui/src/views/chat/chatModel.ts`) is three of these, so one lost keepalive
/// never trips it. A documented constant, not a setting (`docs/howto/usage.md`).
///
/// [FR-UI-50]: ../../docs/specs/requirements/FR-UI-50.md
const CHAT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// What the chat routes are built over: the serve's default engine and backing
/// (what [`MemberEngine`] and [`WorkspaceRoot`] resolve a request against), the
/// default member's chat service, and the workspace chat of a workspace serve.
#[derive(Clone)]
pub(crate) struct ChatState {
    engine: Arc<Engine>,
    backing: Arc<Backing<Engine>>,
    chat: Arc<dyn ChatService>,
    workspace_chat: Option<Arc<dyn ChatService>>,
}

impl FromRef<ChatState> for Arc<Engine> {
    fn from_ref(state: &ChatState) -> Self {
        Arc::clone(&state.engine)
    }
}

impl FromRef<ChatState> for Arc<Backing<Engine>> {
    fn from_ref(state: &ChatState) -> Self {
        Arc::clone(&state.backing)
    }
}

impl FromRef<ChatState> for Arc<dyn ChatService> {
    fn from_ref(state: &ChatState) -> Self {
        Arc::clone(&state.chat)
    }
}

impl ChatState {
    /// The state over `context`: `member` (or the configured member chat) for the
    /// default member, and — under a workspace backing only — `workspace` (or the
    /// configured workspace chat).
    ///
    /// The configured workspace chat stitches over a contract bridge and
    /// build-dependency cache of its own ([S-482]); a single root mints no query
    /// backing, so it has no workspace chat, injected or not.
    ///
    /// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
    pub(crate) fn new(
        context: &WebContext,
        member: Option<Arc<dyn ChatService>>,
        workspace: Option<Arc<dyn ChatService>>,
    ) -> Self {
        let engine = Arc::clone(&context.engine);
        let backing = Arc::clone(&context.backing);
        let chat = member.unwrap_or_else(|| {
            Arc::new(ConfiguredChatService::new(Arc::clone(&engine), workspace_root_of(&backing)))
        });
        let workspace_chat = backing.as_federated().is_some().then(|| {
            workspace.or_else(|| {
                agent_core::XserviceBacking::federated(
                    Arc::clone(&backing),
                    Arc::new(ContractBridge::new()),
                )
                .map(|xservice| {
                    Arc::new(WorkspaceChatService::new(
                        xservice.with_build_deps(Arc::new(BuildDependencies::new())),
                    )) as Arc<dyn ChatService>
                })
            })
        });
        Self { engine, backing, chat, workspace_chat: workspace_chat.flatten() }
    }

    /// The default member's chat service.
    #[cfg(test)]
    pub(crate) fn chat(&self) -> &Arc<dyn ChatService> {
        &self.chat
    }

    /// The workspace chat, when this serve has one.
    #[cfg(test)]
    pub(crate) fn workspace_chat(&self) -> Option<&Arc<dyn ChatService>> {
        self.workspace_chat.as_ref()
    }
}

/// Every route the extension mounts over `context` ([`crate::ChatExtension`]).
pub(crate) fn routes(
    context: &WebContext,
    member: Option<Arc<dyn ChatService>>,
    workspace: Option<Arc<dyn ChatService>>,
) -> Vec<WebRoute> {
    let state = ChatState::new(context, member, workspace);
    vec![
        // ── The chat turn (S-170, FR-UI-18/19, ADR-40). With `Accept:
        // text/event-stream` it streams the orchestrator's plan / subagent-activity /
        // token events as SSE under the unchanged self-only CSP (no WebSocket);
        // otherwise it renders the buffered answer. A GET is a browser navigation to
        // the SPA's Chat view, so it serves the SPA shell.
        WebRoute::shell(CHAT_POST_ROUTE),
        WebRoute::post(CHAT_POST_ROUTE, chat_turn, state.clone()),
        // ── The conversation-history read/delete API (S-209, FR-UI-26, ADR-47):
        // two GET reads (the thread list and one thread's ordered messages, ADR-28)
        // and the per-thread writes — delete (S-209), rename (S-623) and truncate
        // (S-624) — each a POST to `…/{id}/{verb}` (DELETE is `405` under the method
        // guard, so a mutation rides a POST verb).
        WebRoute::get(CHAT_THREADS_ROUTE, chat_threads, state.clone()),
        WebRoute::get("/api/v1/chat/threads/:id", chat_thread_messages, state.clone()),
        WebRoute::post("/api/v1/chat/threads/:id/delete", chat_thread_delete, state.clone()),
        WebRoute::post("/api/v1/chat/threads/:id/rename", chat_thread_rename, state.clone()),
        WebRoute::post("/api/v1/chat/threads/:id/truncate", chat_thread_truncate, state.clone()),
        // ── The workspace chat (S-482, FR-WS-34, ADR-71): the workspace roster on
        // its own route and store — the `/chat` turn and threads contract
        // unchanged, answering for the workspace (no `?repo=`), configured from the
        // workspace tier alone, persisted to `<workspace root>/.logos/chat.db`. A
        // single-root serve answers all six `404`.
        WebRoute::post(WORKSPACE_CHAT_POST_ROUTE, workspace_chat_turn, state.clone()),
        WebRoute::get(WORKSPACE_CHAT_THREADS_ROUTE, workspace_chat_threads, state.clone()),
        WebRoute::get(
            "/api/v1/workspace/chat/threads/:id",
            workspace_chat_thread_messages,
            state.clone(),
        ),
        WebRoute::post(
            "/api/v1/workspace/chat/threads/:id/delete",
            workspace_chat_thread_delete,
            state.clone(),
        ),
        WebRoute::post(
            "/api/v1/workspace/chat/threads/:id/rename",
            workspace_chat_thread_rename,
            state.clone(),
        ),
        WebRoute::post(
            "/api/v1/workspace/chat/threads/:id/truncate",
            workspace_chat_thread_truncate,
            state,
        ),
    ]
}

/// The chat service for this request's member scope (S-250, [FR-UI-29]).
///
/// The state's service is bound to the **default** engine (and, in the route
/// tests, is a mock provider). That is right for single-root and for an unscoped
/// workspace request. But a `?repo=`-scoped request is reading member B, so
/// answering its chat turn from member A's graph would state one member's answers
/// under another member's name ([NFR-RA-05]) — so bind a service to the resolved
/// engine instead. `Arc::ptr_eq` is the test: same engine ⇒ the state's service
/// (mock seams preserved); a different engine ⇒ a service for that member,
/// resolving its inherited chat halves against the same workspace root
/// ([ADR-67]).
///
/// A scoped member's service is a member chat like any other: single-backing,
/// with no cross-service reach ([S-481]) — that is the workspace chat's.
///
/// [FR-UI-29]: ../../docs/specs/requirements/FR-UI-29.md
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
/// [S-481]: ../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
pub(crate) fn chat_for(
    injected: &Arc<dyn ChatService>,
    default: &Arc<Engine>,
    scoped: Arc<Engine>,
    backing: &Arc<Backing<Engine>>,
) -> Arc<dyn ChatService> {
    if Arc::ptr_eq(default, &scoped) {
        Arc::clone(injected)
    } else {
        Arc::new(ConfiguredChatService::new(scoped, workspace_root_of(backing)))
    }
}

/// The workspace chat service, extracted for its turn route ([S-482]) — or, when
/// this serve has none (a single root), the workspace family's `404`, answered
/// from the request head before the body is read. The state holds the `Option`,
/// the extractor yields the service, and the 404 is decided once.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
struct WorkspaceChat(Arc<dyn ChatService>);

#[axum::async_trait]
impl FromRequestParts<ChatState> for WorkspaceChat {
    type Rejection = Response;

    async fn from_request_parts(_parts: &mut Parts, state: &ChatState) -> Result<Self, Self::Rejection> {
        state.workspace_chat.clone().map(Self).ok_or_else(web::not_a_workspace)
    }
}

/// The thread id of a per-thread write (`…/{id}/{verb}`), or the method guard's
/// own `405` when the segment is not the `i64` rowid the store keys on.
///
/// The router admits a `POST` by the pattern it matched, and `:id` matches any
/// segment. While the web adapter enumerated these writes itself, a non-integer id
/// was refused `405` before any handler ran, so a near-miss path was never an
/// admitted write; this keeps that contract, rather than answering it as a
/// malformed admitted request (`400`).
struct ThreadId(i64);

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for ThreadId {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let refused = || {
            (StatusCode::METHOD_NOT_ALLOWED, "not an enumerated per-thread write: the thread id is not an integer")
                .into_response()
        };
        let UrlPath(raw) = UrlPath::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| refused())?;
        raw.parse().map(Self).map_err(|_| refused())
    }
}

/// The chat turn handler (S-170, [FR-UI-18]/[FR-UI-19], [ADR-40]): forward the
/// turn to the [`ChatService`] and either **stream** the orchestrator's events as
/// Server-Sent Events or render the **buffered** answer, per the client's `Accept`
/// — the surface holds no agent logic.
///
/// - `Accept: text/event-stream` → an SSE response streaming the plan,
///   subagent-activity, and answer events incrementally under the **unchanged**
///   self-only CSP (the web surface's outer CSP layer stamps the streaming
///   response's headers before its body flows); the body owns the turn's abort
///   guard, so a client disconnect cancels the in-flight turn ([FR-UI-19]).
/// - otherwise → the buffered final answer (the non-streaming fallback,
///   [FR-UI-19]), honest on a halt/fault ([NFR-CC-04]).
///
/// [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md
/// [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
/// [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_turn(
    State(chat): State<Arc<dyn ChatService>>,
    State(default): State<Arc<Engine>>,
    State(backing): State<Arc<Backing<Engine>>>,
    MemberEngine(engine): MemberEngine,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    // Answer from the member the user is actually reading (S-250), resolving its
    // chat halves against the workspace root it may inherit them from ([ADR-67]).
    let chat = chat_for(&chat, &default, engine, &backing);
    serve_turn(chat, &headers, &form).await
}

/// The workspace chat turn handler ([S-482], [FR-WS-34]): [`chat_turn`]'s contract
/// over the workspace chat service — the same form, the same SSE / buffered
/// answer per `Accept` ([`serve_turn`]), the same guards — but no member is
/// resolved, so `?repo=` is not read. A single-root serve never reaches here: the
/// [`WorkspaceChat`] extractor answers `404` first.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
/// [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
async fn workspace_chat_turn(
    WorkspaceChat(chat): WorkspaceChat,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    serve_turn(chat, &headers, &form).await
}

/// The `400` for a turn whose `preset` names none of the built-in presets
/// ([FR-UI-49]). It lists the accepted values and never echoes the one sent.
///
/// [FR-UI-49]: ../../docs/specs/requirements/FR-UI-49.md
fn unknown_preset_response() -> Response {
    let accepted: Vec<&str> = Preset::ALL.iter().map(|p| p.wire_name()).collect();
    (
        StatusCode::BAD_REQUEST,
        format!("unknown chat `preset`; expected one of: {}", accepted.join(", ")),
    )
        .into_response()
}

/// Run one chat turn on `chat` from the turn form and answer per `Accept` — the
/// body [`chat_turn`] and [`workspace_chat_turn`] share, so the two routes cannot
/// drift on the form fields, the empty-question and unknown-preset refusals or the
/// SSE / buffered split ([FR-UI-19], [FR-UI-49]).
///
/// [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md
/// [FR-UI-49]: ../../docs/specs/requirements/FR-UI-49.md
async fn serve_turn(
    chat: Arc<dyn ChatService>,
    headers: &HeaderMap,
    form: &HashMap<String, String>,
) -> Response {
    // The turn's prompt-only preset ([FR-UI-49]): an absent field is Default (the
    // no-JS form sends none); any other value must name one of the fixed four
    // exactly, or the turn is refused here — before the service starts it, so no
    // provider is ever called for it.
    let preset = match form.get("preset") {
        None => Preset::Default,
        Some(value) => match Preset::from_wire(value) {
            Some(preset) => preset,
            None => return unknown_preset_response(),
        },
    };
    let question = form
        .get("q")
        .or_else(|| form.get("message"))
        .map(|q| q.trim().to_string())
        .unwrap_or_default();
    if question.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "a chat turn needs a non-empty `q` (the user message)",
        )
            .into_response();
    }
    // An optional existing thread to append to; an unparsable value starts a fresh
    // thread rather than failing the turn.
    let thread_id = form.get("thread").and_then(|t| t.trim().parse::<i64>().ok());

    let stream = chat.start_turn(question, thread_id, preset);

    if wants_event_stream(headers) {
        // Stream the turn. A `: keepalive` comment every 15 s while the turn is open
        // keeps intermediaries from idling the connection between sparse
        // orchestrator events, and is the byte the client's 45 s silence watchdog
        // counts as liveness ([FR-UI-50]). A comment is not an event, so the event
        // contract is unchanged for every consumer.
        //
        // [FR-UI-50]: ../../docs/specs/requirements/FR-UI-50.md
        Sse::new(service::sse_body(stream))
            .keep_alive(
                KeepAlive::new()
                    .interval(CHAT_KEEPALIVE_INTERVAL)
                    .text("keepalive"),
            )
            .into_response()
    } else {
        // Progressive-enhancement fallback: drain the turn server-side and render
        // the complete answer for the same turn (FR-UI-19).
        let answer = stream.into_buffered().await;
        (StatusCode::OK, answer).into_response()
    }
}

/// Flatten an `anyhow` store error into its display chain for the honest `500`
/// body — the failure is shown, never papered over.
fn err_text(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// One conversation's list-row (S-209, [FR-UI-26], [ADR-47]): the exact contract
/// the history rail reads — `id`, `title`, `updated_at`, and nothing else.
/// Deliberately narrower than the store's [`ChatThread`] (it drops `created_at`)
/// so the thread-list payload carries only what the rail renders and no secret
/// ever rides it ([NFR-SE-07]).
///
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
#[derive(serde::Serialize)]
struct ThreadSummary {
    /// The thread rowid (stable for the life of the store).
    id: i64,
    /// The auto-derived conversation title (S-208).
    title: String,
    /// Unix-seconds time of the most recent appended message (or creation) — the
    /// most-recent-first sort key.
    updated_at: i64,
}

/// `GET /api/v1/chat/threads` → the conversation list (S-209, [FR-UI-26],
/// [ADR-47], [ADR-28]): every persisted thread as a [`ThreadSummary`],
/// most-recent-first (the store's `ORDER BY updated_at DESC, id DESC`). A pure
/// loopback, same-origin read carrying no secret ([NFR-SE-07]). Runs the blocking
/// SQLite read on the pool through the [`bridge`] ([ADR-03]) — like every other
/// read handler — so it also flows through the [ADR-13] `surface=web` telemetry
/// chokepoint; a store fault is an honest `500` ([NFR-CC-04]). Member-scoped
/// (S-250): the list is that member's `.logos/chat.db`.
///
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
/// [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_threads(MemberEngine(engine): MemberEngine) -> Response {
    threads_response(bridge(engine, "chat_threads", Surface::Web, move |e| list_threads_at(e.root())).await)
}

/// The thread list of the conversation store under `root` — the read the member
/// and workspace thread lists share.
fn list_threads_at(root: &Path) -> Result<Vec<ChatThread>> {
    ChatStore::open(root)?.list_threads()
}

/// Render a thread-list read as the [`ThreadSummary`] rows the rail reads, or an
/// honest `500` ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
fn threads_response(result: Result<Vec<ChatThread>>) -> Response {
    match result {
        Ok(threads) => {
            let summaries: Vec<ThreadSummary> = threads
                .into_iter()
                .map(|t| ThreadSummary {
                    id: t.id,
                    title: t.title,
                    updated_at: t.updated_at,
                })
                .collect();
            Json(summaries).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// `GET /api/v1/chat/threads/{id}` → one thread's ordered transcript (S-209,
/// [FR-UI-26], [ADR-47], [ADR-28]): every message in stored `ordinal` order, each
/// with its tool traces — the [`ChatMessage`]s the rail hydrates on select. A GET
/// carrying no secret ([NFR-SE-07]); a request for a thread that does not exist is
/// an honest `404` (distinct from an empty-but-real thread), never a misleading
/// empty `200` ([NFR-CC-04]). Runs on the pool through the [`bridge`] ([ADR-03],
/// [ADR-13] telemetry); a store fault is a `500`.
///
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
/// [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_thread_messages(
    MemberEngine(engine): MemberEngine,
    UrlPath(thread_id): UrlPath<i64>,
) -> Response {
    messages_response(
        bridge(engine, "chat_thread_messages", Surface::Web, move |e| {
            thread_messages_at(e.root(), thread_id)
        })
        .await,
    )
}

/// One thread's ordered messages in the store under `root`, or `None` when no
/// thread has that id.
fn thread_messages_at(root: &Path, thread_id: i64) -> Result<Option<Vec<ChatMessage>>> {
    let store = ChatStore::open(root)?;
    // Distinguish "no such thread" (→ 404) from "a real thread with no
    // messages" (→ an honest empty list) — `messages` alone cannot.
    match store.thread(thread_id)? {
        Some(_) => store.messages(thread_id).map(Some),
        None => Ok(None),
    }
}

/// Render a messages read: the transcript, an honest `404` for an unknown thread,
/// or a `500` ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
fn messages_response(result: Result<Option<Vec<ChatMessage>>>) -> Response {
    match result {
        Ok(Some(messages)) => Json(messages).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no chat thread with that id").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// `POST /api/v1/chat/threads/{id}/delete` → per-thread delete (S-209,
/// [FR-UI-26], [FR-UI-20], [ADR-47], [ADR-31]): the granular deletion that
/// supersedes the global Clear-history. Backed by [`ChatStore::delete_thread`]
/// (S-208) — one `DELETE FROM chat_threads WHERE id=?` whose live cascade wipes
/// that thread's messages, scratchpad, and working memory (the S-168/S-175 FK
/// contract), so no orphaned memory survives. Already proven same-origin +
/// intentional by the web surface's intent guard (a forged or intent-less delete
/// never reaches here, [NFR-SE-06]). A hit is `204 No Content`; a miss
/// (`delete_thread == false`, no thread had that id) is an idempotent `404`; a
/// store fault is an honest `500` ([NFR-CC-04]). The blocking SQLite work runs on
/// the pool through the [`bridge`] ([ADR-03], [ADR-13] telemetry).
///
/// [FR-UI-26]: ../../docs/specs/requirements/FR-UI-26.md
/// [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
/// [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_thread_delete(MemberEngine(engine): MemberEngine, ThreadId(thread_id): ThreadId) -> Response {
    thread_write_response(
        bridge(engine, "chat_thread_delete", Surface::Web, move |e| {
            delete_thread_at(e.root(), thread_id)
        })
        .await,
    )
}

/// Delete one thread (and, by the store's cascade, its memory) from the store
/// under `root`; `false` when no thread had that id.
fn delete_thread_at(root: &Path, thread_id: i64) -> Result<bool> {
    ChatStore::open(root)?.delete_thread(thread_id)
}

/// Render a per-thread write (delete, rename, truncate): `204` on a hit, a `404`
/// on a miss (idempotent for a delete), a `500` on a store fault ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
fn thread_write_response(result: Result<bool>) -> Response {
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no chat thread with that id").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// `POST /api/v1/chat/threads/{id}/rename` → rename one conversation (S-623,
/// [FR-UI-48], [ADR-47] as extended by CR-204, [ADR-31]): the form field `title`
/// becomes `chat_threads.title`, through [`ChatStore::rename_thread`]. Shaped
/// like [`chat_thread_delete`]: already proven same-origin + intentional by the
/// intent guard ([NFR-SE-06]), the blocking SQLite work on the pool through the
/// [`bridge`]. A rename is `204`; a missing, blank or over-long `title` is a `400`
/// that touches no store; an unknown id a `404`; a store fault a `500`
/// ([NFR-CC-04]).
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_thread_rename(
    MemberEngine(engine): MemberEngine,
    ThreadId(thread_id): ThreadId,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(title) = rename_title(&form) else {
        return rename_refused();
    };
    thread_write_response(
        bridge(engine, "chat_thread_rename", Surface::Web, move |e| {
            rename_thread_at(e.root(), thread_id, &title)
        })
        .await,
    )
}

/// The new title a rename form carries, shaped for storage
/// ([`normalize_thread_title`](chat_agent::normalize_thread_title)); `None` when
/// the `title` field is missing, blank or over-long.
fn rename_title(form: &HashMap<String, String>) -> Option<String> {
    form.get("title").and_then(|raw| chat_agent::normalize_thread_title(raw))
}

/// The `400` a rename without a usable title gets — before any store is opened,
/// so the title is unchanged ([FR-UI-48]).
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
fn rename_refused() -> Response {
    (
        StatusCode::BAD_REQUEST,
        format!(
            "a rename needs a non-empty `title` of at most {} characters",
            chat_agent::THREAD_TITLE_MAX
        ),
    )
        .into_response()
}

/// Rename one thread in the store under `root`; `false` when no thread had that id.
fn rename_thread_at(root: &Path, thread_id: i64, title: &str) -> Result<bool> {
    ChatStore::open(root)?.rename_thread(thread_id, title)
}

/// `POST /api/v1/chat/threads/{id}/truncate` → cut one conversation back to before
/// a question (S-624, [FR-UI-46], [FR-UI-20], [ADR-47] as extended by CR-204,
/// [ADR-31]): the form field `user_turn` names the question (0-based, counting the
/// thread's stored user messages), and that question, everything after it and the
/// per-thread memory written after the cut are deleted in one transaction, through
/// [`ChatStore::truncate_after`]. Edit-and-resend and Regenerate call it before
/// re-running the turn, so the store keeps one answer per turn. A question past
/// the stored ones leaves nothing to cut (`204`).
///
/// Shaped like [`chat_thread_rename`]: already proven same-origin + intentional by
/// the intent guard ([NFR-SE-06]), the blocking SQLite work on the pool through
/// the [`bridge`]. A truncate is `204`; a missing or non-numeric `user_turn` is a
/// `400` that touches no store; an unknown id a `404`; a store fault a `500`
/// ([NFR-CC-04]).
///
/// [FR-UI-46]: ../../docs/specs/requirements/FR-UI-46.md
/// [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
/// [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
async fn chat_thread_truncate(
    MemberEngine(engine): MemberEngine,
    ThreadId(thread_id): ThreadId,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(user_turn) = truncate_user_turn(&form) else {
        return truncate_refused();
    };
    thread_write_response(
        bridge(engine, "chat_thread_truncate", Surface::Web, move |e| {
            truncate_thread_at(e.root(), thread_id, user_turn)
        })
        .await,
    )
}

/// The question a truncate form names — its `user_turn` field as a non-negative
/// integer; `None` when the field is missing or is not one.
fn truncate_user_turn(form: &HashMap<String, String>) -> Option<u32> {
    form.get("user_turn").and_then(|raw| raw.trim().parse().ok())
}

/// The `400` a truncate without a usable `user_turn` gets — before any store is
/// opened, so the conversation is unchanged ([FR-UI-46]).
///
/// [FR-UI-46]: ../../docs/specs/requirements/FR-UI-46.md
fn truncate_refused() -> Response {
    (
        StatusCode::BAD_REQUEST,
        "a truncate needs a `user_turn`: the 0-based index of the question to cut back to",
    )
        .into_response()
}

/// Cut thread `thread_id` in the store under `root` back to before its
/// `user_turn`-th question; `false` when no thread had that id. A question past the
/// stored ones leaves nothing to cut. Resolving the question and deleting are two
/// statements, which is safe: ordinals never change and an append only adds past
/// the resolved one, so the cut still removes it.
fn truncate_thread_at(root: &Path, thread_id: i64, user_turn: u32) -> Result<bool> {
    let mut store = ChatStore::open(root)?;
    match store.user_turn_ordinal(thread_id, user_turn)? {
        Some(ordinal) => store.truncate_after(thread_id, ordinal - 1),
        None => Ok(store.thread(thread_id)?.is_some()),
    }
}

/// `GET /api/v1/workspace/chat/threads` → the workspace chat's conversation list
/// ([S-482]): [`chat_threads`] over `<workspace root>/.logos/chat.db`. The root
/// holds no graph, so the read crosses on [`run_blocking`] with no engine, as the
/// workspace config routes do; a single-root serve answers `404`.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
async fn workspace_chat_threads(WorkspaceRoot(root): WorkspaceRoot) -> Response {
    threads_response(
        run_blocking("workspace_chat_threads", Surface::Web, move || {
            if workspace_store_exists(&root) {
                list_threads_at(&root)
            } else {
                Ok(Vec::new())
            }
        })
        .await,
    )
}

/// Whether the workspace chat's store exists yet. The workspace threads routes read
/// through this rather than letting [`ChatStore::open`] create one: the workspace
/// root holds no graph, and a read — or a delete of a conversation that cannot
/// exist — must not leave a `chat.db` behind at a root nobody has chatted at (no
/// conversations is the answer, not a fresh store).
fn workspace_store_exists(root: &Path) -> bool {
    chat_agent::db_path(root).exists()
}

/// `GET /api/v1/workspace/chat/threads/{id}` → one workspace conversation's
/// transcript ([S-482]), as [`chat_thread_messages`].
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
async fn workspace_chat_thread_messages(
    WorkspaceRoot(root): WorkspaceRoot,
    UrlPath(thread_id): UrlPath<i64>,
) -> Response {
    messages_response(
        run_blocking("workspace_chat_thread_messages", Surface::Web, move || {
            if workspace_store_exists(&root) {
                thread_messages_at(&root, thread_id)
            } else {
                Ok(None)
            }
        })
        .await,
    )
}

/// `POST /api/v1/workspace/chat/threads/{id}/delete` → delete one workspace
/// conversation ([S-482]), as [`chat_thread_delete`] — intent-guarded like every
/// mutating `POST`.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
async fn workspace_chat_thread_delete(WorkspaceRoot(root): WorkspaceRoot, ThreadId(thread_id): ThreadId) -> Response {
    thread_write_response(
        run_blocking("workspace_chat_thread_delete", Surface::Web, move || {
            Ok(workspace_store_exists(&root) && delete_thread_at(&root, thread_id)?)
        })
        .await,
    )
}

/// `POST /api/v1/workspace/chat/threads/{id}/rename` → rename one workspace
/// conversation ([S-482], S-623), as [`chat_thread_rename`] — intent-guarded like
/// every mutating `POST`. A workspace that has never chatted has no conversation to
/// rename: `404`, and no `chat.db` is created at its root.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
async fn workspace_chat_thread_rename(
    WorkspaceRoot(root): WorkspaceRoot,
    ThreadId(thread_id): ThreadId,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(title) = rename_title(&form) else {
        return rename_refused();
    };
    thread_write_response(
        run_blocking("workspace_chat_thread_rename", Surface::Web, move || {
            Ok(workspace_store_exists(&root) && rename_thread_at(&root, thread_id, &title)?)
        })
        .await,
    )
}

/// `POST /api/v1/workspace/chat/threads/{id}/truncate` → cut one workspace
/// conversation back to before a question ([S-482], S-624), as
/// [`chat_thread_truncate`] — intent-guarded like every mutating `POST`. A
/// workspace that has never chatted has no conversation to cut: `404`, and no
/// `chat.db` is created at its root.
///
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
async fn workspace_chat_thread_truncate(
    WorkspaceRoot(root): WorkspaceRoot,
    ThreadId(thread_id): ThreadId,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(user_turn) = truncate_user_turn(&form) else {
        return truncate_refused();
    };
    thread_write_response(
        run_blocking("workspace_chat_thread_truncate", Surface::Web, move || {
            Ok(workspace_store_exists(&root) && truncate_thread_at(&root, thread_id, user_turn)?)
        })
        .await,
    )
}
