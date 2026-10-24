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
//! accessors — `status`, the read-only `latest_*` twins, `coverage_*`,
//! `config_read`, and the pure navigation readers (the wiki's read models are the
//! `wiki` extension's routes since S-644, under the same rule). Loading any
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
//! new degradation rule of its own.
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
use std::sync::Arc;

use axum::{
    extract::{Form, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use logos_core::config::{self as core_config, ConfigReadModel, TierSaveOutcome};
use logos_core::federation::{
    open_state, query as fed_query,
    workspace_statistics, xservice_build_deps, Backing, BoundedReachability, BuildDependencies,
    ContractBridge, DegradedRollup, EngineRegistry, Federation, ReachabilityScope,
    WorkspaceGovernance, WorkspaceStatistics,
};
use logos_core::federation::manifest::{self, ManifestDocument, ManifestSaveOutcome};
use logos_core::history::security::Selector;
use logos_core::history::{CoverageStatus, HotspotReport, TemporalReport};
use logos_core::model::NodeKind;
use logos_core::observability::Surface;
use logos_core::security::{SecurityPage, SecuritySummary};
use logos_core::models::navigation::{
    BranchOverlapResult, GraphElements, GraphGranularity, ImpactIntersectionResult, ImpactResult,
    LanguageComposition, NodeInfo, PrecedentResult, SearchResult, StatusInfo,
};
use logos_core::models::quality::{
    DsmReport, EvolutionReport, GateResult, LanguagesInfo, LatestHealth, RulesReport, ScanResult,
    StatsInfo, VerifyReport,
};
use logos_core::Engine;
use logos_ext::ui::UiManifest;
use logos_ext::web::OverviewField;
use logos_ext::ExtensionHost;

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
pub struct ApiError {
    /// The flattened façade error chain (`{e:#}`), exactly as the HTML views show
    /// it — actionable, never swallowed.
    pub error: String,
}

/// Serialize `model` as a `200 application/json` body — the success arm shared by
/// every endpoint.
pub fn ok<T: Serialize>(model: T) -> Response {
    Json(model).into_response()
}

/// Map a failed read-model to a `500` with the explicit [`ApiError`] chain — the
/// failure is surfaced, not masked ([NFR-RA-05]).
pub fn fail(err: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: format!("{err:#}") })).into_response()
}

/// Collapse a composed `Result` bundle into its response: the serialized bundle on
/// success, the honest `500` error chain on failure.
pub fn respond<T: Serialize>(model: anyhow::Result<T>) -> Response {
    match model {
        Ok(model) => ok(model),
        Err(err) => fail(err),
    }
}

// ── Dashboard / Overview (mirrors `crate::overview`, [FR-UI-09]) ──────────────

/// The Dashboard bundle ([FR-UI-09]): every read-model the verdict-rich landing
/// composes, each field traceable to its `Engine` read-model. Built over the
/// **read-only** accessors so the load is write-free ([ADR-28]). The
/// `composition` defaults on a read fault, and `security` states its fault
/// (`OverviewSecurity::error`) — mirroring the view's own per-widget
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
    /// The Security widget's figures ([FR-SC-08], S-789): the stored summary,
    /// read with no version probe, or the fault that kept it from being read.
    security: OverviewSecurity,
    /// The fields the active extensions add ([`OverviewField`], [FR-DS-04]),
    /// after the core's, each under its own key — the `wiki` extension's
    /// `overview_page`, the Project Overview its Dashboard card renders (S-644).
    /// A key a core field already uses is never added.
    ///
    /// [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
    #[serde(flatten)]
    extensions: serde_json::Map<String, serde_json::Value>,
}

/// The core's own keys in [`OverviewModel`] — an extension field named like one
/// is dropped rather than allowed to shadow it, as the MCP adapter drops an
/// extension tool named like a core tool. Pinned to the model's serialised keys
/// (`the_core_overview_keys_are_the_models_own`).
const OVERVIEW_CORE_KEYS: [&str; 8] =
    ["status", "composition", "languages", "gate", "coverage", "rules", "stats", "security"];

/// The Dashboard's Security widget data ([FR-SC-08], S-789): exactly one of
/// `summary` and `error`. A security read that fails (an invalid
/// `.logos/rules.toml`, consent record or ignore file) is stated by the widget
/// and never blanks the page ([NFR-CC-04]), nor reads as zero findings: the
/// summary is absent, not empty.
///
/// [FR-SC-08]: ../../docs/specs/requirements/FR-SC-08.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
struct OverviewSecurity {
    summary: Option<SecuritySummary>,
    error: Option<String>,
}

impl OverviewSecurity {
    /// The widget data over `e`: the stored summary, no scanner started.
    fn read(e: &Engine) -> Self {
        match e.security_summary() {
            Ok(summary) => Self { summary: Some(summary), error: None },
            Err(err) => Self { summary: None, error: Some(format!("{err:#}")) },
        }
    }
}

/// The extension fields the overview serves, in registration order: a field whose
/// key a core field or an earlier field already holds is dropped, so the first
/// registered wins — the MCP adapter's rule for a tool named like a registered one.
fn admitted_overview_fields(fields: Vec<OverviewField>) -> Vec<OverviewField> {
    let mut taken: Vec<&'static str> = OVERVIEW_CORE_KEYS.to_vec();
    fields
        .into_iter()
        .filter(|field| {
            if taken.contains(&field.key()) {
                tracing::warn!(
                    target: "logos::web",
                    field = field.key(),
                    "an extension overview field is named like a registered field; keeping the registered one"
                );
                return false;
            }
            taken.push(field.key());
            true
        })
        .collect()
}

/// The Dashboard bundle over one engine: the core's read-models, then `fields`.
fn read_overview(e: &Engine, fields: &[OverviewField]) -> anyhow::Result<OverviewModel> {
    Ok(OverviewModel {
        status: e.status(),
        // The view degrades a composition-read fault to the empty card, never
        // blanks the page — mirror that here ([NFR-CC-04]).
        composition: e.language_composition().unwrap_or_default(),
        languages: e.languages(),
        gate: e.latest_gate()?,
        // The evidence alone: the Dashboard probes no coverage tool (S-737).
        coverage: e.coverage_evidence()?,
        rules: e.check_rules(None, false)?,
        stats: e.stats(None),
        security: OverviewSecurity::read(e),
        extensions: fields
            .iter()
            .map(|field| Ok((field.key().to_string(), field.read(e)?)))
            .collect::<anyhow::Result<_>>()?,
    })
}

/// `GET /api/v1/overview` — the Dashboard data ([FR-UI-09], [FR-UI-21]): the
/// core's read-models and the active extensions' fields, all read inside one
/// bridge over the requested member.
pub(crate) async fn overview(
    MemberEngine(engine): MemberEngine,
    State(host): State<ExtensionHost>,
) -> Response {
    let fields = admitted_overview_fields(host.overview_fields());
    let model = bridge(engine, "api_v1_overview", Surface::Web, move |e| read_overview(e, &fields)).await;
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
/// # The "where to start" list ([FR-QM-17], S-505)
///
/// `scan` is the last snapshot's [`ScanResult`], so it already carries that
/// snapshot's advisory ranking as `scan.where_to_start` — read from the same
/// single `latest_health` read as the verdict and the offender lists, and the
/// value [`Engine::improvements`] serves the CLI `improvements` and the MCP tool.
/// No field was added here; `tests/api_v1.rs` pins the equality.
///
/// [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
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
/// # One of the two handlers on this surface that are not [`Surface::Web`]
///
/// The other is [`code_refs`]; both are app chrome ([BR-42]) and they are a closed
/// set, pinned by
/// `every_handler_names_its_surface_and_only_chrome_handlers_name_the_shell`.
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
/// `every_handler_names_its_surface_and_only_chrome_handlers_name_the_shell` guards the
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

// ── Extensions (the host this serve composes, [FR-UI-51]) ─────────────────────

/// What `GET /api/v1/extensions` answers: the active extensions, each with the
/// UI manifest it contributes.
#[derive(Debug, Serialize)]
pub(crate) struct ExtensionsInfo {
    /// The **active** extensions, in registration order. A registered extension
    /// whose feature is not enabled is not here: it contributes nothing anywhere.
    extensions: Vec<ExtensionInfo>,
}

/// One active extension.
#[derive(Debug, Serialize)]
pub(crate) struct ExtensionInfo {
    /// The extension's id.
    id: &'static str,
    /// What it adds to the SPA — `null` for an extension with no UI.
    ui: Option<UiManifest>,
}

/// `GET /api/v1/extensions` — which extensions are active, and their UI
/// manifests ([FR-UI-51], CR-213, [ADR-74]): what the SPA will read on
/// boot to decide which navigation entries, views and config panels to mount.
///
/// A read of the [`ExtensionHost`] the router was composed with, so it reaches no
/// engine: it crosses no adapter boundary and records no telemetry event, hence
/// it names no telemetry surface and is not app chrome in
/// `every_handler_names_its_surface_and_only_chrome_handlers_name_the_shell`'s
/// sense — the closed chrome set stays `status` and `code_refs`. A host with no
/// extension answers `{"extensions":[]}`.
///
/// It sits in the ordinary guard stack: `GET`-only ([`crate::method_guard`]),
/// loopback-only ([`crate::host_guard`]) and same-origin (no cross-origin grant is
/// added; the self-only CSP stamps it).
///
/// [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
/// [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
pub(crate) async fn extensions(State(host): State<ExtensionHost>) -> Response {
    ok(ExtensionsInfo {
        extensions: host
            .active()
            .iter()
            .map(|extension| ExtensionInfo {
                id: extension.id(),
                ui: extension.ui_manifest(),
            })
            .collect(),
    })
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

/// `GET /api/v1/files[?untested][?production_scope][?with_findings]` — the
/// Files & Risk data ([FR-UI-21]). The `?untested` toggle scopes the board to
/// files lacking fresh positive coverage, exactly as the server-rendered view's
/// filter does; the `?production_scope` toggle drops whole test files from the
/// candidate set before ranking ([FR-UI-05], [CR-076]) — the same optional
/// filter the CLI `--production-scope` flag and MCP `production_scope` argument
/// expose, reached through the same [`Engine::latest_hotspots_with_findings`]
/// call. `?with_findings` is the findings filter ([FR-SC-14]): hotspots with a
/// fresh unsuppressed finding rank first, as the CLI `--with-findings` flag and
/// the MCP `with_findings` argument do; the findings column rides every board
/// once a scan is stored.
///
/// [FR-SC-14]: ../../docs/specs/requirements/FR-SC-14.md
pub(crate) async fn files(
    MemberEngine(engine): MemberEngine,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let untested = wants_flag(&params, "untested");
    let production_scope = wants_flag(&params, "production_scope");
    let with_findings = wants_flag(&params, "with_findings");
    let model = bridge(engine, "api_v1_files", Surface::Web, move |e| -> anyhow::Result<FilesModel> {
        Ok(FilesModel {
            status: e.status(),
            hotspots: e.latest_hotspots_with_findings(Some(50), untested, production_scope, with_findings)?,
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
/// the untested hotspot board ([FR-CV-07]). Both reads are read-only. The
/// status carries the coverage runner's state — each tool's state and the
/// execution consent (S-737) — for the page's run controls, read with no
/// version probe: a page load starts no tool process (ADR-38).
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
            coverage: e.coverage_status_probing(logos_core::coverage::Probes::Never)?,
            untested: e.latest_hotspots(Some(20), true, false)?,
        })
    })
    .await;
    respond(model)
}

// ── Security (S-670, [FR-SC-09]) ──────────────────────────────────────────────

/// The Security bundle ([FR-SC-09]): the index status and the page's one
/// read-model — scanner matrix, findings in graph context, both OWASP editions
/// and, with a `rules.toml [security]` table, the gate band
/// ([`Engine::security_page`]). Read-only: it runs no scan — only the version
/// command of a scanner the project's scan consent names, once per process for
/// each binary (HF-3) — and writes no store.
///
/// [FR-SC-09]: ../../docs/specs/requirements/FR-SC-09.md
#[derive(Debug, Serialize)]
pub(crate) struct SecurityModel {
    status: StatusInfo,
    security: SecurityPage,
}

/// `GET /api/v1/security` — the Security page data ([FR-SC-09]). Member-scoped
/// through `?repo=` like every project view. Unfiltered: the page filters the
/// listing and switches edition client-side, so neither refetches.
///
/// [FR-SC-09]: ../../docs/specs/requirements/FR-SC-09.md
pub(crate) async fn security(MemberEngine(engine): MemberEngine) -> Response {
    let model = bridge(engine, "api_v1_security", Surface::Web, |e| -> anyhow::Result<SecurityModel> {
        Ok(SecurityModel { status: e.status(), security: e.security_page()? })
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

// ── Code references (chat answer links, [FR-UI-47]) ───────────────────────────

/// The most distinct spans one resolution request resolves ([FR-UI-47]: the
/// candidate count per turn is capped). The chat client sends at most this many
/// (the chat client's own `CODE_REF_CAP`); a request carrying
/// more is not refused — the excess is counted in `over_cap` and left unresolved,
/// so a hand-built URL cannot buy unbounded lookups.
pub(crate) const CODE_REF_CAP: usize = 24;

/// The longest span considered a candidate. A backticked span longer than any
/// plausible path or symbol is prose or code, and is not looked up.
const CODE_REF_MAX_LEN: usize = 200;

/// What a candidate span LOOKS like — the only thing decided from its text.
/// Whether it links is decided by the graph, never by this shape ([NFR-CC-04]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeRefShape<'a> {
    /// A project-relative path, any `:line` / `:line-line` suffix removed.
    Path(&'a str),
    /// A bare identifier, any trailing `()` removed.
    Symbol(&'a str),
}

/// Classify one backticked span ([FR-UI-47]): a repository path, a `path:line`, or
/// a symbol name — else `None`, and it is never looked up.
///
/// - A bare identifier (`build_router`, `MarkdownAnswer`, `f()`) is a symbol.
/// - A path is `[A-Za-z0-9_./-]` only and either contains `/` or ends in a
///   `.ext` (`web/src/lib.rs`, `build.rs`, `lib.rs:42`, `src/a.rs:3-9`). A leading
///   `./` is dropped, as the navigation readers do.
/// - Everything else is not a candidate: whitespace (a command line), an absolute
///   or parent-relative path (never project-relative), a URL, a flag, a qualified
///   name (`Engine::node` — the graph's name lookup is by bare name, so it could
///   only ever come back unresolved).
fn code_ref_shape(text: &str) -> Option<CodeRefShape<'_>> {
    let text = text.trim();
    if text.is_empty() || text.len() > CODE_REF_MAX_LEN || text.contains(char::is_whitespace) {
        return None;
    }
    let is_ident = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let callable = text.strip_suffix("()").unwrap_or(text);
    if is_ident(callable) {
        return Some(CodeRefShape::Symbol(callable));
    }
    // `path:line` and `path:line-line` name the file; the line is not a node.
    let path = match text.rsplit_once(':') {
        Some((path, line))
            if !line.is_empty()
                && line.split('-').count() <= 2
                && line.split('-').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            path
        }
        _ => text,
    };
    let path = path.strip_prefix("./").unwrap_or(path);
    let path_chars = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'-'));
    let has_extension = path
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && !stem.ends_with('/') && !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric()));
    let rooted_or_parent = path.starts_with('/') || path.split('/').any(|seg| seg.is_empty() || seg == "..");
    if path_chars && !rooted_or_parent && (path.contains('/') || has_extension) {
        return Some(CodeRefShape::Path(path));
    }
    None
}

/// How one span resolved. Only `File` and `Symbol` carry a seed, and only those
/// two become links ([FR-UI-47], [NFR-CC-04]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodeRefOutcome {
    /// An indexed file: `seed` is its project-relative path, a file-tier seed.
    File,
    /// Exactly one node answers to the name: `seed` is its canonical symbol.
    Symbol,
    /// More than one node answers to the name; no link is chosen for the reader.
    Ambiguous,
    /// Nothing in the graph answers to it, or it is not path- or symbol-shaped.
    Unresolved,
}

/// One span's resolution fact.
#[derive(Debug, Serialize)]
pub(crate) struct CodeRef {
    /// The span exactly as sent.
    text: String,
    outcome: CodeRefOutcome,
    /// The `/graph?seed=` value: the file path for `File`, the canonical symbol
    /// for `Symbol`; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<String>,
}

/// The batched resolution ([FR-UI-47]): one fact per distinct span, in the order
/// sent, up to [`CODE_REF_CAP`].
#[derive(Debug, Serialize)]
pub(crate) struct CodeRefResolution {
    /// The per-request candidate cap this answer was resolved under.
    cap: usize,
    /// Distinct spans beyond the cap — received, not resolved.
    over_cap: usize,
    refs: Vec<CodeRef>,
}

/// Resolve one span against the graph through the existing navigation readers.
///
/// A path is asked of the very accessor the Graph view's link will call —
/// `graph_elements` at the file tier, seeded on the path — so a file links only if
/// that view can seed on it. A name is asked of `node`, whose [FR-NV-15] answer
/// lists every other node the bare name matched: none means exactly one symbol,
/// any means ambiguous. Neither is a new core query ([ADR-01]).
///
/// [FR-NV-15]: ../../docs/specs/requirements/FR-NV-15.md
fn resolve_code_ref(engine: &Engine, text: String) -> CodeRef {
    let (outcome, seed) = match code_ref_shape(&text) {
        Some(CodeRefShape::Path(path)) => {
            // `cap: 1` — only whether the seed is in the graph is read, and the
            // seed's own component is non-empty exactly when it is.
            let elements =
                engine.graph_elements(Some(path), Some(1), None, None, Some(GraphGranularity::File), false);
            if elements.total_nodes > 0 {
                (CodeRefOutcome::File, Some(path.to_string()))
            } else {
                (CodeRefOutcome::Unresolved, None)
            }
        }
        Some(CodeRefShape::Symbol(name)) => {
            let info = engine.node(name, false);
            match info.node {
                Some(detail) if info.alternatives.is_empty() => (CodeRefOutcome::Symbol, Some(detail.symbol.symbol)),
                Some(_) => (CodeRefOutcome::Ambiguous, None),
                None => (CodeRefOutcome::Unresolved, None),
            }
        }
        None => (CodeRefOutcome::Unresolved, None),
    };
    CodeRef { text, outcome, seed }
}

/// `GET /api/v1/code-refs?ref=<span>&ref=…` — resolve the backticked spans of one
/// finalized chat answer against the graph, in one batched read ([FR-UI-47]).
///
/// Member-addressed like every member read: in workspace mode `?repo=` selects
/// the member whose graph answers, which is how the Workspace Chat resolves
/// against the addressed member ([FR-WS-34]). `ref` repeats, so the query is read
/// as ordered pairs (as [`impact_intersection`] reads `item`); spans are
/// de-duplicated in arrival order and capped at [`CODE_REF_CAP`].
///
/// A pure reader ([ADR-28]) that returns resolution facts only — no edges, no
/// code. Infallible at the surface: no `ref`, an empty one, or one that is not
/// path- or symbol-shaped is an honest `unresolved`, never a `4xx` ([NFR-CC-04]).
///
/// # App chrome, not graph navigation
///
/// The chat fires this on **every** finalized answer, up to [`CODE_REF_CAP`]
/// spans each, so a single answer makes up to that many traced `node` /
/// `graph_elements` engine calls. No user asked a graph question: the app
/// resolved its own answer's backticks. So it names [`Surface::Shell`] at the
/// `bridge`, like [`status`], and its events never count as a developer's graph
/// navigation in `logos stats` or the usage figures ([BR-42] — classify by whose
/// question a request answers). They are still recorded: the exclusion
/// re-attributes, it does not destroy ([NFR-CC-04]). Owner ruling at the
/// Sprint 95 review of S-622 (finding 10), HF-1.
///
/// The one-route-one-caller caveat on [`status`] applies here too: the chat is
/// this route's only consumer, and a second one asking a genuine graph question
/// would need its own route.
///
/// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
/// [FR-UI-47]: ../../docs/specs/requirements/FR-UI-47.md
/// [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
pub(crate) async fn code_refs(
    MemberEngine(engine): MemberEngine,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let mut spans: Vec<String> = Vec::new();
    for (key, value) in pairs {
        if key == "ref" && !spans.contains(&value) {
            spans.push(value);
        }
    }
    let over_cap = spans.len().saturating_sub(CODE_REF_CAP);
    spans.truncate(CODE_REF_CAP);
    let resolution = bridge(engine, "api_v1_code_refs", Surface::Shell, move |e| CodeRefResolution {
        cap: CODE_REF_CAP,
        over_cap,
        refs: spans.into_iter().map(|text| resolve_code_ref(e, text)).collect(),
    })
    .await;
    ok(resolution)
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
/// [FR-WS-05]/[FR-WS-06], [FR-WS-15], [FR-WS-16], [FR-WS-39]): the coverage
/// dashboard's data.
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
/// # Each row names its identity ([FR-WS-39])
/// `coordinate` is the repository coordinate the member resolves to — the
/// package slot of every symbol it holds — read off the same read-model the CLI
/// prints. A member resolving to the coordinate an earlier one already holds is
/// `degraded` with `degraded_cause: "duplicate-coordinate"` and is excluded from
/// every fan-out, the bridge and the coverage figures, never folded into the
/// first.
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
/// [FR-WS-39]: ../../docs/specs/requirements/FR-WS-39.md
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
/// Without `?all` the payload carries only the **promotions** — the measured
/// cross-service ones, and apart from them the `live-via-asserted-link` ones a
/// valid `[[links]]` call keeps live — and
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
            let reachability =
                fed_query::workspace_reachability(bridge, registry).bound(ReachabilityScope::new(repo, all));
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
            let governance = fed_query::workspace_check(bridge, registry)?;
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

/// The payload of the workspace security reads ([FR-SC-16], [FR-UI-36]): the
/// per-member roll-up under `security` — [`WorkspaceSecurity`](fed_query::WorkspaceSecurity)
/// for `GET /api/v1/workspace/security`,
/// [`WorkspaceSecurityState`](fed_query::WorkspaceSecurityState) for the
/// stored-only `GET /api/v1/workspace/security/summary` — beside the open-state
/// completeness rider every workspace route carries.
///
/// [FR-SC-16]: ../../docs/specs/requirements/FR-SC-16.md
/// [FR-UI-36]: ../../docs/specs/requirements/FR-UI-36.md
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSecurityAnswer<T> {
    security: T,
    #[serde(flatten)]
    completeness: AnswerCompleteness,
}

/// The one body of the two workspace security reads: `read` over the registry,
/// then the completeness rider **after** it — the open-state ledger the rider
/// reads is filled by the member fan-out, so a rider read first would answer
/// `complete: true` on a cold registry. Written once so the two handlers cannot
/// order it differently.
async fn workspace_security_answer<T>(
    backing: Arc<Backing<Engine>>,
    bridge: Arc<ContractBridge>,
    view: &'static str,
    read: fn(&EngineRegistry<Engine>) -> T,
) -> Response
where
    T: Serialize + Send + 'static,
{
    workspace_fan(backing, bridge, view, Surface::Web, move |registry, _bridge| {
        let security = read(registry);
        WorkspaceSecurityAnswer { security, completeness: AnswerCompleteness::after_reading(registry) }
    })
    .await
}

/// `GET /api/v1/workspace/security` — every member's scanner states and fresh,
/// unsuppressed findings, summed with their denominators
/// ([`workspace_security`](fed_query::workspace_security), [FR-SC-16]): the
/// **`app`-scoped** roll-up of what [`security`] answers for one member.
///
/// A member never scanned reads `never_run`, never zero ([BR-66]); a member that
/// could not be opened or read is a row with its `error` beside the others, and
/// counts against `members_read` of `members_total`. The open-state roll-up
/// (`complete`, `degraded_rollup`) names only the members that would not open
/// ([FR-WS-16]), as on every workspace route: one that opened and then failed
/// its security read is named by its row alone. Infallible at the surface for
/// that reason, so this pairs `workspace_fan` with its plain (non-`try`) form,
/// as [`workspace_statistics_aggregate`] does. Read-only: no scan starts, and
/// each member pays only its consented scanners' version probes, once per
/// process for each binary, as its own page does.
/// Answers the same honest `404` under a single-root backing as every other
/// `/api/v1/workspace/*` route.
///
/// [FR-SC-16]: ../../docs/specs/requirements/FR-SC-16.md
/// [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
/// [BR-66]: ../../docs/specs/software-spec.md#328-security-findings
pub(crate) async fn workspace_security(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_security_answer(backing, bridge, "api_v1_workspace_security", fed_query::workspace_security).await
}

/// `GET /api/v1/workspace/security/summary` — the Workspace Dashboard's
/// Security widget ([`workspace_security_state`](fed_query::workspace_security_state),
/// [FR-UI-36], S-790): each member's scan state and fresh, unsuppressed
/// production severity counts, the figures [`workspace_security`] states for
/// the same stores, read from **stored state only**. Unlike that roll-up it
/// starts no version probe — a consented scanner is not invoked either — and
/// carries no total, mean or score across members ([BR-56]); a Dashboard load
/// is a read a cross-site page can trigger ([ADR-80]).
///
/// A member that could not be opened or read is a row with its `error` beside
/// the others. Infallible at the surface, as [`workspace_security`] is, and the
/// same honest `404` under a single-root backing.
///
/// [FR-UI-36]: ../../docs/specs/requirements/FR-UI-36.md
/// [BR-56]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [ADR-80]: ../../docs/specs/architecture/decisions/ADR-80.md
pub(crate) async fn workspace_security_summary(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_security_answer(backing, bridge, "api_v1_workspace_security_summary", fed_query::workspace_security_state)
        .await
}

/// The payload of `GET /api/v1/workspace/coverage` ([FR-CV-16]): the
/// per-member roll-up under `coverage` beside the open-state completeness rider
/// every workspace route carries.
///
/// [FR-CV-16]: ../../docs/specs/requirements/FR-CV-16.md
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceCoverageAnswer {
    coverage: fed_query::WorkspaceCoverage,
    #[serde(flatten)]
    completeness: AnswerCompleteness,
}

/// `GET /api/v1/workspace/coverage` — every member's coverage state, overall
/// line coverage over its fresh files, fresh files of total, tool outcomes and
/// last run, and how many members have fresh coverage of all
/// ([`workspace_coverage`](fed_query::workspace_coverage), [FR-CV-16], S-740):
/// the **`app`-scoped** roll-up of what [`coverage`] answers for one member.
///
/// A member with nothing ingested reads `never_run`, never 0 %; a member that
/// could not be opened or read is a row with its `error` beside the others. The
/// rider is read **after** the fan-out, which fills the open-state ledger it
/// reads, as [`workspace_security_answer`] orders it. Read-only: each member is
/// read as its own Coverage page reads it, with no version probe, so no tool
/// process starts and no coverage runs — the page has no Run button. Infallible
/// at the surface, and the same honest `404` under a single-root backing as
/// every other `/api/v1/workspace/*` route.
///
/// [FR-CV-16]: ../../docs/specs/requirements/FR-CV-16.md
pub(crate) async fn workspace_coverage(
    State(backing): State<Arc<Backing<Engine>>>,
    State(bridge): State<Arc<ContractBridge>>,
) -> Response {
    workspace_fan(backing, bridge, "api_v1_workspace_coverage", Surface::Web, move |registry, _bridge| {
        let coverage = fed_query::workspace_coverage(registry);
        WorkspaceCoverageAnswer { coverage, completeness: AnswerCompleteness::after_reading(registry) }
    })
    .await
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

/// Render a workspace-root write or a security-triage write (S-672): the
/// outcome on success, else the [`ApiError`] at the status the member config
/// routes map the same fault to — `422` for a validation fault, `500` for an
/// I/O one ([`config_write_status`]).
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
/// body carries the live-vs-reindex deltas, a capped leaked/orphaned-symbol
/// sample and capped lost/extra edge lists the Config-tab control ([spa-frontend], S-207) renders.
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

// ── Security scan job (S-673, [FR-SC-15], [ADR-80] §6) ───────────────────────
//
// The Run scan button: a detached, single-flight scan per member on the core's
// job runner, off the request path and off the writer task, so every other
// route keeps answering while it runs. Start and cancel are intent-guarded
// POSTs (`SECURITY_SCAN_POST_ROUTES`); the GET replays the job's per-scanner
// events for the panel that follows it.
//
// [FR-SC-15]: ../../docs/specs/requirements/FR-SC-15.md
// [ADR-80]: ../../docs/specs/architecture/decisions/ADR-80.md

use axum::body::Bytes;
use logos_core::jobs::lock::{peek, Busy};
use logos_core::jobs::{AnalysisJobs, JobKind, JobSnapshot, JobState, Refused, Started};

/// What `POST /api/v1/security/scan` reads. An empty body is `{}`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanStartRequest {
    /// The scanners the consent dialog showed and the user accepted
    /// ([FR-SC-07]). Consent is recorded for these alone: a scanner pending
    /// consent that the dialog did not show is asked for again (`409`).
    ///
    /// [FR-SC-07]: ../../docs/specs/requirements/FR-SC-07.md
    #[serde(default)]
    consent: Vec<String>,
}

/// The member's scan job, running or last run; `null` when none ran in this
/// serve.
#[derive(Debug, Serialize)]
pub(crate) struct ScanJobModel {
    job: Option<JobSnapshot>,
}

/// The `409` a start without consent answers while scanners lack it: the
/// dialog lists exactly these.
#[derive(Debug, Serialize)]
pub(crate) struct ConsentRequired {
    error: String,
    consent_required: Vec<&'static str>,
}

/// What a start resolved to.
enum ScanStart {
    Started(Started),
    ConsentRequired(Vec<&'static str>),
    /// A `tool-install` job runs for the member (S-747): nothing started.
    Busy,
    /// A scan or install started elsewhere — over MCP, from the CLI, in
    /// another process — holds the project's lock (S-758): nothing started.
    HeldElsewhere(Busy),
}

/// `GET /api/v1/security/scan[?repo=]` — the member's scan job with every
/// event it reported so far, oldest first. Reads memory only.
pub(crate) async fn security_scan_job(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    ok(ScanJobModel { job: jobs.snapshot(JobKind::SecurityScan, engine.root()) })
}

/// `POST /api/v1/security/scan[?repo=]` — start the member's scan, or join the
/// one running (`joined: true`, the same job). Answers at once; the scan runs
/// on its own thread. While an installed scanner is not in the project's
/// consent record and `consent` does not name it, nothing starts and the
/// answer is `409` naming every scanner still pending. Otherwise the pending
/// scanners — each one the dialog showed — are recorded in the user-level
/// consent file, never in the repository, and the scan runs. While a scan or
/// install started elsewhere holds the project's lock (S-758), `409` naming
/// it, and nothing is recorded or started.
pub(crate) async fn security_scan_start(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    body: Bytes,
) -> Response {
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        ScanStartRequest::default()
    } else {
        match serde_json::from_slice::<ScanStartRequest>(&body) {
            Ok(request) => request,
            Err(e) => {
                let error = format!("the scan request is not {{\"consent\": [scanner, …]}}: {e}");
                return (StatusCode::BAD_REQUEST, Json(ApiError { error })).into_response();
            }
        }
    };
    let member = Arc::clone(&engine);
    let started = bridge(engine, "api_v1_security_scan_start", Surface::Web, move |e| -> anyhow::Result<ScanStart> {
        let root = e.root().to_path_buf();
        // Refused before anything is recorded, as the install start is;
        // `start_exclusive` below holds the same rule under the runner's
        // lock.
        if jobs
            .snapshot(JobKind::ToolInstall, &root)
            .is_some_and(|job| job.state == JobState::Running)
        {
            return Ok(ScanStart::Busy);
        }
        let running = jobs
            .snapshot(JobKind::SecurityScan, &root)
            .is_some_and(|job| job.state == JobState::Running);
        if !running {
            if let Some(busy) = peek(&e.security_lock_file()?)? {
                return Ok(ScanStart::HeldElsewhere(busy));
            }
            let pending = e.security_consent_pending()?;
            if pending.iter().any(|s| !request.consent.iter().any(|accepted| accepted == s)) {
                return Ok(ScanStart::ConsentRequired(pending));
            }
            if !pending.is_empty() {
                e.security_record_consent(&pending)?;
            }
        }
        // Consent is now recorded for every scanner the dialog showed; one
        // installed since is refused by the scan itself, never consented to on
        // the user's behalf.
        // An install job (S-747) runs its own scan once it is done; a second
        // scan beside it is refused, never run over the same store, as is one
        // beside a scan or install the project's lock names (S-758).
        let lock_file = e.security_lock_file()?;
        let started = jobs.start_exclusive(JobKind::SecurityScan, &root, &lock_file, &[JobKind::ToolInstall], move |job| {
            logos_core::observability::in_surface(Surface::Web, || {
                let summary = member.security_scan_reporting(false, |step| {
                    job.emit(serde_json::to_value(step).unwrap_or_default());
                })?;
                Ok(serde_json::to_value(summary)?)
            })
        })?;
        Ok(match started {
            Ok(started) => ScanStart::Started(started),
            Err(Refused::Running(_)) => ScanStart::Busy,
            Err(Refused::Elsewhere(busy)) => ScanStart::HeldElsewhere(busy),
        })
    })
    .await;
    match started {
        Ok(ScanStart::Started(started)) => ok(started),
        Ok(ScanStart::Busy) => {
            let error = "scanners are being installed for this project; wait for the install to finish or cancel it, then scan".to_string();
            (StatusCode::CONFLICT, Json(ApiError { error })).into_response()
        }
        Ok(ScanStart::HeldElsewhere(busy)) => (StatusCode::CONFLICT, Json(ApiError { error: busy.to_string() })).into_response(),
        Ok(ScanStart::ConsentRequired(scanners)) => {
            let error = format!(
                "a scan needs consent first: {} may reach the network to refresh rules or vulnerability data",
                scanners.join(", ")
            );
            (StatusCode::CONFLICT, Json(ConsentRequired { error, consent_required: scanners })).into_response()
        }
        Err(e) => fail(e),
    }
}

/// `POST /api/v1/security/scan/cancel[?repo=]` — cancel the member's running
/// scan: every scanner process group is killed and no scanner starts after.
/// Answers the job as it stands (still `running` until the scan notices, an
/// instant later); a job that already ended is left as it is; `null` when
/// none ran.
pub(crate) async fn security_scan_cancel(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    ok(ScanJobModel { job: jobs.cancel(JobKind::SecurityScan, engine.root()) })
}

// ── Scanner install (S-747, [FR-SC-24], [FR-SC-22]) ──────────────────────────
//
// A not-installed row's Install, an installed row's Update, and Install & scan:
// a `tool-install` job per member on the same runner as Run scan, cancellable,
// its per-tool steps (then the scan's per-scanner steps) replayed by the GET.
// Start and cancel are intent-guarded POSTs (`SECURITY_INSTALL_POST_ROUTES`).
// Nothing is downloaded until the one consent dialog has answered every line
// the install needs; until then the start answers `409` with the plan the
// dialog shows, records nothing and starts no process (S-766).
//
// [FR-SC-24]: ../../docs/specs/requirements/FR-SC-24.md
// [FR-SC-22]: ../../docs/specs/requirements/FR-SC-22.md

use logos_core::tools::surface::{InstallAdmission, InstallAsk, InstallPlan};

/// What `POST /api/v1/security/install` reads. An empty body is `{}`: the
/// default set, no scan, nothing answered.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallStartRequest {
    /// The tools a row names; absent for Install & scan's default set.
    #[serde(default)]
    tools: Option<Vec<String>>,
    /// Replace an older managed copy with the pin (a row's Update).
    #[serde(default)]
    update: bool,
    /// Scan once installed (Install & scan).
    #[serde(default)]
    scan: bool,
    /// The offered opt-in tools the user ticked.
    #[serde(default)]
    opt_in: Vec<String>,
    /// The `<tool> <version>` lines the dialog showed and the user accepted.
    #[serde(default)]
    consent: Vec<String>,
    /// The tools whose licence line the user ticked.
    #[serde(default)]
    accept_licences: Vec<String>,
    /// The scanners whose network consent the user accepted.
    #[serde(default)]
    scan_consent: Vec<String>,
}

/// The `409` of a start with a line still unanswered: the plan the consent
/// dialog shows.
#[derive(Debug, Serialize)]
pub(crate) struct InstallConsentRequired {
    error: String,
    plan: InstallPlan,
}

/// What an install start resolved to.
enum InstallStart {
    Started(Started),
    Ask(InstallPlan),
    /// A job of this kind runs for the member — an install, or a scan:
    /// nothing started.
    Busy(JobKind),
    /// A scan or install started elsewhere holds the project's lock (S-758):
    /// nothing started.
    HeldElsewhere(Busy),
}

/// `GET /api/v1/security/install[?repo=]` — the member's `tool-install` job,
/// running or last run, with every step it reported so far. Reads memory only.
pub(crate) async fn security_install_job(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    ok(ScanJobModel { job: jobs.snapshot(JobKind::ToolInstall, engine.root()) })
}

/// `POST /api/v1/security/install[?repo=]` — start the member's install.
/// While a line it needs is unanswered — an install consent, a licence, the
/// project's network consent for the scan — nothing is recorded or fetched,
/// no process starts (admission decides on recorded state only, never a
/// copy's probed version) and the answer is `409` with the plan. Otherwise the project's network
/// consent is recorded for the scanners the dialog showed, and the job
/// installs (recording the install consent for exactly the lines accepted),
/// then scans when asked. While an install or a scan runs for the member,
/// `409` and nothing starts: unlike Run scan's, a second install is never
/// joined, since its request may name other tools than the running one. The
/// same holds for a scan or install started elsewhere — over MCP, from the
/// CLI, in another process — whose `409` names it (S-758).
pub(crate) async fn security_install_start(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    body: Bytes,
) -> Response {
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        InstallStartRequest::default()
    } else {
        match serde_json::from_slice::<InstallStartRequest>(&body) {
            Ok(request) => request,
            Err(e) => {
                let error = format!("the install request is not a valid install: {e}");
                return (StatusCode::BAD_REQUEST, Json(ApiError { error })).into_response();
            }
        }
    };
    let ask = InstallAsk {
        tools: request.tools,
        update: request.update,
        scan: request.scan,
        opt_in: request.opt_in,
        consent: request.consent,
        accept_licences: request.accept_licences,
        scan_consent: request.scan_consent,
    };
    let member = Arc::clone(&engine);
    let started = bridge(engine, "api_v1_security_install_start", Surface::Web, move |e| -> anyhow::Result<InstallStart> {
        let root = e.root().to_path_buf();
        // Refused before anything is recorded; `start_exclusive` below
        // holds the same rule under the runner's lock.
        let busy = [JobKind::ToolInstall, JobKind::SecurityScan]
            .into_iter()
            .find(|kind| jobs.snapshot(*kind, &root).is_some_and(|job| job.state == JobState::Running));
        if let Some(kind) = busy {
            return Ok(InstallStart::Busy(kind));
        }
        if let Some(busy) = peek(&e.security_lock_file()?)? {
            return Ok(InstallStart::HeldElsewhere(busy));
        }
        let (request, scan) = match e.security_install_admit(&ask)? {
            InstallAdmission::Ask(plan) => return Ok(InstallStart::Ask(plan)),
            InstallAdmission::Ready { request, scan, scan_consent } => {
                if !scan_consent.is_empty() {
                    let names: Vec<&str> = scan_consent.iter().map(String::as_str).collect();
                    e.security_record_consent(&names)?;
                }
                (request, scan)
            }
        };
        let excluded = [JobKind::ToolInstall, JobKind::SecurityScan];
        let lock_file = e.security_lock_file()?;
        let started = jobs.start_exclusive(JobKind::ToolInstall, &root, &lock_file, &excluded, move |job| {
            logos_core::observability::in_surface(Surface::Web, || {
                let done = member.security_install_job(&request, scan, |step| {
                    job.emit(serde_json::to_value(step).unwrap_or_default());
                })?;
                Ok(serde_json::to_value(done)?)
            })
        })?;
        Ok(match started {
            Ok(started) => InstallStart::Started(started),
            Err(Refused::Running(running)) => InstallStart::Busy(running.kind),
            Err(Refused::Elsewhere(busy)) => InstallStart::HeldElsewhere(busy),
        })
    })
    .await;
    match started {
        Ok(InstallStart::Started(started)) => ok(started),
        Ok(InstallStart::Ask(plan)) => {
            let error = "the install needs your consent first: each tool below is downloaded, verified and run on this machine".to_string();
            (StatusCode::CONFLICT, Json(InstallConsentRequired { error, plan })).into_response()
        }
        Ok(InstallStart::Busy(kind)) => {
            let error = match kind {
                JobKind::ToolInstall => "an install is already running on this project; wait for it to finish or cancel it, then install again",
                JobKind::SecurityScan => "a scan is running on this project; wait for it to finish or cancel it, then install",
                // Never excluded: a coverage run goes beside an install (S-738).
                JobKind::CoverageRun => "a coverage run is running on this project; wait for it to finish or cancel it, then install",
            }
            .to_string();
            (StatusCode::CONFLICT, Json(ApiError { error })).into_response()
        }
        Ok(InstallStart::HeldElsewhere(busy)) => (StatusCode::CONFLICT, Json(ApiError { error: busy.to_string() })).into_response(),
        Err(e) => fail(e),
    }
}

/// `POST /api/v1/security/install/cancel[?repo=]` — cancel the member's
/// running install: the download, unpack or scanner running is killed, no
/// partial tool is left, and the job's last step names the tools installed by
/// then. Answers the job as it stands; `null` when none ran.
pub(crate) async fn security_install_cancel(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    ok(ScanJobModel { job: jobs.cancel(JobKind::ToolInstall, engine.root()) })
}

// ── Coverage run job (S-738, [FR-CV-15], [ADR-87] §5) ────────────────────────
//
// The Run coverage button's server side: a `coverage-run` job per member on
// the runner the security scan runs on, independent of it — a scan and a
// coverage run go side by side, and a second start joins the running run.
// Start and cancel are intent-guarded POSTs (`COVERAGE_RUN_POST_ROUTES`).
// Nothing starts until the project's execution consent covers the run: a
// start that the consent dialog has not answered for every tool it lacks —
// and for the custom command's exact text — answers `409` with what is
// pending, records nothing and starts no process ([FR-CV-13]). The GET
// answers the job as it stands, or, to a client that accepts
// `text/event-stream`, streams its events as they happen.
//
// [FR-CV-15]: ../../docs/specs/requirements/FR-CV-15.md
// [FR-CV-13]: ../../docs/specs/requirements/FR-CV-13.md
// [ADR-87]: ../../docs/specs/architecture/decisions/ADR-87.md

use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use logos_core::coverage::consent::{ConsentAnswer, PendingConsent};
use logos_core::coverage::CoverageJobStart;
use logos_core::jobs::JobHandle;

/// How often the event stream reads the job for what is new.
const COVERAGE_STREAM_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// What `POST /api/v1/coverage/run` reads. An empty body is `{}`: every
/// enabled, installed tool, nothing answered.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageRunStartRequest {
    /// Run only this tool — a registry name or `custom` — rather than all.
    #[serde(default)]
    tool: Option<String>,
    /// The tools the consent dialog showed and the user accepted (`custom`
    /// for the custom command).
    #[serde(default)]
    consent: Vec<String>,
    /// The custom command's text as the dialog showed it: the consent pins
    /// exactly this text, so one changed since is asked for again.
    #[serde(default)]
    refresh_cmd: Option<String>,
}

/// The `409` of a start the consent does not cover yet: what the dialog
/// shows — the tools, and the custom command verbatim.
#[derive(Debug, Serialize)]
pub(crate) struct CoverageConsentRequired {
    error: String,
    /// What is pending; its `refresh_cmd` is the exact text a start echoes
    /// back to accept it.
    consent_required: PendingConsent,
    /// The custom command as the dialog displays it: control and bidi
    /// characters escaped, as the terminal consent shows it
    /// ([`shown`](logos_core::coverage::consent::shown)), so that the command
    /// a user reads is the command that runs.
    refresh_cmd_shown: Option<String>,
}

/// `GET /api/v1/coverage/run[?repo=]` — the member's `coverage-run` job,
/// running or last run, with every event it reported so far; `null` when none
/// ran in this serve. Reads memory only. With `Accept: text/event-stream` the
/// answer is a stream instead ([`coverage_run_stream`]).
pub(crate) async fn coverage_run_job(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    headers: HeaderMap,
) -> Response {
    if crate::wants_event_stream(&headers) {
        return coverage_run_stream(jobs.handle(JobKind::CoverageRun, engine.root())).into_response();
    }
    ok(ScanJobModel { job: jobs.snapshot(JobKind::CoverageRun, engine.root()) })
}

/// The member's coverage job as Server-Sent Events: each event the job kept
/// — the replay first, so a late subscriber catches up — as a `step` whose
/// data is the [`JobEvent`](logos_core::jobs::JobEvent), then one `end` whose
/// data is `{"job": …}`, the job as it ended without its events, and the
/// stream closes. The stream follows the job that was there when it opened
/// (`job`), never a later run that takes its place. With no job, the `end`
/// carries `{"job": null}` at once. An event dropped past the job's bound is
/// never sent.
fn coverage_run_stream(
    job: Option<JobHandle>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    struct Follow {
        job: Option<JobHandle>,
        /// The `seq` of the first event not sent yet.
        next: u64,
        queued: std::collections::VecDeque<Event>,
        ended: bool,
    }
    fn json_event(name: &str, data: &impl Serialize) -> Event {
        let event = Event::default().event(name);
        event.clone().json_data(data).unwrap_or(event)
    }
    let follow = Follow { job, next: 0, queued: std::collections::VecDeque::new(), ended: false };
    let stream = futures_util::stream::unfold(follow, |mut f| async move {
        loop {
            if let Some(event) = f.queued.pop_front() {
                return Some((Ok(event), f));
            }
            if f.ended {
                return None;
            }
            // One snapshot is taken under the job's lock: once it reads ended,
            // every event the job will ever report is in it.
            let Some(mut job) = f.job.as_ref().map(JobHandle::snapshot) else {
                f.queued.push_back(json_event("end", &ScanJobModel { job: None }));
                f.ended = true;
                continue;
            };
            let from = f.next;
            for step in job.events.iter().filter(|step| step.seq >= from) {
                f.queued.push_back(json_event("step", step));
                f.next = step.seq + 1;
            }
            if job.state != JobState::Running {
                job.events.clear();
                f.queued.push_back(json_event("end", &ScanJobModel { job: Some(job) }));
                f.ended = true;
            } else if f.queued.is_empty() {
                tokio::time::sleep(COVERAGE_STREAM_POLL).await;
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod coverage_stream_tests {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use logos_core::jobs::{AnalysisJobs, JobKind, JobState};
    use serde_json::json;

    use super::coverage_run_stream;

    /// A review finding (S-738): when the run a stream follows ends and the
    /// next one starts within one poll, the stream still ends with the run it
    /// followed — its `end`, its result — and sends none of the next run's
    /// events.
    #[tokio::test]
    async fn the_stream_ends_with_the_run_it_followed_never_the_next() {
        let jobs = std::sync::Arc::new(AnalysisJobs::new());
        let root = Path::new("/project");
        let (release, hold) = mpsc::channel::<()>();
        jobs.start(JobKind::CoverageRun, root, move |ctx| {
            for i in 0..3 {
                ctx.emit(json!({ "run": "A", "i": i }));
            }
            hold.recv().ok();
            Ok(json!("A's summary"))
        })
        .unwrap();
        let mut body = coverage_run_stream(jobs.handle(JobKind::CoverageRun, root)).into_response().into_body();
        let mut text = String::new();
        while text.matches("event: step").count() < 3 {
            let frame = body.frame().await.expect("a frame").unwrap();
            text.push_str(&String::from_utf8_lossy(frame.data_ref().unwrap()));
        }

        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while jobs.snapshot(JobKind::CoverageRun, root).unwrap().state == JobState::Running {
            assert!(Instant::now() < deadline, "A never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        jobs.start(JobKind::CoverageRun, root, |ctx| {
            for i in 0..5 {
                ctx.emit(json!({ "run": "B", "i": i }));
            }
            std::thread::sleep(Duration::from_millis(600));
            Ok(json!("B's summary"))
        })
        .unwrap();

        let rest = body.collect().await.unwrap().to_bytes();
        text.push_str(&String::from_utf8_lossy(&rest));
        assert!(!text.contains(r#""run":"B""#), "the stream sent the next run's events: {text}");
        let end = text.split("event: end").nth(1).expect("an end event");
        assert!(end.contains("A's summary") && !end.contains("B's summary"), "the end is not A's: {end}");
    }
}

/// `POST /api/v1/coverage/run[?repo=]` — start the member's coverage run, or
/// join the one running (`joined: true`, the same job). Answers at once; the
/// run goes on its own thread, beside any scan. While the project's execution
/// consent lacks a tool the run would start, or the custom command's exact
/// text, and the request's `consent` and `refresh_cmd` do not accept all of
/// it, nothing is recorded or started and the answer is `409` with what is
/// pending. Otherwise exactly what was pending is recorded in the user-level
/// consent record, never in the repository, and the run starts.
pub(crate) async fn coverage_run_start(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    body: Bytes,
) -> Response {
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        CoverageRunStartRequest::default()
    } else {
        match serde_json::from_slice::<CoverageRunStartRequest>(&body) {
            Ok(request) => request,
            Err(e) => {
                let error = format!(
                    "the coverage run request is not {{\"tool\"?: …, \"consent\": [tool, …], \"refresh_cmd\"?: …}}: {e}"
                );
                return (StatusCode::BAD_REQUEST, Json(ApiError { error })).into_response();
            }
        }
    };
    let started = run_blocking("api_v1_coverage_run_start", Surface::Web, move || {
        let answer = ConsentAnswer { tools: &request.consent, refresh_cmd: request.refresh_cmd.as_deref() };
        logos_core::coverage::job::start(&engine, &jobs, request.tool.as_deref(), Some(&answer), Surface::Web)
    })
    .await;
    match started {
        Ok(CoverageJobStart::Started(started)) => ok(started),
        Ok(CoverageJobStart::Ask(pending)) => {
            let error = format!(
                "a coverage run needs consent first: it builds this project and runs its tests ({}), which executes \
                 the project's own code and may reach the network",
                pending.tools.join(", ")
            );
            let refresh_cmd_shown = pending.refresh_cmd.as_deref().map(logos_core::coverage::consent::shown);
            let refused = CoverageConsentRequired { error, consent_required: pending, refresh_cmd_shown };
            (StatusCode::CONFLICT, Json(refused)).into_response()
        }
        Err(e) => fail(e),
    }
}

/// `POST /api/v1/coverage/run/cancel[?repo=]` — cancel the member's running
/// coverage run: the tool running is killed with its whole process group —
/// a build daemon or a forked test JVM in it included — and no tool starts
/// after. Answers the job as it stands (still `running` until the run
/// notices, an instant later); a job that already ended is left as it is;
/// `null` when none ran.
pub(crate) async fn coverage_run_cancel(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
) -> Response {
    ok(ScanJobModel { job: jobs.cancel(JobKind::CoverageRun, engine.root()) })
}

// ── Install and Add dependency before a coverage run (S-750, [FR-CV-17], [FR-CV-18]) ──
//
// The Coverage tools widget's **Install** (a managed coverage tool missing from
// the machine) and **Add dependency** (a tool's missing coverage dev
// dependency): each starts the member's `coverage-run` job with the install or
// the addition as its first step, then the run — followed and cancelled on the
// run's own routes. Intent-guarded POSTs (`COVERAGE_PREPARE_POST_ROUTES`).
// One dialog answers every consent the start needs; until it has, the answer
// is `409` with what it shows — the run's `consent_required` as Run coverage's
// `409` carries it, plus the install lines and the dependency's exact command
// and files — and nothing is recorded, downloaded or run.
//
// [FR-CV-17]: ../../docs/specs/requirements/FR-CV-17.md
// [FR-CV-18]: ../../docs/specs/requirements/FR-CV-18.md

use logos_core::coverage::job::{DependencyAsk, Prepare, PrepareAnswer, PreparedStart};
use logos_core::tools::surface::CoverageInstallLine;

/// What `POST /api/v1/coverage/install` and `/dependency` read. An empty body
/// is `{}`: nothing answered.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoveragePrepareRequest {
    /// The registry tool whose dependency to add; the dependency route only.
    #[serde(default)]
    tool: Option<String>,
    /// The tools the dialog showed for the execution consent.
    #[serde(default)]
    consent: Vec<String>,
    /// The custom command's exact text, when the dialog showed it.
    #[serde(default)]
    refresh_cmd: Option<String>,
    /// The install lines accepted: `<tool> <version>`, or the `rustup` command.
    #[serde(default)]
    install: Vec<String>,
    /// The dependency commands accepted, exactly as shown.
    #[serde(default)]
    dependency: Vec<String>,
}

/// The `409` of a prepared start with a line unanswered: Run coverage's
/// question, plus the install lines and the dependency to add.
#[derive(Debug, Serialize)]
pub(crate) struct CoveragePrepareRequired {
    error: String,
    consent_required: PendingConsent,
    refresh_cmd_shown: Option<String>,
    install: Vec<CoverageInstallLine>,
    dependencies: Vec<DependencyAsk>,
}

/// The `422` of a dependency Logos will not add here: why, and the command to
/// run by hand.
#[derive(Debug, Serialize)]
pub(crate) struct CoverageDependencyRefused {
    error: String,
    hand_command: String,
}

/// `POST /api/v1/coverage/install[?repo=]` — install the missing managed
/// coverage tools and `llvm-tools-preview`, then run coverage, as the
/// member's `coverage-run` job ([FR-CV-17]).
pub(crate) async fn coverage_install_start(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    body: Bytes,
) -> Response {
    coverage_prepared_start(jobs, engine, body, false).await
}

/// `POST /api/v1/coverage/dependency[?repo=]` — add `tool`'s missing coverage
/// dev dependency with the project's own package manager, then run coverage,
/// as the member's `coverage-run` job ([FR-CV-18]). A dependency Logos will
/// not add here is `422`, naming the command to run by hand.
pub(crate) async fn coverage_dependency_start(
    State(jobs): State<Arc<AnalysisJobs>>,
    MemberEngine(engine): MemberEngine,
    body: Bytes,
) -> Response {
    coverage_prepared_start(jobs, engine, body, true).await
}

/// Both prepared starts: `dependency` is the route. Answers at once; the job
/// goes on its own thread. A coverage run already going is `409`, never
/// joined, since it would not ready the tool; so is a scan or install holding
/// the project's lock, for an install.
async fn coverage_prepared_start(jobs: Arc<AnalysisJobs>, engine: Arc<Engine>, body: Bytes, dependency: bool) -> Response {
    let bad = |error: String| (StatusCode::BAD_REQUEST, Json(ApiError { error })).into_response();
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        CoveragePrepareRequest::default()
    } else {
        match serde_json::from_slice::<CoveragePrepareRequest>(&body) {
            Ok(request) => request,
            Err(e) => {
                return bad(format!(
                    "the request is not {{\"tool\"?: …, \"consent\": [tool, …], \"refresh_cmd\"?: …, \"install\": [line, …], \
                     \"dependency\": [command, …]}}: {e}"
                ))
            }
        }
    };
    let prepare = match (dependency, request.tool.clone()) {
        (true, Some(tool)) => Prepare::Dependency(tool),
        (true, None) => return bad("Add dependency names the tool whose dependency to add: {\"tool\": …}".to_string()),
        (false, None) => Prepare::Install,
        (false, Some(_)) => return bad("Install takes no tool: it installs every missing managed coverage tool".to_string()),
    };
    let started = run_blocking("api_v1_coverage_prepared_start", Surface::Web, move || {
        let answer = PrepareAnswer {
            run: ConsentAnswer { tools: &request.consent, refresh_cmd: request.refresh_cmd.as_deref() },
            install: &request.install,
            dependency: &request.dependency,
        };
        logos_core::coverage::job::start_prepared(&engine, &jobs, &prepare, &answer, Surface::Web)
    })
    .await;
    let conflict = |error: String| (StatusCode::CONFLICT, Json(ApiError { error })).into_response();
    match started {
        Ok(PreparedStart::Started(started)) => ok(started),
        Ok(PreparedStart::Ask(pending)) => {
            let error = "this needs your consent first: each line below is downloaded and run, or changes the \
                         project, on this machine"
                .to_string();
            let refresh_cmd_shown = pending.run.refresh_cmd.as_deref().map(logos_core::coverage::consent::shown);
            let required = CoveragePrepareRequired {
                error,
                consent_required: pending.run,
                refresh_cmd_shown,
                install: pending.install,
                dependencies: pending.dependencies,
            };
            (StatusCode::CONFLICT, Json(required)).into_response()
        }
        Ok(PreparedStart::Refused { reason, hand_command }) => {
            let error = format!("{reason}. Nothing was changed. Run it by hand: {hand_command}");
            (StatusCode::UNPROCESSABLE_ENTITY, Json(CoverageDependencyRefused { error, hand_command })).into_response()
        }
        Ok(PreparedStart::Running) => conflict(
            "a coverage run is going on this project; wait for it to finish or cancel it, then start again".to_string(),
        ),
        Ok(PreparedStart::Busy(busy)) => conflict(busy.to_string()),
        Err(e) => fail(e),
    }
}

// ── Security triage (S-672, [FR-SC-12]) ──────────────────────────────────────
//
// The suppression dialog's two writes: add one entry to the committed
// `.logos/security-ignore.toml`, and remove one. Both are intent-guarded POSTs
// (`SECURITY_TRIAGE_POST_ROUTES`) scoped to the member (`?repo=`). A refused
// entry or a hand-spelled file is `422` with the reason and no write; a failed
// write is `500`.
//
// [FR-SC-12]: ../../docs/specs/requirements/FR-SC-12.md

/// What `POST /api/v1/security/suppress` reads: one finding's `fingerprint`,
/// or a `path` glob optionally narrowed by `rule` and `scanner` (S-770).
/// Exactly one of `fingerprint` and `path` is checked when the selector is
/// built, so both or neither is a `422` that says so.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SuppressRequest {
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    rule: Option<String>,
    #[serde(default)]
    scanner: Option<String>,
    reason: String,
    /// `YYYY-MM-DD`, the last day the entry suppresses; absent or `null`
    /// suppresses until removed.
    #[serde(default)]
    expires: Option<String>,
}

/// What `POST /api/v1/security/unsuppress` reads: the selector of the entry
/// to remove, as a finding's `suppression.by` or the entry itself spells it.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsuppressRequest {
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    rule: Option<String>,
    #[serde(default)]
    scanner: Option<String>,
}

/// A triage body, or the `400` that names the shape it should have had.
fn triage_body<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, (StatusCode, Json<ApiError>)> {
    serde_json::from_slice(body).map_err(|e| {
        let error = format!("the triage request is not {shape}: {e}");
        (StatusCode::BAD_REQUEST, Json(ApiError { error }))
    })
}

/// `POST /api/v1/security/suppress[?repo=]` — add a suppression for one
/// finding, or for every finding under a path: the written file, the entry and
/// the file's entry count.
pub(crate) async fn security_suppress(MemberEngine(engine): MemberEngine, body: Bytes) -> Response {
    let request: SuppressRequest = match triage_body(
        &body,
        r#"{"fingerprint": …} or {"path": …, "rule"?: …, "scanner"?: …}, with "reason": … and "expires": "YYYY-MM-DD" | null"#,
    ) {
        Ok(request) => request,
        Err(refused) => return refused.into_response(),
    };
    let outcome = bridge(engine, "api_v1_security_suppress", Surface::Web, move |e| {
        let selector = Selector::from_parts(request.fingerprint, request.path, request.rule, request.scanner)?;
        e.security_suppress(selector, &request.reason, request.expires.as_deref())
    })
    .await;
    written(outcome)
}

/// `POST /api/v1/security/unsuppress[?repo=]` — remove one suppression entry:
/// the written file, the entry removed and the entries left.
pub(crate) async fn security_unsuppress(MemberEngine(engine): MemberEngine, body: Bytes) -> Response {
    let request: UnsuppressRequest =
        match triage_body(&body, r#"{"fingerprint": …} or {"path": …, "rule"?: …, "scanner"?: …}"#) {
            Ok(request) => request,
            Err(refused) => return refused.into_response(),
        };
    let outcome = bridge(engine, "api_v1_security_unsuppress", Surface::Web, move |e| {
        let selector = Selector::from_parts(request.fingerprint, request.path, request.rule, request.scanner)?;
        e.security_unsuppress(selector)
    })
    .await;
    written(outcome)
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

    /// The core-key list the overview's shadow guard reads is the model's own: the
    /// keys a bare overview serialises, no more and no fewer — so a core field
    /// added later is guarded the day it lands.
    #[test]
    fn the_core_overview_keys_are_the_models_own() {
        let dir = tempfile::TempDir::new().expect("temp root");
        let engine = Engine::start(dir.path()).expect("the engine starts");
        let model = read_overview(&engine, &[]).expect("the overview reads");
        let value = serde_json::to_value(model).expect("serialise");
        let mut served: Vec<&str> = value.as_object().expect("an object").keys().map(String::as_str).collect();
        let mut core = OVERVIEW_CORE_KEYS.to_vec();
        served.sort_unstable();
        core.sort_unstable();
        assert_eq!(served, core);
    }

    /// A field that fails fails the bundle, as a required core read does — never a
    /// `200` carrying a `null` the Dashboard card would read as "not written yet".
    #[test]
    fn a_failing_overview_field_fails_the_bundle() {
        let dir = tempfile::TempDir::new().expect("temp root");
        let engine = Engine::start(dir.path()).expect("the engine starts");
        let failing =
            OverviewField::new("page", |_: &Engine| -> anyhow::Result<()> { anyhow::bail!("the page read failed") });
        let err = read_overview(&engine, &[failing]).expect_err("a failing field fails the bundle");
        assert_eq!(err.to_string(), "the page read failed");
    }

    /// An extension field named like a core field, or like an earlier extension
    /// field, is dropped: the first registered wins, as in the MCP adapter.
    #[test]
    fn an_overview_field_never_shadows_a_registered_one() {
        let field = |key: &'static str, value: &'static str| {
            OverviewField::new(key, move |_: &Engine| Ok::<_, anyhow::Error>(value))
        };
        let admitted = admitted_overview_fields(vec![
            field("status", "shadow"),
            field("page", "first"),
            field("page", "second"),
            field("card", "card"),
        ]);
        let dir = tempfile::TempDir::new().expect("temp root");
        let engine = Engine::start(dir.path()).expect("the engine starts");
        let read: Vec<(&str, serde_json::Value)> =
            admitted.iter().map(|f| (f.key(), f.read(&engine).expect("reads"))).collect();
        assert_eq!(read, [("page", serde_json::json!("first")), ("card", serde_json::json!("card"))]);
    }

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

    /// The shape gate decides only what is LOOKED UP ([FR-UI-47]); each accepted
    /// form sits beside the near miss one character away that must not be.
    #[test]
    fn code_ref_shape_admits_paths_lines_and_bare_names_and_rejects_their_near_misses() {
        use CodeRefShape::{Path, Symbol};
        for (span, want) in [
            ("build_router", Some(Symbol("build_router"))),
            ("MarkdownAnswer", Some(Symbol("MarkdownAnswer"))),
            ("f()", Some(Symbol("f"))),
            ("_private", Some(Symbol("_private"))),
            ("web/src/lib.rs", Some(Path("web/src/lib.rs"))),
            ("./web/src/lib.rs", Some(Path("web/src/lib.rs"))),
            ("web/src/lib.rs:42", Some(Path("web/src/lib.rs"))),
            ("web/src/lib.rs:42-57", Some(Path("web/src/lib.rs"))),
            ("build.rs", Some(Path("build.rs"))),
            ("web/ui", Some(Path("web/ui"))),
            // Near misses.
            ("f(x)", None),
            ("9lives", None),
            ("Engine::node", None),
            ("web/src/lib.rs:", None),
            ("web/src/lib.rs:4a", None),
            ("web/src/lib.rs:1-2-3", None),
            ("web/src/lib.rs:-2", None),
            ("/etc/passwd", None),
            ("../secret.rs", None),
            ("web//lib.rs", None),
            ("web/", None),
            (".gitignore", None),
            ("lib.", None),
            ("https://example.com/a.rs", None),
            ("cargo test", None),
            ("--release", None),
            ("", None),
        ] {
            assert_eq!(code_ref_shape(span), want, "`{span}`");
        }
        let long = "a".repeat(CODE_REF_MAX_LEN + 1);
        assert_eq!(code_ref_shape(&long), None, "a span over the length bound is not looked up");
        assert_eq!(
            code_ref_shape(&"a".repeat(CODE_REF_MAX_LEN)),
            Some(Symbol(&"a".repeat(CODE_REF_MAX_LEN))),
            "the bound itself is admitted"
        );
    }
}
