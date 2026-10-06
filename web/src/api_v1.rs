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
//! pre-existing `/api/*` endpoints (graph-elements, graph query, impact)
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
    extract::{Form, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use logos_core::config::{self as core_config, ConfigReadModel, TierSaveOutcome};
use logos_core::federation::{
    app_wide_reachability, open_state, query as fed_query, workspace_governance,
    workspace_statistics, xservice_build_deps, Backing, BoundedReachability, BuildDependencies,
    ContractBridge, DegradedRollup, EngineRegistry, Federation, ReachabilityScope,
    WorkspaceGovernance, WorkspaceStatistics,
};
use logos_core::federation::manifest::{self, ManifestDocument, ManifestSaveOutcome};
use logos_core::history::{CoverageStatus, HotspotReport, TemporalReport};
use logos_core::model::NodeKind;
use logos_core::observability::Surface;
use logos_core::models::navigation::{
    BranchOverlapResult, GraphElements, ImpactIntersectionResult, ImpactResult,
    LanguageComposition, NodeInfo, PrecedentResult, SearchResult, StatusInfo,
};
use logos_core::models::quality::{
    DsmReport, EvolutionReport, GateResult, LanguagesInfo, LatestHealth, RulesReport, ScanResult,
    StatsInfo, VerifyReport,
};
use logos_core::wiki::{AnchorProvenance, DocCategory, WikiHit, WikiPage, WikiStatus};
use logos_core::Engine;

use crate::member::{workspace_root_of, MemberEngine, WorkspaceRoot};
use crate::query::QueryResponse;
use crate::{
    bridge, config_write_status, parse_edge_types, parse_granularity, parse_intent, parse_layers,
    query, run_blocking,
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
/// `composition` defaults on a read fault — mirroring the view's own per-widget
/// degradation ([NFR-CC-04]); a genuinely-required read failing aborts the whole
/// bundle to a `500`.
#[derive(Debug, Serialize)]
pub(crate) struct OverviewModel {
    status: StatusInfo,
    composition: LanguageComposition,
    languages: LanguagesInfo,
    gate: GateResult,
    coverage: CoverageStatus,
    /// The architecture-rules report ([FR-GV-02]) backing the Dashboard's
    /// Rule-findings widget: green when `passed`, red on violations, muted
    /// onboarding when no `rules.toml` (`rules_present == false`).
    rules: RulesReport,
    stats: StatsInfo,
    /// The agent Project-Overview wiki page, or `null` when none is written yet
    /// (an honest absence, not an error — [NFR-CC-04]).
    overview_page: Option<WikiPage>,
}

/// `GET /api/v1/overview` — the Dashboard data ([FR-UI-09], [FR-UI-21]).
pub(crate) async fn overview(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_overview", Surface::Web, |e| -> anyhow::Result<OverviewModel> {
        Ok(OverviewModel {
            status: e.status(),
            // The view degrades a composition-read fault to the empty card, never
            // blanks the page — mirror that here ([NFR-CC-04]).
            composition: e.language_composition().unwrap_or_default(),
            languages: e.languages(),
            gate: e.latest_gate()?,
            coverage: e.coverage_status()?,
            rules: e.check_rules(None, false)?,
            stats: e.stats(None),
            overview_page: e.wiki_read(crate::wiki::PROJECT_OVERVIEW_SLUG)?,
        })
    })
    .await;
    respond(model)
}

// ── Health (mirrors `crate::health`, [FR-UI-04]) ──────────────────────────────

/// The Health bundle ([FR-UI-04]): the read-only gate verdict, the last persisted
/// scan (metrics + temporal tier), and the snapshot-series evolution — all
/// **read-only** twins so a load writes no `metric_snapshots` row ([ADR-28]).
///
/// `gate` and `scan` are projections of **one** read of the last persisted
/// snapshot ([CR-135] §3.2), so within a single response they can never describe
/// two generations. The shape is unchanged by that guarantee: the pair is
/// destructured at the handler, and the serialized DTO is byte-identical.
///
/// [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
#[derive(Debug, Serialize)]
pub(crate) struct HealthModel {
    status: StatusInfo,
    gate: GateResult,
    scan: ScanResult,
    evolution: EvolutionReport,
}

/// `GET /api/v1/health` — the Health data ([FR-UI-04], [FR-UI-21]).
///
/// # One read, two projections ([CR-135] §3.2)
///
/// The gate verdict and the scan result come from
/// [`Engine::latest_health`](logos_core::Engine::latest_health) — a single read
/// of the last persisted snapshot — and **not** from `latest_gate()` plus
/// `latest_scan()`. Those two each read that same logical row, so calling both
/// here opened two reads in two transactions: a `scan` committing between them
/// left one card describing the older generation and the other the newer, with
/// nothing in the payload saying so. That was reproduced during [S-406]'s review
/// as a no-signal callout beside a fully populated quality grid.
///
/// `status` and `evolution` are read once each and stay separate reads: neither
/// is an operand of the verdict, so joining them to the snapshot would buy no
/// atomicity, only a wider read. They are no longer entirely unrelated to it,
/// though — the staleness label below takes `status.indexed` as its discriminant
/// and its date from `evolution` — so what that costs is stated there rather
/// than left to be rediscovered.
/// `the_health_handler_reads_the_snapshot_once` (in [`crate`]'s test module)
/// pins the composition.
///
/// # What the staleness label reads ([CR-135] §3.2, §8)
///
/// The SPA labels a *populated* verdict as history when `status.indexed` is
/// false, and dates that label from `evolution.snapshots`' **last** point — the
/// same `metric_snapshots` row the pair is projected from (the series is emitted
/// `ORDER BY id` and windowed to its tail; the snapshot is read
/// `ORDER BY id DESC LIMIT 1`). The CR's §8 assumption — that the timestamp
/// needed to date the label is already in the payload, with no schema change —
/// is therefore **discharged as confirmed**, and no field was added here: both
/// facts the classification needs (`status.indexed` and that date) are already
/// serialized. The separate reads stay separate: the date qualifies the figures,
/// it is never an operand of the verdict, so a concurrent `scan` can move the
/// date and never the figures or the verdict.
///
/// The window is worth naming precisely rather than waving at, because it is
/// wider than "before the snapshot is read": this handler takes **three** reads
/// in source order — snapshot, `status`, `evolution` — so a `scan` can land in
/// either gap, and `scan` reconciles before it scores ([ADR-11]). One landing
/// between the `status` read and the `evolution` read can therefore re-index the
/// graph *and* persist a newer row, leaving this one response labelled stale on
/// the older `status` while dated from the newer row. Bounded (the date only),
/// self-correcting on the next load, and the accepted price of [CR-135] §3.3
/// leaving `status`/`evolution` outside the single-read seam. The named remedy,
/// if that is ever judged too high, is a computed `snapshot_at` on
/// [`HealthModel`] derived from the evolution report this handler already holds
/// — no extra read, no change to the seam.
///
/// [ADR-11]: ../../docs/specs/architecture/decisions/ADR-11.md
/// [S-406]: ../../docs/planning/journal.md#s-406-a-readout-names-a-step-that-can-change-what-it-reports
/// [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
pub(crate) async fn health(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_health", Surface::Web, |e| -> anyhow::Result<HealthModel> {
        let LatestHealth { gate, scan } = e.latest_health()?;
        Ok(HealthModel {
            status: e.status(),
            gate,
            scan,
            evolution: e.evolution(None)?,
        })
    })
    .await;
    respond(model)
}

// ── Status (the FR-NV-07 index-health read-model, [FR-UI-34]) ─────────────────

/// `GET /api/v1/status` — the [FR-NV-07] index-health read-model ([FR-UI-21],
/// [FR-UI-34], [CR-097]), serving the app header's graph-state readout
/// (`graph_revision`, `node_count`, `edge_count`).
///
/// An **ordinary member** of this surface, deliberately: a thin [`bridge`]
/// projection of [`Engine::status`], member-scoped through the same
/// [`MemberEngine`] extractor as its siblings, introducing **no new core query**
/// ([ADR-01], [ADR-43]) and reading through the non-persisting accessor so a load
/// writes no store ([ADR-28], [FR-UI-03]). It carries the status projection and
/// nothing else — the header previously set one boolean by fetching the ~1 MB
/// [FR-UI-04] Health bundle and discarding all but its 831-byte `status` block;
/// the Health bundle itself is unchanged and remains the Health view's own payload.
///
/// [`StatusInfo`] is infallible at the surface, so this pairs `bridge` with `ok`
/// rather than the fallible `respond`, exactly as [`statistics`] does. An
/// un-indexed project is reported honestly as `indexed: false` for the client to
/// render as a not-indexed state — never dressed up as a measurement ([NFR-CC-04],
/// [NFR-RA-05]).
///
/// # The one handler on this surface that is not [`Surface::Web`]
///
/// The app header re-issues this read on **every** client-side navigation
/// ([FR-UI-34]) — a request the user's own navigation caused incidentally, not
/// a question the user asked. So it names [`Surface::Shell`] at the `bridge`
/// and its events never enter a usage figure ([FR-OB-09] widened by [CR-097],
/// [BR-42]). They are still recorded: the exclusion re-attributes, it does not
/// destroy ([NFR-CC-04]).
///
/// Naming it **here** is the whole point. Classifying the engine's `status`
/// read instead would take `logos status` — a command a developer typed — down
/// with it, and would say nothing about the *next* chrome read, which may well
/// call a navigation tool. The test the classification applies is whose
/// question a request answers, and only the adapter is in a position to know.
///
/// # The assumption this rests on: one route, one caller
///
/// Keying the classification to the **route** is a proxy for keying it to the
/// caller, and the two agree only while this endpoint has exactly one consumer.
/// It does today — the app header is the sole caller in the SPA — but the
/// proxy is what would break first. This is the cheapest read-model on the
/// surface, which makes it the likeliest to be reused, and a second consumer
/// asking a genuine question would have its reads silently excluded with
/// nothing failing. If one appears, it needs its own route (or the
/// classification needs to move to the caller), not a second caller here.
/// `every_handler_names_its_surface_and_only_status_names_the_shell` guards the
/// handler side of this; the consumer side is an assumption, stated here
/// because nothing enforces it.
///
/// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
/// [CR-097]: ../../docs/requests/CR-097-header-graph-state-readout.md
/// [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
pub(crate) async fn status(MemberEngine(engine): MemberEngine) -> Response {
    let info: StatusInfo = bridge(engine, "api_v1_status", Surface::Shell, |e| e.status()).await;
    ok(info)
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
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let window = q.get("window").and_then(|w| w.trim().parse::<u32>().ok());
    let info: StatsInfo = bridge(engine, "api_v1_statistics", Surface::Web, move |e| e.stats(window)).await;
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
pub(crate) async fn architecture(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_architecture", Surface::Web, |e| -> anyhow::Result<ArchitectureModel> {
        Ok(ArchitectureModel { status: e.status(), dsm: e.dsm(None, false)? })
    })
    .await;
    respond(model)
}

// ── Gaps (mirrors `crate::gaps`, [FR-UI-04]) ──────────────────────────────────

/// The Rule-findings bundle ([FR-UI-04]): the architecture-rules report
/// ([FR-GV-02]). Read-only.
#[derive(Debug, Serialize)]
pub(crate) struct GapsModel {
    status: StatusInfo,
    rules: RulesReport,
}

/// `GET /api/v1/gaps` — the Rule-findings data ([FR-UI-21]).
pub(crate) async fn gaps(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_gaps", Surface::Web, |e| -> anyhow::Result<GapsModel> {
        Ok(GapsModel {
            status: e.status(),
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

/// `GET /api/v1/files[?untested][?production_scope]` — the Files & Risk data
/// ([FR-UI-21]). The `?untested` toggle scopes the board to files lacking fresh
/// positive coverage, exactly as the server-rendered view's filter does; the
/// `?production_scope` toggle drops whole test files from the candidate set
/// before ranking ([FR-UI-05], [CR-076]) — the same optional filter the CLI
/// `--production-scope` flag and MCP `production_scope` argument expose,
/// reached through the same [`Engine::latest_hotspots`] call.
pub(crate) async fn files(
    MemberEngine(engine): MemberEngine,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let untested = wants_flag(&params, "untested");
    let production_scope = wants_flag(&params, "production_scope");
    let model = bridge(engine, "api_v1_files", Surface::Web, move |e| -> anyhow::Result<FilesModel> {
        Ok(FilesModel {
            status: e.status(),
            hotspots: e.latest_hotspots(Some(50), untested, production_scope)?,
            temporal: e.latest_temporal_report()?,
        })
    })
    .await;
    respond(model)
}

/// `true` when the query requests a boolean toggle (bare `?key` or `?key=1`) —
/// the same extraction the server-rendered Files view uses for `?untested`, now
/// shared with `?production_scope` ([CR-076]).
fn wants_flag(params: &HashMap<String, String>, key: &str) -> bool {
    params.get(key).is_some_and(|v| v.is_empty() || v != "0")
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
pub(crate) async fn coverage(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_coverage", Surface::Web, |e| -> anyhow::Result<CoverageModel> {
        Ok(CoverageModel {
            status: e.status(),
            coverage: e.coverage_status()?,
            untested: e.latest_hotspots(Some(20), true, false)?,
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
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let seed = q.get("seed").cloned().filter(|s| !s.is_empty());
    let cap = q.get("cap").and_then(|c| c.parse::<usize>().ok());
    let layers = parse_layers(&q);
    let edge_types = parse_edge_types(&q);
    let granularity = parse_granularity(&q);
    let intent = parse_intent(&q);
    let elements: GraphElements = bridge(engine, "api_v1_graph", Surface::Web, move |e| {
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
    MemberEngine(engine): MemberEngine,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let response: QueryResponse = bridge(engine, "api_v1_query", Surface::Web, move |e| query::run(e, &params)).await;
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
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(seed) = q.get("seed").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
    else {
        return ok(ImpactResult::default());
    };
    let result: ImpactResult = bridge(engine, "api_v1_impact", Surface::Web, move |e| e.impact(&seed, None)).await;
    ok(result)
}

/// `GET /api/v1/impact-intersection?item=<id>=<sym>[,<sym>]&item=…[&depth=<n>]`
/// — which planned work items collide, and on what ([FR-NV-11], CR-114).
///
/// The JSON twin of the `impact-intersection` CLI command and the MCP tool of
/// the same name: all three hand their raw `item` strings to the **one**
/// [`Engine::impact_intersection`] accessor, which owns the
/// `<id>=<symbol>,…` parse — so the three surfaces cannot drift in what they
/// accept ([ADR-01], [NFR-MA-02]). `item` repeats, which is why this handler
/// reads the query as ordered pairs rather than the map the other endpoints use.
///
/// A pure reader ([ADR-28]). Infallible at the surface: no `item` at all, a
/// malformed spec, or a symbol the graph does not know all resolve to an honest
/// read-model carrying its warnings and coverage limits — never a `404` or a
/// `400` ([NFR-CC-04]).
pub(crate) async fn impact_intersection(
    MemberEngine(engine): MemberEngine,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let items: Vec<String> = pairs
        .iter()
        .filter(|(key, _)| key == "item")
        .map(|(_, value)| value.clone())
        .collect();
    let depth = pairs
        .iter()
        .find(|(key, _)| key == "depth")
        .and_then(|(_, value)| value.parse::<usize>().ok());
    let result: ImpactIntersectionResult = bridge(engine, "api_v1_impact_intersection", Surface::Web, move |e| {
        e.impact_intersection(&items, depth)
    })
    .await;
    ok(result)
}

/// `GET /api/v1/precedent?target=<symbol|path>[&limit=<n>]` — nodes
/// structurally analogous to one symbol or file ([FR-NV-12], CR-114).
///
/// The JSON twin of the `precedent` CLI command and the MCP tool of the same
/// name: all three hand their raw target string to the **one**
/// [`Engine::precedent`] accessor, which owns the symbol-then-file resolution
/// rule, so the three surfaces cannot drift in what they accept ([ADR-01],
/// [NFR-MA-02]).
///
/// A pure reader ([ADR-28]). Infallible at the surface: a missing target, an
/// unknown symbol, or a target with no structure to compare all resolve to an
/// honest read-model whose `empty_reason` names why — never a `404` or a `400`
/// ([NFR-CC-04]). A non-numeric `limit` falls back to the default rather than
/// rejecting the request.
pub(crate) async fn precedent(
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    // A missing or blank `target` is handed to the engine as `""` rather than
    // short-circuited to `PrecedentResult::default()`. The default is empty in
    // the one way this read-model must never be: no `notion`, no `ranked_by`,
    // no coverage statement and no `empty_reason` — the four things
    // `precedent_shell` exists to put on EVERY answer. Delegating keeps this
    // surface's degenerate case identical to the other three ([ADR-01]) and
    // keeps [FR-NV-12] AC 4 total: the core answers `target_unresolved` for an
    // empty target, with the whole shell populated.
    let target = q.get("target").map(|s| s.trim().to_string()).unwrap_or_default();
    let limit = q.get("limit").and_then(|value| value.parse::<usize>().ok());
    let result: PrecedentResult =
        bridge(engine, "api_v1_precedent", Surface::Web, move |e| e.precedent(&target, limit)).await;
    ok(result)
}
/// `GET /api/v1/branch-overlap?ref=<r>&ref=<r>[&base=<r>][&merge=<r>]` — which
/// git refs collide, and what a merge did not carry ([FR-NV-13], CR-114).
///
/// The JSON twin of the `branch-overlap` CLI command and the MCP tool of the
/// same name: all three hand their raw ref strings to the **one**
/// [`Engine::branch_overlap`] accessor, so the three surfaces cannot drift in
/// what they accept ([ADR-01], [NFR-MA-02]). `ref` repeats, which is why this
/// handler reads the query as ordered pairs rather than the map most endpoints
/// use.
///
/// A pure reader ([ADR-28]) — every git call underneath is a local read of the
/// object database. Infallible at the surface: no `ref` at all, a ref that does
/// not resolve, or a project that is not a git repository all produce an honest
/// read-model carrying its warnings and coverage limits, never a `4xx`
/// ([NFR-CC-04]).
pub(crate) async fn branch_overlap(
    MemberEngine(engine): MemberEngine,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    // `ref` repeats and is read positionally; `base`/`merge` are single. Empty
    // values are NOT filtered here — `Engine::branch_overlap` owns that rule so
    // `?base=` and `--base ""` behave identically ([ADR-01]).
    let refs: Vec<String> = pairs
        .iter()
        .filter(|(key, _)| key == "ref")
        .map(|(_, value)| value.clone())
        .collect();
    let pick = |wanted: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == wanted)
            .map(|(_, value)| value.clone())
    };
    let (base, merge) = (pick("base"), pick("merge"));
    let result: BranchOverlapResult = bridge(engine, "api_v1_branch_overlap", Surface::Web, move |e| {
        e.branch_overlap(&refs, base.as_deref(), merge.as_deref())
    })
    .await;
    ok(result)
}

// ── Node (single-symbol detail, [FR-NV-04]) ───────────────────────────────────

/// `GET /api/v1/node?symbol=<sym>[&code=1]` — the full node read-model for one
/// symbol ([FR-NV-04]): metadata, immediate edges, and (with `?code=1`) the source
/// excerpt. [`NodeInfo`] is infallible at the surface — an unknown symbol resolves
/// to an honest empty read-model carrying `warnings`, never a `404` ([NFR-CC-04]).
/// A missing/empty `symbol` is a client error (`400`).
pub(crate) async fn node(
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(symbol) = q.get("symbol").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
    else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `symbol` query parameter is required".into() }))
            .into_response();
    };
    let include_code = truthy(q.get("code"));
    let info: NodeInfo = bridge(engine, "api_v1_node", Surface::Web, move |e| e.node(&symbol, include_code)).await;
    ok(info)
}

// ── Search (whole-graph FTS, [FR-NV-01]) ──────────────────────────────────────

/// `GET /api/v1/search?q=<term>[&kind=<k>][&limit=<n>]` — the ranked whole-graph
/// symbol search read-model ([FR-NV-01]). [`SearchResult`] is infallible at the
/// surface (a failure degrades to an empty result with `warnings`); a
/// missing/empty `q` is a client error (`400`). An unrecognised `kind` is dropped
/// (the search runs unfiltered), matching the read-model's lenient contract.
pub(crate) async fn search(
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(term) = q.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string) else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `q` query parameter is required".into() }))
            .into_response();
    };
    let kind = q.get("kind").and_then(|k| NodeKind::from_wire(k.trim()));
    let limit = q.get("limit").and_then(|n| n.parse::<usize>().ok());
    let result: SearchResult = bridge(engine, "api_v1_search", Surface::Web, move |e| e.search(&term, kind, limit)).await;
    ok(result)
}

// ── Cross-service workspace fan-out (S-249, [FR-WS-06], [ADR-52]) ─────────────
//
// The `/api/v1/workspace/*` surface the workspace SPA ([S-250]) will consume: each
// endpoint serialises one `query::*` read-model over the member [`EngineRegistry`]
// (the federated [`Backing`]), repo-qualified and `?repo=`-scopable. They are the
// only handlers that reach the registry rather than the single default engine, and
// they answer an honest `404` when serving single-root (a plain repo, or
// `--standalone`) — the registry is never allocated there, so the single-root path
// pays nothing for them ([ADR-52]).

/// Read a trimmed, non-empty query parameter, else `None` — the shared optional
/// accessor for the workspace surface's `?repo=`/`?limit=` params.
pub(crate) fn opt_param(q: &HashMap<String, String>, key: &str) -> Option<String> {
    q.get(key).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The single-root `404` for the workspace surface: honest, machine-clean, and
/// self-describing — this is not a workspace, so the fan-out has nothing to answer.
/// Shared with the [`WorkspaceRoot`] extractor, so the config routes (S-450) refuse
/// in the family's own words rather than a second spelling of them.
pub(crate) fn not_a_workspace() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            error: "not a workspace: the /api/v1/workspace/* cross-service surface is served only \
                    under a workspace manifest (this is a single-root serve, or --standalone)"
                .to_string(),
        }),
    )
        .into_response()
}

/// Fan a `query::*` read-model over the member registry on the blocking pool (the
/// [ADR-03] `spawn_blocking` hop, exactly like [`bridge`]), or answer `404` when
/// serving single-root. `call` receives the registry and the shared bridge, so a
/// route-stitching read (`route-providers`/`callers`/`impact`) resolves the bridge
/// edges inside the blocking closure.
///
/// # This is the surface's *second* adapter boundary, and it carries the same rule
///
/// A fan reaches `Engine` read-models that emit telemetry through the chokepoint
/// exactly as [`bridge`]'s do, so it takes the same required `surface` parameter
/// and crosses the same [`run_blocking`] hop, which installs the surface scope
/// inside the `spawn_blocking` closure. Without it the build-failure rule ([BR-42]) would hold for one half
/// of this router and not the other — and a workspace route could not declare
/// itself shell chrome even when it is, because it would have no way to say so.
///
/// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
async fn workspace_fan<T, F>(
    backing: Arc<Backing<Engine>>,
    bridge: Arc<ContractBridge>,
    view: &'static str,
    surface: Surface,
    call: F,
) -> Response
where
    F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> T + Send + 'static,
    T: Serialize + Send + 'static,
{
    match workspace_read(backing, bridge, view, surface, call).await {
        Some(model) => ok(model),
        None => not_a_workspace(),
    }
}

/// The **fallible** twin of [`workspace_fan`], for the one workspace read-model
/// whose core entry point returns a `Result`: [`workspace_governance`] fails when
/// the manifest declares a rule whose symbol glob will not compile.
///
/// Identical in every other respect — same `404` guard, same blocking hop, same
/// surface scope, same render log — because both are the same function
/// [`workspace_read`] with a different success arm. It is written that way rather
/// than copied so the two can never drift: a hand-mirrored second copy of the
/// fan-out is precisely the divergence [ADR-01] is about.
///
/// The failure is surfaced as the honest `500` [`respond`] renders everywhere
/// else on this surface ([NFR-RA-05]) — never a fabricated empty report, which on
/// this route would read as "no rules declared" ([NFR-CC-04]).
async fn workspace_fan_try<T, F>(
    backing: Arc<Backing<Engine>>,
    bridge: Arc<ContractBridge>,
    view: &'static str,
    surface: Surface,
    call: F,
) -> Response
where
    F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> anyhow::Result<T> + Send + 'static,
    T: Serialize + Send + 'static,
{
    match workspace_read(backing, bridge, view, surface, call).await {
        Some(model) => respond(model),
        None => not_a_workspace(),
    }
}

/// The shared body of both fan-out adapters: the single-root guard, then the
/// [`run_blocking`] hop the handler adapters share (the [ADR-03]
/// `spawn_blocking`, the surface scope and the render-timing log). `None` means
/// the backing is not federated — the one condition on which
/// both adapters answer [`not_a_workspace`], kept here so neither can answer it on
/// a different test.
async fn workspace_read<T, F>(
    backing: Arc<Backing<Engine>>,
    bridge: Arc<ContractBridge>,
    view: &'static str,
    surface: Surface,
    call: F,
) -> Option<T>
where
    F: FnOnce(&EngineRegistry<Engine>, &ContractBridge) -> T + Send + 'static,
    T: Send + 'static,
{
    backing.as_federated()?;
    // The shared hop re-raises a panic crossing the pool, exactly as `bridge`
    // does. (That says nothing about read-model *errors*: where a read-model has
    // them they ride inside `T` and `workspace_fan_try` renders them.)
    let out = run_blocking(view, surface, move || {
        let registry = backing
            .as_federated()
            .expect("federated backing checked before spawn");
        call(registry, &bridge)
    })
    .await;
    Some(out)
}

/// `GET /api/v1/workspace/roster` — the manifest-only roster (workspace name,
/// default member, member names) the SPA shell probes on **every** page load to
/// decide its mode and populate the member selector ([FR-WS-06], [FR-UI-29]).
///
/// Engine-free by construction ([`workspace_roster`](fed_query::workspace_roster)):
/// unlike [`workspace_status`] it starts no member, so the shell's boot probe cannot
/// eagerly warm all N members and undo the warm-only-the-default policy
/// ([NFR-PE-10]). Answers the same honest `404` under a single-root backing — which
/// is exactly how the SPA discovers it is NOT a workspace.
pub(crate) async fn workspace_roster(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_fan(backing, bridge, "api_v1_workspace_roster", Surface::Web, |registry, _bridge| {
        fed_query::workspace_roster(registry)
    })
    .await
}

/// `GET /api/v1/workspace/status` — per-member index freshness, warm state and
/// open state, the warm and degraded roll-ups, and the 3-state cross-service
/// coverage summary ([`workspace_status`](fed_query::workspace_status),
/// [FR-WS-05]/[FR-WS-06], [FR-WS-15], [FR-WS-16]): the coverage dashboard's
/// data.
///
/// Each member row carries `warm_state` (`warm` / `deferred` / `degraded`;
/// `deferred` is honest and non-alarming — the member indexes lazily on first
/// query). `warming` is never reported without a live supervisor signal, and the
/// roll-up then **omits** the `warming` key rather than sending `0`: a consumer
/// must read its absence as *not knowable*, never as *none warming*
/// ([NFR-CC-04]).
///
/// # Two axes on one row ([FR-WS-16])
/// Beside `warm_state` — which is about **index presence** — each row carries
/// `open_state`, about **store openability**: `opened` (a member the budget
/// later evicted still reads `opened`, because eviction reclaims a success),
/// `not-attempted`, or `degraded` with a `degraded_reason` and, where the
/// diagnostic identifies one, a `degraded_cause`. The two are never merged.
///
/// A consumer that renders any figure from this payload must read
/// `degraded_rollup.covers_all_members` and `coverage.covers_all_members`: when
/// either is `false` the figures cover fewer than all members. Both
/// `spec_conformance_ratio` and `egress_resolution` are **absent** when nothing
/// was measured, so a bar rendering either must show "not measured" rather than
/// an empty or full bar ([NFR-CC-04]).
///
/// # The headline is a resolved-edge count ([CR-120], [BR-51], [CR-127])
/// `coverage.resolved_cross_service_edges` counts the edges resolved from a
/// captured **invocation** — a caller→callee call, a producer→consumer publish —
/// and is never rendered without `coverage.egress_resolution` beside it, the rate
/// at which captured egress sites resolve at all. `resolved_edges_summary`
/// carries both as one composed line for exactly that reason, and
/// `egress_resolution_measured` is the rate's explicit denominator. **Render the
/// composed line; never re-derive either half from the other.** Until S-403 T1 the
/// two halves were counted over differently-filtered populations and the line
/// contradicted itself in one sentence — *"0 resolved cross-service edges; egress
/// resolution 0.032 (5 of 155 egress sites resolved)"* ([CR-127] §3.1). They are
/// now one walk of one population, and this surface's job is to publish them, not
/// to reconstruct them.
///
/// A resolved edge is not necessarily an edge the **bridge drew**, and the two are
/// published under separate names for that reason — `coverage.bridge_invocation_edges`
/// on the `workspace/reachability` rider, deliberately NOT folded into this count
/// ([CR-127] §3.2). They diverge wherever a resolution draws several edges (a
/// fan-out topic) or none (an ambiguous composition). Until S-420 ([CR-133]) they
/// also diverged on the HTTP arm, where the coverage tier composed a target from
/// committed configuration and the bridge did not, so a `config-bound` row resolved
/// here and seeded no cross-service reachability root. Both arms now key on the
/// committed value: on the 84-member reference estate, read twice on 2026-09-18
/// with and without that arm, this count holds at **51** while the seeded count
/// moves **33 → 51**. **No floor is asserted on either figure.**
///
/// [CR-133]: ../../docs/requests/CR-133-bridge-keys-http-consumer-on-committed-target.md
///
/// [CR-127]: ../../docs/requests/CR-127-resolved-edge-counter-contradicts-its-payload.md
///
/// `bound_ratio` is **retired and no longer sent**. Its formula survives as
/// `spec_conformance_ratio`, which reports how far this workspace's declarations
/// line up with its controllers and is never a measure of cross-service coupling:
/// on the reference estate it read `0.287` over a workspace with zero
/// caller→callee edges ([CR-120] §2).
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
///
/// # The coverage counts are two populations ([CR-120], [FR-WS-05])
/// Every row of `coverage.references` carries `intake` — `contract-surface` for a
/// declared endpoint, `invocation` for a captured call site — in **every** state,
/// bound and non-bound alike, and `coverage.by_intake` reports the four
/// classification counters split by it. The two populations sum to the four
/// top-level counters, so the split can never report less than the headline.
///
/// A consumer that renders `bound` without the split renders two different things
/// as one: on the 84-member reference workspace the split is 81 `contract-surface`
/// and **45** `invocation` bound rows (2026-09-18; it was 81 and 15 on 2026-09-13,
/// before the broker arm admitted committed values), so a bare `bound: 126` says
/// nothing about whether any outbound call site resolves — and it read `81` and
/// **0** for as long as none did. **No floor is asserted on either figure.** This
/// surface carries the split because the CLI and MCP do; the parity is the
/// requirement, not a convenience ([FR-WS-05]).
///
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
///
/// # Declared documentation/mock members are reported apart ([FR-WS-32])
/// A member the manifest declares `kind = "documentation"` or `"mock"` has its
/// contract-surface rows moved out of the four counters, `by_intake` and
/// `spec_conformance_ratio`, into `coverage.declared_apart` — the rows, their
/// counts, each declared member with its row count, and a `summary` stating them
/// over their denominator. A consumer rendering the headline on a workspace that
/// carries `declared_apart` must render that line beside it ([BR-51]).
/// `kind_candidates` names undeclared members that hold API documents and no
/// runnable source: a hint for a human, never a classification. Both keys are
/// absent when there is nothing to report, so an undeclared workspace's payload is
/// unchanged. This route serializes the same read-model the CLI and MCP print.
///
/// [FR-WS-32]: ../../docs/specs/requirements/FR-WS-32.md
///
/// # Declared contracts and bound externals ride beside the headlines ([BR-57])
/// When a member holds a vendored spec, `coverage.declared_contracts` carries the
/// declared-contract relation ([FR-WS-31]) — `declared_contract_pairs` beside the
/// spec documents read, each contract's document and target, the named-external
/// registry — and `coverage.bound_external` the external join, `bound_external`
/// beside the `no-provider-in-workspace` REST rows it judged, each bound row
/// naming the matched operation and the committed base path with its sources.
/// Both are **declared, never observed**: no figure above counts either, and a
/// bound row stays `no-provider-in-workspace`. This payload is where the service
/// map reads both from — the map already fetches it — so neither the SPA nor
/// `workspace/route-providers` pays the coverage walk a second time. Both keys are
/// absent over a workspace with no vendored spec.
///
/// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [FR-WS-31]: ../../docs/specs/requirements/FR-WS-31.md
///
/// [FR-WS-15]: ../../docs/specs/requirements/FR-WS-15.md
/// [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
pub(crate) async fn workspace_status(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_fan(backing, bridge, "api_v1_workspace_status", Surface::Web, |registry, _bridge| {
        fed_query::workspace_status(registry)
    })
    .await
}

/// `GET /api/v1/workspace/route-providers[?repo=<member>]` — the resolved
/// cross-service route bindings (the service map), optionally scoped to routes a
/// member provides ([`xservice_route_providers`](fed_query::xservice_route_providers)).
///
/// # The bindings only, by design (S-461)
/// The CLI and MCP twins add the declared relations beside the bindings
/// ([`with_declared`](fed_query::XserviceRouteProviders::with_declared)); this
/// route does not. Its one consumer, the service map, reads them from
/// `workspace/status`, which it fetches anyway and which already walked the
/// coverage tier they come from. Adding them here would pay that walk twice per
/// map load, and it is not cached, while the bindings are. The keys are therefore
/// absent here, so this answer is what it was before S-461.
pub(crate) async fn workspace_route_providers(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let repo = opt_param(&q, "repo");
    workspace_fan(backing, bridge, "api_v1_workspace_route_providers", Surface::Web, move |registry, bridge| {
        fed_query::xservice_route_providers(&fed_query::bridge_read(bridge, registry), repo.as_deref())
    })
    .await
}

/// `GET /api/v1/workspace/build-deps[?repo=<member>]` — the build-dependency
/// relation ([`xservice_build_deps`], [FR-WS-33], [FR-WS-05]): per member, its
/// `builds_against` and `built_against_by` rows naming kind, scope and artifact,
/// the headline with its denominator, and the cross-context model hint.
///
/// A **build** dependency, never a runtime coupling ([BR-58]): the service map
/// draws it only behind a legend toggle that is off by default, and no runtime
/// figure reads it. The relation is joined on the first request and cached on
/// member sync-stamps in the holder beside the bridge ([ADR-52]): nothing is
/// read at startup. The SPA requests it when the service map opens over a
/// workspace whose status carries a build headline — before the toggle, since
/// the cross-context hint is shown whatever the toggle says — and never over one
/// without. The same read-model the CLI and MCP print.
///
/// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
/// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
pub(crate) async fn workspace_build_deps(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    State(deps): State<Arc<BuildDependencies>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let repo = opt_param(&q, "repo");
    workspace_fan(backing, bridge, "api_v1_workspace_build_deps", Surface::Web, move |registry, _bridge| {
        xservice_build_deps(&deps.relation(registry), repo.as_deref())
    })
    .await
}

/// `GET /api/v1/workspace/search?q=<term>[&kind=<k>][&limit=<n>][&repo=<member>]` —
/// cross-service full-text search fanned across the members
/// ([`xservice_search`](fed_query::xservice_search)). A missing/empty `q` is a
/// `400`; an unrecognised `kind` is dropped (the search runs unfiltered), matching
/// the single-root [`search`] contract.
pub(crate) async fn workspace_search(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(term) = opt_param(&q, "q") else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `q` query parameter is required".into() }))
            .into_response();
    };
    let kind = q.get("kind").and_then(|k| NodeKind::from_wire(k.trim()));
    let limit = opt_param(&q, "limit").and_then(|n| n.parse::<usize>().ok());
    let repo = opt_param(&q, "repo");
    workspace_fan(backing, bridge, "api_v1_workspace_search", Surface::Web, move |registry, _bridge| {
        fed_query::xservice_search(registry, &term, kind, limit, repo.as_deref())
    })
    .await
}

/// `GET /api/v1/workspace/callers?symbol=<s>[&limit=<n>][&repo=<member>]` — each
/// member's intra-repo callers plus the cross-service consumers that reach the
/// symbol over a bridge edge ([`xservice_callers`](fed_query::xservice_callers)). A
/// missing/empty `symbol` is a `400`.
pub(crate) async fn workspace_callers(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(symbol) = opt_param(&q, "symbol") else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `symbol` query parameter is required".into() }))
            .into_response();
    };
    let limit = opt_param(&q, "limit").and_then(|n| n.parse::<usize>().ok());
    let repo = opt_param(&q, "repo");
    workspace_fan(backing, bridge, "api_v1_workspace_callers", Surface::Web, move |registry, bridge| {
        let inputs = fed_query::reachability_inputs(bridge, registry);
        fed_query::xservice_callers(registry, &inputs, &symbol, limit, repo.as_deref())
    })
    .await
}

/// `GET /api/v1/workspace/impact?symbol=<s>[&depth=<n>][&repo=<member>]` — the
/// seed member(s)' impact plus each far-side impact stitched across a bridge edge
/// ([`xservice_impact`](fed_query::xservice_impact)). A missing/empty `symbol` is a
/// `400`.
pub(crate) async fn workspace_impact(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(symbol) = opt_param(&q, "symbol") else {
        return (StatusCode::BAD_REQUEST, Json(ApiError { error: "a `symbol` query parameter is required".into() }))
            .into_response();
    };
    let depth = opt_param(&q, "depth").and_then(|n| n.parse::<usize>().ok());
    let repo = opt_param(&q, "repo");
    workspace_fan(backing, bridge, "api_v1_workspace_impact", Surface::Web, move |registry, bridge| {
        let inputs = fed_query::reachability_inputs(bridge, registry);
        fed_query::xservice_impact(registry, &inputs, &symbol, depth, repo.as_deref())
    })
    .await
}

// ── `federation::reach` and `federation::governance` join the fan-out (S-427,
// [FR-WS-28], [ADR-01]) ──────────────────────────────────────────────────────
//
// Both read-models ([FR-WS-12], [FR-WS-13]) already answered on **two** surfaces —
// the CLI (`logos workspace reachability` / `check`) and MCP
// (`mcp::server::workspace_reachability` / `workspace_check`) — and on neither web
// route. These two GETs add the third rendering and nothing else: each runs the
// **same call sequence** `cli/src/xservice.rs` runs — `edges` then the read-model —
// and serialises the result. No new core query, no second computation, no figure
// the read-model does not already carry ([ADR-01], [NFR-MA-02]).
//
// Three renderings is three places for one figure to drift, so each pair is pinned
// by a test rather than by intent: CLI↔HTTP field-for-field in
// `cli/tests/xservice_surface.rs` (the story's spine), and the MCP bounded-default
// contract in `mcp/tests/reachability_bound.rs`.

/// Whether a workspace fan-out answer covers every member it is an answer about
/// ([FR-WS-16], [NFR-CC-04]).
///
/// # Why these two routes need it and the rest of the fan-out does not
/// The CLI states incompleteness in its **exit code** and a stderr notice
/// ([`DegradedRollup::notice`]); `workspace/status` states it *inside* its payload,
/// folded into a member table it already has. Reachability and governance have
/// neither: an HTTP `200` has no exit code, and neither read-model carries a member
/// table. Without this rider a partial fan-out would reach the SPA as a complete
/// one, which is exactly what [FR-WS-16] forbids.
///
/// # It rides *beside* the read-model, never inside it
/// The payload is `{ <read-model key>: …, complete, degraded_rollup }`, so the
/// read-model under its own key stays byte-identical to what the CLI prints —
/// including `check`'s bare `null`, the honest empty [NFR-CC-04] gave it, which
/// wrapping would have destroyed. That is what lets the parity test compare the
/// two renderings field-for-field.
#[derive(Debug, Serialize)]
pub(crate) struct AnswerCompleteness {
    /// `false` when a declared member was attempted and could not be **opened**:
    /// the fan-out behind this answer is partial, and says so rather than letting
    /// a `200` present it as whole ([FR-WS-16]).
    ///
    /// This is [`DegradedRollup::all_opened`] — the predicate the CLI derives its
    /// exit code from — and deliberately **not**
    /// [`covers_all_members`](DegradedRollup::covers_all_members), which is also
    /// `false` for a member laziness never attempted. That is a coverage fact, not
    /// a failure, and reporting a healthy scoped answer as incomplete would be its
    /// own untruth ([BR-45]).
    ///
    /// [BR-45]: ../../docs/specs/software-spec.md#327-workspace-federation
    complete: bool,
    /// The open-state roll-up, **naming** each member that could not be opened —
    /// the identical [`DegradedRollup`] `workspace/status` already publishes, so
    /// the SPA reads one shape across all three routes rather than a third
    /// per-route vocabulary ([FR-WS-16]).
    degraded_rollup: DegradedRollup,
}

impl AnswerCompleteness {
    /// Read the registry's open-state ledger and roll it up.
    ///
    /// **Call this after the read-model has run, never before.** The ledger is
    /// complete only once the answer's own walks have happened, which is why
    /// `run_workspace` reads it last too — the ordering is the contract.
    ///
    /// What hoisting the call actually reports depends on the surface, and neither
    /// answer is this one: on the CLI the registry is a fresh per-command object, so
    /// every member reads `not-attempted`; on this serve surface the registry
    /// outlives the request, so it reads the **previous** answer's fan-out. Both are
    /// a different question than the one the payload claims to answer.
    ///
    /// The once-per-answer open-failure discipline is inherited whole from
    /// [CR-105]: the read-model mints its own [`AnswerScope`] internally, so a
    /// broken member is attempted once per request and re-attempted on the next
    /// one. Nothing here re-implements it.
    ///
    /// [`AnswerScope`]: logos_core::federation::registry::AnswerScope
    /// [CR-105]: ../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
    fn after_reading(registry: &EngineRegistry<Engine>) -> Self {
        let rollup = open_state::rollup(&registry.open_states());
        Self {
            complete: rollup.all_opened(),
            degraded_rollup: rollup,
        }
    }
}

/// The `GET /api/v1/workspace/reachability` payload ([FR-WS-28]).
///
/// The contract [S-428](../../docs/planning/sprints/sprint-74.md) consumes: the
/// bounded union view under `reachability`, byte-identical to
/// `logos workspace reachability --json`, plus the completeness rider.
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceReachabilityAnswer {
    /// The [`BoundedReachability`] projection — `view`, `advisory`, the applied
    /// `scope`, the `coverage` rider, per-member tallies, `skipped_members`, the
    /// promotions, and `dead` (`null` under the promotions-only default).
    reachability: BoundedReachability,
    #[serde(flatten)]
    completeness: AnswerCompleteness,
}

/// The `GET /api/v1/workspace/check` payload ([FR-WS-28]).
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceGovernanceAnswer {
    /// The [`WorkspaceGovernance`] report, byte-identical to
    /// `logos workspace check --json` — **`null` when the workspace declares no
    /// rules**. That is the honest empty: no output at all, never a fabricated
    /// zero-violation report that would read as a passing one ([ADR-56],
    /// [NFR-CC-04]).
    governance: Option<WorkspaceGovernance>,
    #[serde(flatten)]
    completeness: AnswerCompleteness,
}

/// `GET /api/v1/workspace/reachability[?repo=<member>][&all]` — the app-wide
/// cross-service reachability union view ([FR-WS-12], [FR-WS-28]), the HTTP twin
/// of `logos workspace reachability`.
///
/// # Bounded by default, and it says so ([S-294], [CR-084], [NFR-CC-04])
/// Without `?all` the payload carries only the cross-service **promotions** and
/// `reachability.dead` is `null` — *suppressed*, which is deliberately distinct
/// from `[]` ("computed, and genuinely empty"). Every applied bound is echoed in
/// `reachability.scope`, so a bounded reply can never be read as the complete dead
/// set. `?repo=<member>` scopes the tallies and claims to one member; a degraded
/// member is still named workspace-wide, because a scope must not hide it.
///
/// `?all` lifts the bound only on an explicit opt-in — bare `?all`, or
/// `?all=1|true|on|yes`. `?all=0|false|no|off`, and any value the reader does not
/// recognise, leave the promotions-only default in place: a flag that unbounds a
/// payload must fail closed, not guess.
///
/// `reachability.advisory` is always `true` and `reachability.coverage` rides every
/// claim: this view is never a gate input ([ADR-56]), and no claim on it is
/// readable without the coverage it rests on.
///
/// [CR-084]: ../../docs/requests/CR-084-reachability-payload-filter.md
/// [FR-WS-12]: ../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-28]: ../../docs/specs/requirements/FR-WS-28.md
/// [ADR-56]: ../../docs/specs/architecture/decisions/ADR-56.md
pub(crate) async fn workspace_reachability(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let repo = opt_param(&q, "repo");
    // The `--all` escape hatch, read fail-closed: bare `?all` or a canonical truthy
    // token lifts the bound, and anything else — `0`, `false`, `no`, `off` — leaves
    // it in place (see [`wants_optin_flag`]). The inversion to `promotions_only`
    // stays in `ReachabilityScope::new` where the CLI leaves it, so this handler
    // parses params and nothing else ([NFR-MA-02]).
    let all = wants_optin_flag(&q, "all");
    workspace_fan(
        backing,
        bridge,
        "api_v1_workspace_reachability",
        Surface::Web,
        move |registry, bridge| {
            let edges = fed_query::edges(bridge, registry);
            let reachability =
                app_wide_reachability(registry, &edges).bound(ReachabilityScope::new(repo, all));
            WorkspaceReachabilityAnswer {
                reachability,
                completeness: AnswerCompleteness::after_reading(registry),
            }
        },
    )
    .await
}

/// `GET /api/v1/workspace/check` — the workspace governance report ([FR-WS-13],
/// [FR-WS-28]), the HTTP twin of `logos workspace check`.
///
/// # Advisory, and structurally incapable of gating ([ADR-56])
/// A violation moves no member's gated signal and no exit code — there is no exit
/// code here to move, and the response is `200` whether the rules hold or not. The
/// report is published; what to do about it is the reader's call.
///
/// # The honest empty is `null`
/// A workspace declaring no rules produces **no report at all**, not a passing one:
/// `governance` is `null`, exactly as the CLI prints it ([NFR-CC-04]).
///
/// [FR-WS-13]: ../../docs/specs/requirements/FR-WS-13.md
pub(crate) async fn workspace_check(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_fan_try(
        backing,
        bridge,
        "api_v1_workspace_check",
        Surface::Web,
        move |registry, bridge| {
            let edges = fed_query::edges(bridge, registry);
            let governance = workspace_governance(registry.federation(), &edges)?;
            Ok(WorkspaceGovernanceAnswer {
                governance,
                completeness: AnswerCompleteness::after_reading(registry),
            })
        },
    )
    .await
}

/// `GET /api/v1/workspace/statistics[?window=<days>]` — every member's telemetry
/// summed over one trailing window ([`workspace_statistics`], [FR-UI-37],
/// [FR-OB-04]): the **`app`-scoped** twin of [`statistics`], which answers for one
/// member.
///
/// # It costs no member engine ([NFR-PE-10] — the binding constraint)
/// The obvious fan-out would call `Engine::stats` per member and construct N
/// engines on one view load, undoing the warm-only-the-default policy the
/// federated serve exists to keep. This reads each member's `telemetry.db`
/// directly instead, so loading the view leaves the resident-engine count exactly
/// where a `workspace status` over the same workspace left it — asserted on
/// connection count in `logos-core/tests/workspace_statistics_engine_free.rs`.
///
/// # It states the population it summed over ([NFR-CC-04])
/// `members_read` of `members_total` is the denominator of **every** figure in the
/// payload, and `unread` names each member that contributed nothing with its
/// reason (`absent` / `locked` / `unreadable`). A consumer must render the
/// denominator beside the totals; `covers_all_members` is the marker that governs
/// them.
///
/// The **awaiting-data** state is `calls_total == 0`, which is the app-scoped twin
/// of the member-scoped view's own `isStatsEmpty` predicate — [FR-UI-37] requires
/// the same honest empty state the member-scoped view already renders, and that
/// view keys on `calls_total`. It is *not* `members_read == 0`: a workspace whose
/// members all have migrated but eventless stores reads every one of them and
/// still has nothing to show, and rendering zeros there is the failure
/// [NFR-CC-04] names. `members_read` governs the denominator line instead.
///
/// `?window=<days>` scopes the trailing window; an absent or unparseable value falls
/// back to the core read-model's own default (7, [FR-OB-04]), the same lenient query
/// contract [`statistics`] uses. Infallible at the surface — an unreadable member is
/// named in the payload, never raised as an error — so this pairs `workspace_fan`
/// with its plain (non-`try`) form. Answers the same honest `404` under a single-root
/// backing as every other `/api/v1/workspace/*` route.
///
/// [FR-OB-04]: ../../docs/specs/requirements/FR-OB-04.md
/// [FR-UI-37]: ../../docs/specs/requirements/FR-UI-37.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
pub(crate) async fn workspace_statistics_aggregate(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let window = q.get("window").and_then(|w| w.trim().parse::<u32>().ok());
    workspace_fan(
        backing,
        bridge,
        "api_v1_workspace_statistics",
        Surface::Web,
        move |registry, _bridge| -> WorkspaceStatistics { workspace_statistics(registry, window) },
    )
    .await
}

// ── Wiki (index / search / page, [FR-UI-06]/[FR-WK-05]) ───────────────────────

/// `GET /api/v1/wiki` — the dual-axis `wiki status` read-model ([FR-UI-06],
/// [FR-WK-12]): per-anchor freshness counts, the revision-stale count, and the
/// revision they were computed at. A documented pure read that never prunes
/// ([ADR-28]).
pub(crate) async fn wiki_index(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_wiki", Surface::Web, |e| -> anyhow::Result<WikiStatus> {
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
    MemberEngine(engine): MemberEngine,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let term = q.get("q").cloned().unwrap_or_default().trim().to_string();
    let model = bridge(engine, "api_v1_wiki_search", Surface::Web, move |e| -> anyhow::Result<Vec<WikiHit>> {
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
    MemberEngine(engine): MemberEngine,
    Path(slug): Path<String>,
) -> Response {
    let model = bridge(engine, "api_v1_wiki_page", Surface::Web, move |e| -> anyhow::Result<WikiPageOutcome> {
        // Only the revision is needed, so it is read directly: `status()` would
        // also re-walk every unbound call for its per-language residue (S-589).
        // `0` when it cannot be read, as `status()`'s degraded read-model says.
        let current_revision = e
            .runtime()
            .and_then(|rt| rt.submit_read(|store| store.graph_revision()).ok())
            .unwrap_or(0);
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
    MemberEngine(engine): MemberEngine,
    Path(rel_path): Path<String>,
) -> Response {
    let outcome =
        bridge(engine, "api_v1_wiki_asset", Surface::Web, move |e| resolve_doc_asset(e.root(), &rel_path)).await;
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
pub(crate) async fn wiki_nav(MemberEngine(engine): MemberEngine) -> Response {
    use crate::wiki::{
        DESIGN_DOCS, GUIDED_TOUR, OVERVIEW_ARCHITECTURE, OVERVIEW_ARCHITECTURE_LABEL, SPECS_DOCS,
        SPECS_SRS, SPECS_SRS_LABEL, USER_GUIDE_TIER_TITLE,
    };
    let nav = bridge(engine, "api_v1_wiki_nav", Surface::Web, |e| {
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
///
/// Beside those it carries the **effective** chat resolution and each half's
/// origin ([FR-WS-30], S-448), and the effective wiki model the wiki run resolves
/// (`effective_wiki`, Sprint 77 HF-1). Its workspace root is the one the backing already
/// holds — a federated backing's resolved root, never a discovered one — and a
/// single-root backing supplies none, so no `workspace` origin is reachable there.
///
/// [FR-WS-30]: ../../docs/specs/requirements/FR-WS-30.md
pub(crate) async fn config(
    State(backing): State<Arc<Backing<Engine>>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    let workspace_root = workspace_root_of(&backing);
    let model = bridge(engine, "api_v1_config", Surface::Web, move |e| -> anyhow::Result<ConfigReadModel> {
        e.config_read(workspace_root.as_deref())
    })
    .await;
    respond(model)
}

// ── The workspace root as a config root ([FR-WS-30], S-450, [ADR-40]) ─────────
//
// One read and two writes over `<workspace-root>/.logos/`, the second config tier
// `resolve_chat` inherits from. Each calls core's engine-free workspace-tier seam
// (`read_workspace_documents`, `write_workspace_config`, `write_workspace_secret`),
// which reaches the SAME parser and writers the member route reaches through its
// engine — so validate-before-write, the atomic replace and the credential's 0o600
// mode come with it unchanged ([NFR-RA-07], [NFR-SE-07]) — and emits the same
// telemetry event the façade method would, under the `Surface::Web` scope
// `run_blocking` enters. No `Engine` is involved because constructing one at the
// workspace root is the fault [ADR-40]'s exception exists to avoid: the root
// holds no graph, and an engine would open a store there. That is also why no
// apply route exists at this scope.
//
// [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md
// [NFR-RA-07]: ../../docs/specs/requirements/NFR-RA-07.md

/// `GET /api/v1/workspace/config` — the workspace root's config tier as its editor
/// group loads it ([FR-WS-30], [FR-UI-38], S-451): the literal `config.toml`, the
/// fingerprint a save must post back, and its parse verdict; the **masked**
/// credential ([NFR-SE-07]); and the effective-chat slice
/// ([`WorkspaceTierDocument`]).
///
/// The slice is resolved with **no** tier above ([`read_workspace_documents`]),
/// because the workspace root has none. Its origins are therefore relative to
/// the root this payload reads: `member` means *declared at this root*, `unset`
/// that it is not, and `workspace` never appears — nothing here is inherited.
///
/// Like the manifest read beside it, a file that does not parse or validate is a
/// `200` — `parsed: null` (or `chat_key: null`) with the fault in `error` (or
/// `chat_key_error`), by file and position or key only, never a fragment of the
/// file — not a `500`: the editor is the
/// repair path, and it needs the document and its fingerprint to repair it. Only
/// an unreadable `config.toml` is a `500`. A member's `GET /api/v1/config` is
/// unchanged and stays fail-loud over a broken tier it inherits from.
///
/// [`read_workspace_documents`]: logos_core::config::read_workspace_documents
/// [`WorkspaceTierDocument`]: logos_core::config::WorkspaceTierDocument
/// [FR-WS-30]: ../../docs/specs/requirements/FR-WS-30.md
/// [FR-UI-38]: ../../docs/specs/requirements/FR-UI-38.md
pub(crate) async fn workspace_config(WorkspaceRoot(root): WorkspaceRoot) -> Response {
    let model = run_blocking("api_v1_workspace_config", Surface::Web, move || {
        core_config::read_workspace_documents(&root)
    })
    .await;
    respond(model)
}

/// One member's **effective** chat read roots, as the Workspace Chat's consent
/// disclosure names them ([S-485], [FR-WS-34]): the roots a repo-addressed source
/// call on this member reads through, resolved exactly as its sandbox resolves
/// them ([`ChatResolution::read_roots_origin`]).
///
/// [S-485]: ../../docs/planning/journal.md#s-485-workspace-chat-in-the-workspace-section-and-no-member-chat-in-workspace-mode
/// [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
/// [`ChatResolution::read_roots_origin`]: logos_core::config::ChatResolution::read_roots_origin
#[derive(Debug, Serialize)]
pub(crate) struct MemberChatReadRoots {
    /// The member's repo-qualified manifest name.
    name: String,
    /// Where the member's effective `[chat]` policy came from; `null` when its
    /// chat config cannot be read — its addressed source calls then fail on
    /// their own, which is where the workspace turn's read-root check leaves
    /// that fault too (`chat::workspace`).
    policy_origin: Option<core_config::ChatOrigin>,
    /// The root that declared [`read_roots`](Self::read_roots), which is the
    /// root relative entries resolve against: `member` or `workspace`. `null`
    /// alongside a `null` origin.
    declared_by: Option<core_config::ChatOrigin>,
    /// The effective `[chat] read_roots`, as declared. Empty when none are, or
    /// when the config cannot be read.
    read_roots: Vec<String>,
}

/// `GET /api/v1/workspace/config/read-roots` — every member's effective chat
/// read roots ([S-485], [FR-WS-34]), in the federation's member order: the
/// engine-free aggregate the Workspace Chat's consent banner names before any
/// outbound call ([NFR-SE-07]).
///
/// Built from [`resolve_chat`](core_config::resolve_chat)`(member, Some(workspace
/// root))` per member — config reads only. It is a sibling of
/// [`workspace_config`] rather than a fan-out over `GET /api/v1/config?repo=<m>`
/// because that route's [`MemberEngine`] extractor starts the member's engine, so
/// rendering the banner would warm every member ([NFR-PE-10], the 2026-10-02
/// planning decision). A member whose config cannot be read is listed with a
/// `null` origin rather than failing the whole read: one broken member must not
/// hide the roots every other member discloses. A single-root serve answers the
/// family's `404`.
///
/// [S-485]: ../../docs/planning/journal.md#s-485-workspace-chat-in-the-workspace-section-and-no-member-chat-in-workspace-mode
/// [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
/// [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
pub(crate) async fn workspace_chat_read_roots(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    // Through the shared fan-out adapter like every other workspace read: its
    // single-root guard, blocking hop and surface scope. The closure reads the
    // federation's roster only — it never asks the registry for an engine.
    workspace_fan(
        backing,
        bridge,
        "api_v1_workspace_chat_read_roots",
        Surface::Web,
        |registry, _bridge| member_chat_read_roots(registry.federation()),
    )
    .await
}

/// [`workspace_chat_read_roots`]'s read-model over `federation`, touching no engine.
fn member_chat_read_roots(federation: &Federation) -> Vec<MemberChatReadRoots> {
    let workspace_root = federation.root.as_path();
    federation
        .members
        .iter()
        .map(|member| match core_config::resolve_chat(&member.root, Some(workspace_root)) {
            Ok(resolution) => MemberChatReadRoots {
                name: member.name.clone(),
                policy_origin: Some(resolution.policy_origin),
                declared_by: Some(resolution.read_roots_origin()),
                read_roots: resolution.policy.read_roots,
            },
            Err(_) => MemberChatReadRoots {
                name: member.name.clone(),
                policy_origin: None,
                declared_by: None,
                read_roots: Vec::new(),
            },
        })
        .collect()
}

/// `POST /api/v1/workspace/config/save` → [`write_workspace_config`], the
/// workspace root's `config.toml` saved the way the manifest is
/// ([`workspace_manifest_save`], [FR-UI-38], [FR-WS-30]). Form fields:
/// `content=<toml>`, `fingerprint=<hex>` (the one the read returned), and
/// optionally `file=config` — the only document accepted.
///
/// - `400` for any `file` other than `config` — workspace governance is declared
///   in the manifest ([FR-WS-13]), so a `rules.toml` here would be read by
///   nothing — and `400` when `fingerprint` is missing: a save that cannot say
///   what it was made against cannot be checked for a clobber.
/// - `200` with `outcome: "written"`, or `"unchanged"` for a candidate
///   byte-identical to disk (nothing written).
/// - `409` with `outcome: "conflict"` and the document on disk now: the file
///   changed since the load, and nothing was written.
/// - `422` for a candidate the parser rejects (the file byte-identical), `500` for
///   an I/O fault.
///
/// The candidate is validated, never the file it replaces, so a save over a
/// broken tier file — the one that fails every inheriting member's config read —
/// succeeds and repairs it. It writes only under `<workspace-root>/.logos/`.
///
/// [`write_workspace_config`]: logos_core::config::write_workspace_config
/// [FR-WS-13]: ../../docs/specs/requirements/FR-WS-13.md
pub(crate) async fn workspace_config_save(
    WorkspaceRoot(root): WorkspaceRoot,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let refuse = |error: &str| {
        (StatusCode::BAD_REQUEST, Json(ApiError { error: error.to_string() })).into_response()
    };
    if !matches!(form.get("file").map(String::as_str), None | Some("config")) {
        return refuse("the workspace root carries the config document only (expected file=config)");
    }
    let Some(loaded) = form.get("fingerprint").map(|f| f.trim().to_string()).filter(|f| !f.is_empty())
    else {
        return refuse(
            "a workspace config save must carry the fingerprint its read returned (fingerprint=…)",
        );
    };
    let content = form.get("content").cloned().unwrap_or_default();
    let outcome = run_blocking("api_v1_workspace_config_save", Surface::Web, move || {
        core_config::write_workspace_config(&root, &content, &loaded)
    })
    .await;
    match outcome {
        Ok(conflict @ TierSaveOutcome::Conflict { .. }) => {
            (StatusCode::CONFLICT, Json(conflict)).into_response()
        }
        other => written(other),
    }
}

/// `POST /api/v1/workspace/config/secret` → [`write_workspace_secret`], the [`write_secret`] writer at the workspace root
/// ([FR-WS-30], [NFR-SE-07]): write (or, blank, clear) the credential every member
/// that declares none inherits. Form field: `api_key=<raw>`.
///
/// Write-only: the response carries the masked outcome (presence + last-4), the
/// file is written owner-only (0o600), and the managed workspace-root `.gitignore`
/// keeps it out of version control where the root is a git working tree. The
/// writer merges into the existing store, so an unparsable one is a `422` that
/// leaves it byte-identical rather than being silently overwritten.
///
/// [`write_workspace_secret`]: logos_core::config::write_workspace_secret
/// [`write_secret`]: logos_core::config::write_secret
/// [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md
pub(crate) async fn workspace_config_secret(
    WorkspaceRoot(root): WorkspaceRoot,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let api_key = form.get("api_key").cloned().unwrap_or_default();
    let outcome = run_blocking("api_v1_workspace_config_secret", Surface::Web, move || {
        core_config::write_workspace_secret(&root, &api_key)
    })
    .await;
    written(outcome)
}

// ── The workspace manifest as an editable document ([FR-UI-38], S-430) ────────
//
// One read and one write over `logos.workspace.toml` itself — the file that
// governs N repositories — through core's whole-manifest write path
// (`manifest::read_workspace_manifest` / `manifest::save_workspace_manifest`),
// which books one `config_read` / `config_write` telemetry event per call as the
// workspace config routes above do, validates the
// candidate with the parser `discover` runs, writes nothing for a byte-identical
// result, refuses a save made against a manifest that changed on disk since the
// load, and otherwise writes the candidate verbatim through the shared atomic
// publish. Like the config routes above, no engine is involved: the manifest is a
// file at the workspace root, so no member is started, reindexed or written.
//
// [FR-UI-38]: ../../docs/specs/requirements/FR-UI-38.md

/// `GET /api/v1/workspace/manifest` — the literal manifest, the fingerprint the
/// editor must post back with its save, and the parse verdict over it
/// ([`ManifestDocument`], [FR-UI-38]) — plus whether the `[governance]` family on
/// disk is the one this serve is evaluating.
///
/// A manifest that no longer parses is a `200` with `parsed: null` and the
/// parser's message in `error`, not a `500`: the editor is the repair path, and
/// it needs the document and its fingerprint to repair it. Only an unreadable
/// file is a `500`. Single-root answers the family's `404`.
///
/// [FR-UI-38]: ../../docs/specs/requirements/FR-UI-38.md
pub(crate) async fn workspace_manifest(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_fan_try(
        backing,
        bridge,
        "api_v1_workspace_manifest",
        Surface::Web,
        |registry, _bridge| -> anyhow::Result<WorkspaceManifestAnswer> {
            let federation = registry.federation();
            let document = manifest::read_workspace_manifest(&federation.root)?;
            let governance_in_effect = document
                .parsed
                .as_ref()
                .is_some_and(|m| m.governance == federation.governance);
            Ok(WorkspaceManifestAnswer {
                document,
                governance_in_effect,
            })
        },
    )
    .await
}

/// The manifest read-model plus the one fact the file cannot state about itself.
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceManifestAnswer {
    #[serde(flatten)]
    document: ManifestDocument,
    /// Whether the `[governance]` family on disk equals the one this serve loaded
    /// at startup — the family `GET /api/v1/workspace/check` evaluates. The serve
    /// never re-reads the manifest, so after a save that changes the rules the
    /// findings beside the editor are over the **previous** rules until the next
    /// `logos serve`; `false` is what lets the view say so rather than present
    /// them as the verdict on what the user just saved ([NFR-CC-04]). Also `false`
    /// when the manifest on disk does not parse.
    ///
    /// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
    governance_in_effect: bool,
}

/// `POST /api/v1/workspace/manifest/save` → [`manifest::save_workspace_manifest`]
/// ([FR-UI-38]), booked as one `config_write` telemetry event. Form fields: `content=<toml>` (the whole candidate manifest) and
/// `fingerprint=<hex>` (the one the editor's read returned).
///
/// - `200` with `outcome: "written"` or `"unchanged"` (nothing written).
/// - `409` with `outcome: "conflict"`: the manifest changed on disk since the
///   load; nothing was written, and the body carries the document on disk now so
///   the editor can offer the user the choice.
/// - `422` for a candidate the parser rejects (the manifest byte-identical), `500`
///   for an I/O fault — the member config routes' mapping ([`config_write_status`]).
/// - `400` when `fingerprint` is missing: a save that cannot say what it was made
///   against cannot be checked for a clobber, so it is not attempted.
///
/// Rides the enumerated allow-list and the unchanged same-origin + intent-token
/// guard like every mutating route ([ADR-31], [NFR-SE-06]); single-root answers the
/// family's `404` before the body is read.
///
/// [FR-UI-38]: ../../docs/specs/requirements/FR-UI-38.md
/// [ADR-31]: ../../docs/specs/architecture/decisions/ADR-31.md
/// [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md
pub(crate) async fn workspace_manifest_save(
    WorkspaceRoot(root): WorkspaceRoot,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let Some(loaded) = form.get("fingerprint").map(|f| f.trim().to_string()).filter(|f| !f.is_empty())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "a manifest save must carry the fingerprint its read returned (fingerprint=…)"
                    .to_string(),
            }),
        )
            .into_response();
    };
    let content = form.get("content").cloned().unwrap_or_default();
    let outcome = run_blocking("api_v1_workspace_manifest_save", Surface::Web, move || {
        manifest::save_workspace_manifest(&root, &content, &loaded)
    })
    .await;
    match outcome {
        Ok(conflict @ ManifestSaveOutcome::Conflict { .. }) => {
            (StatusCode::CONFLICT, Json(conflict)).into_response()
        }
        other => written(other),
    }
}

/// Render a workspace-root write: the outcome on success, else the [`ApiError`]
/// at the status the member routes map the same fault to — `422` for a
/// validation fault, `500` for an I/O one ([`config_write_status`]).
fn written<T: Serialize>(outcome: anyhow::Result<T>) -> Response {
    match outcome {
        Ok(outcome) => ok(outcome),
        Err(err) => {
            (config_write_status(&err), Json(ApiError { error: format!("{err:#}") })).into_response()
        }
    }
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
pub(crate) async fn verify(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_verify", Surface::Web, |e| -> anyhow::Result<VerifyReport> {
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

/// `true` only on an **explicit opt-in**: bare `?key`, or `?key=` with a canonical
/// truthy token. Anything else — `0`, `false`, `no`, `off`, or a value nobody
/// recognises — is `false`.
///
/// # Why this is not [`wants_flag`]
/// [`wants_flag`] treats the literal `"0"` as its *only* off-token, so `?key=false`
/// reads as **on**. That is tolerable for a display toggle like `?untested`, where
/// the wrong branch shows the wrong rows. It is not tolerable for a flag that
/// **lifts a payload bound**: `?all=false` would then emit the very per-repo dead
/// set the caller just asked to keep suppressed — the ~500 KB payload class
/// [CR-084] bounded, delivered on a request that spelled its refusal in a way the
/// reader did not know ([NFR-CC-04]).
///
/// So this reader fails *closed*: an unrecognised value leaves the bound in place.
/// It composes [`truthy`] rather than inventing a third vocabulary for the surface.
///
/// [CR-084]: ../../docs/requests/CR-084-reachability-payload-filter.md
fn wants_optin_flag(params: &HashMap<String, String>, key: &str) -> bool {
    params
        .get(key)
        .is_some_and(|v| v.trim().is_empty() || truthy(Some(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_flag_reads_presence_and_explicit_off() {
        let on = HashMap::from([("untested".to_string(), String::new())]);
        let one = HashMap::from([("untested".to_string(), "1".to_string())]);
        let off = HashMap::from([("untested".to_string(), "0".to_string())]);
        let absent: HashMap<String, String> = HashMap::new();
        assert!(wants_flag(&on, "untested"), "bare ?untested is on");
        assert!(wants_flag(&one, "untested"), "?untested=1 is on");
        assert!(!wants_flag(&off, "untested"), "?untested=0 is off");
        assert!(!wants_flag(&absent, "untested"), "absent is off");
    }

    #[test]
    fn wants_flag_is_shared_by_production_scope() {
        let on = HashMap::from([("production_scope".to_string(), String::new())]);
        let off = HashMap::from([("production_scope".to_string(), "0".to_string())]);
        assert!(wants_flag(&on, "production_scope"), "bare ?production_scope is on");
        assert!(!wants_flag(&off, "production_scope"), "?production_scope=0 is off");
    }

    /// The near misses that made `wants_flag` the wrong reader for a bound-lifting
    /// flag: every one of these spells "no", and `wants_flag` reads four of them as
    /// "yes".
    #[test]
    fn wants_optin_flag_fails_closed_on_every_spelling_of_no() {
        for off in ["0", "false", "FALSE", "no", "off", "nope", "-1"] {
            let q = HashMap::from([("all".to_string(), off.to_string())]);
            assert!(!wants_optin_flag(&q, "all"), "?all={off} must NOT lift the bound");
        }
        assert!(!wants_optin_flag(&HashMap::new(), "all"), "absent is off");
    }

    /// …and it still honours the opt-in the CLI's `--all` corresponds to.
    #[test]
    fn wants_optin_flag_accepts_bare_presence_and_the_canonical_yes_tokens() {
        for on in ["", "1", "true", "TRUE", "on", "yes", "  yes  "] {
            let q = HashMap::from([("all".to_string(), on.to_string())]);
            assert!(wants_optin_flag(&q, "all"), "?all={on:?} must lift the bound");
        }
    }

    /// The divergence is the point: `wants_flag` and `wants_optin_flag` disagree on
    /// exactly the spellings that made this a defect. Pinned so a future tidy-up
    /// cannot quietly collapse the two readers back into one.
    #[test]
    fn the_two_flag_readers_disagree_on_the_written_out_negatives() {
        for off in ["false", "no", "off"] {
            let q = HashMap::from([("all".to_string(), off.to_string())]);
            assert!(wants_flag(&q, "all"), "wants_flag reads ?all={off} as ON (its only off-token is \"0\")");
            assert!(!wants_optin_flag(&q, "all"), "wants_optin_flag reads ?all={off} as OFF");
        }
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
