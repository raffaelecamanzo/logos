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
use logos_core::federation::{discover, Backing, BuildDependencies, ContractBridge, EngineRegistry};
use logos_core::model::EdgeKind;
use logos_core::models::navigation::{GraphGranularity, GraphLayer};
use logos_core::observability::{in_surface, Surface};
use logos_core::Engine;

use crate::member::MemberEngine;
#[cfg(feature = "agents")]
use crate::member::workspace_root_of;

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
// The per-request member scope (S-250, FR-UI-29): the `?repo=` → `Engine`
// extractor every `/api/v1/*` handler resolves its engine through, so the
// workspace SPA's member selector scopes every existing view. Inert (and
// byte-for-byte unchanged) on the single-root path.
mod member;
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

/// Run the requested surface combination in **one process** (FR-UI-01, ADR-04),
/// context-aware over a discovered workspace ([FR-WS-06], [ADR-52]).
///
/// At startup `serve` runs workspace **discovery** ([`discover`]): with **no**
/// manifest up-tree — or with `--standalone` — it serves single-root over one
/// [`Engine`] and one watcher, **byte-for-byte** as today ([`Backing::Single`]);
/// with a manifest it serves workspace mode over the member [`EngineRegistry`]
/// ([`Backing::Federated`]), warming only the **default member** eagerly and
/// leaving the rest lazy ([`EngineRegistry::new_serve_default`], [NFR-PE-10]).
///
/// - `serve --ui` → the web server alone.
/// - `serve --mcp --ui` → both surfaces on one current-thread runtime; the MCP
///   serve loop owns stdout (JSON-RPC only, NFR-RA-01) while the web surface
///   logs to stderr. The first to finish (MCP host disconnect, or a web bind
///   failure) ends the process; the other is dropped. The MCP loop runs against
///   the single/default engine (its federation wiring is a later story).
/// - `serve --mcp` (no `--ui`) → delegates to the MCP loop, same as the
///   default-build path.
///
/// # Errors
/// Fails if discovery hits a malformed manifest, the engine cannot start, the
/// runtime cannot build, the loopback port is already taken (an actionable error
/// naming `--port`, NFR-UX-02), or a serve loop fails irrecoverably.
///
/// [FR-WS-06]: ../../../docs/specs/requirements/FR-WS-06.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn serve_surfaces(root: &Path, mcp: bool, ui: bool, port: u16, standalone: bool) -> Result<()> {
    let backing = Arc::new(resolve_serve_backing(root, standalone)?);
    // One watcher for the whole process **only** on the single-root path
    // (S-022/FR-SY-04); a spawn failure degrades to watcherless serving (reconcile
    // backstops freshness), and the handle's drop on return orphans nothing
    // (NFR-RA-12). Under the federated backing the registry owns member watchers,
    // so there is no separate process watcher to hold here — but it watches the
    // **resident** set, not all N: a member evicted to stay inside the connection
    // budget loses its watcher until its next touch (S-324, NFR-PE-11, ADR-63).
    // The default member below is exempt in practice, because holding its `Arc`
    // for the process lifetime makes it ineligible for eviction.
    let _watcher: Option<logos_core::watch::WatchHandle> = backing.as_single().and_then(|engine| {
        engine
            .watch()
            .inspect_err(|e| tracing::warn!(target: "logos::web", "serving without a watcher: {e:#}"))
            .ok()
    });
    // The engine the shared single-root surfaces (`/api/v1/*`, and the MCP loop)
    // run against: the one engine under `Single`, or the workspace's default
    // member. Resolving it once fails loud if a federated default cannot start.
    let engine = backing
        .default_engine()
        .context("resolving the default engine for the serve surface")?;
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
                r = serve_web(Arc::clone(&backing), port) => r,
                r = mcp::serve_stdio_on(Arc::clone(&engine)) => r,
            },
            (false, true) => serve_web(backing, port).await,
            (true, false) => mcp::serve_stdio_on(engine).await,
            // clap guarantees at least one of --mcp/--ui; defend anyway.
            (false, false) => anyhow::bail!("serve needs --mcp and/or --ui"),
        }
    })
}

/// Resolve the serve [`Backing`] from workspace discovery ([FR-WS-06], [ADR-52])
/// — the context-awareness decision, split out so it is unit-testable without
/// binding a socket or blocking on a serve loop.
///
/// `standalone == true` forces the single-root focus **even under a manifest**:
/// discovery is skipped entirely and the repo is served on its own, byte-for-byte
/// as a plain single-root serve (the `--standalone` escape hatch, [FR-WS-06]).
/// Otherwise [`discover`] walks up-tree: **no** manifest → [`Backing::Single`]
/// (one `Engine`, unchanged); a manifest → [`Backing::Federated`] over an
/// [`EngineRegistry`] that warms only its default member eagerly ([NFR-PE-10]).
///
/// # Errors
/// A malformed manifest (discovery fails loud), or the single-root engine fails
/// to start. A federated default member that fails to start is **not** fatal here
/// — it degrades inside [`EngineRegistry::new_serve_default`]; it surfaces later
/// when [`default_engine`] resolves the shared surface's engine.
pub fn resolve_serve_backing(root: &Path, standalone: bool) -> Result<Backing<Engine>> {
    let federation = if standalone {
        None
    } else {
        discover(root).context("discovering the workspace for the serve surface")?
    };
    Ok(match federation {
        None => {
            let engine = Engine::start(root)
                .map(Arc::new)
                .context("starting the Logos engine for the web surface")?;
            Backing::Single(engine)
        }
        Some(federation) => Backing::Federated(Box::new(EngineRegistry::new_serve_default(
            federation,
        ))),
    })
}

/// Bind the loopback listener and serve the router until the process ends, over
/// the discovered [`Backing`] (single-root or the member registry).
async fn serve_web(backing: Arc<Backing<Engine>>, port: u16) -> Result<()> {
    let listener = bind(port)?;
    let addr = listener
        .local_addr()
        .context("reading the web-surface bound address")?;
    // The carve-out invariant, asserted at the one bind site (ADR-27).
    debug_assert!(addr.ip().is_loopback(), "web surface bound a non-loopback address");
    tracing::info!(target: "logos::web", %addr, "web surface listening (loopback only)");
    // Fail loud on a placeholder UI: a binary compiled without a matching
    // `npm run build` embeds the Node-free placeholder shell, which serves an
    // in-browser "not built" page but is otherwise silent — that is how a release
    // binary once shipped a dead dashboard unnoticed. The consistency guard in
    // `render_shell` cannot catch it (the placeholder references no assets, so it is
    // internally consistent), so warn at startup the moment `--ui` binds.
    if spa::is_placeholder_shell() {
        tracing::warn!(
            target: "logos::web",
            "the web UI is the PLACEHOLDER shell, not a real build — http://{addr}/ will show a \
             'bundle not built' page and the dashboard will not function. Rebuild with \
             `cd web/ui && npm run build` then reinstall the binary with `--features ui` (or \
             `agents`). The JSON API at /api/v1 works regardless."
        );
    }
    listener
        .set_nonblocking(true)
        .context("switching the web listener to non-blocking")?;
    let listener = tokio::net::TcpListener::from_std(listener)
        .context("adopting the loopback listener into tokio")?;
    axum::serve(listener, router_for_backing(backing)?)
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

/// The **enumerated** config-write routes on which [`method_guard`] admits a
/// `POST` (ADR-31, NFR-SE-06) — the chat, wiki-generation and verify constants
/// below are the only other admitted `POST`s; every other path/method stays
/// GET-only (`405`). The three member routes bridge to the [`api-facade`]'s
/// mutating config seam; the three workspace routes write at the workspace root
/// without an engine (below). All six are additionally gated by [`intent_guard`]
/// (same-origin + per-session token).
///
/// - `/config/save` → [`Engine::config_write`] (validated atomic write).
/// - `/config/apply` → [`Engine::config_apply`] (explicit reconcile/re-eval).
/// - `/config/secret` → [`Engine::config_write_secret`] (the masked chat-key
///   write to the gitignored `secrets.toml`, S-169, [FR-CF-06], [NFR-SE-07]).
/// - `/api/v1/workspace/config/save` and `/api/v1/workspace/config/secret` → the
///   **same** two writers, [`write_config`] and [`write_secret`], pointed at the
///   workspace root the backing holds (S-450, [FR-WS-30]). They reach the writers
///   directly rather than through an [`Engine`] because none may be constructed
///   at the workspace root ([ADR-40]); that is also why the workspace tier has
///   **no** apply route here. The config save is refused on a stale load
///   fingerprint exactly as the manifest save below is (S-451).
/// - `/api/v1/workspace/manifest/save` → [`manifest::save_workspace_manifest`], the
///   whole-manifest write path over `logos.workspace.toml` (S-430, [FR-UI-38]):
///   validated by the parser discovery runs, refused on a stale load
///   fingerprint, written verbatim. Engine-free for the same reason.
///
/// The match is exact path equality, so a route mounted with `post(` and missing
/// from this list is refused `405` before it routes. The unit test
/// `every_post_mounted_route_is_admitted_by_the_method_guard` fails for exactly
/// that route, so the omission is caught at build time rather than as a dead
/// Save button ([NFR-SE-06]).
///
/// [`api-facade`]: ../../../docs/specs/architecture/components/api-facade.md
/// [`write_config`]: logos_core::config::write_config
/// [`write_secret`]: logos_core::config::write_secret
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
/// [`manifest::save_workspace_manifest`]: logos_core::federation::manifest::save_workspace_manifest
/// [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
/// [NFR-SE-06]: ../../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
pub const CONFIG_POST_ROUTES: &[&str] = &[
    "/config/save",
    "/config/apply",
    "/config/secret",
    "/api/v1/workspace/config/save",
    "/api/v1/workspace/config/secret",
    "/api/v1/workspace/manifest/save",
];

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

/// The GET thread-list read route (S-209, [FR-UI-26], [ADR-47], [ADR-28]): the
/// conversation-history rail's data seam — every persisted conversation
/// (`id`, `title`, `updated_at`), most-recent-first. A pure loopback, same-origin
/// GET carrying no secret ([NFR-SE-07]); the per-thread messages read hangs off
/// the `{id}` sub-path, and the per-thread **delete** is the one mutating verb
/// under this tree (a `POST` to `…/{id}/delete`, gated by [`intent_guard`]).
///
/// [FR-UI-26]: ../../../docs/specs/requirements/FR-UI-26.md
/// [ADR-47]: ../../../docs/specs/architecture/decisions/ADR-47.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
pub const CHAT_THREADS_ROUTE: &str = "/api/v1/chat/threads";

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
    /// The engine the shared single-root surfaces run against: the one engine
    /// under [`Backing::Single`], or the workspace's warmed default member under
    /// [`Backing::Federated`]. Resolved once at router build so every existing
    /// read handler keeps extracting `State<Arc<Engine>>` **unchanged** — the
    /// single-root path is byte-for-byte as today ([ADR-52]).
    engine: Arc<Engine>,
    /// The serve backing ([ADR-52]): the single engine, or the member registry.
    /// The `/api/v1/workspace/*` fan-out handlers read the registry through it and
    /// answer `404` under [`Backing::Single`] (this is not a workspace, [FR-WS-06]).
    backing: Arc<Backing<Engine>>,
    /// The cross-service contract bridge, cached on member sync-stamps
    /// ([FR-WS-04]); inert under [`Backing::Single`]. Shared with the workspace
    /// handlers so a fan-out stitches over one bridge edge set.
    bridge: Arc<ContractBridge>,
    /// The build-dependency relation, held beside the bridge and joined on its
    /// first query ([FR-WS-33]); inert under [`Backing::Single`]. Never a runtime
    /// coupling: nothing that reads the bridge reads it ([BR-58]).
    build_deps: Arc<BuildDependencies>,
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

impl FromRef<WebState> for Arc<Backing<Engine>> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.backing)
    }
}

impl FromRef<WebState> for Arc<ContractBridge> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.bridge)
    }
}

impl FromRef<WebState> for Arc<BuildDependencies> {
    fn from_ref(state: &WebState) -> Self {
        Arc::clone(&state.build_deps)
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
/// per-session [`IntentToken`]. The single-root production entry point (and the
/// seam the byte-identical `/api/v1/*` regression tests drive).
pub fn router(engine: Arc<Engine>) -> Router {
    router_with_intent(engine, IntentToken::generate())
}

/// Build the router over a discovered [`Backing`] — the context-aware serve entry
/// point ([FR-WS-06], [ADR-52]). Single-root backings behave exactly as
/// [`router`]; a federated backing additionally answers the `/api/v1/workspace/*`
/// fan-out surface over the member registry, its shared `/api/v1/*` routes running
/// against the warmed default member.
///
/// # Errors
/// A federated backing whose default member cannot start (so no engine can answer
/// the shared surface). The single-root backing is infallible.
pub fn router_for_backing(backing: Arc<Backing<Engine>>) -> Result<Router> {
    let engine = backing.default_engine()?;
    Ok(build_router(make_state(engine, backing, IntentToken::generate())))
}

/// Build a workspace router over a member [`EngineRegistry`] — the seam the S-249
/// axum handler tests drive to exercise `/api/v1/workspace/*` end-to-end without a
/// socket. Wraps the registry in [`Backing::Federated`] and delegates to
/// [`router_for_backing`].
///
/// # Errors
/// The workspace's default member cannot start (see [`router_for_backing`]).
pub fn workspace_router(registry: EngineRegistry<Engine>) -> Result<Router> {
    router_for_backing(Arc::new(Backing::Federated(Box::new(registry))))
}

/// Build a workspace router over an explicit [`IntentToken`] — the seam the S-250
/// member-scope tests drive to exercise the **mutating** routes (`/config/save` and
/// friends) end-to-end under `?repo=`, which they cannot do through
/// [`workspace_router`] because [`intent_guard`] (correctly) rejects a `POST` that
/// does not echo the session's token, and that token is minted internally there.
///
/// # Errors
/// The workspace's default member cannot start (see [`router_for_backing`]).
pub fn workspace_router_with_intent(
    registry: EngineRegistry<Engine>,
    intent: IntentToken,
) -> Result<Router> {
    let backing = Arc::new(Backing::Federated(Box::new(registry)));
    let engine = backing.default_engine()?;
    Ok(build_router(make_state(engine, backing, intent)))
}

/// The chat service for this request's member scope (S-250, [FR-UI-29]).
///
/// The injected [`ChatService`](chat::ChatService) is bound to the **default** engine at
/// router build (and, in the carve-out tests, is a mock provider). That is right for
/// single-root and for an unscoped workspace request. But a `?repo=`-scoped request is
/// reading member B, so answering its chat turn from member A's graph would state one
/// member's answers under another member's name ([NFR-RA-05]) — so bind a service to the
/// resolved engine instead. `Arc::ptr_eq` is the test: same engine ⇒ the injected service
/// (mock seams preserved); a different engine ⇒ a service for that member, resolving its
/// inherited chat halves against the same `workspace_root` ([ADR-67]).
///
/// A scoped member's service is a member chat like any other: single-backing, with no
/// cross-service reach ([S-481]) — that is the workspace chat's.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
#[cfg(feature = "agents")]
fn chat_for(
    injected: &Arc<dyn chat::ChatService>,
    default: &Arc<Engine>,
    scoped: Arc<Engine>,
    backing: &Arc<Backing<Engine>>,
) -> Arc<dyn chat::ChatService> {
    if Arc::ptr_eq(default, &scoped) {
        Arc::clone(injected)
    } else {
        Arc::new(chat::ConfiguredChatService::new(scoped, workspace_root_of(backing)))
    }
}

/// The wiki-generation service for this request's member scope (S-250, [FR-UI-29]).
///
/// The load-bearing case is a **write**: the Wiki tab's read-models are member-scoped, so
/// a generation pass launched from it must write into the member whose pages it is
/// showing — not into the workspace default's `wiki.db`. See [`chat_for`] for the
/// same-engine ⇒ injected-service rule that keeps the S-178 mock seam intact.
#[cfg(feature = "agents")]
fn wiki_for(
    injected: &Arc<dyn wikigen::WikiRunService>,
    default: &Arc<Engine>,
    scoped: Arc<Engine>,
    workspace_root: Option<std::path::PathBuf>,
) -> Arc<dyn wikigen::WikiRunService> {
    if Arc::ptr_eq(default, &scoped) {
        Arc::clone(injected)
    } else {
        Arc::new(wikigen::ConfiguredWikiRunService::new(
            scoped,
            workspace_root,
        ))
    }
}

/// Assemble the [`WebState`] from a resolved default `engine` and its `backing`,
/// wiring the `agents`-only chat/wiki seams when present — the one place state is
/// constructed, shared by every router entry point.
fn make_state(engine: Arc<Engine>, backing: Arc<Backing<Engine>>, intent: IntentToken) -> WebState {
    let bridge = Arc::new(ContractBridge::new());
    #[cfg(feature = "agents")]
    {
        let workspace_root = workspace_root_of(&backing);
        let chat: Arc<dyn chat::ChatService> = Arc::new(chat::ConfiguredChatService::new(
            Arc::clone(&engine),
            workspace_root.clone(),
        ));
        let wiki: Arc<dyn wikigen::WikiRunService> = Arc::new(
            wikigen::ConfiguredWikiRunService::new(Arc::clone(&engine), workspace_root),
        );
        WebState {
            engine,
            backing,
            bridge,
            build_deps: Arc::new(BuildDependencies::new()),
            intent,
            chat,
            wiki,
            wiki_state: wikigen::WikiRunState::new(),
        }
    }
    #[cfg(not(feature = "agents"))]
    {
        WebState {
            engine,
            backing,
            bridge,
            build_deps: Arc::new(BuildDependencies::new()),
            intent,
        }
    }
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
    // Single-root backing: the engine IS the default, no registry is allocated —
    // the `/api/v1/workspace/*` surface answers `404` (not a workspace, ADR-52).
    // Under `agents` the state carries the config-resolved chat + wiki-generation
    // seams; under a plain `--features ui` build it is just the engine + intent
    // token, and the chat/wiki routes are never mounted (CR-078, ADR-60).
    let backing = Arc::new(Backing::Single(Arc::clone(&engine)));
    build_router(make_state(engine, backing, intent))
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
    let wiki: Arc<dyn wikigen::WikiRunService> = Arc::new(wikigen::ConfiguredWikiRunService::new(
        Arc::clone(&engine),
        None,
    ));
    let backing = Arc::new(Backing::Single(Arc::clone(&engine)));
    build_router(WebState {
        engine,
        backing,
        bridge: Arc::new(ContractBridge::new()),
        build_deps: Arc::new(BuildDependencies::new()),
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
        Arc::new(chat::ConfiguredChatService::new(Arc::clone(&engine), None));
    let backing = Arc::new(Backing::Single(Arc::clone(&engine)));
    build_router(WebState {
        engine,
        backing,
        bridge: Arc::new(ContractBridge::new()),
        build_deps: Arc::new(BuildDependencies::new()),
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
        // graph/impact/query twins the server-rendered canvas consumed
        // were removed at the S-192 decommission; the SPA consumes only this suite.
        .route("/api/v1/overview", get(api_v1::overview))
        .route("/api/v1/health", get(api_v1::health))
        // The right-sized readout the app header reads on navigation (S-315,
        // FR-UI-34, CR-097) — the FR-NV-07 status projection alone, beside (not
        // instead of) the Health bundle above, which the Health view still owns.
        .route("/api/v1/status", get(api_v1::status))
        .route("/api/v1/architecture", get(api_v1::architecture))
        .route("/api/v1/gaps", get(api_v1::gaps))
        .route("/api/v1/files", get(api_v1::files))
        .route("/api/v1/coverage", get(api_v1::coverage))
        .route("/api/v1/graph", get(api_v1::graph))
        .route("/api/v1/query", get(api_v1::search_query))
        // Read-only Decisions-panel impact read-model (FR-NV-10, FR-DG-02): the
        // JSON the SPA\'s Decisions panel (S-186) builds client-side from.
        .route("/api/v1/impact", get(api_v1::impact))
        .route("/api/v1/impact-intersection", get(api_v1::impact_intersection))
        .route("/api/v1/precedent", get(api_v1::precedent))
        .route("/api/v1/branch-overlap", get(api_v1::branch_overlap))
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
        // ── The cross-service workspace read-model fan-out (S-249, FR-WS-06,
        // ADR-52): the `/api/v1/workspace/*` surface the workspace SPA (S-250) will
        // consume. Each is a GET serialising one `query::*` read-model over the
        // member registry, so the read-only carve-out stack (method/host/CSP) already
        // covers them. Under a single-root `Backing::Single` (a plain repo, or
        // `--standalone`) they answer an honest `404` — the registry is never
        // allocated, so the single-root path pays nothing for these routes.
        // The engine-free roster the SPA shell probes on every load (S-250): it
        // decides workspace-vs-single-root mode and fills the member selector without
        // warming a single member (NFR-PE-10) — `status` below fans out over all of
        // them, so it is fetched by the app-level views that exist to show exactly
        // that (the Workspace tab, and S-428's Workspace Dashboard and Workspace
        // Health) and never by the shell.
        .route("/api/v1/workspace/roster", get(api_v1::workspace_roster))
        .route("/api/v1/workspace/status", get(api_v1::workspace_status))
        .route("/api/v1/workspace/route-providers", get(api_v1::workspace_route_providers))
        // S-464 / FR-WS-33: the build-dependency relation — a build dependency,
        // never a runtime coupling (BR-58); the service map's off-by-default layer.
        .route("/api/v1/workspace/build-deps", get(api_v1::workspace_build_deps))
        .route("/api/v1/workspace/search", get(api_v1::workspace_search))
        .route("/api/v1/workspace/callers", get(api_v1::workspace_callers))
        .route("/api/v1/workspace/impact", get(api_v1::workspace_impact))
        // ── `federation::reach` and `federation::governance` (S-427,
        // [FR-WS-28], [ADR-01]): both already answered on the CLI and over MCP
        // (`mcp::server::workspace_reachability` / `workspace_check`); this is
        // the web surface they never had. Thin serialisations of the very same
        // read-models, so no new core query and no CLI change. Everything the
        // group comment above says — GET, carve-out stack, single-root `404` —
        // governs these two as it does the six.
        .route("/api/v1/workspace/reachability", get(api_v1::workspace_reachability))
        .route("/api/v1/workspace/check", get(api_v1::workspace_check))
        // ── The `app`-scoped telemetry aggregate (S-429, [FR-UI-37],
        // [NFR-PE-10]): every member's `telemetry.db` summed over one window.
        // The twin of `/api/v1/statistics` one scope up — that one answers for
        // the selected member, this one for the workspace — and the reason it is
        // a workspace route and not a `?repo=`-less mode of that one is the
        // engine budget: it reads each member's store directly rather than
        // through `Engine::stats`, so a view load constructs no member engine
        // and the resident count is what a `workspace status` already paid.
        .route("/api/v1/workspace/statistics", get(api_v1::workspace_statistics_aggregate))
        // ── The workspace root as a config root (S-450, [FR-WS-30], [ADR-40]):
        // the read half. The workspace root's own `config.toml`/`secrets.toml`
        // read-model — a filesystem read at a root that holds no graph, so no
        // engine is constructed and no member is warmed. Its two write twins are
        // mounted with the other enumerated POSTs below; single-root answers all
        // three with the family's `404`.
        .route("/api/v1/workspace/config", get(api_v1::workspace_config))
        // ── The workspace manifest as an editable document (S-430, [FR-UI-38]):
        // the read half — the literal manifest, its load fingerprint and the parse
        // verdict, read at the workspace root with no engine. Its save twin is
        // mounted with the enumerated POSTs below; single-root answers both `404`.
        .route("/api/v1/workspace/manifest", get(api_v1::workspace_manifest))
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
        // ── The conversation-history read/delete API (S-209, FR-UI-26, ADR-47).
        // Two GET reads over the read-only seam — the thread list (id, title,
        // updated_at, most-recent-first) and one thread's ordered messages
        // (ADR-28) — plus the per-thread delete that supersedes the global clear:
        // a `POST` to `…/{id}/delete` (DELETE is `405` under `method_guard`, so
        // the mutation rides a POST verb) backed by `ChatStore::delete_thread`
        // (S-208), intent-guarded like every mutating POST and kept in lock-step
        // with `post_route_admitted` below.
        .route(CHAT_THREADS_ROUTE, get(chat_threads))
        .route("/api/v1/chat/threads/:id", get(chat_thread_messages))
        .route("/api/v1/chat/threads/:id/delete", post(chat_thread_delete))
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
        // S-450 / FR-WS-30: the same two writers at the workspace root — the
        // policy document and the credential. Listed in `CONFIG_POST_ROUTES`
        // like the three above; deliberately no apply twin (ADR-40).
        .route("/api/v1/workspace/config/save", post(api_v1::workspace_config_save))
        .route("/api/v1/workspace/config/secret", post(api_v1::workspace_config_secret))
        // S-430 / FR-UI-38: the whole-manifest save — validate, refuse a stale
        // fingerprint, write verbatim. Listed in `CONFIG_POST_ROUTES` like the rest.
        .route("/api/v1/workspace/manifest/save", post(api_v1::workspace_manifest_save))
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
    State(default): State<Arc<Engine>>,
    State(backing): State<Arc<Backing<Engine>>>,
    MemberEngine(engine): MemberEngine,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    // Answer from the member the user is actually reading (S-250), resolving its
    // chat halves against the workspace root it may inherit them from ([ADR-67]).
    let chat = chat_for(&chat, &default, engine, &backing);
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
    State(default): State<Arc<Engine>>,
    State(backing): State<Arc<Backing<Engine>>>,
    MemberEngine(engine): MemberEngine,
    State(run_state): State<wikigen::WikiRunState>,
    headers: HeaderMap,
) -> Response {
    // Generate INTO the member whose pages the tab is showing — never the default's
    // wiki.db (S-250; the Wiki read-models are member-scoped) — inheriting its chat
    // provider and key through the same seam the turn reads ([ADR-67], [ADR-42]).
    let wiki = wiki_for(&wiki, &default, engine, workspace_root_of(&backing));
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

/// Method guard (FR-UI-03 as revised, ADR-31, S-170/S-209/S-206): the surface is
/// GET-only **except** a `POST` to one of the enumerated [`CONFIG_POST_ROUTES`],
/// the [`CHAT_POST_ROUTE`] (a turn), a per-thread delete under
/// [`CHAT_THREADS_ROUTE`] (`…/{id}/delete`, S-209), the [`WIKI_GENERATE_ROUTE`]
/// (the Wiki-tab generation trigger), or the [`VERIFY_POST_ROUTE`] (the deep
/// graph-consistency check). Every other method — and a `POST` to any other
/// path — is answered `405` before any handler runs, so relaxing the read-only
/// posture stays bounded to exactly the config-write/apply seam, the two chat
/// routes, the wiki-generation trigger, and the verify route (NFR-SE-06).
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
    if path == CHAT_POST_ROUTE || is_chat_thread_delete_route(path) || path == WIKI_GENERATE_ROUTE {
        return true;
    }
    false
}

/// Does `path` name the per-thread delete route (`/api/v1/chat/threads/{id}/delete`
/// with an integer `{id}`)? This is the one mutating verb under
/// [`CHAT_THREADS_ROUTE`] (S-209, [ADR-47]) — it replaced the global `/chat/clear`
/// in the [`post_route_admitted`] allow-list. The `{id}` segment must parse as the
/// `i64` rowid the handler extracts, so a malformed or extra-segment path is not
/// admitted here (it stays `405`), never silently reaching the handler.
///
/// [ADR-47]: ../../../docs/specs/architecture/decisions/ADR-47.md
#[cfg(feature = "agents")]
fn is_chat_thread_delete_route(path: &str) -> bool {
    path.strip_prefix(CHAT_THREADS_ROUTE)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_suffix("/delete"))
        .is_some_and(|id| id.parse::<i64>().is_ok())
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
    MemberEngine(engine): MemberEngine,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(file) = parse_policy_file(form.get("file")) else {
        return (StatusCode::BAD_REQUEST, "unknown policy file (expected file=config|rules)")
            .into_response();
    };
    let content = form.get("content").cloned().unwrap_or_default();
    let result = bridge(engine, "config_save", Surface::Web, move |e| e.config_write(file, &content)).await;
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
pub(crate) fn config_write_status(e: &anyhow::Error) -> StatusCode {
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
    MemberEngine(engine): MemberEngine,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(file) = parse_policy_file(form.get("file")) else {
        return (StatusCode::BAD_REQUEST, "unknown policy file (expected file=config|rules)")
            .into_response();
    };
    let result = bridge(engine, "config_apply", Surface::Web, move |e| e.config_apply(file)).await;
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
    MemberEngine(engine): MemberEngine,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let api_key = form.get("api_key").cloned().unwrap_or_default();
    let result = bridge(engine, "config_save_secret", Surface::Web, move |e| {
        e.config_write_secret(&api_key)
    })
    .await;
    match result {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => (config_write_status(&e), err_text(e)).into_response(),
    }
}

/// One conversation's list-row (S-209, [FR-UI-26], [ADR-47]): the exact
/// contract the history rail reads — `id`, `title`, `updated_at`, and nothing
/// else. Deliberately narrower than the store's [`ChatThread`](chat_agent::ChatThread)
/// (it drops `created_at`) so the thread-list payload carries only what the rail
/// renders and no secret ever rides it ([NFR-SE-07]).
#[cfg(feature = "agents")]
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
#[cfg(feature = "agents")]
async fn chat_threads(MemberEngine(engine): MemberEngine) -> Response {
    let result = bridge(engine, "chat_threads", Surface::Web, move |e| {
        let store = chat_agent::ChatStore::open(e.root())?;
        store.list_threads()
    })
    .await;
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
/// with its tool traces — the [`ChatMessage`](chat_agent::ChatMessage)s the rail
/// hydrates on select. A GET carrying no secret ([NFR-SE-07]); a request for a
/// thread that does not exist is an honest `404` (distinct from an empty-but-real
/// thread), never a misleading empty `200` ([NFR-CC-04]). Runs on the pool through
/// the [`bridge`] ([ADR-03], [ADR-13] telemetry); a store fault is a `500`.
#[cfg(feature = "agents")]
async fn chat_thread_messages(
    MemberEngine(engine): MemberEngine,
    axum::extract::Path(thread_id): axum::extract::Path<i64>,
) -> Response {
    let result = bridge(engine, "chat_thread_messages", Surface::Web, move |e| {
        let store = chat_agent::ChatStore::open(e.root())?;
        // Distinguish "no such thread" (→ 404) from "a real thread with no
        // messages" (→ an honest empty list) — `messages` alone cannot.
        match store.thread(thread_id)? {
            Some(_) => store.messages(thread_id).map(Some),
            None => Ok(None),
        }
    })
    .await;
    match result {
        Ok(Some(messages)) => Json(messages).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no chat thread with that id").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// `POST /api/v1/chat/threads/{id}/delete` → per-thread delete (S-209,
/// [FR-UI-26], [FR-UI-20], [ADR-47], [ADR-31]): the granular deletion that
/// supersedes the global Clear-history. Backed by
/// [`ChatStore::delete_thread`](chat_agent::ChatStore::delete_thread) (S-208) —
/// one `DELETE FROM chat_threads WHERE id=?` whose live cascade wipes that
/// thread's messages, scratchpad, and working memory (the S-168/S-175 FK
/// contract), so no orphaned memory survives. Already proven same-origin +
/// intentional by [`intent_guard`] (a forged or intent-less delete never reaches
/// here, [NFR-SE-06]). A hit is `204 No Content`; a miss (`delete_thread ==
/// false`, no thread had that id) is an idempotent `404`; a store fault is an
/// honest `500` ([NFR-CC-04]). The blocking SQLite work runs on the pool through
/// the [`bridge`] ([ADR-03], [ADR-13] telemetry), exactly like the other handlers.
#[cfg(feature = "agents")]
async fn chat_thread_delete(
    MemberEngine(engine): MemberEngine,
    axum::extract::Path(thread_id): axum::extract::Path<i64>,
) -> Response {
    let result = bridge(engine, "chat_thread_delete", Surface::Web, move |e| {
        let mut store = chat_agent::ChatStore::open(e.root())?;
        store.delete_thread(thread_id)
    })
    .await;
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no chat thread with that id").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, err_text(e)).into_response(),
    }
}

/// The ADR-03 submit-and-await bridge: run one blocking `Engine` call on the
/// blocking pool (tokio never enters logos-core), attributed to the [`Surface`]
/// the handler names, and emit the per-render log line through the tracing
/// chokepoint (ADR-13).
///
/// # Why every caller names its surface
///
/// `surface` used to be the literal `"web"` here, which is true of the
/// *process* and says nothing about **whose question** a handler answers.
/// Almost every route answers the user's — [`Surface::Web`] — but the app
/// header's graph-state readout ([FR-UI-34]) answers nobody's: the shell
/// re-issues it on every client-side navigation, so counting it would make the
/// shell's own furniture the loudest event in the store ([BR-42], [CR-097]).
/// Only the adapter knows which it is; the engine chokepoint underneath sees
/// one `status` read and cannot tell the header's from `logos status` typed by
/// a developer ([ADR-01]).
///
/// So the surface is a **parameter, not a default**: a handler added without
/// one does not compile, which is what makes "an unclassified read is a build
/// failure, never a silent inclusion" ([BR-42]) true at this boundary and not
/// merely intended. `agent-core`'s chat bridges hardcode [`Surface::Chat`] —
/// correctly, since only one agent reaches that module — and their
/// `in_chat_surface` doc prescribes this shape for the next caller: "give the
/// bridges a surface parameter and let each caller name itself". This is that
/// caller.
///
/// The scope is entered **inside** the `spawn_blocking` closure, not around the
/// `await`: [`in_surface`] scopes per thread, and the blocking pool is where
/// the engine — and so the telemetry event — actually runs. Resolution
/// therefore happens once per request at this adapter boundary, never per
/// engine call, and costs one thread-local `Cell` read on the emission path
/// ([NFR-OO-02]).
///
/// [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
/// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
/// [CR-097]: ../../docs/requests/CR-097-header-graph-state-readout.md
/// [FR-UI-34]: ../../docs/specs/requirements/FR-UI-34.md
/// [NFR-OO-02]: ../../docs/specs/requirements/NFR-OO-02.md
pub(crate) async fn bridge<T, F>(
    engine: Arc<Engine>,
    view: &'static str,
    surface: Surface,
    call: F,
) -> T
where
    F: FnOnce(&Engine) -> T + Send + 'static,
    T: Send + 'static,
{
    run_blocking(view, surface, move || call(&engine)).await
}

/// The blocking hop the handler adapters share — [`bridge`], the workspace
/// fan-out and the workspace config routes: run `call` on the blocking pool ([ADR-03]) inside the [`in_surface`] scope its
/// caller names, and log the render timing. [`bridge`] hands it an engine call;
/// the workspace fan-out (`api_v1::workspace_read`) a registry call; the
/// workspace config routes (S-450) a filesystem call at a root that has no
/// engine. One body, so the three cannot drift on the scope, the pool or the
/// panic rule. (Not every hop on this surface: member resolution and the agent
/// services still cross on a bare `spawn_blocking`, and name their surface — or
/// need none — on their own.)
///
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
pub(crate) async fn run_blocking<T, F>(view: &'static str, surface: Surface, call: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let started = Instant::now();
    let out = tokio::task::spawn_blocking(move || in_surface(surface, call))
        .await
        // A fallible call's errors ride inside `T`; a panic crossing the pool is
        // a bug — re-raise rather than mask it.
        .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()));
    tracing::info!(
        target: "logos::web",
        surface = surface.as_str(),
        view,
        duration_ms = started.elapsed().as_millis() as u64,
        "page render",
    );
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The agent surfaces resolve their service against the request's member (S-250).
    ///
    /// The rule has two halves and both matter: the **default** engine must keep the
    /// injected service (that is the S-170/S-178 mock seam the carve-out tests drive),
    /// while a **scoped** engine must get a service bound to THAT member — otherwise the
    /// Wiki tab, whose read-models are member-scoped, would launch a generation pass that
    /// writes into the workspace default's `wiki.db` (a cross-member write, [NFR-RA-05]).
    #[cfg(feature = "agents")]
    #[test]
    fn the_agent_services_follow_the_member_scope() {
        use tempfile::TempDir;

        let default_dir = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();
        let default = Arc::new(Engine::start(default_dir.path()).expect("default engine"));
        let other = Arc::new(Engine::start(other_dir.path()).expect("member engine"));

        let injected_chat: Arc<dyn chat::ChatService> =
            Arc::new(chat::ConfiguredChatService::new(Arc::clone(&default), None));
        let injected_wiki: Arc<dyn wikigen::WikiRunService> = Arc::new(
            wikigen::ConfiguredWikiRunService::new(Arc::clone(&default), None),
        );

        let single = Arc::new(Backing::Single(Arc::clone(&default)));

        // Unscoped (or single-root): the injected service is used verbatim — the mock
        // seam the chat/wiki carve-out tests inject is never bypassed.
        assert!(Arc::ptr_eq(
            &chat_for(&injected_chat, &default, Arc::clone(&default), &single),
            &injected_chat
        ));
        assert!(Arc::ptr_eq(
            &wiki_for(&injected_wiki, &default, Arc::clone(&default), None),
            &injected_wiki
        ));

        // Scoped to another member: a DIFFERENT service, bound to that member's engine —
        // so the turn is answered from, and the wiki written into, the member on screen.
        assert!(!Arc::ptr_eq(
            &chat_for(&injected_chat, &default, Arc::clone(&other), &single),
            &injected_chat
        ));
        assert!(!Arc::ptr_eq(
            &wiki_for(&injected_wiki, &default, Arc::clone(&other), None),
            &injected_wiki
        ));
    }

    /// [S-481]: under a federated backing the chat service is still the member chat —
    /// the default member's (built in `make_state`) and a `?repo=`-scoped member's (built
    /// in `chat_for`, bound to that member, not the injected default) — and neither is
    /// handed any cross-service reach: `ConfiguredChatService` no longer takes a query
    /// backing, so there is none to hand. Re-targeted from S-431's
    /// `the_chat_service_gets_cross_service_reach_exactly_under_a_federated_backing`;
    /// the reach itself moved to the workspace roster (`chat-agent`'s `WorkspaceRoster`).
    ///
    /// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
    #[cfg(feature = "agents")]
    #[test]
    fn a_federated_backing_still_binds_each_member_chat_to_its_member() {
        use logos_core::federation::{EngineRegistry, Federation, Member, RegistryMode};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let members: Vec<Member> = ["api", "web"]
            .into_iter()
            .map(|name| {
                std::fs::create_dir_all(tmp.path().join(name)).unwrap();
                Member { name: name.to_string(), root: tmp.path().join(name) }
            })
            .collect();
        let federation = Federation {
            name: "shop".to_string(),
            root: tmp.path().to_path_buf(),
            members,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
        };
        let federated: Arc<Backing<Engine>> = Arc::new(Backing::Federated(Box::new(
            EngineRegistry::new(federation, RegistryMode::Lazy),
        )));
        let default = Arc::new(Engine::start(tmp.path().join("api")).expect("default engine"));
        let other = Arc::new(Engine::start(tmp.path().join("web")).expect("member engine"));

        let state = make_state(Arc::clone(&default), Arc::clone(&federated), IntentToken::generate());
        assert!(Arc::ptr_eq(
            &chat_for(&state.chat, &default, Arc::clone(&default), &federated),
            &state.chat
        ));
        assert!(!Arc::ptr_eq(&chat_for(&state.chat, &default, other, &federated), &state.chat));
    }

    /// The [`bridge`] installs the boundary scope its caller names, and
    /// installs it **inside** the blocking closure — where the engine (and so
    /// the telemetry event) actually runs ([FR-OB-09], [CR-097]).
    ///
    /// Asserting it here rather than in `logos-core` is the point: the core
    /// proves the scope *works*, this proves the web adapter *enters* it. The
    /// probe closure stands in for a real handler body, which is exactly what
    /// `bridge` hands to the engine. Mirrors
    /// `both_engine_bridges_run_under_the_chat_surface` in `agent-core`; the
    /// shape itself is prescribed by `in_chat_surface`'s doc, not that test's.
    #[tokio::test]
    async fn the_bridge_runs_the_engine_call_under_the_surface_its_caller_names() {
        let dir = tempfile::tempdir().expect("temp project root");
        let engine = Arc::new(Engine::open(dir.path()));

        let seen = bridge(Arc::clone(&engine), "probe", Surface::Shell, |_| {
            logos_core::observability::current_surface_override()
        })
        .await;
        assert_eq!(
            seen,
            Some(Surface::Shell),
            "a chrome handler's engine call is attributed to the shell"
        );

        let seen = bridge(engine, "probe", Surface::Web, |_| {
            logos_core::observability::current_surface_override()
        })
        .await;
        assert_eq!(
            seen,
            Some(Surface::Web),
            "and an ordinary handler's to the web surface it is served from"
        );

        assert_eq!(
            logos_core::observability::current_surface_override(),
            None,
            "and the scope does not leak back to the serve loop's thread"
        );
    }

    /// Every handler on this surface names a surface, exactly one names
    /// [`Surface::Shell`], and that one is `status` ([CR-097], [BR-42]).
    ///
    /// # Why a whitelist and not a search for `Shell`
    ///
    /// Three distinct regressions live on this boundary and only a whitelist
    /// catches all three:
    ///
    /// 1. a **second** handler classified as shell chrome — its endpoint's
    ///    reads would leave the usage figures with nothing failing, the
    ///    [CR-091] closed-list defect one layer up;
    /// 2. the shell classification **moving** off `status` to another handler;
    /// 3. a handler naming some **third** surface — `Surface::Cli` as a typo
    ///    books a web read against the CLI's adoption figures, which *adds* to
    ///    a bucket rather than excluding from one, so no exclusion test can
    ///    ever see it.
    ///
    /// The required parameter on [`bridge`] and [`workspace_fan`] makes an
    /// *unclassified* handler a build failure; it says nothing about a
    /// *mis*-classified one. This is that other direction.
    ///
    /// # Robust to formatting, and it reads both files
    ///
    /// An earlier form matched `Surface::Shell` and `bridge(` on **one physical
    /// line** of `api_v1.rs` alone. Both halves were evadable, and a review
    /// proved it by running the evasions: wrapping the call across lines the way
    /// rustfmt does made a second shell handler invisible, and the six `bridge`
    /// sites in `lib.rs` — three of them `/api/v1` routes — were never read at
    /// all. So this scans both sources, strips comments, and locates each
    /// occurrence by the `async fn` that encloses it rather than by its line.
    ///
    /// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
    /// [CR-091]: ../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
    /// [CR-097]: ../../docs/requests/CR-097-header-graph-state-readout.md
    /// Production code only: the test module's own mentions of a marker are not
    /// handler code, and comments are prose about them.
    ///
    /// Shared by the two source-scanning pins below rather than written twice:
    /// a second copy that drifted from this one would make whichever pin held
    /// the stale copy quietly measure something else.
    fn production_code(source: &str) -> String {
        let code = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map_or(source, |(before, _)| before);
        code.lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Is the match at `at` a whole identifier, or the tail of a longer one?
    /// `latest_gate(` is a substring of `not_latest_gate(`, and a bare substring
    /// search would book that unrelated call against the accessor it happens to
    /// end with.
    fn whole_identifier(code: &str, at: usize) -> bool {
        code[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    }

    /// The body of the function whose header begins at the first occurrence of
    /// `header` — from its opening brace to the matching close, by brace depth.
    ///
    /// # Why this and not [`enclosing_fn`]
    ///
    /// They answer different questions and only one of them is right for a pin
    /// on a SINGLE function. `enclosing_fn` attributes an occurrence anywhere in
    /// the file to the nearest preceding `async fn`, which is what a census of
    /// every handler needs. It has no notion of where that function *ends*, and
    /// review demonstrated both consequences on real source: a sync helper
    /// defined after a handler is booked against that handler (`api_v1.rs`
    /// already interleaves them — `wants_flag` between `files` and `coverage`),
    /// so a pin false-fires on correct code; and an `async fn` nested inside a
    /// handler re-parents everything after it, so a genuine second read inside
    /// the handler goes undetected. Brace depth has both boundaries.
    ///
    /// String literals are skipped, so a brace inside one cannot unbalance the
    /// walk. An unbalanced walk **panics** rather than returning a short slice:
    /// a pin that silently scans the wrong region is worse than one that fails
    /// loudly. Run over comment-stripped code, so a brace in prose cannot reach
    /// it either.
    fn fn_body<'a>(code: &'a str, header: &str) -> &'a str {
        let start = code
            .find(header)
            .unwrap_or_else(|| panic!("`{header}` is not in this source"));
        let open = start
            + code[start..]
                .find('{')
                .unwrap_or_else(|| panic!("`{header}` has no body"));
        let bytes = code.as_bytes();
        let (mut depth, mut i, mut in_str) = (0usize, open, false);
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_str => i += 1,
                b'"' => in_str = !in_str,
                b'{' if !in_str => depth += 1,
                b'}' if !in_str => {
                    depth -= 1;
                    if depth == 0 {
                        return &code[open + 1..i];
                    }
                }
                _ => {}
            }
            i += 1;
        }
        panic!("unbalanced braces walking the body of `{header}`");
    }

    /// The name of **any** `fn` enclosing `at` — `fn`, `async fn` and `pub fn`
    /// alike.
    ///
    /// A deliberate sibling of [`enclosing_fn`] rather than a widening of it —
    /// and **not** because widening would disturb today's attribution. Review
    /// measured that claim and it is false: over the 36 `Surface::` sites the
    /// census reads, the two walkers agree on 35 and differ on exactly one —
    /// the wiki-generation site, where `enclosing_fn` yields the sentinel
    /// `<no enclosing async fn>`. Substituting this function throughout would
    /// be a no-op on every pre-existing site.
    ///
    /// They are kept apart for what each whitelist is *about*. The handler
    /// census asks "which **handler** is classified how", and there
    /// `<no enclosing async fn>` is the more useful answer than a helper's
    /// name: it says the marker is not inside a handler at all. The
    /// wiki-generation site genuinely is not — it is a `WikiRunService::
    /// start_run` impl, a plain `fn` — so it is attributed by this function
    /// instead, and its whitelist entry names the `fn` a reader can go and
    /// find.
    fn enclosing_any_fn(code: &str, at: usize) -> &str {
        let mut cursor = &code[..at];
        loop {
            let Some(start) = cursor.rfind("fn ") else {
                return "<no enclosing fn>";
            };
            if whole_identifier(cursor, start) {
                let rest = &code[start + "fn ".len()..];
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                return &rest[..end];
            }
            cursor = &cursor[..start];
        }
    }

    /// The name of the `async fn` enclosing `at` — how an occurrence is
    /// attributed to a handler without depending on line layout.
    fn enclosing_fn(code: &str, at: usize) -> &str {
        code[..at]
            .rfind("async fn ")
            .map(|start| {
                let rest = &code[start + "async fn ".len()..];
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                &rest[..end]
            })
            .unwrap_or("<no enclosing async fn>")
    }

    #[test]
    fn every_handler_names_its_surface_and_only_status_names_the_shell() {
        // Three files cross an adapter boundary: the 23 `bridge`, 8
        // `workspace_fan` + 1 `workspace_fan_try` sites and the 3 workspace-config
        // `run_blocking` sites (S-450, no engine) in `api_v1.rs`, the 6 `bridge`
        // sites in `lib.rs` (three of them `/api/v1/chat/*` routes), and the
        // wiki-generation pass in `wikigen/configured.rs`, which crosses on a
        // bare `spawn_blocking` and so names its surface directly ([CR-139]).
        let sources = [
            ("api_v1.rs", production_code(include_str!("api_v1.rs"))),
            ("lib.rs", production_code(include_str!("lib.rs"))),
            (
                "wikigen/configured.rs",
                production_code(include_str!("wikigen/configured.rs")),
            ),
        ];

        let mut shell_sites: Vec<(&str, &str)> = Vec::new();
        let mut wikigen_sites: Vec<(&str, &str)> = Vec::new();
        let mut web_sites = 0usize;
        for (file, code) in &sources {
            for (at, marker) in code.match_indices("Surface::") {
                let rest = &code[at + marker.len()..];
                let end = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                match &rest[..end] {
                    "Shell" => shell_sites.push((file, enclosing_fn(code, at))),
                    // Attributed by ANY enclosing `fn`: the site is a
                    // `WikiRunService::start_run` impl, not an `async fn`
                    // handler.
                    "WikiGen" => wikigen_sites.push((file, enclosing_any_fn(code, at))),
                    "Web" => web_sites += 1,
                    // The bare `use …::Surface;` import names no variant and
                    // does not reach here; anything else is a handler claiming
                    // a surface this adapter must never claim.
                    "" => {}
                    other => panic!(
                        "{file}: {} names Surface::{other} — this adapter serves the web \
                         surface and its own shell chrome, nothing else. A third surface \
                         here re-attributes a web read into another surface's figures \
                         (FR-OB-09, BR-42)",
                        enclosing_fn(code, at)
                    ),
                }
            }
        }

        assert_eq!(
            shell_sites.len(),
            1,
            "exactly one handler is shell chrome, got {shell_sites:?}"
        );
        assert_eq!(
            shell_sites[0],
            ("api_v1.rs", "status"),
            "and it is the status handler ([FR-UI-34]), got {shell_sites:?}"
        );
        // The same whitelist shape, for the same three regressions, on the
        // CR-139 surface: a SECOND site claiming to be Logos's own generator
        // would put a developer's reads into the generator's figures, and the
        // classification MOVING off the materialize call would put the
        // generator's back into the developer's.
        //
        // Deliberately `== 1`, not the `web_sites >= 34` floor a few lines
        // below, and the asymmetry is the point: a floor is right for the
        // ORDINARY surface, where a new handler is routine, and wrong for a
        // surface that claims to be somebody specific. This mirrors
        // `shell_sites.len() == 1` directly above. Review raised `>= 1` as an
        // alternative; it was rejected because it discards regression (1) —
        // the second claimant — which is the whole reason a non-web surface
        // gets a whitelist rather than a count.
        //
        // The cost is real and is recorded rather than removed: a legitimate
        // second WikiGen site means editing TWO files, this whitelist and
        // `web/tests/wikigen_enumeration.rs`'s `DECLARED_SITES`. That is
        // intended — the two guards answer different questions (is the
        // classification where we think it is / is every engine path declared)
        // — but a maintainer meeting it for the first time should not have to
        // rediscover why.
        assert_eq!(
            wikigen_sites.len(),
            1,
            "exactly one site is the wiki generation pass, got {wikigen_sites:?}"
        );
        assert_eq!(
            wikigen_sites[0],
            ("wikigen/configured.rs", "start_run"),
            "and it is the run the generation trigger spawns ([FR-OB-13], \
             [CR-139]), got {wikigen_sites:?}"
        );
        // Every other boundary crossing is the plain web surface.
        //
        // This is a FLOOR, not a census, and the distinction matters because the
        // comment here used to claim the opposite ("derived, not hardcoded: a new
        // handler raises both sides together"). 34 is hardcoded and a new handler
        // raises only the left side — adding one takes `web_sites` to 35 and this
        // assertion does not move. What it actually buys is anti-vacuity: it
        // proves `production_code` returned real source rather than an empty
        // strip, the same false-green shape as a `logos check` over zero rules.
        //
        // The exact assertions are the two above, on the Shell side; the
        // unclassified case is a compile error, since `bridge` and
        // `workspace_fan` both take `surface` as a required parameter.
        assert!(
            web_sites >= 34,
            "the other handlers all name Surface::Web (found {web_sites})"
        );
    }

    /// The census above names its two sources literally; this asserts that
    /// naming them is still enough — **no other `.rs` file under `web/src/`
    /// carries a surface marker.**
    ///
    /// # Why a filesystem walk and not a third `include_str!`
    ///
    /// `include_str!` cannot glob, so
    /// [`every_handler_names_its_surface_and_only_status_names_the_shell`]
    /// embeds `api_v1.rs` and `lib.rs` by name — while `bridge` is
    /// `pub(crate)` across a crate with ten other source files. A new module
    /// naming `Surface::Cli`, or crossing the adapter boundary through `bridge`,
    /// `workspace_fan` or the [`run_blocking`] hop beneath both (which takes its
    /// surface as a parameter, so a caller may hold it in a variable), would be
    /// classified by nothing and that census would
    /// not notice: its whitelist is exact about the files it reads and silent
    /// about the files it does not.
    ///
    /// Sprint 72's review found this **latent, not live** — no such site
    /// existed when it was recorded, which is exactly when the guard is cheap.
    /// Sprint 72 sprint review, deferred item 5.9; decided 2026-09-20.
    ///
    /// # Comments stripped, test modules deliberately NOT
    ///
    /// It does not reuse [`production_code`], and the first draft of this guard
    /// did. That helper truncates at the first `#[cfg(test)] mod tests {` and
    /// keeps nothing after it — exact for the two sources it was written for,
    /// since both end with their test module, and false for an arbitrary file.
    /// **Proven rather than reasoned:** a `Surface::Cli` const appended to
    /// `query.rs`, whose test module starts at line 365 of 437, sat after the
    /// truncation point and this test passed. A guard that reports clean by
    /// construction is the failure class the whole census exists to prevent, so
    /// this strips comments only and reads every line.
    ///
    /// The cost is the other direction: a *test* outside these two files that
    /// names a surface would red this. That is accepted — today no such site
    /// exists, and a test that needs the vocabulary is itself a file the census
    /// does not read.
    ///
    /// # What it cannot see
    ///
    /// An unclassified engine call that carries **no marker at all** — one that
    /// reaches the engine inside a bare `spawn_blocking`, naming no `Surface`
    /// and calling none of the helpers.
    ///
    /// `web/src/wikigen/configured.rs` was that shape, and [CR-139] closed it:
    /// the pass now names [`Surface::WikiGen`], so the file is in `SCANNED`
    /// above and its site is whitelisted by the census. The *class* is not
    /// closed, only that instance. What covers `web/src/wikigen/` now is a
    /// second, differently-shaped census — `web/tests/wikigen_enumeration.rs`
    /// enumerates every engine-reaching site in that module from a directory
    /// walk and compares it against a declared, classified table, so a marker
    /// is not what makes a site visible there. This guard's reach is still
    /// exactly "a `Surface`/`bridge`/`workspace_fan`/`run_blocking` marker in a file the
    /// census does not read", and it is stated here so the next audit starts
    /// from that rather than from an assumption.
    ///
    /// [CR-139]: ../../docs/requests/CR-139-the-wiki-generation-pass-names-its-own-surface.md
    #[test]
    fn no_other_source_under_web_src_carries_a_surface_marker() {
        const SCANNED: [&str; 3] = ["api_v1.rs", "lib.rs", "wikigen/configured.rs"];
        const MARKERS: [&str; 4] = ["Surface::", "bridge(", "workspace_fan(", "run_blocking("];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen_scanned: Vec<String> = Vec::new();
        let mut walked: Vec<String> = Vec::new();
        let mut offenders: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
            for entry in entries {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .expect("walked from the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                walked.push(rel.clone());
                if SCANNED.contains(&rel.as_str()) {
                    seen_scanned.push(rel.clone());
                    // NOT a blanket skip. The census reads these three through
                    // `production_code`, which truncates at the first
                    // `#[cfg(test)] mod tests {` — so the census covers a
                    // SCANNED file's production half and NOTHING after it,
                    // while this walk used to cover the whole file. Skipping
                    // outright therefore leaves each SCANNED file's own test
                    // module read by NEITHER guard.
                    //
                    // That is not hypothetical and it is not inherited: adding
                    // `wikigen/configured.rs` to SCANNED (S-435, CR-139) opened
                    // exactly that hole in a file that HAS a test module, and a
                    // `Surface::Cli` planted there passed the entire `web`
                    // suite. It is the Sprint 72 appendix 5.9 false-green shape
                    // recreated by the commit that cites it. So the tail — the
                    // region the census provably cannot see — is scanned here.
                    // One file is exempt, and the reason is not "it was
                    // noisy": `lib.rs` is where this scanner LIVES, so its test
                    // module necessarily writes down the very markers the
                    // scanner searches for — `MARKERS`, the census's
                    // `match_indices("Surface::")`, the panic strings. Those
                    // are the needle, not a classification site. Scanning them
                    // reported 12 offenders that classify nothing, which is
                    // how this exemption was found rather than assumed. Every
                    // OTHER scanned file's test module has no business naming a
                    // surface, and is checked below.
                    if rel == "lib.rs" {
                        continue;
                    }
                    let source = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
                    let Some((_, tail)) = source.split_once("\n#[cfg(test)]\nmod tests {") else {
                        // No test module ⇒ `production_code` truncates nothing
                        // and the census genuinely read the whole file.
                        continue;
                    };
                    let tail = tail
                        .lines()
                        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
                        .collect::<Vec<_>>()
                        .join("\n");
                    for marker in MARKERS {
                        for (at, _) in tail.match_indices(marker) {
                            if whole_identifier(&tail, at) {
                                offenders.push(format!(
                                    "{rel}: `{marker}` in its own test module, past the \
                                     point `production_code` truncates — read by neither \
                                     guard"
                                ));
                            }
                        }
                    }
                    continue;
                }
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
                // Comments only — prose *about* a marker is not a marker — and
                // every line of the file, test module included. See the header:
                // truncating at the test module made this guard false-green.
                let code = source
                    .lines()
                    .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
                    .collect::<Vec<_>>()
                    .join("\n");
                for marker in MARKERS {
                    for (at, _) in code.match_indices(marker) {
                        if whole_identifier(&code, at) {
                            offenders
                                .push(format!("{rel}: `{marker}` in {}", enclosing_fn(&code, at)));
                        }
                    }
                }
            }
        }

        // Anti-vacuity, the same shape as the census's own `web_sites` floor: a
        // walk rooted at the wrong directory, or one that matched no `.rs` file,
        // reports zero offenders and means nothing.
        seen_scanned.sort_unstable();
        let mut expected: Vec<String> = SCANNED.iter().map(|s| (*s).to_string()).collect();
        expected.sort_unstable();
        assert_eq!(
            seen_scanned,
            expected,
            "the walk did not find both census sources under {}; it saw {walked:?}",
            root.display()
        );
        assert!(
            walked.len() > SCANNED.len(),
            "the walk found only the census sources ({walked:?}) — `web/src` has more"
        );

        assert!(
            offenders.is_empty(),
            "these sources carry a surface marker that \
             `every_handler_names_its_surface_and_only_status_names_the_shell` never reads, \
             so whatever they classify is classified by nothing (FR-OB-09, BR-42). \
             Classify the site AND add its file to that census's `sources` list: {offenders:?}"
        );
    }


    /// The Health handler reads the last persisted snapshot **once**, through
    /// the single-read seam, and never as a `latest_gate` + `latest_scan` pair
    /// ([FR-UI-04], [CR-135] §3.2).
    ///
    /// # Why a source scan and not a race
    ///
    /// The defect is a window *between* two reads. A test that races the
    /// handler to land a `scan` inside that window is flaky by nature: against
    /// the broken composition it fails only sometimes, which is the one
    /// direction a regression test must not be unreliable in, and [CR-135] §7
    /// settles it — "pin the invariant structurally … rather than racing the
    /// handler. The reproduction is evidence the bug exists, not the regression
    /// test."
    ///
    /// So the invariant is pinned where it is actually decided. How many reads
    /// one response takes is a property of *which accessor the handler calls*,
    /// and that is a fact about this source. This assertion fails against the
    /// two-read composition the handler carried until this story
    /// (`gate: e.latest_gate()?` beside `scan: e.latest_scan()?`).
    ///
    /// # Scoped to `health`, not to the file
    ///
    /// `overview` legitimately reads the standalone verdict and must keep
    /// doing so — `latest_gate` and `latest_scan` are kept for their other
    /// callers, they are not removed. Every occurrence is therefore attributed
    /// to its enclosing `async fn`, and the second assertion pins that sibling
    /// as the reason the scope is a handler rather than the file: a change that
    /// "fixed" the pin by deleting the other caller would fail there.
    ///
    /// [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
    #[test]
    fn the_health_handler_reads_the_snapshot_once() {
        let code = production_code(include_str!("api_v1.rs"));

        // The matcher probed with the near miss it must reject — one character
        // from matching, and the only way to know is to run it.
        let decoy = "async fn health() { e.not_latest_gate(); e.latest_gate(); }";
        assert_eq!(
            decoy
                .match_indices("latest_gate(")
                .filter(|(at, _)| whole_identifier(decoy, *at))
                .count(),
            1,
            "`not_latest_gate(` ends with the accessor's name and is not a call to it"
        );

        /// Which snapshot-reading accessors `body` calls. Located in the whole
        /// comment-stripped body rather than per line, so wrapping the call the
        /// way rustfmt does cannot hide it; bounded to that body, so neither a
        /// sibling helper outside it nor a nested `async fn` inside it can move
        /// an occurrence across the boundary.
        fn snapshot_reads(body: &str) -> Vec<&str> {
            let mut found: Vec<&str> = Vec::new();
            for accessor in ["latest_health", "latest_gate", "latest_scan"] {
                let hits = body
                    .match_indices(&format!("{accessor}("))
                    .filter(|(at, _)| whole_identifier(body, *at))
                    .count();
                found.extend(std::iter::repeat_n(accessor, hits));
            }
            found.sort_unstable();
            found
        }

        let health = snapshot_reads(fn_body(&code, "async fn health("));
        assert_eq!(
            health,
            ["latest_health"],
            "the Health handler must read the last persisted snapshot exactly once, through \
             the single-read seam. Two reads of the same logical row in one response can \
             describe two generations with nothing in the payload saying so (CR-135 §3.2); \
             found {health:?}"
        );

        // The sibling that still wants the standalone verdict, and the reason
        // the pin above is scoped to one handler rather than to the file: the
        // seam re-points `latest_gate`/`latest_scan`, it does not remove them
        // (CR-135 §3.2). A "fix" that deleted this caller would fail here.
        assert_eq!(
            snapshot_reads(fn_body(&code, "async fn overview(")),
            ["latest_gate"],
            "the Overview handler still reads the standalone verdict"
        );
    }

    /// Every route mounted with `post(` is admitted by [`method_guard`]'s own
    /// predicate, and every enumerated config-write route is mounted
    /// ([NFR-SE-06], S-450).
    ///
    /// The guard matches on exact path equality, so a `POST` route added to the
    /// router and not to the allow-list is refused `405` before it routes: safe,
    /// but a Save button that silently never saves. This reads the router's own
    /// route table out of this file and asks [`post_route_admitted`] — the very
    /// function the guard calls — about each `POST` path, so the omission reds
    /// here instead. The reverse direction catches an allowance left behind by a
    /// removed route, which would admit a `POST` to a path nothing handles.
    ///
    /// Formatting-independent: the scan balances parentheses from each `.route(`
    /// rather than reading lines, and resolves a path given by constant
    /// (`VERIFY_POST_ROUTE`) as well as an inline literal. A `:param` segment is
    /// probed with a value its handler would accept.
    #[test]
    fn every_post_mounted_route_is_admitted_by_the_method_guard() {
        let code = production_code(include_str!("lib.rs"));

        // `(path, is POST-mounted)` for every `.route(` call in the router.
        let mut routes: Vec<(String, bool)> = Vec::new();
        for (at, marker) in code.match_indices(".route(") {
            let rest = &code[at + marker.len()..];
            let mut depth = 0usize;
            let end = rest
                .char_indices()
                .find(|&(_, c)| match c {
                    '(' => {
                        depth += 1;
                        false
                    }
                    ')' if depth == 0 => true,
                    ')' => {
                        depth -= 1;
                        false
                    }
                    _ => false,
                })
                .map(|(i, _)| i)
                .expect("every `.route(` call closes");
            let args = &rest[..end];
            let first = args.split(',').next().expect("a path argument").trim();
            let path = match first.strip_prefix('"') {
                Some(literal) => literal.trim_end_matches('"').to_string(),
                // A path given by constant: resolve it from its declaration here.
                None => {
                    let decl = format!("const {first}: &str = \"");
                    let from = code
                        .find(&decl)
                        .unwrap_or_else(|| panic!("route constant `{first}` is declared in lib.rs"));
                    let tail = &code[from + decl.len()..];
                    tail[..tail.find('"').expect("a closing quote")].to_string()
                }
            };
            let probe = path
                .split('/')
                .map(|seg| if seg.starts_with(':') { "1" } else { seg })
                .collect::<Vec<_>>()
                .join("/");
            routes.push((probe, args.contains("post(")));
        }

        let posts: Vec<&str> = routes
            .iter()
            .filter(|(_, post)| *post)
            .map(|(path, _)| path.as_str())
            .collect();
        // Anti-vacuity: a scan that matched nothing would pass the loop below.
        assert!(routes.len() > 20, "the route-table scan found only {} routes", routes.len());
        for listed in CONFIG_POST_ROUTES {
            assert!(
                posts.contains(listed),
                "`{listed}` is in CONFIG_POST_ROUTES but no `post(` route mounts it — the \
                 allowance admits a POST that nothing handles"
            );
        }

        // Under a listen-only build the agent routes are compiled out of the
        // router AND out of the guard, so they are the one legitimate exception.
        #[cfg(not(feature = "agents"))]
        let agents_only = [CHAT_POST_ROUTE, "/api/v1/chat/threads/1/delete", WIKI_GENERATE_ROUTE];
        #[cfg(feature = "agents")]
        let agents_only: [&str; 0] = [];
        let refused: Vec<&str> = posts
            .iter()
            .copied()
            .filter(|path| !agents_only.contains(path) && !post_route_admitted(path))
            .collect();
        assert!(
            refused.is_empty(),
            "these routes are mounted with `post(` but the method guard refuses them \
             (405 before routing) — add each to CONFIG_POST_ROUTES or its own enumerated \
             constant: {refused:?}"
        );
    }

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
