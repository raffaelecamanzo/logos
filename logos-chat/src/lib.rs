//! `logos-chat` — the `chat` extension ([S-643], CR-213, [ADR-74]).
//!
//! The agentic chat over the code graph — its turn and conversation-history
//! routes, the workspace chat, the `[chat]` config table and the `chat.api_key`
//! secret, and its SPA views and config panel — composed into a binary through
//! the extension host like any optional capability. The `logos` binary's `main`
//! registers [`ChatExtension`] under its `agents` feature; no core crate
//! (`logos-core`, `mcp`, `web`, the `cli` library) depends on this one
//! ([ADR-74] §3). This crate depends on `web` for the helpers a route needs to
//! scope a request the way the core's routes do.
//!
//! # What it contributes
//!
//! | Contribution point | What |
//! |---|---|
//! | web routes | the turn `POST /chat` (and its SPA shell on `GET`), the thread list, messages and per-thread delete, rename and truncate under `/api/v1/chat/threads`, and the workspace chat's twins ([`routes`]) |
//! | `POST` allow-list | every `POST` above, by declaring it ([`logos_ext::web::WebRoute::post`]) |
//! | config table | `[chat]`, validated as [`ChatConfig`] ([FR-CF-09]) |
//! | secret key | `chat.api_key` |
//! | UI manifest | the member Chat and the Workspace Chat, and the chat config panel in both Config views ([FR-UI-51]) |
//! | telemetry tools | none: a chat turn's engine calls are the core's own tools, attributed to the `chat` surface by `agent-core`'s bridges ([FR-OB-10]) |
//!
//! [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
//! [FR-OB-10]: ../../docs/specs/requirements/FR-OB-10.md

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use logos_core::config::ChatConfig;
use logos_core::federation::{Backing, EngineRegistry};
use logos_core::Engine;
use logos_ext::ExtensionHost;
use web::IntentToken;
use logos_ext::ui::{UiConfigPanel, UiManifest, UiNavGroup, UiNavItem, UiNavScope};
use logos_ext::web::{WebContext, WebRoute};
use logos_ext::{Extension, TomlValue};

pub mod routes;
mod service;

pub use routes::{
    CHAT_POST_ROUTE, CHAT_THREADS_ROUTE, WORKSPACE_CHAT_POST_ROUTE, WORKSPACE_CHAT_THREADS_ROUTE,
};
pub use service::{spawn_turn, ChatFrame, ChatService, ChatStream, TurnTarget};

/// The extension's id — the name `GET /api/v1/extensions` reports.
pub const CHAT_ID: &str = "chat";

/// The `config.toml` table the extension owns.
pub const CHAT_TABLE: &str = "chat";

/// The `secrets.toml` key the extension owns.
pub const CHAT_API_KEY: &str = "chat.api_key";

/// The member Chat's navigation id, view route and the workspace entry that
/// replaces it in workspace mode.
const MEMBER_NAV_ID: &str = "chat";
const MEMBER_VIEW: &str = "/chat";
const WORKSPACE_NAV_ID: &str = "workspace-chat";
const WORKSPACE_VIEW: &str = "/workspace-chat";

/// The id of the chat config panel: the API key card beneath a member's `[chat]`
/// fields, and the workspace chat tier (policy and credential) in the workspace's
/// Config view — one panel per Config view, the SPA's registration choosing which.
pub const CONFIG_PANEL_ID: &str = "chat";

/// The chat extension ([S-643]).
///
/// [`ChatExtension::new`] serves the configured chat: each serve's member chat
/// resolves its provider from `[chat]` and the key from `secrets.toml`, inheriting
/// the workspace tier's halves where the member declares none ([ADR-67]), and a
/// workspace serve adds the workspace chat. The `with_*` constructors inject a
/// service instead — the seam the route tests drive a mock provider through.
///
/// [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
/// [ADR-67]: ../../docs/specs/architecture/decisions/ADR-67.md
#[derive(Clone, Default)]
pub struct ChatExtension {
    member: Option<Arc<dyn ChatService>>,
    workspace: Option<Arc<dyn ChatService>>,
}

impl ChatExtension {
    /// The configured chat — what the `logos` binary registers.
    pub fn new() -> Self {
        Self::default()
    }

    /// The configured chat, with `service` answering the turns of the serve's
    /// default member in place of the configured one. A `?repo=`-scoped turn for
    /// another member still gets a service bound to that member ([`routes`]).
    pub fn with_service(service: Arc<dyn ChatService>) -> Self {
        Self { member: Some(service), ..Self::default() }
    }

    /// The configured chat, with `service` as the workspace chat of a workspace
    /// serve. A single-root serve has no workspace chat whatever is injected: its
    /// workspace chat routes answer `404`.
    pub fn with_workspace_service(service: Arc<dyn ChatService>) -> Self {
        Self { workspace: Some(service), ..Self::default() }
    }
}

impl Extension for ChatExtension {
    fn id(&self) -> &'static str {
        CHAT_ID
    }

    fn config_tables(&self) -> &'static [&'static str] {
        &[CHAT_TABLE]
    }

    /// `[chat]` is valid exactly when it parses as [`ChatConfig`] and passes its
    /// bounds — the checks the core ran on it while it owned the table.
    fn validate_config_table(&self, _table: &str, value: &TomlValue) -> Result<(), String> {
        ChatConfig::validate_table(value)
    }

    fn secret_keys(&self) -> &'static [&'static str] {
        &[CHAT_API_KEY]
    }

    fn web_routes(&self, context: &WebContext) -> Vec<WebRoute> {
        routes::routes(context, self.member.clone(), self.workspace.clone())
    }

    fn ui_manifest(&self) -> Option<UiManifest> {
        Some(UiManifest {
            nav: vec![
                // Offered in single-root and `--standalone` serves; in a workspace the
                // chat is the Workspace Chat, and the member Chat gives way to it
                // (S-485, ADR-71).
                UiNavItem {
                    id: MEMBER_NAV_ID,
                    label: "Chat",
                    path: MEMBER_VIEW,
                    group: UiNavGroup::A,
                    scope: UiNavScope::Member,
                    replaced_in_workspace_by: Some(WORKSPACE_NAV_ID),
                },
                UiNavItem {
                    id: WORKSPACE_NAV_ID,
                    label: "Chat",
                    path: WORKSPACE_VIEW,
                    group: UiNavGroup::A,
                    scope: UiNavScope::App,
                    replaced_in_workspace_by: None,
                },
            ],
            views: vec![MEMBER_VIEW, WORKSPACE_VIEW],
            config_panels: vec![UiConfigPanel {
                id: CONFIG_PANEL_ID,
                label: "Chat",
                table: CHAT_TABLE,
            }],
            dashboard_cards: Vec::new(),
        })
    }
}

// ── Router seams ─────────────────────────────────────────────────────────────
//
// The web surface composed with this extension, as the `logos` binary serves it
// (`web::hosted_router` over a host holding it) — the seams the route tests drive.
// They were the web adapter's own (`web::router_with_chat` and friends) while chat
// lived there, and keep their shape so the suites moved unchanged (S-643).

/// A host composing `extension` alone.
fn host_with(extension: ChatExtension) -> ExtensionHost {
    ExtensionHost::new()
        .register(extension)
        .expect("a host with one extension registers it")
}

/// The single-root router over `engine` with the configured chat, minting no
/// token of its own: the seam that presents `intent` (and forges others).
pub fn router_with_intent(engine: Arc<Engine>, intent: IntentToken) -> Router {
    single_root(engine, intent, ChatExtension::new())
}

/// The single-root router over `engine` with `chat` answering the default
/// member's turns — the seam the route tests inject a mock-provider service
/// through, proving the SSE route end-to-end (incremental events, clean teardown,
/// zero real egress) without a live provider ([UAT-UI-07]).
///
/// [UAT-UI-07]: ../../docs/specs/requirements/UAT-UI-07.md
pub fn router_with_chat(engine: Arc<Engine>, intent: IntentToken, chat: Arc<dyn ChatService>) -> Router {
    single_root(engine, intent, ChatExtension::with_service(chat))
}

/// The workspace router over `registry` with the configured chat.
///
/// # Errors
/// The workspace's default member cannot start.
pub fn workspace_router_with_intent(registry: EngineRegistry<Engine>, intent: IntentToken) -> Result<Router> {
    workspace(registry, intent, ChatExtension::new())
}

/// The workspace router over `registry` with `chat` as the workspace chat — the
/// seam the S-482 route tests inject a mock-provider service through on
/// `POST /workspace/chat`.
///
/// # Errors
/// The workspace's default member cannot start.
pub fn workspace_router_with_chat(
    registry: EngineRegistry<Engine>,
    intent: IntentToken,
    chat: Arc<dyn ChatService>,
) -> Result<Router> {
    workspace(registry, intent, ChatExtension::with_workspace_service(chat))
}

fn single_root(engine: Arc<Engine>, intent: IntentToken, extension: ChatExtension) -> Router {
    web::hosted_router(Arc::new(Backing::Single(engine)), intent, &host_with(extension))
        .expect("a single-root backing always has its engine")
}

fn workspace(registry: EngineRegistry<Engine>, intent: IntentToken, extension: ChatExtension) -> Result<Router> {
    web::hosted_router(Arc::new(Backing::Federated(Box::new(registry))), intent, &host_with(extension))
}

#[cfg(test)]
mod tests;
