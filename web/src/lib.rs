//! `web` — the third adapter: a thin, feature-gated axum surface serving the
//! localhost dashboard over [`logos_core::Engine`] read-models (CR-012, ADR-01,
//! ADR-27, NFR-MA-02).
//!
//! This crate compiles into the `logos` binary **only under the non-default
//! `ui` cargo feature** (cli/Cargo.toml). Its axum/hyper stack therefore never
//! enters the default-feature dependency tree the no-network fitness function
//! guards (NFR-SE-01) — the offline carve-out (ADR-27).
//!
//! # Carve-out invariants (BR-33, ADR-27, [UAT-UI-02])
//! - **Loopback only.** The listener binds [`BIND_ADDR`] (`127.0.0.1`), a
//!   compile-time constant: no flag, env var, or config key can change it
//!   (a revisable v1 posture, ADR-27). [`bind`] is the single bind site.
//! - **No egress in the listen-only build.** Under `--features ui` alone the
//!   surface only ever *listens and answers* — it never dials, and no network
//!   client crate is in its graph (the only socket is the loopback listener).
//!   The chat / wiki-**generation** egress client (`rig`/`reqwest`) is compiled
//!   in only under the additional `agents` feature (CR-078, ADR-60); even then
//!   egress stays user-initiated and consent-gated to a user-configured endpoint
//!   (ADR-40) — the loopback/CSP/CSRF postures below are unchanged either way.
//! - **GET-only, except the enumerated config-write/apply routes.** Every
//!   non-GET request is answered `405` ([`method_guard`]) before any handler
//!   runs, **except** a `POST` to one of the enumerated [`CONFIG_POST_ROUTES`]
//!   — the only mutating seam (FR-UI-03 as revised, NFR-SE-06, ADR-31). Those
//!   `POST`s must additionally clear the same-origin + per-session intent
//!   (CSRF) guard ([`intent_guard`]) or they are answered `403`.
//! - **DNS-rebinding defense.** A request whose `Host` is not a loopback host
//!   is answered `403` ([`host_guard`]).
//! - **Self-only CSP.** Every response — success or error — carries the
//!   restrictive [`CSP`] header ([`csp_headers`]), browser-enforcing no egress.
//!
//! # Thin-adapter discipline (ADR-01, ADR-03, FR-UI-03)
//! Handlers compose presentation-only DTOs from `Engine` read-models and submit
//! work to the core via the [`bridge`] (a `spawn_blocking` hop, exactly like
//! [`mcp`]'s tool router). tokio stays confined to this surface.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
// The SSE keep-alive interval is used only by the agent handlers (chat turn /
// wiki-generation trigger), so it is `agents`-only — a plain `--features ui`
// build serves the listen-only dashboard and never streams (CR-078, ADR-60).
#[cfg(feature = "agents")]
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    extract::{Form, FromRef, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
// The Server-Sent Events response types back the chat/wiki streaming handlers
// only, so they compile solely under `agents` (CR-078, ADR-60).
#[cfg(feature = "agents")]
use axum::response::sse::{KeepAlive, Sse};
use logos_core::config::{ConfigError, PolicyFile};
use logos_core::model::EdgeKind;
use logos_core::models::navigation::{GraphGranularity, GraphLayer};
use logos_core::Engine;

mod api_v1;
// The chat and wiki-**generation** surfaces are the LLM egress carve-out
// (CR-078, ADR-60): they hold the only edges to chat-agent / wiki-agent /
// agent-core (and, through them, `rig` + `reqwest`), so they compile only under
// `agents`. The wiki-**view** (`mod wiki` below) and every read-model stay in the
// listen-only dashboard, present under `--features ui` alone.
#[cfg(feature = "agents")]
pub mod chat;
pub mod components;
mod markdown;
mod query;
pub mod spa;
mod wiki;
#[cfg(feature = "agents")]
pub mod wikigen;

/// The loopback bind address — a **compile-time constant** (ADR-27). The
/// carve-out boundary: the listener never opens on any other interface.
pub const BIND_ADDR: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// The default web-surface port (FR-UI-01); `--port N` overrides it.
pub const DEFAULT_PORT: u16 = 4983;

/// The self-only Content-Security-Policy stamped on every response (BR-33,
/// FR-UI-02). `default-src 'self'` forbids every external fetch; the remaining
/// directives lock down embedding, form submission, and plugin/object loading
/// so the no-egress posture is browser-enforced, not merely server-promised.
const CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'none'; \
                   frame-ancestors 'none'; object-src 'none'";

// ── Surface orchestration (FR-UI-01) ───────────────────────────────────────

/// Run the requested surface combination in **one process** over **one
/// `Engine`** and **one watcher** (FR-UI-01, ADR-04).
///
/// - `serve --ui` → the web server alone.
/// - `serve --mcp --ui` → both surfaces on one current-thread runtime; the MCP
///   serve loop owns stdout (JSON-RPC only, NFR-RA-01) while the web surface
///   logs to stderr. The first to finish (MCP host disconnect, or a web bind
///   failure) ends the process; the other is dropped.
/// - `serve --mcp` (no `--ui`) → delegates to the MCP loop, same as the
///   default-build path.
///
/// # Errors
/// Fails if the engine cannot start, the runtime cannot build, the loopback
/// port is already taken (an actionable error naming `--port`, NFR-UX-02), or a
/// serve loop fails irrecoverably.
pub fn serve_surfaces(root: &Path, mcp: bool, ui: bool, port: u16) -> Result<()> {
    let engine = Engine::start(root)
        .map(Arc::new)
        .context("starting the Logos engine for the web surface")?;
    // One watcher for the whole process (S-022/FR-SY-04); a spawn failure
    // degrades to watcherless serving (reconcile backstops freshness), and the
    // handle's drop on return orphans nothing (NFR-RA-12).
    let _watcher = engine
        .watch()
        .inspect_err(|e| tracing::warn!(target: "logos::web", "serving without a watcher: {e:#}"))
        .ok();
    // Current-thread runtime: HTTP + MCP I/O only (ADR-03). `enable_all` brings
    // the I/O driver the loopback TcpListener needs; Engine work runs on the
    // blocking pool via the submit-and-await bridge.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building the serve I/O runtime")?;
    runtime.block_on(async move {
        match (mcp, ui) {
            // Both: race the two serve loops on one runtime (NFR-RA-01 — MCP
            // keeps stdout; the web surface is stderr-only).
            (true, true) => tokio::select! {
                r = serve_web(Arc::clone(&engine), port) => r,
                r = mcp::serve_stdio_on(Arc::clone(&engine)) => r,
            },
            (false, true) => serve_web(engine, port).await,
            (true, false) => mcp::serve_stdio_on(engine).await,
            // clap guarantees at least one of --mcp/--ui; defend anyway.
            (false, false) => anyhow::bail!("serve needs --mcp and/or --ui"),
        }
    })
}

/// Bind the loopback listener and serve the router until the process ends.
async fn serve_web(engine: Arc<Engine>, port: u16) -> Result<()> {
    let listener = bind(port)?;
    let addr = listener
        .local_addr()
        .context("reading the web-surface bound address")?;
    // The carve-out invariant, asserted at the one bind site (ADR-27).
    debug_assert!(addr.ip().is_loopback(), "web surface bound a non-loopback address");
    tracing::info!(target: "logos::web", %addr, "web surface listening (loopback only)");
    listener
        .set_nonblocking(true)
        .context("switching the web listener to non-blocking")?;
    let listener = tokio::net::TcpListener::from_std(listener)
        .context("adopting the loopback listener into tokio")?;
    axum::serve(listener, router(engine))
        .await
        .context("the web serve loop failed")
}

/// Bind the loopback listener on [`BIND_ADDR`] at `port` — the **single bind
/// site** (ADR-27). A port conflict becomes an actionable error naming the
/// `--port` remedy (NFR-UX-02).
pub fn bind(port: u16) -> Result<std::net::TcpListener> {
    std::net::TcpListener::bind((BIND_ADDR, port)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!(
                "the web UI port {port} is already in use; choose another with `--port <N>`"
            )
        } else {
            anyhow::Error::new(e).context(format!("binding the web UI on {BIND_ADDR}:{port}"))
        }
    })
}

// ── Router skeleton (FR-UI-03, ADR-31) ──────────────────────────────────────

/// The **enumerated** mutating routes — the *only* paths on which [`method_guard`]
/// admits a `POST` (ADR-31, NFR-SE-06). Every other path/method stays GET-only
/// (`405`). Both bridge to the [`api-facade`]'s mutating config seam, and both
/// are additionally gated by [`intent_guard`] (same-origin + per-session token).
///
/// - `/config/save` → [`Engine::config_write`] (validated atomic write).
/// - `/config/apply` → [`Engine::config_apply`] (explicit reconcile/re-eval).
/// - `/config/secret` → [`Engine::config_write_secret`] (the masked chat-key
///   write to the gitignored `secrets.toml`, S-169, [FR-CF-06], [NFR-SE-07]).
///
/// [`api-facade`]: ../../../docs/specs/architecture/components/api-facade.md
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
pub const CONFIG_POST_ROUTES: &[&str] =
    &["/config/save", "/config/apply", "/config/secret"];

/// The enumerated chat `POST` route (S-170, [FR-UI-19], [NFR-SE-06]): the only
/// **non**-config path on which [`method_guard`] admits a `POST`. It carries a
/// chat turn and streams the orchestrator's events back as Server-Sent Events
/// (text/event-stream) under the unchanged self-only CSP, or — without
/// `Accept: text/event-stream` — renders the buffered answer (the [FR-UI-19]
/// progressive-enhancement fallback). Like the config `POST`s it is gated by
/// [`intent_guard`] (same-origin + per-session token); the streaming request rides
/// the `POST` precisely so it keeps that intent proof, which a `GET` `EventSource`
/// cannot carry ([NFR-SE-06]). The agent logic lives in [`chat-agent`] ([ADR-01]).
///
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [`chat-agent`]: ../../../docs/specs/architecture/components/chat-agent.md
pub const CHAT_POST_ROUTE: &str = "/chat";

/// The enumerated Clear-history `POST` route (S-171, [FR-UI-18], [FR-UI-20]): the
/// Chat view's other mutating action, wiping the conversation **and** its
/// per-thread memory. One `DELETE FROM chat_threads` cascades through messages,
/// scratchpad, and working memory (the S-168/S-175 FK contract), so a single
/// call clears both. Like every mutating `POST` it is admitted by
/// [`method_guard`] and gated by [`intent_guard`] (same-origin + per-session
/// token).
///
/// [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
/// [FR-UI-20]: ../../../docs/specs/requirements/FR-UI-20.md
pub const CHAT_CLEAR_ROUTE: &str = "/chat/clear";

/// The enumerated wiki-generation trigger `POST` route (S-178, [FR-WK-18],
/// [FR-UI-19], [NFR-SE-06]): the Wiki tab posts here on open to launch a
/// background, single-run [`wiki-agent`] generation pass and stream its per-page
/// [`WikiProgress`](wikigen::WikiFrame) back as Server-Sent Events
/// (`text/event-stream`) under the unchanged self-only CSP — or, without
/// `Accept: text/event-stream`, the buffered summary (the [FR-UI-19]
/// progressive-enhancement fallback). Starting a run **mutates** (consent-gated
/// egress + `wiki write`), so — exactly like the chat routes — it is admitted by
/// [`method_guard`] and gated by [`intent_guard`] (same-origin + per-session token);
/// the streaming request rides the `POST` precisely so it keeps that intent proof,
/// which a `GET` `EventSource` cannot carry ([NFR-SE-06]). The generation logic
/// lives in [`wiki-agent`] ([ADR-01], [ADR-42]); the surface holds none.
///
/// [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
/// [`wiki-agent`]: ../../../docs/specs/architecture/components/wiki-agent.md
pub const WIKI_GENERATE_ROUTE: &str = "/wiki/generate";

/// The enumerated deep-`verify` `POST` route (S-206, [FR-UI-25], [FR-GV-19],
/// [ADR-46]): the on-demand graph-consistency check the Config tab (S-207) posts
/// to. It is the one **read-model** action admitted as a `POST` — it rides the
/// mutating-method slot (not a `GET`) precisely so it carries the same-origin +
/// per-session intent-token proof [`intent_guard`] enforces on every `POST`
/// ([NFR-SE-06], [ADR-31]); a `GET` could not. The handler
/// ([`api_v1::verify`](crate::api_v1)) runs the seconds-to-minutes shadow reindex
/// on the blocking pool via the [`bridge`] ([ADR-03]), so the serve loop is never
/// blocked; the live store is read-only and no external origin is dialed
/// ([NFR-SE-01], [ADR-46]). Kept in lock-step with [`method_guard`], which
/// consults it.
///
/// [FR-UI-25]: ../../../docs/specs/requirements/FR-UI-25.md
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
/// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
pub const VERIFY_POST_ROUTE: &str = "/api/v1/verify";

/// The request header carrying the per-session intent (CSRF) token on a mutating
/// `POST` (NFR-SE-06, ADR-31). A **custom** header is deliberate: a cross-origin
/// page cannot set it without a CORS preflight the surface never grants, so it is
/// a second factor beyond the same-origin check — see [`intent_guard`].
pub const INTENT_HEADER: &str = "x-logos-intent";

/// A per-session intent (CSRF) token (NFR-SE-06, ADR-31): 256 bits of OS entropy,
/// hex-encoded. Minted once per [`router`] (i.e. once per `serve` session) and
/// embedded by the Config view (S-099) into each mutating form; every mutating
/// `POST` must echo it in the [`INTENT_HEADER`] or [`intent_guard`] rejects it.
///
/// Cheap to clone (an `Arc<str>`): it lives both in the router state (so the
/// Config view can read it) and in the [`intent_guard`] middleware state.
#[derive(Clone)]
pub struct IntentToken(Arc<str>);

impl IntentToken {
    /// Mint a fresh token from the platform CSPRNG. Panics only if the OS RNG is
    /// unavailable — a process that cannot read entropy cannot safely serve a
    /// mutating surface, so failing loud at startup is correct (NFR-SE-06).
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("OS RNG unavailable — cannot mint an intent token");
        let mut hex = String::with_capacity(64);
        for b in bytes {
            hex.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            hex.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
        }
        IntentToken(hex.into())
    }

    /// The token string the Config view (S-099) embeds into its mutating forms.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant-time equality against a candidate token, so a forged-token
    /// rejection leaks no timing signal about how many bytes matched.
    fn matches(&self, candidate: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), candidate.as_bytes());
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

/// The router state: the shared [`Engine`] plus the per-session [`IntentToken`].
///
/// [`FromRef`] lets every existing read-only handler keep extracting
/// `State<Arc<Engine>>` unchanged, while the mutating-route middleware/handlers
/// reach the token via `State<IntentToken>` — one composite state, no churn to
/// the read-only GET handlers.
#[derive(Clone)]
pub(crate) struct WebState {
    engine: Arc<Engine>,
    intent: IntentToken,
    /// The chat seam (S-170): production resolves the configured provider; the
    /// carve-out tests inject a mock-provider service. Behind an [`Arc`] so the
    /// composite state stays cheap to clone. `agents`-only — the listen-only
    /// dashboard carries no chat surface (CR-078, ADR-60).
    #[cfg(feature = "agents")]
    chat: Arc<dyn chat::ChatService>,
    /// The wiki-generation seam (S-178): production resolves the configured wiki
    /// model and drives [`run_configured`](wiki_agent::run_configured); the
    /// carve-out tests inject a mock-provider service. Behind an [`Arc`] so the
    /// composite state stays cheap to clone. `agents`-only — the listen-only
    /// dashboard serves the wiki-**view** but holds no generation trigger
    /// (CR-078, ADR-60).
    #[cfg(feature = "agents")]
    wiki: Arc<dyn wikigen::WikiRunService>,
    /// The single-run lock **and** connection-independent run registry (S-178,
    /// S-222, [FR-WK-18], [CR-056]): gates the Wiki-tab trigger so a (re-)open while
    /// a pass is in flight starts no second run, and owns the in-flight run's
    /// lifetime in application state so a dropped SSE body no longer aborts it — the
    /// run completes server-side. Cheap to clone (an `Arc<Mutex<…>>`). `agents`-only.
    #[cfg(feature = "agents")]
    wiki_state: wikigen::WikiRunState,
}

impl FromRef<WebState> for Arc<Engine> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.engine)
    }
}

impl FromRef<WebState> for IntentToken {
    fn from_ref(state: &WebState) -> Self {
        state.intent.clone()
    }
}

#[cfg(feature = "agents")]
impl FromRef<WebState> for Arc<dyn chat::ChatService> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.chat)
    }
}

#[cfg(feature = "agents")]
impl FromRef<WebState> for Arc<dyn wikigen::WikiRunService> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.wiki)
    }
}

#[cfg(feature = "agents")]
impl FromRef<WebState> for wikigen::WikiRunState {
    fn from_ref(state: &WebState) -> Self {
        state.wiki_state.clone()
    }
}

/// Build the router with the carve-out middleware stack, minting a fresh
/// per-session [`IntentToken`]. The single production entry point.
pub fn router(engine: Arc<Engine>) -> Router {
    router_with_intent(engine, IntentToken::generate())
}

/// Build the router over an explicit [`IntentToken`] — the seam the carve-out
/// fitness tests drive so they can present the session's valid token (and forge
/// invalid ones) without reaching into server internals.
///
/// Layers apply outermost-last, so the order is: [`csp_headers`] (outermost —
/// stamps **every** response, including the guard rejections) → [`host_guard`]
/// (403 on a non-loopback `Host`) → [`method_guard`] (405 on any non-GET except
/// the enumerated config `POST`s) → [`intent_guard`] (403 on a forged/cross-origin
/// mutating `POST`) → routes.
pub fn router_with_intent(engine: Arc<Engine>, intent: IntentToken) -> Router {
    // Under `agents` the state carries the config-resolved chat + wiki-generation
    // seams; under a plain `--features ui` build it is just the engine + intent
    // token, and the chat/wiki routes are never mounted (CR-078, ADR-60).
    #[cfg(feature = "agents")]
    let state = {
        let chat: Arc<dyn chat::ChatService> =
            Arc::new(chat::ConfiguredChatService::new(Arc::clone(&engine)));
        let wiki: Arc<dyn wikigen::WikiRunService> =
            Arc::new(wikigen::ConfiguredWikiRunService::new(Arc::clone(&engine)));
        WebState {
            engine,
            intent,
            chat,
            wiki,
            wiki_state: wikigen::WikiRunState::new(),
        }
    };
    #[cfg(not(feature = "agents"))]
    let state = WebState { engine, intent };
    build_router(state)
}

/// Build the router over an explicit [`IntentToken`] **and** chat service — the
/// seam the chat carve-out tests drive to inject a mock-provider
/// [`ChatService`](chat::ChatService), proving the SSE route end-to-end (incremental
/// events, clean teardown, zero real egress) without a live provider ([UAT-UI-07]).
/// The wiki-generation seam gets the config-resolved production service.
/// Production uses [`router`]/[`router_with_intent`], which supply both
/// config-resolved services.
///
/// [UAT-UI-07]: ../../../docs/specs/requirements/UAT-UI-07.md
///
/// `agents`-only: the mock-provider chat surface exists solely in the egress build
/// (CR-078, ADR-60).
#[cfg(feature = "agents")]
pub fn router_with_chat(
    engine: Arc<Engine>,
    intent: IntentToken,
    chat: Arc<dyn chat::ChatService>,
) -> Router {
    let wiki: Arc<dyn wikigen::WikiRunService> =
        Arc::new(wikigen::ConfiguredWikiRunService::new(Arc::clone(&engine)));
    build_router(WebState {
        engine,
        intent,
        chat,
        wiki,
        wiki_state: wikigen::WikiRunState::new(),
    })
}

/// Build the router over an explicit [`IntentToken`] **and** wiki-generation service
/// — the seam the S-178 wiki carve-out test drives to inject a mock-provider
/// [`WikiRunService`](wikigen::WikiRunService), proving the trigger/SSE route
/// end-to-end (exactly-one-run under the single-run lock, per-page streaming, clean
/// teardown, zero real egress) without a live provider ([FR-WK-18]). The chat seam
/// gets the config-resolved production service. Production uses
/// [`router`]/[`router_with_intent`].
///
/// `agents`-only: the mock-provider wiki-generation surface exists solely in the
/// egress build (CR-078, ADR-60).
#[cfg(feature = "agents")]
pub fn router_with_wiki(
    engine: Arc<Engine>,
    intent: IntentToken,
    wiki: Arc<dyn wikigen::WikiRunService>,
) -> Router {
    let chat: Arc<dyn chat::ChatService> =
        Arc::new(chat::ConfiguredChatService::new(Arc::clone(&engine)));
    build_router(WebState {
        engine,
        intent,
        chat,
        wiki,
        wiki_state: wikigen::WikiRunState::new(),
    })
}

/// Build the router over a fully-constructed [`WebState`] — the shared skeleton
/// every entry point (production and the single-seam test seams) delegates to.
///
/// The read-only dashboard route table and the enumerated config-write/apply seam
/// are always mounted; the chat and wiki-generation routes are added **only under
/// `agents`** (CR-078, ADR-60). A plain `--features ui` build therefore serves the
/// listen-only dashboard (graph/query/wiki-**view**) with no dialing seam, and
/// [`method_guard`] keeps the (unmounted) chat/wiki paths GET-only.
fn build_router(state: WebState) -> Router {
    // The intent-guard layer needs its own clone of the token; the rest lives in
    // `state`, moved into `.with_state` below.
    let intent = state.intent.clone();
    let router = Router::new()
        // ── The embedded client-side SPA shell (CR-049, FR-UI-22, ADR-43) ─────
        // `/` is the SPA front door: the embedded Vite + React `index.html` with
        // the per-session intent token injected as a `<meta name="logos-intent">`
        // tag (S-185, NFR-SE-06, ADR-31). Every tab is now a client-side route the
        // SPA renders; an unmatched HTML navigation falls back to this same shell
        // (see `.fallback` below) so a refresh on a client route survives. The
        // server-rendered view stack and its legacy `/assets/*` table were removed
        // at the CR-049 decommission (S-192, FR-UI-22): there is one rendering model.
        .route("/", get(spa_shell))
        // ── The same-origin `/api/v1/*` JSON read-model API (FR-UI-21, ADR-43) ──
        // The only data seam now: one read-only handler per view\'s data, each a
        // `Json` serialization of an `Engine` read-model (or a presentation bundle
        // of read-models), composed in `api_v1`. No new core query — thin-adapter
        // discipline (ADR-01). All GET, so the `method_guard`/`host_guard`/
        // `csp_headers` stack already covers them. The legacy `/api/*` (non-v1)
        // graph/impact/query/quadrant twins the server-rendered canvas consumed
        // were removed at the S-192 decommission; the SPA consumes only this suite.
        .route("/api/v1/overview", get(api_v1::overview))
        .route("/api/v1/health", get(api_v1::health))
        .route("/api/v1/architecture", get(api_v1::architecture))
        .route("/api/v1/gaps", get(api_v1::gaps))
        .route("/api/v1/files", get(api_v1::files))
        .route("/api/v1/coverage", get(api_v1::coverage))
        .route("/api/v1/quadrant", get(api_v1::quadrant))
        .route("/api/v1/graph", get(api_v1::graph))
        .route("/api/v1/query", get(api_v1::search_query))
        // Read-only Decisions-panel impact read-model (FR-NV-10, FR-DG-02): the
        // JSON the SPA\'s Decisions panel (S-186) builds client-side from.
        .route("/api/v1/impact", get(api_v1::impact))
        .route("/api/v1/node", get(api_v1::node))
        .route("/api/v1/search", get(api_v1::search))
        .route("/api/v1/wiki", get(api_v1::wiki_index))
        // The tiered wiki menu IA the SPA Wiki tab renders (S-189, [FR-UI-06]);
        // composed from the same `crate::wiki` constants the read-models share.
        .route("/api/v1/wiki/nav", get(api_v1::wiki_nav))
        .route("/api/v1/wiki/search", get(api_v1::wiki_search))
        .route("/api/v1/wiki/page/*slug", get(api_v1::wiki_page))
        // The same-origin, read-only doc-image asset route (S-270, [FR-WK-27],
        // [ADR-58]): presented pages' rewritten `<img src>` values resolve here. It
        // serves image files from the doc roots only, path-sandboxed by
        // canonicalized-prefix containment; GET, so the read-only carve-out stack
        // already covers it, and the assets are same-origin so the self-only CSP is
        // unchanged.
        .route("/api/v1/wiki/asset/*path", get(api_v1::wiki_asset))
        .route("/api/v1/config", get(api_v1::config))
        // The enriched telemetry read-model the Statistics tab consumes (S-234,
        // FR-OB-04/FR-UI-27): a thin `Engine::stats(window)` pass-through, `?window=`
        // scoping the trailing window (default 7). GET, so the read-only carve-out
        // stack already covers it.
        .route("/api/v1/statistics", get(api_v1::statistics))
        // The one intent-guarded read-model POST (S-206, FR-UI-25, ADR-46): the
        // deep graph-consistency check the Config tab (S-207) posts to. It rides
        // the mutating-method slot so it keeps the same-origin + intent-token proof
        // `intent_guard` enforces on every POST; the handler runs the shadow reindex
        // off the serve loop via the `bridge`. Kept in lock-step with
        // `VERIFY_POST_ROUTE`, which `method_guard` consults.
        .route(VERIFY_POST_ROUTE, post(api_v1::verify));

    // ── The chat + wiki-generation surface: the LLM egress carve-out, mounted
    // only under `agents` (CR-078, ADR-60). A plain `--features ui` build omits
    // these routes; `method_guard` then keeps their paths GET-only (a POST is
    // `405`), so the listen-only dashboard exposes no dialing seam.
    #[cfg(feature = "agents")]
    let router = router
        // ── The chat turn (S-170, FR-UI-18/19, ADR-40): the one non-config
        // intent-guarded POST. With `Accept: text/event-stream` it streams the
        // orchestrator's plan / subagent-activity / token events as SSE under the
        // unchanged self-only CSP (no WebSocket); otherwise it renders the buffered
        // answer. Kept in lock-step with `CHAT_POST_ROUTE`, which `method_guard`
        // consults. A GET to `/chat` is a browser navigation to the SPA's Chat
        // client route, so it serves the SPA shell (the POST carries the turn).
        .route(CHAT_POST_ROUTE, get(spa_shell).post(chat_turn))
        // S-171 / FR-UI-20: Clear-history wipes the conversation AND its
        // per-thread memory (the `chat_threads` delete cascades, S-168/S-175).
        // Intent-guarded like every mutating POST; kept in lock-step with
        // `method_guard` below.
        .route(CHAT_CLEAR_ROUTE, post(chat_clear))
        // ── The wiki-generation trigger (S-178, FR-WK-18, FR-UI-19, ADR-42): the
        // Wiki tab POSTs here on open. Under the single-run lock it launches a
        // background wiki-agent pass and, with `Accept: text/event-stream`, streams
        // the per-page WikiProgress as SSE under the unchanged self-only CSP;
        // otherwise it renders the buffered summary (the FR-UI-19 fallback). Kept in
        // lock-step with `WIKI_GENERATE_ROUTE`, which `method_guard` consults. A GET
        // to `/wiki/generate` is a browser navigation, so it serves the SPA shell.
        .route(WIKI_GENERATE_ROUTE, get(spa_shell).post(wiki_generate));

    router
        // ── The only non-GET surface (ADR-31, NFR-SE-06): enumerated, bounded
        // config-write/apply routes that bridge to the mutating façade seam. Kept
        // in lock-step with `CONFIG_POST_ROUTES`, which `method_guard` consults.
        .route("/config/save", post(config_save))
        .route("/config/apply", post(config_apply))
        // S-169 / FR-CF-06: the masked chat-key write to gitignored secrets.toml.
        .route("/config/secret", post(config_save_secret))
        // The SPA history fallback (ADR-43): an unmatched **HTML navigation** GET
        // returns the shell so a client-side route survives a refresh, and a
        // root-level embedded asset (e.g. `/theme-init.js`) resolves from the
        // bundle; any other unmatched GET stays an honest `404`. Non-GET never
        // reaches here (`method_guard` answers `405` before routing).
        .fallback(spa_fallback)
        .with_state(state)
        // Innermost custom layer → runs just before routing, after `method_guard`
        // has already filtered to GET + the enumerated config/chat POSTs.
        .layer(from_fn_with_state(intent, intent_guard))
        .layer(from_fn(method_guard))
        .layer(from_fn(host_guard))
        .layer(from_fn(csp_headers))
}

/// The chat turn handler (S-170, [FR-UI-18]/[FR-UI-19], [ADR-40]): forward the
/// turn to the [`ChatService`](chat::ChatService) and either **stream** the
/// orchestrator's events as Server-Sent Events or render the **buffered** answer,
/// per the client's `Accept` — the surface holds no agent logic ([ADR-01]).
///
/// - `Accept: text/event-stream` → an SSE response streaming the plan,
///   subagent-activity, and answer events incrementally under the **unchanged**
///   self-only CSP (the outer [`csp_headers`] layer stamps the streaming
///   response's headers before its body flows); the body owns the turn's abort
///   guard, so a client disconnect cancels the in-flight turn ([FR-UI-19]).
/// - otherwise → the buffered final answer (the non-streaming fallback,
///   [FR-UI-19]), honest on a halt/fault
///   ([NFR-CC-04]).
///
/// The route is already same-origin + intent-token gated by [`intent_guard`]
/// ([NFR-SE-06]) — the streaming request rides this `POST` so it keeps that proof.
///
/// [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[cfg(feature = "agents")]
async fn chat_turn(
    State(chat): State<Arc<dyn chat::ChatService>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
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

    let stream = chat.start_turn(question, thread_id);

    if wants_event_stream(&headers) {
        // Stream the turn. KeepAlive comments keep intermediaries from idling the
        // connection between sparse orchestrator events.
        Sse::new(chat::sse_body(stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response()
    } else {
        // Progressive-enhancement fallback: drain the turn server-side and render
        // the complete answer for the same turn (FR-UI-19).
        let answer = stream.into_buffered().await;
        (StatusCode::OK, answer).into_response()
    }
}

/// The wiki-generation trigger handler (S-178, [FR-WK-18], [FR-UI-19], [ADR-42]):
/// gate the Wiki-tab open through the single-run lock, then start a background
/// [`wiki-agent`](wiki_agent) pass on the [`WikiRunService`](wikigen::WikiRunService)
/// and either **stream** its per-page [`WikiProgress`](wikigen::WikiFrame) as
/// Server-Sent Events or render the **buffered** summary, per the client's `Accept`
/// — the surface holds no generation logic ([ADR-01]).
///
/// - lock **acquired** + `Accept: text/event-stream` → an SSE response streaming the
///   run/page-lifecycle events incrementally under the **unchanged** self-only CSP
///   (the outer [`csp_headers`] layer stamps the streaming response's headers before
///   its body flows); the body is only a **subscriber** to the run's broadcast, so a
///   client disconnect does **not** abort the run — it completes server-side, owned
///   by [`WikiRunState`] ([CR-056], [S-222], [FR-UI-19]);
/// - run **already in flight** + `Accept: text/event-stream` → the Wiki tab has
///   **reopened** mid-run: **re-attach** ([`WikiRunState::subscribe`]) to the SAME
///   run's progress rather than starting a second one or reporting `busy` — the
///   reattached stream replays the run's cumulative history first, then continues
///   live, so the reopened tab's "N of M" is exact from its first render, never a
///   fresh "page 1 of N" ([S-223], [FR-WK-18] as amended by [CR-056], [FR-UI-19]);
/// - run already in flight, **no** SSE opt-in → the buffered no-JS fallback cannot
///   usefully wait out an unknown-length reattach, so it keeps reporting the
///   in-flight run honestly as `busy` rather than blocking the response until the
///   run drains;
/// - lock acquired, no SSE opt-in → the buffered summary (the non-streaming
///   fallback, [FR-UI-19]), honest on a configure-first / halt / fault
///   ([NFR-CC-04]).
///
/// The route is already same-origin + intent-token gated by [`intent_guard`]
/// ([NFR-SE-06]) — the streaming request rides this `POST` so it keeps that proof.
/// The Wiki tab sends no body; a work-list check + configure-first are decided by
/// the runner ([`run_configured`](wiki_agent::run_configured)), not here.
///
/// [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
/// [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
/// [CR-056]: ../../../docs/requests/CR-056-wiki-generation-usability.md
/// [S-222]: ../../../docs/planning/journal.md#s-222-connection-resilient-auto-continuing-background-generation-run
/// [S-223]: ../../../docs/planning/journal.md#s-223-wiki-tab-re-attach-to-the-in-flight-run-and-cumulative-progress
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [`wiki-agent`]: ../../../docs/specs/architecture/components/wiki-agent.md
#[cfg(feature = "agents")]
async fn wiki_generate(
    State(wiki): State<Arc<dyn wikigen::WikiRunService>>,
    State(run_state): State<wikigen::WikiRunState>,
    headers: HeaderMap,
) -> Response {
    let streaming = wants_event_stream(&headers);
    // The single-run lock ([FR-WK-18]): begin → own the one connection-independent
    // background run. The run's lifetime is owned by `run_state`, not this response
    // body ([CR-056], [S-222]).
    //
    // Already in flight: a **streaming** reopen re-attaches to the SAME run instead
    // of starting a second one ([S-223]) — this is what the Wiki tab actually sends
    // (it always requests `text/event-stream`, [FR-UI-19]). The **buffered** no-JS
    // fallback keeps the simpler honest `busy` report: it cannot progressively render
    // a reattach, and blocking the whole response on an unknown-length live run would
    // be a worse fallback than today's instant honest answer.
    let stream = match run_state.begin() {
        Some((guard, sink, stream)) => {
            wiki.start_run(guard, sink);
            stream
        }
        None if streaming => run_state.subscribe().unwrap_or_else(wikigen::WikiRunStream::busy),
        None => wikigen::WikiRunStream::busy(),
    };

    if streaming {
        // Stream the run. KeepAlive comments keep intermediaries from idling the
        // connection between sparse per-page events.
        Sse::new(wikigen::sse_body(stream))
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response()
    } else {
        // Progressive-enhancement fallback: drain the run server-side and render
        // the honest summary (FR-UI-19).
        let summary = stream.into_buffered().await;
        (StatusCode::OK, summary).into_response()
    }
}

/// Does the client accept Server-Sent Events? True iff the `Accept` header names
/// `text/event-stream` — the SPA's streaming client sets it via `fetch`; absent
/// it, a non-streaming buffered turn is rendered (the defensive fallback for a
/// client that does not request SSE, [FR-UI-19]).
///
/// [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
#[cfg(feature = "agents")]
fn wants_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| {
            accept
                .split(',')
                .any(|media| media.trim().starts_with("text/event-stream"))
        })
}

/// Stamp the self-only CSP on every response (BR-33, FR-UI-02). Outermost layer
/// so even the `403`/`405`/`404` rejections carry it.
async fn csp_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    resp
}

/// DNS-rebinding defense (FR-UI-01): reject any request whose `Host` header is
/// not a loopback host with `403`. A missing `Host` is allowed — the listener
/// is already loopback-bound, and absence is not a rebinding vector.
async fn host_guard(req: Request, next: Next) -> Response {
    if let Some(host) = req.headers().get(header::HOST) {
        let ok = host.to_str().map(is_loopback_host).unwrap_or(false);
        if !ok {
            return (StatusCode::FORBIDDEN, "loopback host only").into_response();
        }
    }
    next.run(req).await
}

/// Method guard (FR-UI-03 as revised, ADR-31, S-170/S-171/S-206): the surface is
/// GET-only **except** a `POST` to one of the enumerated [`CONFIG_POST_ROUTES`],
/// the [`CHAT_POST_ROUTE`] (a turn), the [`CHAT_CLEAR_ROUTE`] (Clear-history), the
/// [`WIKI_GENERATE_ROUTE`] (the Wiki-tab generation trigger), or the
/// [`VERIFY_POST_ROUTE`] (the deep graph-consistency check). Every other
/// method — and a `POST` to any other path — is answered `405` before any handler
/// runs, so relaxing the read-only posture stays bounded to exactly the
/// config-write/apply seam, the two chat routes, the wiki-generation trigger, and
/// the verify route (NFR-SE-06).
/// The admitted `POST`s are gated again by [`intent_guard`] for same-origin +
/// intent-token defense.
async fn method_guard(req: Request, next: Next) -> Response {
    let method = req.method();
    let path = req.uri().path();
    let allowed = method == Method::GET || (method == Method::POST && post_route_admitted(path));
    if !allowed {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            "the Logos web UI is read-only except the enumerated config-write/apply, chat, wiki-generation, and verify routes",
        )
            .into_response();
    }
    next.run(req).await
}

/// Is a `POST` to `path` admitted past [`method_guard`] (every other method/path
/// is `405`)? The config-write/apply seam ([`CONFIG_POST_ROUTES`]) and the
/// deep-`verify` route ([`VERIFY_POST_ROUTE`]) are always admitted; the chat and
/// wiki-generation routes are admitted **only under `agents`** (CR-078, ADR-60).
/// In a listen-only `--features ui` build those routes are never mounted, so a
/// `POST` to them stays GET-only (`405`) rather than reaching a handler.
fn post_route_admitted(path: &str) -> bool {
    if CONFIG_POST_ROUTES.contains(&path) || path == VERIFY_POST_ROUTE {
        return true;
    }
    #[cfg(feature = "agents")]
    if path == CHAT_POST_ROUTE || path == CHAT_CLEAR_ROUTE || path == WIKI_GENERATE_ROUTE {
        return true;
    }
    false
}

/// Same-origin + per-session intent (CSRF) guard for the mutating surface
/// (NFR-SE-06, ADR-31). Runs after [`method_guard`], so the only `POST`s it sees
/// are already bounded to the enumerated config + chat `POST` routes; GET requests pass straight
/// through (the read views carry no intent token). A mutating `POST` is admitted
/// only when **both** hold, else it is rejected `403` with no handler run (no
/// write, no pipeline):
///
/// 1. **Same-origin.** The `Origin` header is present and its authority equals
///    the request's (already loopback-validated) `Host`. A cross-origin page's
///    browser-set `Origin` is its own, so a forged write is rejected; a missing
///    `Origin` on a mutating request is rejected too (modern browsers always send
///    it on `POST`).
/// 2. **Intent token.** The [`INTENT_HEADER`] carries the exact per-session token
///    (constant-time compared). A cross-origin page cannot read the token (the
///    self-only CSP / same-origin policy forbid reading the page) nor set the
///    custom header without a CORS preflight the surface never grants.
async fn intent_guard(State(intent): State<IntentToken>, req: Request, next: Next) -> Response {
    if req.method() == Method::POST {
        if !is_same_origin(req.headers()) {
            return (StatusCode::FORBIDDEN, "cross-origin write rejected").into_response();
        }
        let token_ok = req
            .headers()
            .get(INTENT_HEADER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|candidate| intent.matches(candidate));
        if !token_ok {
            return (StatusCode::FORBIDDEN, "missing or invalid intent token").into_response();
        }
    }
    next.run(req).await
}

/// Is the request same-origin? True iff an `Origin` header is present and its
/// authority (scheme stripped) equals the `Host` header. `Host` is already
/// loopback-validated by [`host_guard`], so this binds the write to the same
/// loopback origin the dashboard is served from.
fn is_same_origin(headers: &header::HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    match (host, origin) {
        // An `Origin` is `scheme://authority` with no path; compare the authority
        // to the `Host` value. The compare is ASCII-case-insensitive to match
        // `host_guard`'s `localhost` handling (hostnames are case-insensitive; IP
        // literals are case-invariant), so the two guards enforce one consistent
        // notion of the loopback origin. Equality ⇒ same origin.
        (Some(host), Some(origin)) => origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host)),
        _ => false,
    }
}

/// Is `value` (a raw `Host` header, possibly `host:port` or `[ipv6]:port`) a
/// loopback host? Accepts `localhost` and any loopback IP literal.
fn is_loopback_host(value: &str) -> bool {
    let host = strip_port(value).trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Strip a trailing `:port` from a `Host` value, handling bracketed IPv6
/// (`[::1]:4983` → `::1`) without mangling a bare IPv6 literal.
fn strip_port(value: &str) -> &str {
    if let Some(rest) = value.strip_prefix('[') {
        // `[ipv6]:port` → the bytes inside the brackets.
        return rest.split(']').next().unwrap_or(rest);
    }
    match value.rsplit_once(':') {
        // host:port — only when the suffix is numeric and the host is not an
        // unbracketed IPv6 literal (which itself contains ':').
        Some((host, port)) if !host.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            host
        }
        _ => value,
    }
}

/// Parse the canvas's `?layers=` re-budgeting filter (S-122, FR-UI-15): a
/// comma-separated list of layer wire tokens (`code`/`doc`/`artifact`). `None`
/// when the param is absent (no filter); `Some(vec)` — possibly empty — when it is
/// present, dropping any unrecognised token so a malformed request degrades to a
/// looser filter rather than a 4xx. A present-but-empty list filters every layer
/// out (the honest empty graph a user who deselected all layers expects).
pub(crate) fn parse_layers(q: &HashMap<String, String>) -> Option<Vec<GraphLayer>> {
    q.get("layers")
        .map(|raw| raw.split(',').filter_map(|t| GraphLayer::from_wire(t.trim())).collect())
}

/// Parse the canvas's `?edge_types=` re-budgeting filter (S-122, FR-UI-15): a
/// comma-separated list of edge-kind wire tokens (`calls`/`imports`/…). Same
/// contract as [`parse_layers`] — absent ⇒ no filter, present ⇒ the recognised
/// subset (unknown tokens dropped), empty ⇒ filter every edge out.
pub(crate) fn parse_edge_types(q: &HashMap<String, String>) -> Option<Vec<EdgeKind>> {
    q.get("edge_types")
        .map(|raw| raw.split(',').filter_map(|t| EdgeKind::from_wire(t.trim())).collect())
}

/// Parse the canvas's `?granularity=` semantic cluster-zoom tier (S-124,
/// FR-UI-15, ADR-36): a single `module`/`file`/`symbol` token selecting the
/// existing module-rollup / file-rollup / visualization hydration view (ADR-34).
/// `None` when absent or unrecognised — the accessor then defaults to the symbol
/// tier (the pre-S-124 snapshot), so a malformed token degrades to the default
/// rather than a 4xx, mirroring [`parse_layers`].
pub(crate) fn parse_granularity(q: &HashMap<String, String>) -> Option<GraphGranularity> {
    q.get("granularity").and_then(|raw| GraphGranularity::from_wire(raw.trim()))
}

/// Parse the canvas's `?intent=` documentation-intent overlay toggle (S-128/S-129,
/// FR-UI-16, ADR-37): the off-by-default "Intent / governing docs" control. Off
/// unless the param is explicitly truthy (`1`/`true`/`on`/`yes`, case-insensitive),
/// so an absent, empty, or unrecognised value keeps the byte-identical structural
/// snapshot rather than erroring — the same degrade-don't-4xx contract as the
/// layer/edge/tier filters. When on, the accessor reserves a separate bounded
/// budget for the governing-doc nodes adjacent to the kept code (ADR-37).
pub(crate) fn parse_intent(q: &HashMap<String, String>) -> bool {
    // The `?intent=` toggle is exactly the shared truthy-token contract — one
    // predicate, defined once in [`api_v1::truthy`], so the token set can't drift.
    crate::api_v1::truthy(q.get("intent"))
}

/// Flatten an `anyhow` governance error into its display chain for an error
/// panel — the failure is shown, never papered over (web-surface failure mode).
fn err_text(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// GET to an unknown, non-navigation path → `404` (a non-GET would already be
/// `405`). HTML navigations are caught earlier by [`spa_fallback`].
async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

// ── The embedded SPA shell (CR-049, FR-UI-22, ADR-43, S-185) ──────────────────

/// Serve the embedded client-side SPA shell at `/` with the per-session intent
/// token injected as a `<meta name="logos-intent">` tag ([`spa::served_shell`],
/// NFR-SE-06, ADR-31). The outer [`csp_headers`] layer stamps the **unchanged**
/// self-only CSP, so the shell loads under the byte-identical policy; the SPA
/// reads the token once and echoes it in [`INTENT_HEADER`] on mutating requests,
/// which [`intent_guard`] then validates exactly as it does the legacy forms.
///
/// A pure static read — it touches no `Engine` store, so a shell load mutates
/// nothing (ADR-28). The masked chat key is never on this surface (NFR-SE-07): the
/// token is the only secret-adjacent value, and it is the CSRF token by design.
async fn spa_shell(State(intent): State<IntentToken>) -> Response {
    render_shell(&intent)
}

/// Render the intent-injected shell document, or an honest `500` if the bundle is
/// somehow not embedded (a committed placeholder guarantees it is). Shared by the
/// `/` route and the history fallback.
fn render_shell(intent: &IntentToken) -> Response {
    // Consistency guard (CR-049 white-page trap): a binary built without a matching
    // `npm run build` embeds a shell that references hashed `/assets/*` it does not
    // carry — the browser then loads the shell `200` but `404`s on the JS bundle,
    // leaving a silent blank page. Detect that here and serve a loud, self-describing
    // diagnostic instead. Empty on a consistent bundle, so the happy path is unchanged.
    let missing = spa::missing_shell_assets();
    if !missing.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(spa::inconsistent_bundle_page(&missing)),
        )
            .into_response();
    }
    match spa::served_shell(intent.as_str()) {
        Some(html) => Html(html).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the SPA shell bundle is not embedded",
        )
            .into_response(),
    }
}

/// The SPA history fallback (ADR-43 incremental migration): an unmatched GET that
/// is a **browser navigation** (`Accept` names `text/html`) returns the shell, so
/// a refresh on a client-side route resolves to the SPA rather than a `404`; any
/// other unmatched GET (an asset/XHR miss, no HTML in `Accept`) stays an honest
/// `404` ([`not_found`]). A non-GET never reaches the fallback — [`method_guard`]
/// answers it `405` before routing. Since the S-192 decommission there are no
/// server-rendered routes — every tab is an SPA client route, so all such HTML
/// navigations resolve to the shell here.
///
/// Before the HTML-navigation heuristic it resolves a **root-level** embedded SPA
/// asset: a Vite build copies `web/ui/public/*` (e.g. the no-flash `theme-init.js`,
/// S-193/FR-UI-23) to the bundle root — not under `/assets/` — and the served
/// shell references them by absolute path (`<script src="/theme-init.js">`). Those
/// requests are classic-script/icon fetches (`Accept: */*`), so without this branch
/// they would fall straight through to a `404`, silently disabling the no-flash
/// theme bootstrap in a real release build. `index.html` is excluded — it is the
/// shell, served via `/` (and the `wants_html` branch) so the intent token is
/// always injected. Served byte-verbatim from the binary, so the offline/zero-egress
/// posture (NFR-SE-01) and the byte-identical self-only CSP (NFR-SE-06, stamped by
/// the outer [`csp_headers`] layer) are unaffected.
async fn spa_fallback(State(intent): State<IntentToken>, uri: Uri, headers: HeaderMap) -> Response {
    let tail = uri.path().trim_start_matches('/');
    if !tail.is_empty() && tail != "index.html" {
        if let Some((mime, bytes)) = spa::asset(tail) {
            return (
                [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-cache")],
                bytes.into_owned(),
            )
                .into_response();
        }
    }
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|accept| accept.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    if wants_html {
        render_shell(&intent)
    } else {
        not_found().await
    }
}

// ── Mutating handlers (ADR-31, NFR-SE-06) ────────────────────────────────────
// The only non-GET handlers on the surface. Reached only after `method_guard`
// (POST bounded to the enumerated routes) and `intent_guard` (same-origin +
// per-session token) have both passed, so by the time a handler runs the write
// is already proven same-origin and intentional. Each bridges to the mutating
// façade seam over the `spawn_blocking` `bridge`, exactly like the read handlers
// — the engine owns validation, atomicity, and the apply pipeline (S-096/S-097).
//
// CSP constraint for consumers (S-099/S-100): the self-only `CSP` carries
// `form-action 'none'`, so a **native** `<form method="post">` submission to
// these routes is silently blocked by the browser before the request is sent.
// The SPA POSTs via `fetch` (XHR-class requests are unaffected by `form-action`),
// setting the `x-logos-intent` header from the session token. The bodies below
// are read as urlencoded form data.

/// Map the `file` form field to its [`PolicyFile`]; `None` for anything else.
fn parse_policy_file(raw: Option<&String>) -> Option<PolicyFile> {
    match raw.map(String::as_str) {
        Some("config") => Some(PolicyFile::Config),
        Some("rules") => Some(PolicyFile::Rules),
        _ => None,
    }
}

/// `POST /config/save` → [`Engine::config_write`] (FR-UI-12, BR-35): validate the
/// edited candidate against the load path and, only if valid, replace the file
/// atomically. Form fields: `file=config|rules`, `content=<toml>`. A rejected
/// candidate is a validation fault (`422` with the typed message), and the engine
/// leaves the file byte-identical — no partial write (S-096, NFR-RA-07). Save
/// runs **no** pipeline; applying is the separate [`config_apply`] step.
async fn config_save(
    State(engine): State<Arc<Engine>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(file) = parse_policy_file(form.get("file")) else {
        return (StatusCode::BAD_REQUEST, "unknown policy file (expected file=config|rules)")
            .into_response();
    };
    let content = form.get("content").cloned().unwrap_or_default();
    let result = bridge(engine, "config_save", move |e| e.config_write(file, &content)).await;
    match result {
        Ok(outcome) => Json(outcome).into_response(),
        // Honest-error translation at the façade boundary (ADR-14): a validation
        // fault (bad TOML / unknown key / glob / range) is the client's edit → 422
        // with the file left byte-identical (S-096, NFR-RA-07); an I/O fault (the
        // atomic write or a read failing) is a server-side fault → 500, mirroring
        // `config_apply`. Distinguishing them keeps the S-100 consumer from
        // reading a disk failure as "your edit was invalid".
        Err(e) => (config_write_status(&e), err_text(e)).into_response(),
    }
}

/// Map a `config_write` error to its HTTP status: an I/O fault
/// ([`ConfigError::Io`]/[`ConfigError::Write`]) is a server-side `500`; every
/// validation fault (and any non-[`ConfigError`]) is a client-side `422`.
fn config_write_status(e: &anyhow::Error) -> StatusCode {
    match e.downcast_ref::<ConfigError>() {
        Some(ConfigError::Io { .. } | ConfigError::Write { .. }) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

/// `POST /config/apply` → [`Engine::config_apply`] (FR-UI-13): the explicit Apply
/// — reconcile/index for `config.toml` or a governance re-eval for `rules.toml`.
/// Form field: `file=config|rules`. Runs the blocking engine job on the pool via
/// [`bridge`] (ADR-03), never the surface thread. A structural failure surfaces
/// as honest `500` error text, never a blank or stale figure; the async progress
/// panel is the S-100 consumer's concern.
async fn config_apply(
    State(engine): State<Arc<Engine>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(file) = parse_policy_file(form.get("file")) else {
        return (StatusCode::BAD_REQUEST, "unknown policy file (expected file=config|rules)")
            .into_response();
    };
    let result = bridge(engine, "config_apply", move |e| e.config_apply(file)).await;
    match result {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// `POST /config/secret` → [`Engine::config_write_secret`] (S-169, FR-CF-06,
/// NFR-SE-07): write (or clear) the chat API key into the gitignored
/// `secrets.toml`. Form field: `api_key=<raw>` (blank clears the key).
///
/// The key is **write-only**: the response is the masked outcome (presence +
/// last-4) — the raw key is never echoed in the body, and it never touches the
/// checked-in `config.toml` (it goes to `secrets.toml`). Error mapping mirrors
/// [`config_save`]: an invalid existing store is a `422`, an I/O fault a `500`.
async fn config_save_secret(
    State(engine): State<Arc<Engine>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let api_key = form.get("api_key").cloned().unwrap_or_default();
    let result = bridge(engine, "config_save_secret", move |e| {
        e.config_write_secret(&api_key)
    })
    .await;
    match result {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => (config_write_status(&e), err_text(e)).into_response(),
    }
}

/// `POST /chat/clear` → Clear-history (S-171, [FR-UI-18], [FR-UI-20]): wipe every
/// conversation **and** its per-thread memory. A single
/// [`ChatStore::clear_history`](chat_agent::ChatStore::clear_history)
/// (`DELETE FROM chat_threads`) cascades through messages, scratchpad, and
/// working memory (the S-168/S-175 FK contract), so no orphaned memory survives.
/// Already proven same-origin + intentional by the guards. The blocking SQLite
/// work runs on the pool ([ADR-03]); a store fault is an honest `500`, never a
/// silent success ([NFR-CC-04]). Returns the number of threads removed.
#[cfg(feature = "agents")]
async fn chat_clear(State(engine): State<Arc<Engine>>) -> Response {
    let root = engine.root().to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let mut store = chat_agent::ChatStore::open(&root)?;
        store.clear_history()
    })
    .await;
    match result {
        Ok(Ok(removed)) => Json(removed).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
        Err(_join) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the clear-history task failed unexpectedly",
        )
            .into_response(),
    }
}

/// The ADR-03 submit-and-await bridge: run one blocking `Engine` call on the
/// blocking pool (tokio never enters logos-core) and emit the per-render
/// telemetry event (surface=web) through the tracing chokepoint (ADR-13).
pub(crate) async fn bridge<T, F>(engine: Arc<Engine>, view: &'static str, call: F) -> T
where
    F: FnOnce(&Engine) -> T + Send + 'static,
    T: Send + 'static,
{
    let started = Instant::now();
    let out = tokio::task::spawn_blocking(move || call(&engine))
        .await
        // The Engine read-models are infallible at the surface (ADR-14); a
        // panic crossing the pool is a core bug — re-raise rather than mask it.
        .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()));
    tracing::info!(
        target: "logos::web",
        surface = "web",
        view,
        duration_ms = started.elapsed().as_millis() as u64,
        "page render",
    );
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_addr_is_the_loopback_compile_time_constant() {
        assert!(BIND_ADDR.is_loopback());
        assert_eq!(BIND_ADDR, Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(DEFAULT_PORT, 4983);
    }

    #[test]
    fn loopback_hosts_accepted_external_hosts_rejected() {
        for ok in [
            "localhost",
            "localhost:4983",
            "127.0.0.1",
            "127.0.0.1:4983",
            "[::1]",
            "[::1]:4983",
            "::1",
        ] {
            assert!(is_loopback_host(ok), "{ok} should be a loopback host");
        }
        for bad in [
            "evil.example.com",
            "evil.example.com:4983",
            "10.0.0.5",
            "169.254.1.1:80",
            "",
        ] {
            assert!(!is_loopback_host(bad), "{bad} must be rejected");
        }
    }
}
