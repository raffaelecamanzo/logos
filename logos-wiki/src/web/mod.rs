//! The wiki's web surface (feature `web`): its read routes, its generation
//! trigger and its Dashboard field, contributed to the localhost web surface
//! through the extension host ([S-644], [FR-UI-06], [FR-WK-18], [FR-UI-09]).
//!
//! The web adapter mounts these beside its core route table, inside its guard
//! stack, while the `wiki` extension is active; the generation `POST` is admitted
//! by the method guard because the extension declares it ([`WebRoute::post`]).
//! The handlers use the adapter's public helpers — the member extractor, the
//! blocking bridge, the JSON replies — so a wiki route resolves, reports and fails
//! exactly as a core route does.
//!
//! [S-644]: ../../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension
//! [FR-UI-06]: ../../../docs/specs/requirements/FR-UI-06.md
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md

mod generate;
mod markdown;
mod menu;
mod routes;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::FromRef;
use axum::Router;
use logos_core::federation::{Backing, EngineRegistry};
use logos_core::Engine;
use logos_ext::web::{OverviewField, WebContext, WebRoute};
use logos_ext::ExtensionHost;
use web::IntentToken;

pub use generate::{
    spawn_run, WikiFrame, WikiGenerator, WikiRunGuard, WikiRunService, WikiRunState, WikiRunStream,
    WikiSink, WIKI_GENERATE_ROUTE,
};

use crate::{WikiEngine as _, WikiExtension, OVERVIEW_FIELD};

/// What the read routes are built over: the serve's default engine and backing,
/// which the web adapter's [`web::MemberEngine`] resolves each request against.
#[derive(Clone)]
struct ReadState {
    engine: Arc<Engine>,
    backing: Arc<Backing<Engine>>,
}

impl FromRef<ReadState> for Arc<Engine> {
    fn from_ref(state: &ReadState) -> Self {
        Arc::clone(&state.engine)
    }
}

impl FromRef<ReadState> for Arc<Backing<Engine>> {
    fn from_ref(state: &ReadState) -> Self {
        Arc::clone(&state.backing)
    }
}

/// Every route the extension mounts over `context` ([`crate::WikiExtension`]):
/// the read routes, and — with a generator — the generation trigger.
pub(crate) fn routes(context: &WebContext, generator: Option<Arc<dyn WikiGenerator>>) -> Vec<WebRoute> {
    let state = ReadState {
        engine: Arc::clone(&context.engine),
        backing: Arc::clone(&context.backing),
    };
    let mut routes = vec![
        // The wiki read API (S-189, FR-UI-06): the dual-axis `wiki status`, the
        // tiered menu IA the SPA Wiki tab renders, the FTS search, and the
        // presentation bundle of one page — every one read-only (ADR-28).
        WebRoute::get("/api/v1/wiki", routes::wiki_index, state.clone()),
        WebRoute::get("/api/v1/wiki/nav", routes::wiki_nav, state.clone()),
        WebRoute::get("/api/v1/wiki/search", routes::wiki_search, state.clone()),
        WebRoute::get("/api/v1/wiki/page/*slug", routes::wiki_page, state.clone()),
        // The same-origin, path-sandboxed doc-image route presented pages'
        // `<img src>` values resolve to (FR-WK-27, ADR-58).
        WebRoute::get("/api/v1/wiki/asset/*path", routes::wiki_asset, state),
    ];
    if let Some(generator) = generator {
        routes.extend(generate::generation_routes(context, generator));
    }
    routes
}

/// The field the extension adds to the Dashboard's overview read-model: the
/// agent-written Project Overview page, or `null` while none is written — an
/// honest absence, not an error ([FR-UI-09], [NFR-CC-04]).
///
/// [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn overview_field() -> OverviewField {
    OverviewField::new(OVERVIEW_FIELD, |engine: &Engine| engine.wiki_read(menu::PROJECT_OVERVIEW_SLUG))
}

// ── Router seams ─────────────────────────────────────────────────────────────
//
// The web surface composed with this extension, as the `logos` binary serves it
// (`web::hosted_router` over a host holding it) — the seams the wiki route tests
// drive. They were the web adapter's own (`web::router`, `web::router_with_wiki`)
// while the wiki lived there, and keep their shape so the suites moved unchanged.

/// A generator that answers every member with one injected service — the seam
/// through which a route test drives a mock provider.
struct Injected(Arc<dyn WikiRunService>);

impl WikiGenerator for Injected {
    fn service(&self, _engine: Arc<Engine>, _workspace_root: Option<PathBuf>) -> Arc<dyn WikiRunService> {
        Arc::clone(&self.0)
    }
}

/// A host composing `extension` alone.
fn host_with(extension: WikiExtension) -> ExtensionHost {
    ExtensionHost::new()
        .register(extension)
        .expect("a host with one extension registers it")
}

/// The single-root router over `engine` with the wiki and no in-app generation —
/// the listen-only dashboard's wiki.
pub fn router(engine: Arc<Engine>) -> Router {
    web::hosted_router(Arc::new(Backing::Single(engine)), IntentToken::generate(), &host_with(WikiExtension::new()))
        .expect("a single-root backing always has its engine")
}

/// The single-root router over `engine`, presenting `intent`, with `service` running
/// every generation pass — the seam the S-178 tests inject a mock-provider service
/// through, proving the trigger/SSE route end to end (exactly one run under the
/// single-run lock, per-page streaming, clean teardown, zero real egress) without a
/// live provider ([FR-WK-18]).
///
/// [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
pub fn router_with_wiki(engine: Arc<Engine>, intent: IntentToken, service: Arc<dyn WikiRunService>) -> Router {
    let extension = WikiExtension::with_generator(Arc::new(Injected(service)));
    web::hosted_router(Arc::new(Backing::Single(engine)), intent, &host_with(extension))
        .expect("a single-root backing always has its engine")
}

/// The single-root router over `engine`, presenting `intent`, with `generator`
/// binding each member's run service — the production shape under `agents`
/// when `generator` is the `wiki-agent`'s configured one.
pub fn router_with_generator(engine: Arc<Engine>, intent: IntentToken, generator: Arc<dyn WikiGenerator>) -> Router {
    web::hosted_router(Arc::new(Backing::Single(engine)), intent, &host_with(WikiExtension::with_generator(generator)))
        .expect("a single-root backing always has its engine")
}

/// The workspace router over `registry` with the wiki, `generator` (if any) running
/// its generation passes.
///
/// # Errors
/// The workspace's default member cannot start.
pub fn workspace_router(
    registry: EngineRegistry<Engine>,
    intent: IntentToken,
    generator: Option<Arc<dyn WikiGenerator>>,
) -> Result<Router> {
    let extension = generator.map_or_else(WikiExtension::new, WikiExtension::with_generator);
    web::hosted_router(Arc::new(Backing::Federated(Box::new(registry))), intent, &host_with(extension))
}
