//! The internal same-origin **`/api/v1/*` JSON read-model API** ([FR-UI-21],
//! [ADR-43]) — the data seam the embedded client-side SPA ([spa-frontend])
//! consumes.
//!
//! One **read-only** handler per view's data: each endpoint serializes one
//! `Engine` read-model — or, for a view that composes several, a presentation
//! bundle whose every field *is* a read-model — over the [`bridge`](crate::bridge)
//! (the [ADR-03] `spawn_blocking` hop). The handlers **select, project, and join**
//! existing read-models and nothing more: no new `Engine` core query, no figure
//! the read-models do not already carry ([ADR-01], [NFR-MA-02], [NFR-RA-05]). The
//! pre-existing `/api/*` endpoints (graph-elements, graph query, impact, quadrant)
//! were **subsumed** into this surface and removed at the S-192 decommission:
//! their `/api/v1` twins reuse the very same readers, and this `/api/v1/*` suite
//! is now the sole data seam the embedded SPA consumes ([ADR-43]).
//!
//! # Read-only by construction ([FR-UI-03], [ADR-28])
//! Every endpoint is a `GET` and reads through the façade's **non-persisting**
//! accessors — `status`, the read-only `latest_*` twins, `coverage_*`, the wiki
//! read models, `config_read`, and the pure navigation readers. Loading any
//! endpoint once or repeatedly mutates no store (the no-write-on-read invariant
//! the [`crate`] `method_guard` keeps GET-only is preserved here verbatim). The
//! config-read endpoint returns the **masked** chat key only — masked by
//! construction in [`ConfigReadModel`](logos_core::config::ConfigReadModel)
//! ([FR-CF-06], [NFR-SE-07]).
//!
//! # Honest failures ([NFR-RA-05], web-surface failure mode)
//! A fallible read that genuinely fails (a store/IO fault) is answered `500` with
//! an explicit [`ApiError`] — never a blank or fabricated figure. A read the
//! corresponding server-rendered view *degrades* rather than fails (the language
//! composition, the temporal hotspot board) mirrors that policy exactly — a
//! defaulted composition, an `Option` hotspot board — so the API layer invents no
//! new degradation rule of its own. An absent wiki page is an honest `404`.
//!
//! These DTO shapes are **internal to the bundled frontend** (same binary, same
//! version): not a public API, deliberately outside the versioned-contract
//! discipline of [NFR-UX-06] ([FR-UI-21]).
//!
//! [FR-UI-21]: ../../docs/specs/requirements/FR-UI-21.md
//! [FR-UI-03]: ../../docs/specs/requirements/FR-UI-03.md
//! [FR-CF-06]: ../../docs/specs/requirements/FR-CF-06.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-MA-02]: ../../docs/specs/requirements/NFR-MA-02.md
//! [NFR-UX-06]: ../../docs/specs/requirements/NFR-UX-06.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
//! [ADR-43]: ../../docs/specs/architecture/decisions/ADR-43.md
//! [spa-frontend]: ../../docs/specs/architecture/components/spa-frontend.md

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use logos_core::config::ConfigReadModel;
use logos_core::history::{CoverageCrossReport, CoverageStatus, HotspotReport, TemporalReport};
use logos_core::model::NodeKind;
use logos_core::models::navigation::{
    GraphElements, ImpactResult, LanguageComposition, NodeInfo, SearchResult, StatusInfo,
};
use logos_core::models::quality::{
    DsmReport, EvolutionReport, GateResult, LanguagesInfo, RulesReport, ScanResult, StatsInfo,
    TestGapsReport, VerifyReport,
};
use logos_core::wiki::{AnchorProvenance, DocCategory, WikiHit, WikiPage, WikiStatus};
use logos_core::Engine;

use crate::query::QueryResponse;
use crate::{
    bridge, parse_edge_types, parse_granularity, parse_intent, parse_layers, query,
};

/// The honest error body ([NFR-RA-05]): a fallible read-model that genuinely
/// fails is reported as an explicit `500` payload, never papered over with a
/// blank or fabricated figure (web-surface failure mode). Mirrors the per-widget
/// error panels the server-rendered views render.
#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    /// The flattened façade error chain (`{e:#}`), exactly as the HTML views show
    /// it — actionable, never swallowed.
    pub error: String,
}

/// Serialize `model` as a `200 application/json` body — the success arm shared by
/// every endpoint.
fn ok<T: Serialize>(model: T) -> Response {
    Json(model).into_response()
}

/// Map a failed read-model to a `500` with the explicit [`ApiError`] chain — the
/// failure is surfaced, not masked ([NFR-RA-05]).
fn fail(err: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: format!("{err:#}") })).into_response()
}

/// Collapse a composed `Result` bundle into its response: the serialized bundle on
/// success, the honest `500` error chain on failure.
fn respond<T: Serialize>(model: anyhow::Result<T>) -> Response {
    match model {
        Ok(model) => ok(model),
        Err(err) => fail(err),
    }
}

// ── Dashboard / Overview (mirrors `crate::overview`, [FR-UI-09]) ──────────────

/// The Dashboard bundle ([FR-UI-09]): every read-model the verdict-rich landing
/// composes, each field traceable to its `Engine` read-model. Built over the
/// **read-only** accessors so the load is write-free ([ADR-28]). The
/// `composition` defaults on a read fault and `hotspots` is `Option` — mirroring
/// the view's own per-widget degradation ([NFR-CC-04]); a genuinely-required read
/// failing aborts the whole bundle to a `500`.
#[derive(Debug, Serialize)]
pub(crate) struct OverviewModel {
    status: StatusInfo,
    composition: LanguageComposition,
    languages: LanguagesInfo,
    gate: GateResult,
    coverage: CoverageStatus,
    gaps: TestGapsReport,
    stats: StatsInfo,
    /// The agent Project-Overview wiki page, or `null` when none is written yet
    /// (an honest absence, not an error — [NFR-CC-04]).
    overview_page: Option<WikiPage>,
    cross: CoverageCrossReport,
    /// The advisory hotspot board supplying the trust card's architectural weight;
    /// `null` when the temporal tier is unavailable (the view's `.ok()` degrade).
    hotspots: Option<HotspotReport>,
}

/// `GET /api/v1/overview` — the Dashboard data ([FR-UI-09], [FR-UI-21]).
pub(crate) async fn overview(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_overview", |e| -> anyhow::Result<OverviewModel> {
        Ok(OverviewModel {
            status: e.status(),
            // The view degrades a composition-read fault to the empty card, never
            // blanks the page — mirror that here ([NFR-CC-04]).
            composition: e.language_composition().unwrap_or_default(),
            languages: e.languages(),
            gate: e.latest_gate()?,
            coverage: e.coverage_status()?,
            gaps: e.test_gaps(None, false)?,
            stats: e.stats(None),
            overview_page: e.wiki_read(crate::wiki::PROJECT_OVERVIEW_SLUG)?,
            cross: e.coverage_cross()?,
            // A hotspot read needs git history; the view treats it as optional
            // (`.ok()`), so the trust card can still degrade to an unweighted share.
            hotspots: e.latest_hotspots(None, false).ok(),
        })
    })
    .await;
    respond(model)
}

// ── Health (mirrors `crate::health`, [FR-UI-04]) ──────────────────────────────

/// The Health bundle ([FR-UI-04]): the read-only gate verdict, the last persisted
/// scan (metrics + temporal tier), and the snapshot-series evolution — all
/// **read-only** twins so a load writes no `metric_snapshots` row ([ADR-28]).
#[derive(Debug, Serialize)]
pub(crate) struct HealthModel {
    status: StatusInfo,
    gate: GateResult,
    scan: ScanResult,
    evolution: EvolutionReport,
}

/// `GET /api/v1/health` — the Health data ([FR-UI-04], [FR-UI-21]).
pub(crate) async fn health(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_health", |e| -> anyhow::Result<HealthModel> {
        Ok(HealthModel {
            status: e.status(),
            gate: e.latest_gate()?,
            scan: e.latest_scan()?,
            evolution: e.evolution(None)?,
        })
    })
    .await;
    respond(model)
}

// ── Statistics (mirrors `logos stats`, [FR-OB-04]/[FR-UI-27]) ─────────────────

/// `GET /api/v1/statistics[?window=<days>]` — the enriched telemetry read-model
/// ([FR-OB-04], [FR-UI-27], [FR-UI-21]) the Statistics tab ([spa-frontend], S-235)
/// consumes: per-`(surface, tool)` usage, latency percentiles, the honestly-labeled
/// reads/tokens-saved estimate, the daily-activity series, and the dev-vs-`main`
/// origin split. A thin `bridge` pass-through of the [`Engine::stats`] read-model —
/// it selects and projects nothing beyond what that read-model already carries: no
/// new core query, no write ([ADR-01], [ADR-43], [ADR-28]).
///
/// `?window=<days>` scopes the trailing window; an absent or unparseable value falls
/// back to the core read-model's own default (7, [FR-OB-04]), matching the lenient
/// query contract the other endpoints use (`graph`'s `?cap=`). [`StatsInfo`] is
/// infallible at the surface — an empty store degrades to a zeroed model carrying the
/// "no telemetry recorded yet" warning, never an error ([NFR-CC-04]) — so this pairs
/// `bridge` with `ok`, not the fallible `respond`.
pub(crate) async fn statistics(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let window = q.get("window").and_then(|w| w.trim().parse::<u32>().ok());
    let info: StatsInfo = bridge(engine, "api_v1_statistics", move |e| e.stats(window)).await;
    ok(info)
}

// ── Architecture / Cycles (mirrors `crate::architecture`, [FR-UI-04]) ─────────

/// The Architecture bundle ([FR-UI-04], CR-038): the dependency-structure matrix
/// reframed cycles-first.
#[derive(Debug, Serialize)]
pub(crate) struct ArchitectureModel {
    status: StatusInfo,
    dsm: DsmReport,
}

/// `GET /api/v1/architecture` — the Architecture / Cycles data ([FR-UI-21]).
pub(crate) async fn architecture(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_architecture", |e| -> anyhow::Result<ArchitectureModel> {
        Ok(ArchitectureModel { status: e.status(), dsm: e.dsm(None, false)? })
    })
    .await;
    respond(model)
}

// ── Gaps (mirrors `crate::gaps`, [FR-UI-04]) ──────────────────────────────────

/// The Gaps bundle ([FR-UI-04]): the blast-radius-ranked test-gaps read-model and
/// the architecture-rules report. Both are read-only.
#[derive(Debug, Serialize)]
pub(crate) struct GapsModel {
    status: StatusInfo,
    test_gaps: TestGapsReport,
    rules: RulesReport,
}

/// `GET /api/v1/gaps` — the Gaps data ([FR-UI-21]).
pub(crate) async fn gaps(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_gaps", |e| -> anyhow::Result<GapsModel> {
        Ok(GapsModel {
            status: e.status(),
            test_gaps: e.test_gaps(None, false)?,
            rules: e.check_rules(None, false)?,
        })
    })
    .await;
    respond(model)
}

// ── Files & Risk (mirrors `crate::analytics::files`, [FR-UI-05]) ──────────────

/// The Files & Risk bundle ([FR-UI-05], CR-038): the ranked hotspot board (the
/// spine) joined with the per-file temporal facts. Both read through the
/// **read-only** `latest_*` accessors, so the load mines nothing ([ADR-28]).
#[derive(Debug, Serialize)]
pub(crate) struct FilesModel {
    status: StatusInfo,
    hotspots: HotspotReport,
    temporal: TemporalReport,
}

/// `GET /api/v1/files[?untested]` — the Files & Risk data ([FR-UI-21]). The
/// `?untested` toggle scopes the board to files lacking fresh positive coverage,
/// exactly as the server-rendered view's filter does.
pub(crate) async fn files(
    State(engine): State<Arc<Engine>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let untested = wants_untested(&params);
    let model = bridge(engine, "api_v1_files", move |e| -> anyhow::Result<FilesModel> {
        Ok(FilesModel {
            status: e.status(),
            hotspots: e.latest_hotspots(Some(50), untested)?,
            temporal: e.latest_temporal_report()?,
        })
    })
    .await;
    respond(model)
}

/// `true` when the query requests the `--untested` hotspot filter (`?untested` or
/// `?untested=1`) — the same extraction the server-rendered Files view uses.
fn wants_untested(params: &HashMap<String, String>) -> bool {
    params.get("untested").is_some_and(|v| v.is_empty() || v != "0")
}

// ── Coverage (mirrors `crate::analytics::coverage`, [FR-UI-05]) ───────────────

/// The Coverage bundle ([FR-UI-05]): the coverage status read-model joined with
/// the untested hotspot board ([FR-CV-07]). Both reads are read-only.
#[derive(Debug, Serialize)]
pub(crate) struct CoverageModel {
    status: StatusInfo,
    coverage: CoverageStatus,
    untested: HotspotReport,
}

/// `GET /api/v1/coverage` — the Coverage data ([FR-UI-21]).
pub(crate) async fn coverage(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_coverage", |e| -> anyhow::Result<CoverageModel> {
        Ok(CoverageModel {
            status: e.status(),
            coverage: e.coverage_status()?,
            untested: e.latest_hotspots(Some(20), true)?,
        })
    })
    .await;
    respond(model)
}

// ── Quadrant (mirrors `crate::quadrant`, [FR-UI-17]) ──────────────────────────

/// The Quadrant bundle ([FR-UI-17], CR-036): the reachability×coverage cross
/// read-model plus the hotspot board supplying urgency weight. The `hotspots`
/// field is `Option` — the view degrades to an unweighted urgency on a read fault
/// rather than blanking the page ([NFR-CC-04]).
#[derive(Debug, Serialize)]
pub(crate) struct QuadrantModel {
    status: StatusInfo,
    cross: CoverageCrossReport,
    hotspots: Option<HotspotReport>,
}

/// `GET /api/v1/quadrant` — the Quadrant cross read-model ([FR-UI-21]). The SPA
/// computes the scatter points client-side from this cross; the legacy
/// `/api/quadrant` (presentation-shaped scatter points) stays wired for the
/// server-rendered view's `quadrant.js`.
pub(crate) async fn quadrant(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_quadrant", |e| -> anyhow::Result<QuadrantModel> {
        Ok(QuadrantModel {
            status: e.status(),
            cross: e.coverage_cross()?,
            hotspots: e.latest_hotspots(None, false).ok(),
        })
    })
    .await;
    respond(model)
}

// ── Graph (subsumes `/api/graph`, [FR-UI-08]/[FR-UI-15]/[FR-UI-16]) ───────────

/// `GET /api/v1/graph` — the read-only nodes+edges snapshot the interactive canvas
/// consumes ([FR-UI-08], [ADR-29]). Identical contract to the legacy `/api/graph`:
/// `?seed=` scopes, `?cap=` bounds, `?layers=`/`?edge_types=` re-budget,
/// `?granularity=` selects the cluster-zoom tier, `?intent=` toggles the
/// documentation-intent overlay. A pure reader — the fetch mutates no store
/// ([ADR-28]). [`GraphElements`] is infallible at the surface (a read fault
/// degrades to the honest empty snapshot in the read-model itself).
pub(crate) async fn graph(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let seed = q.get("seed").cloned().filter(|s| !s.is_empty());
    let cap = q.get("cap").and_then(|c| c.parse::<usize>().ok());
    let layers = parse_layers(&q);
    let edge_types = parse_edge_types(&q);
    let granularity = parse_granularity(&q);
    let intent = parse_intent(&q);
    let elements: GraphElements = bridge(engine, "api_v1_graph", move |e| {
        e.graph_elements(seed.as_deref(), cap, layers.as_deref(), edge_types.as_deref(), granularity, intent)
    })
    .await;
    ok(elements)
}

// ── Query (subsumes `/api/query`, [FR-UI-14]) ─────────────────────────────────

/// `GET /api/v1/query` — the read-only structured + relational whole-graph query
/// ([FR-UI-14], [ADR-35]). Reuses the same [`query::run`] composition the legacy
/// `/api/query` endpoint serves: a `verb` (`callers-of`/`callees-of`/`impact-of`)
/// with a `target` runs the relational form, otherwise a `q` term with optional
/// `kind`/`layer`/`file` filters runs the ranked filter form. Pure composition of
/// read-only read-models — no new engine primitive ([ADR-35]); an empty result is
/// an honest `200` no-matches payload, never an error ([NFR-CC-04]).
pub(crate) async fn search_query(
    State(engine): State<Arc<Engine>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let response: QueryResponse = bridge(engine, "api_v1_query", move |e| query::run(e, &params)).await;
    ok(response)
}

// ── Impact / Decisions (subsumes the HTML `/api/impact`, [FR-NV-10]/[FR-DG-02]) ─

/// `GET /api/v1/impact?seed=<symbol>` — the read-only transitive-impact + doc-trace
/// read-model ([FR-NV-10], S-037) the SPA's Decisions & docs panel ([FR-DG-02])
/// consumes. The JSON twin of the legacy HTML `/api/impact` fragment endpoint:
/// both read the **same** [`Engine::impact`] accessor, so the panel is now built
/// client-side from the read-model (the SPA owns the presentation) instead of a
/// server-rendered HTML fragment. A pure reader — the fetch mutates no store
/// ([ADR-28]). [`ImpactResult`] is infallible at the surface: an unknown seed
/// resolves to an honest empty read-model carrying `suggestions`, never a `404`
/// ([NFR-CC-04]). An absent/empty `seed` is the honest empty default (`200`), so
/// the panel renders its opening prompt without an error.
pub(crate) async fn impact(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(seed) = q.get("seed").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
    else {
        return ok(ImpactResult::default());
    };
    let result: ImpactResult = bridge(engine, "api_v1_impact", move |e| e.impact(&seed, None)).await;
    ok(result)
}

// ── Node (single-symbol detail, [FR-NV-04]) ───────────────────────────────────

/// `GET /api/v1/node?symbol=<sym>[&code=1]` — the full node read-model for one
/// symbol ([FR-NV-04]): metadata, immediate edges, and (with `?code=1`) the source
/// excerpt. [`NodeInfo`] is infallible at the surface — an unknown symbol resolves
/// to an honest empty read-model carrying `warnings`, never a `404` ([NFR-CC-04]).
/// A missing/empty `symbol` is a client error (`400`).
pub(crate) async fn node(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(symbol) = q.get("symbol").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
    else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `symbol` query parameter is required".into() }))
            .into_response();
    };
    let include_code = truthy(q.get("code"));
    let info: NodeInfo = bridge(engine, "api_v1_node", move |e| e.node(&symbol, include_code)).await;
    ok(info)
}

// ── Search (whole-graph FTS, [FR-NV-01]) ──────────────────────────────────────

/// `GET /api/v1/search?q=<term>[&kind=<k>][&limit=<n>]` — the ranked whole-graph
/// symbol search read-model ([FR-NV-01]). [`SearchResult`] is infallible at the
/// surface (a failure degrades to an empty result with `warnings`); a
/// missing/empty `q` is a client error (`400`). An unrecognised `kind` is dropped
/// (the search runs unfiltered), matching the read-model's lenient contract.
pub(crate) async fn search(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(term) = q.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string) else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `q` query parameter is required".into() }))
            .into_response();
    };
    let kind = q.get("kind").and_then(|k| NodeKind::from_wire(k.trim()));
    let limit = q.get("limit").and_then(|n| n.parse::<usize>().ok());
    let result: SearchResult = bridge(engine, "api_v1_search", move |e| e.search(&term, kind, limit)).await;
    ok(result)
}

// ── Wiki (index / search / page, [FR-UI-06]/[FR-WK-05]) ───────────────────────

/// `GET /api/v1/wiki` — the dual-axis `wiki status` read-model ([FR-UI-06],
/// [FR-WK-12]): per-anchor freshness counts, the revision-stale count, and the
/// revision they were computed at. A documented pure read that never prunes
/// ([ADR-28]).
pub(crate) async fn wiki_index(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_wiki", |e| -> anyhow::Result<WikiStatus> {
        e.wiki_status()
    })
    .await;
    respond(model)
}

/// `GET /api/v1/wiki/search?q=<term>` — the FTS search over the wiki ([FR-WK-05]).
/// Each hit carries its staleness flag exactly as `wiki search` reports it;
/// read-only — no `wiki.db` write ([ADR-28]). An empty `q` is an honest empty list
/// (`200`), never an error.
pub(crate) async fn wiki_search(
    State(engine): State<Arc<Engine>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let term = q.get("q").cloned().unwrap_or_default().trim().to_string();
    let model = bridge(engine, "api_v1_wiki_search", move |e| -> anyhow::Result<Vec<WikiHit>> {
        if term.is_empty() {
            Ok(Vec::new())
        } else {
            e.wiki_search(&term, false)
        }
    })
    .await;
    respond(model)
}

/// The agent wiki-page **presentation bundle** ([FR-UI-06], S-189) the migrated SPA
/// reader mounts. It carries the page's provenance + per-anchor freshness verbatim
/// from the [`WikiPage`] read-model, **plus the server-rendered, already-safe HTML
/// body** (`rendered_html`) and the derived freshness verdict (`regen_pending`).
///
/// The Markdown→HTML render is done **server-side** by the same [`crate::markdown`]
/// (comrak) the legacy view uses — so the XSS-neutralization boundary stays on the
/// server (raw HTML / dangerous URLs dropped; ` ```mermaid ` fences rewritten to
/// `.mermaid` blocks the client renders) and the SPA mounts a string it can trust
/// (`dangerouslySetInnerHTML`) without shipping a client Markdown engine. The
/// single page `h1` is rendered from `title`; a body that opens by repeating the
/// title is suppressed before render, so the title shows exactly once
/// ([FR-UI-06] single-title).
#[derive(Debug, Serialize)]
pub(crate) struct WikiPageView {
    slug: String,
    title: String,
    /// The server-rendered, comrak-sanitized HTML body — empty for a `placeholder`.
    rendered_html: String,
    /// A known scaffold slug with no agent prose yet renders the honest "not yet
    /// generated" placeholder (`200`), never a fabricated body ([NFR-CC-04]).
    placeholder: bool,
    /// Provenance — `null` on a placeholder (no page written yet).
    generator: Option<String>,
    written_head: Option<String>,
    marker: Option<&'static str>,
    built_at_revision: Option<u64>,
    /// Per-anchor freshness; empty for an unanchored (overview) or placeholder page.
    anchors: Vec<AnchorProvenance>,
    stale: bool,
    has_missing: bool,
    /// Derived "stale — regeneration pending": the graph advanced past the page's
    /// built-at revision ([FR-WK-12]). Computed on read, written nowhere ([ADR-28]).
    regen_pending: bool,
    /// The graph revision the verdict was derived against (the banner's "graph now").
    current_revision: u64,
}

/// `GET /api/v1/wiki/page/*slug` — the agent wiki-page presentation bundle
/// ([FR-UI-06], S-189): provenance, per-anchor freshness, and the **server-rendered,
/// already-safe HTML body** the SPA reader mounts (comrak does the XSS
/// neutralization server-side; the SPA renders the `.mermaid` blocks client-side as
/// today). A present page is `200`; a known **scaffold** slug with no prose yet is a
/// `200` placeholder (the honest "not yet generated" state, [NFR-CC-04]); any other
/// unknown slug is an honest `404` — never a fabricated body. Read-only ([ADR-28]).
pub(crate) async fn wiki_page(
    State(engine): State<Arc<Engine>>,
    Path(slug): Path<String>,
) -> Response {
    let model = bridge(engine, "api_v1_wiki_page", move |e| -> anyhow::Result<WikiPageOutcome> {
        let current_revision = e.status().graph_revision;
        match e.wiki_read(&slug)? {
            Some(page) => {
                let regen_pending =
                    logos_core::wiki::revision_pending(page.built_at_revision, current_revision);
                // Render the title-suppressed body server-side (the comrak safety
                // boundary, [FR-UI-06] single-title); the SPA mounts the result.
                let rendered_html = crate::markdown::render(
                    &crate::markdown::suppress_leading_title_heading(&page.body, &page.title),
                );
                Ok(WikiPageOutcome::Page(Box::new(WikiPageView {
                    slug: page.slug,
                    title: page.title,
                    rendered_html,
                    placeholder: false,
                    generator: Some(page.generator),
                    written_head: Some(page.written_head),
                    marker: Some(page.marker),
                    built_at_revision: Some(page.built_at_revision),
                    anchors: page.anchors,
                    stale: page.stale,
                    has_missing: page.has_missing,
                    regen_pending,
                    current_revision,
                })))
            }
            // A known scaffold slug with no prose yet is the honest placeholder
            // (200), mirroring the server-rendered reader; any other slug is 404.
            // A User Guide `guide/*` slug ([FR-WK-23]) is not in the fixed
            // `scaffold_label` set (its file set is dynamic, per project), so it is
            // checked separately against the current `docs/howto/*.md` files before
            // falling back to 404 — a page `wiki materialize` has not yet written
            // still lands on an honest placeholder, never a 404 ([NFR-CC-04]).
            None => {
                let guide_label = e
                    .wiki_guide_pages()
                    .into_iter()
                    .find(|(guide_slug, _)| *guide_slug == slug)
                    .map(|(_, label)| label);
                match guide_label.or_else(|| crate::wiki::scaffold_label(&slug).map(str::to_string)) {
                    Some(label) => Ok(WikiPageOutcome::Placeholder(Box::new(WikiPageView {
                        slug,
                        title: label,
                        rendered_html: String::new(),
                        placeholder: true,
                        generator: None,
                        written_head: None,
                        marker: None,
                        built_at_revision: None,
                        anchors: Vec::new(),
                        stale: false,
                        has_missing: false,
                        regen_pending: false,
                        current_revision,
                    }))),
                    None => Ok(WikiPageOutcome::Missing(slug)),
                }
            }
        }
    })
    .await;
    match model {
        Ok(WikiPageOutcome::Page(view)) | Ok(WikiPageOutcome::Placeholder(view)) => ok(*view),
        Ok(WikiPageOutcome::Missing(slug)) => (
            StatusCode::NOT_FOUND,
            Json(ApiError { error: format!("no wiki page at `{slug}`") }),
        )
            .into_response(),
        Err(err) => fail(err),
    }
}

/// The three outcomes of a wiki-page read: a present page, an honest placeholder
/// for a known scaffold slug, or a genuine miss (`404`). Boxed views keep the
/// variants size-balanced (clippy `large_enum_variant`).
enum WikiPageOutcome {
    Page(Box<WikiPageView>),
    Placeholder(Box<WikiPageView>),
    Missing(String),
}

// ── Wiki doc-asset serving (same-origin, path-sandboxed, [FR-WK-27]/[ADR-58]) ──

/// The repo-relative **doc roots** the asset route serves image files from — the
/// only directories a request may resolve into ([FR-WK-27]). Their *canonicalized*
/// form is the structural containment boundary ([NFR-SE-04], the read-only source
/// sandbox's posture).
const DOC_ASSET_ROOTS: &[&str] = &["docs/specs", "docs/howto"];

/// Map a filename extension to the image content-type the asset route serves, or
/// `None` for a non-image file ([FR-WK-27]: "only image content-types are served").
/// Kept in lockstep with the transform's allow-list
/// (`logos_core::wiki::present::is_image_path`). An actual `.<ext>` is required — a
/// dotless name is never an image.
fn image_content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

/// The outcome of resolving a doc-asset request: a served image, or one of the two
/// distinct refusals the [`wiki_asset`] route reports.
enum AssetOutcome {
    /// The sandboxed, image-typed file — its content-type and bytes.
    Image(&'static str, Vec<u8>),
    /// The request is not an image file — refused (`415`, only images are served).
    NotImage,
    /// The path escapes the doc roots, or the file is absent/unreadable — refused
    /// (`404`, leaking nothing about what lies outside the sandbox).
    Denied,
}

/// Resolve a doc-asset request **structurally** ([FR-WK-27], [NFR-SE-04]): serve an
/// image file only when it canonicalizes to a path **inside** a canonicalized doc
/// root.
///
/// The containment check is a **canonicalized-prefix** test — never string matching.
/// [`std::fs::canonicalize`] resolves every `.`/`..` segment and symlink and fails on
/// an absent file, so the real target path is what [`FsPath::starts_with`]
/// (component-wise) is tested against a real doc root: a `..` traversal, an absolute
/// path, or a symlink pointing outside all resolve away *before* the prefix test and
/// fail it. The doc roots are canonicalized too, so a symlink in the repo prefix
/// (e.g. macOS `/tmp` → `/private/tmp`) cannot cause a spurious mismatch.
fn resolve_doc_asset(root: &FsPath, rel_path: &str) -> AssetOutcome {
    // Only image files are ever served — a cheap gate before any filesystem work.
    let Some(mime) = image_content_type(rel_path) else {
        return AssetOutcome::NotImage;
    };
    // Canonicalize the allowed roots, skipping one that is absent in this project.
    let allowed: Vec<PathBuf> =
        DOC_ASSET_ROOTS.iter().filter_map(|r| std::fs::canonicalize(root.join(r)).ok()).collect();
    // Canonicalize the requested target — resolves `..`/`.`/symlinks, errors if absent.
    let Ok(full) = std::fs::canonicalize(root.join(rel_path)) else {
        return AssetOutcome::Denied;
    };
    // Structural containment: the real target must sit within a real doc root.
    if !allowed.iter().any(|base| full.starts_with(base)) {
        return AssetOutcome::Denied;
    }
    match std::fs::read(&full) {
        Ok(bytes) => AssetOutcome::Image(mime, bytes),
        // A directory or unreadable node with an image-looking name → refused.
        Err(_) => AssetOutcome::Denied,
    }
}

/// `GET /api/v1/wiki/asset/*path` — the same-origin, read-only **doc-image asset
/// route** ([FR-WK-27], [ADR-58]) that presented pages' `<img src>` values resolve to
/// (rewritten by the [FR-WK-25] transform, `logos_core::wiki::present::rewrite_refs`).
/// It serves image files from the doc roots (`docs/specs/**`, `docs/howto/**`) only,
/// **path-sandboxed** by canonicalized-prefix containment ([`resolve_doc_asset`],
/// [NFR-SE-04]): a `..`/absolute/symlink escape is an honest `404`, a non-image path
/// a `415`, and only image content-types are served. A served image also carries
/// `X-Content-Type-Options: nosniff`, so a browser honors the declared image type and
/// never MIME-sniffs a doc file into an active document (defense in depth for the
/// `image/svg+xml` type, atop the unchanged self-only CSP). Read-only — the fetch
/// mutates no store ([ADR-28]); the assets are **same-origin**, so the self-only CSP
/// is byte identical (no `data:` inlining, no external host).
///
/// The blocking filesystem read runs on the pool via the [`bridge`](crate::bridge)
/// ([ADR-03]), never the current-thread serve loop.
///
/// [FR-WK-27]: ../../docs/specs/requirements/FR-WK-27.md
/// [FR-WK-25]: ../../docs/specs/requirements/FR-WK-25.md
/// [NFR-SE-04]: ../../docs/specs/requirements/NFR-SE-04.md
/// [ADR-58]: ../../docs/specs/architecture/decisions/ADR-58.md
/// [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md
/// [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
pub(crate) async fn wiki_asset(
    State(engine): State<Arc<Engine>>,
    Path(rel_path): Path<String>,
) -> Response {
    let outcome =
        bridge(engine, "api_v1_wiki_asset", move |e| resolve_doc_asset(e.root(), &rel_path)).await;
    match outcome {
        AssetOutcome::Image(mime, bytes) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "no-cache"),
                // Honor the declared image type; never sniff a doc file into an active
                // document (belt-and-suspenders for `image/svg+xml`, atop the CSP).
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        AssetOutcome::NotImage => (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the wiki asset route serves image files only",
        )
            .into_response(),
        AssetOutcome::Denied => (StatusCode::NOT_FOUND, "no such wiki asset").into_response(),
    }
}

// ── Wiki navigation IA (four-tier menu, [FR-UI-06]) ───────────────────────────

/// One leaf of a {@link WikiNavTier}: a page link by its fixed slug + label.
#[derive(Debug, Serialize)]
pub(crate) struct WikiNavItem {
    slug: String,
    label: String,
}

/// One tier of the wiki menu — its title and its discrete page links.
#[derive(Debug, Serialize)]
pub(crate) struct WikiNavTier {
    title: String,
    items: Vec<WikiNavItem>,
}

/// The wiki menu **information architecture** ([FR-UI-06], CR-034/CR-035/CR-039/
/// CR-062) the SPA renders: **Summary / User Guide / Design / Specs** tiers plus a
/// top-level Search link — the User Guide tier present only when
/// [`Engine::wiki_guide_pages`] is non-empty ([FR-WK-23]). Composed from the
/// **same** [`crate::wiki`] constants the server-rendered menu uses (the
/// `DocCategory` slug/title contract + the Summary scaffold), so the two menus can
/// never disagree; Frontend Design appears in Design only when its source doc
/// exists (read through the engine, never a filesystem scan).
#[derive(Debug, Serialize)]
pub(crate) struct WikiNav {
    tiers: Vec<WikiNavTier>,
    /// The top-level Search link label (a sibling of the tiers, CR-039).
    search_label: String,
}

/// `GET /api/v1/wiki/nav` — the wiki menu IA ([FR-UI-06], S-189, CR-062). A pure
/// read: the fixed Summary/Design/Specs slug contracts, a single engine read for
/// Frontend-Design presence, and a single engine read for the dynamic User Guide
/// page set; it touches no store ([ADR-28]).
pub(crate) async fn wiki_nav(State(engine): State<Arc<Engine>>) -> Response {
    use crate::wiki::{
        DESIGN_DOCS, GUIDED_TOUR, OVERVIEW_ARCHITECTURE, OVERVIEW_ARCHITECTURE_LABEL, SPECS_DOCS,
        SPECS_SRS, SPECS_SRS_LABEL, USER_GUIDE_TIER_TITLE,
    };
    let nav = bridge(engine, "api_v1_wiki_nav", |e| {
        let item = |slug: &str, label: &str| WikiNavItem { slug: slug.into(), label: label.into() };
        let doc_item = |c: DocCategory| WikiNavItem { slug: c.slug().into(), label: c.title().into() };

        // Summary — the agent-tier Overview scaffold. The architecture narrative
        // left this tier (CR-062/ADR-57): it is now the presented
        // docs/specs/architecture.md under Design, so GUIDED_TOUR no longer carries
        // it and no filter is needed here.
        let summary = GUIDED_TOUR.iter().map(|(slug, label)| item(slug, label)).collect();

        // User Guide — one page per docs/howto/*.md file (CR-062, [FR-WK-23]);
        // absent entirely when docs/howto/ has no files, never an empty tier.
        let guide_items: Vec<WikiNavItem> =
            e.wiki_guide_pages().into_iter().map(|(slug, label)| item(&slug, &label)).collect();
        let user_guide = (!guide_items.is_empty())
            .then(|| WikiNavTier { title: USER_GUIDE_TIER_TITLE.into(), items: guide_items });

        // Design — the presented Architecture page, the consolidated ADRs/Components/
        // Integrations docs, and Frontend Design only when its source doc exists.
        let mut design = vec![item(OVERVIEW_ARCHITECTURE, OVERVIEW_ARCHITECTURE_LABEL)];
        design.extend(DESIGN_DOCS.iter().map(|&c| doc_item(c)));
        if e.wiki_doc_category_present(DocCategory::FrontendDesign) {
            design.push(doc_item(DocCategory::FrontendDesign));
        }

        // Specs — the presented SRS hub (CR-064, [FR-WK-26]) first, then the
        // consolidated requirement / UAT documents.
        let mut specs = vec![item(SPECS_SRS, SPECS_SRS_LABEL)];
        specs.extend(SPECS_DOCS.iter().map(|&c| doc_item(c)));

        let mut tiers = vec![WikiNavTier { title: "Summary".into(), items: summary }];
        tiers.extend(user_guide);
        tiers.push(WikiNavTier { title: "Design".into(), items: design });
        tiers.push(WikiNavTier { title: "Specs".into(), items: specs });

        WikiNav { tiers, search_label: "Search".into() }
    })
    .await;
    ok(nav)
}

// ── Config-read (masked key only, [FR-UI-12]/[FR-CF-06]) ──────────────────────

/// `GET /api/v1/config` — the config read-model ([FR-UI-12], [ADR-31]): the current
/// `config.toml`/`rules.toml` documents plus the **masked** chat key (presence +
/// last-4 only — masked by construction in [`ConfigReadModel`], the raw secret is
/// never serialized; [FR-CF-06], [NFR-SE-07]). A pure filesystem read — it touches
/// no graph store, so a load mutates nothing ([FR-UI-03], [ADR-28]).
pub(crate) async fn config(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_config", |e| -> anyhow::Result<ConfigReadModel> {
        e.config_read()
    })
    .await;
    respond(model)
}

// ── Deep verify (the one intent-guarded read-model POST, [FR-UI-25]/[FR-GV-19]) ─

/// `POST /api/v1/verify` — the on-demand **deep graph-consistency check**
/// ([FR-UI-25], [FR-GV-19], [ADR-46]): reindex the project into a throwaway shadow
/// store via the always-purge `index` path, then diff node/edge/file counts and
/// symbol sets against the **read-only** live graph, returning the
/// [`VerifyReport`] verbatim as JSON. `ok:true` on a clean store; on drift the
/// body carries the live-vs-reindex deltas and a capped leaked/orphaned-symbol
/// sample the Config-tab control ([spa-frontend], S-207) renders.
///
/// Unlike every other `/api/v1` endpoint this is a **`POST`**, not a `GET`: it
/// rides the enumerated mutating-method slot ([`VERIFY_POST_ROUTE`](crate::VERIFY_POST_ROUTE))
/// so it carries the same-origin + per-session intent-token proof the
/// [`intent_guard`](crate) already enforces on every `POST` ([NFR-SE-06],
/// [ADR-31]) — a `GET` could not. It is nonetheless a **read-model action**: the
/// shadow reindex reads the project tree and writes only its own throwaway store,
/// the live store is opened read-only for the census, and no external origin is
/// dialed ([NFR-RA-05], [NFR-SE-01]).
///
/// The seconds-to-minutes reindex runs on the blocking pool via the
/// [`bridge`](crate::bridge) ([ADR-03] `spawn_blocking`), so the current-thread
/// serve loop stays free to answer concurrent reads while a verify is in flight —
/// the [ADR-46] risk-register mitigation. A genuine engine/reindex fault is an
/// honest `500` ([`ApiError`]), never a fabricated `CONSISTENT` ([NFR-RA-05]).
///
/// [FR-UI-25]: ../../docs/specs/requirements/FR-UI-25.md
/// [FR-GV-19]: ../../docs/specs/requirements/FR-GV-19.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
/// [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
/// [ADR-46]: ../../docs/specs/architecture/decisions/ADR-46.md
/// [spa-frontend]: ../../docs/specs/architecture/components/spa-frontend.md
pub(crate) async fn verify(State(engine): State<Arc<Engine>>) -> Response {
    let model = bridge(engine, "api_v1_verify", |e| -> anyhow::Result<VerifyReport> {
        e.verify()
    })
    .await;
    respond(model)
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Is an optional query value truthy? Matches the canvas's `?intent=` contract:
/// `1`/`true`/`on`/`yes` (case-insensitive). Absent/empty/unrecognised ⇒ false.
pub(crate) fn truthy(raw: Option<&String>) -> bool {
    raw.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_untested_reads_presence_and_explicit_off() {
        let on = HashMap::from([("untested".to_string(), String::new())]);
        let one = HashMap::from([("untested".to_string(), "1".to_string())]);
        let off = HashMap::from([("untested".to_string(), "0".to_string())]);
        let absent: HashMap<String, String> = HashMap::new();
        assert!(wants_untested(&on), "bare ?untested is on");
        assert!(wants_untested(&one), "?untested=1 is on");
        assert!(!wants_untested(&off), "?untested=0 is off");
        assert!(!wants_untested(&absent), "absent is off");
    }

    #[test]
    fn truthy_accepts_the_canonical_truthy_tokens_only() {
        for t in ["1", "true", "TRUE", "on", "Yes"] {
            assert!(truthy(Some(&t.to_string())), "{t} is truthy");
        }
        for f in ["0", "false", "", "off", "no", "maybe"] {
            assert!(!truthy(Some(&f.to_string())), "{f} is not truthy");
        }
        assert!(!truthy(None), "absent is not truthy");
    }
}
