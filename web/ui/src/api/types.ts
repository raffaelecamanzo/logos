/*
 * Typed mirrors of the same-origin `/api/v1/*` JSON read-models (S-186, CR-049,
 * FR-UI-21). These shapes are the wire contract the embedded SPA consumes — they
 * mirror the Rust read-model structs (`logos-core/src/models/navigation.rs`,
 * `web/src/query.rs`, `web/src/api_v1.rs`) field-for-field. They are internal to
 * the bundled frontend (same binary, same version), not a public API.
 *
 * Wire conventions (mirrored from the Rust serde derives):
 *   - `NodeKind` / `EdgeKind` serialize to lower snake_case tokens (`function`,
 *     `type_uses`, `forbidden_dependency`); kept as `string` here and keyed into
 *     the canvas color/style maps by that token.
 *   - `GraphLayer` is the closed set `"code" | "doc" | "artifact"`.
 *   - Optional Rust fields (`Option<T>`) are `T | null` on the wire.
 *
 * This is the shared type surface S-187–S-189 extend with their own read-models.
 */

/** The presentation layer a graph node renders in (mirrors `GraphLayer`). */
export type GraphLayer = "code" | "doc" | "artifact";

/** The semantic cluster-zoom tier (mirrors `GraphGranularity`). */
export type GraphGranularity = "symbol" | "file" | "module";

/** A node-ontology kind wire token (snake_case), e.g. `function`, `requirement`. */
export type NodeKind = string;

/** An edge-relationship kind wire token (snake_case), e.g. `calls`, `type_uses`. */
export type EdgeKind = string;

/** One vertex of a {@link GraphElements} snapshot (mirrors `GraphElementNode`). */
export interface GraphElementNode {
  /** Stable id — the canonical symbol (symbol tier) or file/module key (rollup). */
  id: string;
  /** Human-facing label (node name, file path, or module name). */
  label: string;
  /** Ontology kind, or `null` for a rollup-cluster vertex. */
  kind: NodeKind | null;
  /** The presentation layer this node renders in. */
  layer: GraphLayer;
}

/** One typed, directed edge of a {@link GraphElements} snapshot. */
export interface GraphElementEdge {
  /** The {@link GraphElementNode.id} the edge points from. */
  source: string;
  /** The {@link GraphElementNode.id} the edge points to. */
  target: string;
  /** The relationship kind, or `null` for a rollup-cluster edge. */
  edge_type: EdgeKind | null;
}

/** The read-only nodes+edges snapshot the canvas consumes (mirrors `GraphElements`). */
export interface GraphElements {
  /** The seed the snapshot was scoped to, or `null` for the whole graph. */
  seed: string | null;
  /** The semantic cluster-zoom tier the snapshot was taken at. */
  granularity: GraphGranularity;
  /** The visible-element cap applied to the selection. */
  cap: number;
  /** In-scope node count before the cap. */
  total_nodes: number;
  /** In-scope edge count before the cap. */
  total_edges: number;
  /** How many in-scope nodes the cap elided (never silently dropped). */
  elided_nodes: number;
  /** How many in-scope edges the cap elided. */
  elided_edges: number;
  /** The rendered nodes, deterministically ordered by id. */
  nodes: GraphElementNode[];
  /** The rendered edges among the rendered nodes. */
  edges: GraphElementEdge[];
  /** Degradation channel — a failed read is reported, not thrown. */
  warnings: string[];
}

/** A lightweight reference to a symbol (mirrors `SymbolRef`). */
export interface SymbolRef {
  /** The canonical SCIP symbol string — the canvas node id. */
  symbol: string;
  /** The human-facing name. */
  name: string;
  /** The node ontology kind (snake_case wire token). */
  kind: NodeKind;
  /** Project-relative defining file, when bound. */
  file: string | null;
  /** 1-based start line of the declaration, when recorded. */
  line: number | null;
}

/** A documentation→node trace link in an {@link ImpactResult} (mirrors `TraceLink`). */
export interface TraceLink {
  /** The linked node (a SymbolRef is flattened into the link by the Rust serde). */
  symbol: string;
  name: string;
  kind: NodeKind;
  file: string | null;
  line: number | null;
  /** The documentation edge kind connecting the queried node to this one. */
  via: EdgeKind;
}

/** One reachable symbol in an impact direction set (mirrors `ImpactEntry`). */
export interface ImpactEntry {
  /** The flattened SymbolRef. */
  symbol: string;
  name: string;
  kind: NodeKind;
  file: string | null;
  line: number | null;
  /** BFS distance from the queried symbol (1 = direct). */
  distance: number;
}

/** The transitive-impact + doc-trace read-model (mirrors `ImpactResult`). */
export interface ImpactResult {
  /** The symbol text as given. */
  query: string;
  /** The node the query resolved to, or `null` for an unknown symbol. */
  resolved: SymbolRef | null;
  /** Traversal depth bound applied to both directions. */
  depth: number;
  upstream_label: string;
  upstream: ImpactEntry[];
  downstream_label: string;
  downstream: ImpactEntry[];
  docs_label: string;
  /** The documentation sections that reference the queried symbol. */
  docs: TraceLink[];
  suggestions: string[];
  warnings: string[];
}

/** One immediate edge of a {@link NodeDetail} (mirrors `EdgeSummary`). */
export interface EdgeSummary {
  /** Inbound (`in`) or outbound (`out`) relative to the queried node. */
  direction: "in" | "out";
  /** The relationship kind. */
  kind: EdgeKind;
  /** The node at the other end (flattened SymbolRef). */
  other: SymbolRef;
}

/** The metadata payload of a resolved node (mirrors `NodeDetail`). */
export interface NodeDetail {
  /** The flattened SymbolRef. */
  symbol: string;
  name: string;
  kind: NodeKind;
  file: string | null;
  line: number | null;
  end_line: number | null;
  /** The declaration signature, when recorded. */
  signature: string | null;
  /** Native annotations (dead-code, duplicate, layer). */
  annotations: string[];
  /** Every immediate edge, both directions. */
  edges: EdgeSummary[];
  /** Source text — only when fetched with `?code=1`. */
  code: string | null;
}

/** The single-symbol detail read-model (mirrors `NodeInfo`). */
export interface NodeInfo {
  query: string;
  /** The resolved node, or `null` for an unknown symbol. */
  node: NodeDetail | null;
  suggestions: string[];
  warnings: string[];
}

/** One ranked query result row (mirrors `QueryHit`). */
export interface QueryHit {
  /** The canonical symbol = the canvas node id (round-trips into a lock target). */
  id: string;
  label: string;
  kind: NodeKind;
  layer: GraphLayer;
  file: string | null;
  line: number | null;
  /** 1-based rank within the ranked result set. */
  rank: number;
}

/** The structured + relational whole-graph query result (mirrors `QueryResponse`). */
export interface QueryResponse {
  /** `"filter"`, `"relation"`, or `"empty"` — which form was interpreted. */
  mode: string;
  /** A human echo of the interpreted query. */
  query: string;
  /** The ranked hits, best-first. */
  hits: QueryHit[];
  /** How many matches existed before the display cap. */
  total: number;
  /** An honest note for the no-matches / guidance states; `null` when there are hits. */
  note: string | null;
  /** "Did you mean" names. */
  suggestions: string[];
  warnings: string[];
}

// ── Dashboard / Health read-models (S-187, FR-UI-09, FR-UI-04) ────────────────
// The wire mirrors the Dashboard (`/api/v1/overview`) and Health (`/api/v1/health`)
// bundles consume. Every field traces to a Rust read-model struct (mirrors
// `logos-core/src/models/navigation.rs`, `models/quality.rs`, `history.rs`,
// `wiki/*`). `Option<T>` is `T | null`; enums serialize to snake_case tokens.

/** Index health + freshness (mirrors `StatusInfo`). */
export interface StatusInfo {
  /** Whether the project has an index at all — the Dashboard/Health empty gate. */
  indexed: boolean;
  file_count: number;
  node_count: number;
  edge_count: number;
  db_path: string;
  db_size_bytes: number;
  /** Unix-seconds string of the last full index, or `null`. */
  last_full_index_at: string | null;
  /** Unix-seconds string of the last incremental sync, or `null`. */
  last_sync_at: string | null;
  graph_revision: number;
  refs_total: number;
  refs_resolved: number;
  refs_unresolved: number;
  /** 0..1 fraction of references resolved. */
  resolution_coverage: number;
  /** The ADR-11 freshness citation prose (internal — not user-facing here). */
  freshness: string;
  warnings: string[];
}

/** One language's node/file presence in the indexed graph (mirrors `LanguageCount`). */
export interface LanguageCount {
  language: string;
  /** Indexed symbol (node) count — a graph fact, not a file count. */
  nodes: number;
  files: number;
}

/** The project-only language composition (mirrors `LanguageComposition`). */
export interface LanguageComposition {
  /** Node-count-descending; empty when nothing is indexed. */
  languages: LanguageCount[];
}

/** A grammar skipped at load (ABI mismatch) (mirrors `SkippedLanguage`). */
export interface SkippedLanguage {
  name: string;
  reason: string;
}

/** The language-plugin registry view (mirrors `LanguagesInfo`); only `skipped` is rendered. */
export interface LanguagesInfo {
  skipped: SkippedLanguage[];
}

/** The quality gate verdict read-model (mirrors `GateResult`). */
export interface GateResult {
  /** Did the run clear the gate? Drives the PASS/FAIL badge. */
  passed: boolean;
  saved: boolean;
  /** The 0–10000 quality signal, or `null` for an empty graph (no signal yet). */
  signal: number | null;
  /** The baseline signal the run is compared against, or `null`. */
  baseline_signal: number | null;
  test_function_count: number;
  threshold: number | null;
  epsilon: number;
  freshness: string;
  message: string;
  warnings: string[];
}

/** Usage of one tool on one surface within the window (mirrors `ToolUsage`). */
export interface ToolUsage {
  /** The recording surface: `"cli"` | `"mcp"` | `"web"` | `"watcher"`. */
  surface: string;
  /** Engine method or pipeline pass name. */
  tool: string;
  calls: number;
  ok_calls: number;
}

/** One UTC day's activity in the window (mirrors `DailyActivity`), keyed by the
 *  `'YYYY-MM-DD'` calendar day; the series is oldest-day-first. */
export interface DailyActivity {
  day: string;
  calls: number;
  ok_calls: number;
}

/** Calls attributed to one dev-vs-`main` bucket (mirrors `OriginUsage`,
 *  [FR-OB-08]): `"main"` for the primary checkout (and legacy NULL rows), or
 *  `"dev"` for all worktree branches combined. */
export interface OriginUsage {
  origin: string;
  calls: number;
  ok_calls: number;
}

/**
 * Usage telemetry (mirrors `StatsInfo`, [FR-OB-04], [FR-OB-08]) — the enriched
 * read-model the Statistics tab and the Dashboard's Activity card both read. All
 * fields are honest read-model projections; the `*_estimate` fields are labeled
 * **estimates** (NFR-CC-04), never presented as measured truth.
 *
 * The additive S-233 series (`calls_by_tool`, `activity_by_day`, `calls_by_origin`,
 * `artifact_bindings`) are always present on the wire; a consumer reading only the
 * headline fields (the Dashboard) simply ignores them. An empty store yields a
 * zeroed model carrying a `warnings` note — never an error.
 *
 * IMPORTANT: `calls_by_origin` deliberately omits rolled-up days (the `daily_rollup`
 * table carries no `origin` column), so its call sum can be **less than**
 * `calls_total` over a window old enough to reach aged-out rollups — never assume
 * the origin split sums to `calls_total`.
 */
export interface StatsInfo {
  window_days: number;
  calls_total: number;
  /** Per-`(surface, tool)` usage breakdown, sorted by surface then tool. */
  calls_by_tool: ToolUsage[];
  latency_p50_ms: number;
  latency_p95_ms: number;
  latency_p99_ms: number;
  reads_saved_estimate: number;
  tokens_saved_estimate: number;
  /** Per-relation-class cross-artifact binding counts, keyed by relation token.
   *  Not rendered by the Statistics tab; typed loosely as the read-model carries it. */
  artifact_bindings: Record<string, unknown>;
  /** Daily activity across the window, **oldest day first**. Empty when nothing recorded. */
  activity_by_day: DailyActivity[];
  /** Dev-vs-`main` usage split: at most two buckets (`"dev"` = all worktree branches
   *  combined, `"main"` = primary checkout), `"dev"` first. */
  calls_by_origin: OriginUsage[];
  /** Degradations (e.g. "no telemetry recorded yet"), never an error. */
  warnings: string[];
}

/** One agent wiki page (mirrors `WikiPage`); the Dashboard renders a prose snippet of `body`. */
export interface WikiPage {
  slug: string;
  title: string;
  body: string;
  generator: string;
  built_at_revision: number;
  stale: boolean;
  has_missing: boolean;
}

/** The Dashboard bundle (mirrors `api_v1::OverviewModel`, [FR-UI-09]). */
export interface OverviewModel {
  status: StatusInfo;
  composition: LanguageComposition;
  languages: LanguagesInfo;
  gate: GateResult;
  coverage: CoverageStatus;
  gaps: TestGapsReport;
  stats: StatsInfo;
  /** The Project-Overview wiki page, or `null` when none is written yet (honest absence). */
  overview_page: WikiPage | null;
  cross: CoverageCrossReport;
  /** The hotspot board, or `null` when the temporal tier is unavailable. */
  hotspots: HotspotReport | null;
}

/** A normalized + raw metric pair (mirrors `MetricValue`). */
export interface MetricValue {
  /** The raw dimension value. */
  raw: number;
  /** The [0,1] normalized value (reprojected onto the 0–10000 score bar). */
  normalized: number;
}

/** The per-dimension metric snapshot (mirrors `MetricSnapshot`). */
export interface MetricSnapshot {
  modularity: MetricValue;
  acyclicity: MetricValue;
  depth: MetricValue;
  equality: MetricValue;
  redundancy: MetricValue;
  nesting: MetricValue;
  conciseness: MetricValue;
  /** `null` when no applicable construct (ADR-21 drop-out) — rendered muted `n/a`, never a zero. */
  cohesion: MetricValue | null;
  /** `null` when no applicable construct (ADR-21 drop-out). */
  focus: MetricValue | null;
  uniqueness: MetricValue;
  thresholds_hash: string;
  node_count: number;
  edge_count: number;
  function_count: number;
  test_function_count: number;
  /** True for an empty graph — the metrics grid renders the honest empty state. */
  empty: boolean;
  aggregate_signal: number | null;
}

/** One worst-offender symbol for a structural dimension (mirrors `Offender`). */
export interface Offender {
  name: string;
  file: string;
  line: number | null;
  /** The deterministic magnitude descriptor (e.g. "nesting depth 6"). */
  detail: string;
}

/** The per-dimension worst-offender lists (mirrors `WorstOffenders`). */
export interface WorstOffenders {
  nesting: Offender[];
  conciseness: Offender[];
  cohesion: Offender[];
  focus: Offender[];
  uniqueness: Offender[];
}

/** The last persisted scan read-model (mirrors `ScanResult`). */
export interface ScanResult {
  /** The 0–10000 signal, or `null` for an empty graph. */
  signal: number | null;
  freshness: string;
  metrics: MetricSnapshot;
  worst_offenders: WorstOffenders;
  warnings: string[];
}

/** One snapshot in the signal-evolution series (mirrors `EvolutionPoint`). */
export interface EvolutionPoint {
  snapshot_id: number;
  created_at: number;
  commit_sha: string | null;
  signal: number | null;
  /** Signed movement vs the previous snapshot, or `null` for the first point. */
  signal_delta: number | null;
}

/** The snapshot-series evolution read-model (mirrors `EvolutionReport`). */
export interface EvolutionReport {
  /** Oldest-first snapshot series; empty when none persisted yet. */
  snapshots: EvolutionPoint[];
  warnings: string[];
}

/** The Health bundle (mirrors `api_v1::HealthModel`, [FR-UI-04]). */
export interface HealthModel {
  status: StatusInfo;
  gate: GateResult;
  scan: ScanResult;
  evolution: EvolutionReport;
}

// ── Files & Risk / Coverage / Quadrant read-models (S-188, FR-UI-11, FR-UI-17) ──
// Typed mirrors of the Rust read-model structs the `/api/v1/files`,
// `/api/v1/coverage`, and `/api/v1/quadrant` bundles serialize (web/src/api_v1.rs):
//   - `HotspotReport`/`Hotspot`/`CoverageCell` (logos-core/src/history/hotspot.rs)
//   - `TemporalReport`/`FileTemporal` (logos-core/src/history/temporal.rs)
//   - `CoverageStatus`/`CoverageFileStatus` (logos-core/src/history/coverage/mod.rs)
//   - `CoverageCrossReport`/`CrossSymbol`/`CrossTotals` (…/coverage/cross.rs)
//   - `StatusInfo` (logos-core/src/models/navigation.rs)
// Integers carrying basis points (0–10000) are named `*_bp`. `Option<T>` is
// `T | null`; an absent figure is rendered `n/a`, never a fabricated `0`
// (NFR-RA-05).

/** Why the temporal tier degraded (mirrors `DegradedReason`, serialized as the
 *  variant name). Drives the honest empty state when the board is empty. */
export type DegradedReason = "NotGit" | "GitAbsent" | "Shallow";

/** Coverage freshness wire token (mirrors `FRESHNESS_*`): a fresh value, a
 *  stale-label-only file, or never-covered `n/a`. */
export type Freshness = "fresh" | "stale" | "n/a";

/** A quadrant tag (mirrors `Quadrant`, `rename_all = "lowercase"`); `null` for a
 *  symbol with no runtime axis (the `n/a` rule — it cannot be placed). */
export type QuadrantTag = "q1" | "q2" | "q3" | "q4";

/** One hotspot/coverage cell (mirrors `CoverageCell`): a fresh value, a stale
 *  label, or `n/a` — never a guessed `0` ([FR-CV-05]). */
export interface CoverageCell {
  /** `"fresh"`, `"stale"`, or `"n/a"`. */
  state: Freshness;
  /** Line coverage in basis points (0–10000); `Some` only when fresh. */
  coverage_bp: number | null;
}

/** One ranked hotspot file (mirrors `Hotspot`, [FR-GH-06]). */
export interface Hotspot {
  /** Repo-relative path. */
  path: string;
  /** `churn_rank × complexity_rank` — higher is hotter. */
  score: number;
  churn_rank: number;
  /** In-window commit count — the churn axis. */
  churn_commits: number;
  complexity_rank: number;
  /** Σ per-function cyclomatic complexity over the file. */
  complexity: number;
  /** Co-change scatter context. */
  co_change_count: number;
  /** Defect-history **heuristic** count (see `HotspotReport.defect_label`). */
  defect_commits: number;
  /** Per-file coverage cell — fresh value / stale label / `n/a`. */
  coverage: CoverageCell;
}

/** The hotspot ranking read-model (mirrors `HotspotReport`, [FR-GH-06]). */
export interface HotspotReport {
  /** Advisory-tier label ([NFR-CC-04]). */
  tier: string;
  /** The mandatory heuristic label on every `defect_commits`. */
  defect_label: string;
  head_sha: string | null;
  config_hash: string;
  limit: number | null;
  /** Files ranked before any `--limit` truncation. */
  ranked_files: number;
  /** The ranked hotspots, highest score first. */
  files: Hotspot[];
  /** Set when the tier degraded; `files` is then empty. */
  degraded: DegradedReason | null;
  /** A one-line first-mine/degraded notice, or `null`. */
  notice: string | null;
  /** Whether the `--untested` filter was applied. */
  untested: boolean;
  /** Coverage-column basis (`"coverage"` or the labeled `"static-reachability"`). */
  coverage_basis: string;
  /** The static-reachability caveat, present only on the fallback basis. */
  coverage_label: string | null;
}

/** One file's temporal metrics over the HEAD-anchored window (mirrors
 *  `FileTemporal`); an absent path renders `n/a`. */
export interface FileTemporal {
  path: string;
  /** Churn — in-window commits touching the file. */
  commit_count: number;
  /** Churn — lines added across those commits. */
  lines_added: number;
  /** Churn — lines deleted across those commits. */
  lines_deleted: number;
  /** Code-age volatility (recency) — whole days since the last in-window change. */
  last_change_age_days: number;
  /** Code-age volatility (dispersion) — std-dev of change ages in whole days. */
  age_dispersion_days: number;
  /** Ownership dispersion — `(1 − dominant-author share) × 10000`. */
  ownership_dispersion_bp: number;
  /** Change entropy in basis points. */
  change_entropy_bp: number;
}

/** The per-file temporal read-model (mirrors `TemporalReport`). */
export interface TemporalReport {
  head_sha: string | null;
  mined_through: string | null;
  config_hash: string;
  window_months: number;
  /** Per-file temporal metrics, canonical path order; absent path = `n/a`. */
  files: FileTemporal[];
  degraded: DegradedReason | null;
  /** Set when this evaluation populated a previously-empty store. */
  first_mine: boolean;
}

/** One covered file's freshness-resolved coverage (mirrors `CoverageFileStatus`). */
export interface CoverageFileStatus {
  path: string;
  /** `"fresh"` or `"stale"`. */
  freshness: Freshness;
  /** Line coverage in basis points; `Some` only when fresh. */
  coverage_bp: number | null;
  instrumented_lines: number;
  covered_lines: number;
}

/** The coverage status read-model (mirrors `CoverageStatus`, [FR-CV-06]). */
export interface CoverageStatus {
  head_sha: string | null;
  config_hash: string | null;
  /** Distinct report formats merged into the snapshot, sorted. */
  formats: string[];
  report_count: number;
  total_files: number;
  fresh_files: number;
  stale_files: number;
  /** Fraction of covered files that are fresh, in basis points; `null` when none. */
  freshness_bp: number | null;
  /** Overall line-coverage aggregate over fresh files, in basis points; `null` = `n/a`. */
  overall_coverage_bp: number | null;
  /** One row per covered file, ordered by path. */
  files: CoverageFileStatus[];
  /** `n/a` notice when no coverage ingested; `null` otherwise. */
  notice: string | null;
  current_head: string | null;
  /** `true` when the artifact lags HEAD (ingested at a different commit). */
  head_stale: boolean;
  /** A one-line refresh prompt when `head_stale`; `null` otherwise. */
  staleness_prompt: string | null;
}

/** One symbol's place in the reachability × runtime-coverage cross (mirrors
 *  `CrossSymbol`, [FR-UI-17]). */
export interface CrossSymbol {
  symbol: string;
  name: string;
  file: string;
  start_line: number | null;
  end_line: number | null;
  /** The static-reachability axis (Y): a test transitively calls it. */
  reachable_from_test: boolean;
  /** The runtime-execution axis (X) in basis points; `null` is `n/a` — an
   *  unresolvable span / non-fresh-covered file / no instrumented line, never `0`. */
  runtime_exec_bp: number | null;
  /** The quadrant, or `null` exactly when `runtime_exec_bp` is `n/a`. */
  quadrant: QuadrantTag | null;
}

/** Quadrant tallies over the resolved symbols (mirrors `CrossTotals`). */
export interface CrossTotals {
  /** Not-reachable + executed — false-green (worst). */
  q1: number;
  /** Reachable + 0% executed — dead / guarded test edge. */
  q2: number;
  /** Not-reachable + 0% executed — true gap. */
  q3: number;
  /** Reachable + executed — trust (best). */
  q4: number;
  /** Symbols with no runtime axis (`n/a`). */
  na_runtime: number;
  /** Every non-test function/method considered. */
  total: number;
}

/** The reachability × runtime-coverage cross read-model (mirrors
 *  `CoverageCrossReport`, [FR-UI-17]). */
export interface CoverageCrossReport {
  head_sha: string | null;
  config_hash: string | null;
  /** `true` when the latest snapshot has at least one fresh covered file. */
  has_fresh_coverage: boolean;
  /** One row per non-test function/method, in canonical order. */
  symbols: CrossSymbol[];
  totals: CrossTotals;
  /** The `n/a` empty-state notice when no coverage was ever ingested; else `null`. */
  notice: string | null;
}

/** `GET /api/v1/files` bundle (mirrors `FilesModel`): the ranked hotspot board
 *  joined with the per-file temporal facts. */
export interface FilesModel {
  status: StatusInfo;
  hotspots: HotspotReport;
  temporal: TemporalReport;
}

/** `GET /api/v1/coverage` bundle (mirrors `CoverageModel`): the coverage status
 *  joined with the untested hotspot board. */
export interface CoverageModel {
  status: StatusInfo;
  coverage: CoverageStatus;
  untested: HotspotReport;
}

/** `GET /api/v1/quadrant` bundle (mirrors `QuadrantModel`): the cross read-model
 *  plus the hotspot board supplying urgency/blast-radius weight (`null` on a read
 *  fault — the view degrades to an unweighted urgency). */
export interface QuadrantModel {
  status: StatusInfo;
  cross: CoverageCrossReport;
  hotspots: HotspotReport | null;
}

// ── Architecture / Cycles (mirrors `ArchitectureModel` / `DsmReport`, S-189) ──────

/** One module row/column of a {@link DsmReport} (mirrors `DsmRow`). */
export interface DsmRow {
  /** The module name (the matrix row/column label). */
  name: string;
  /** The presentation layer, when known. */
  layer: string | null;
}

/** The dependency-structure matrix read-model (mirrors `DsmReport`). The `matrix`
 *  is a square `rows.length × rows.length` grid of dependency counts; a cell above
 *  the diagonal with a non-zero count is a back-edge (a cycle participant). */
export interface DsmReport {
  granularity: string;
  rows: DsmRow[];
  /** Row-major dependency counts; `matrix[i][j]` = i depends on j. */
  matrix: number[][];
  freshness: string;
  warnings: string[];
}

/** The Architecture / Cycles bundle (mirrors `ArchitectureModel`). */
export interface ArchitectureModel {
  status: StatusInfo;
  dsm: DsmReport;
}

// ── Gaps (mirrors `GapsModel` / `TestGapsReport` / `RulesReport`, S-189) ──────────

/** One untested public function (mirrors `TestGap`). */
export interface TestGap {
  name: string;
  file: string;
  /** 1-based declaration line, when recorded. */
  line: number | null;
}

/** A test-smell kind wire token (mirrors `TestSmellKind`, kebab-case). */
export type TestSmellKind = "assertion-free" | "empty-body" | "sleeping";

/** One flagged test smell (mirrors `TestSmell`). */
export interface TestSmell {
  file: string;
  line: number | null;
  name: string;
  kind: TestSmellKind;
}

/** The test-smell appendix (mirrors `TestSmellAppendix`). */
export interface TestSmellAppendix {
  label: string;
  findings: TestSmell[];
  not_analyzed: string[];
}

/** The blast-radius-ranked test-gaps read-model (mirrors `TestGapsReport`). The
 *  `untested` order is the worklist ranking — the view renders it verbatim. */
export interface TestGapsReport {
  untested: TestGap[];
  total_functions: number;
  covered_functions: number;
  /** Coverage ratio out of 10000, or `null` when not computed (the `n/a` rule). */
  coverage_ratio: number | null;
  limit: number;
  truncated: boolean;
  /** The mandatory static-coverage caveat (BR-16) — rendered verbatim. */
  caveat: string;
  freshness: string;
  warnings: string[];
  smells: TestSmellAppendix;
}

/** One architecture-rule violation (mirrors `Violation`). */
export interface Violation {
  rule: string;
  rule_type: string;
  /** `"error"` / `"warning"` / other — drives the severity badge tone. */
  severity: string;
  file: string;
  node_id: number | null;
  message: string;
}

/** The architecture-rules report (mirrors `RulesReport`). */
export interface RulesReport {
  passed: boolean;
  checked_rules: number;
  /** Whether a `.logos/rules.toml` exists — gates the onboarding empty state. */
  rules_present: boolean;
  violations: Violation[];
  freshness: string;
  warnings: string[];
}

/** The Gaps bundle (mirrors `GapsModel`). */
export interface GapsModel {
  status: StatusInfo;
  test_gaps: TestGapsReport;
  rules: RulesReport;
}

// ── Wiki (mirrors the wiki read-models + the S-189 presentation bundles) ──────────

/** Per-anchor freshness verdict (mirrors `Freshness`, lowercase wire tokens). */
export type AnchorFreshness = "fresh" | "stale" | "missing";

/** One page anchor's provenance + freshness (mirrors `AnchorProvenance`). */
export interface AnchorProvenance {
  kind: string;
  entity_id: string;
  freshness: AnchorFreshness;
}

/** The dual-axis `wiki status` freshness read-model (mirrors `WikiStatus`). Only the
 *  fields the landing reads are mirrored; the wire carries the work-list too. */
export interface WikiStatus {
  page_count: number;
  fresh_count: number;
  stale_count: number;
  missing_anchor_count: number;
  /** Pages whose built-at revision the graph has advanced past (regen-pending). */
  revision_stale_count: number;
  /** The graph revision the counts were computed against. */
  current_revision: number;
  freshness_fraction: number;
}

/** One wiki full-text search hit (mirrors `WikiHit`). */
export interface WikiHit {
  slug: string;
  title: string;
  generator: string;
  written_head: string;
  stale: boolean;
  has_missing: boolean;
  built_at_revision: number;
  /** Whether the graph has advanced past the page's built-at revision. */
  revision_pending: boolean;
}

/** The agent wiki-page presentation bundle (mirrors `api_v1::WikiPageView`, S-189):
 *  provenance + per-anchor freshness + the **server-rendered, already-safe HTML
 *  body** the SPA mounts. `rendered_html` is comrak output (the XSS boundary is
 *  server-side); the SPA renders its `.mermaid` blocks client-side. */
export interface WikiPageView {
  slug: string;
  title: string;
  /** Server-rendered, comrak-sanitized HTML; empty for a `placeholder`. */
  rendered_html: string;
  /** A known scaffold slug with no prose yet — the honest "not yet generated" state. */
  placeholder: boolean;
  generator: string | null;
  written_head: string | null;
  marker: string | null;
  built_at_revision: number | null;
  anchors: AnchorProvenance[];
  stale: boolean;
  has_missing: boolean;
  /** Derived "stale — regeneration pending" verdict. */
  regen_pending: boolean;
  /** The graph revision the verdict was derived against. */
  current_revision: number;
}

/** One leaf of a {@link WikiNavTier} (mirrors `api_v1::WikiNavItem`). */
export interface WikiNavItem {
  slug: string;
  label: string;
}

/** One tier of the wiki menu (mirrors `api_v1::WikiNavTier`). */
export interface WikiNavTier {
  title: string;
  items: WikiNavItem[];
}

/** The four-tier wiki menu IA (mirrors `api_v1::WikiNav`, S-189) — the Summary,
 *  Design, and Specs tiers plus a top-level Search link. */
export interface WikiNav {
  tiers: WikiNavTier[];
  search_label: string;
}

// ── Config editor (S-191, FR-UI-12/13, FR-CF-06, ADR-31) ──────────────────────
// The wire mirrors of `logos-core/src/config/writeback.rs` + `secrets.rs`. The
// editor's authoritative document is each file's raw `content`; the `parsed`
// projections only pre-fill the typed fields (the hybrid surface), and the chat
// key is the **masked** projection — the raw secret is never on the wire.

/** The provider family the `[chat]` section selects (mirrors `ChatProvider`). */
export type ChatProvider = "openai" | "anthropic";

/** The masked chat API key — presence + last-4 only (mirrors `MaskedSecret`,
 *  NFR-SE-07). The raw key is never a field, by construction, so it can never be
 *  echoed onto a SPA surface. */
export interface MaskedSecret {
  /** Whether a (non-empty) key is set. */
  present: boolean;
  /** The last ≤4 characters when present (recognisability only), else absent. */
  last4?: string | null;
}

/** The parsed `[chat]` projection used to pre-fill the typed controls (the keys
 *  the typed fields surface; the rest of `[chat]` is edited in the raw pane). */
export interface ParsedChatConfig {
  provider: ChatProvider;
  model?: string | null;
  base_url: string;
}

/** The parsed `[wiki]` projection (mirrors `WikiConfig`, FR-CF-07): the optional
 *  dedicated wiki model. Absent/blank ⇒ the wiki inherits `[chat].model`. The
 *  section always serializes as an object; `model` is omitted when unset. */
export interface ParsedWikiConfig {
  model?: string | null;
}

/** The parsed `config.toml` projection the typed config fields pre-fill from
 *  (mirrors the subset of `Config` the editor formifies). */
export interface ParsedConfig {
  languages: string[];
  include: string[];
  exclude: string[];
  max_file_size: number;
  framework_hints: string[];
  chat: ParsedChatConfig;
  /** The `[wiki]` section (S-176, FR-CF-07) — the dedicated wiki model, inheriting
   *  provider/endpoint/key from `[chat]`. Optional in the type (an older server, or
   *  a config-editor literal, may omit it); the current server always emits it. */
  wiki?: ParsedWikiConfig;
}

/** The parsed `rules.toml` projection the typed rules fields pre-fill from. The
 *  values mirror `Constraints`/`MetricThresholds` (`Option<T>` ⇒ `T | null`).
 *  `max_dead` is either an absolute integer or a delta table — only the absolute
 *  form is typed here; the delta form is edited via the raw pane (mirrors the
 *  server's `MaxDead::as_absolute`). */
export interface ParsedRules {
  constraints: Record<string, number | boolean | { baseline: number; delta: number } | null>;
  metric_thresholds: Record<string, number | null>;
}

/** One policy file's state: repo-relative path, on-disk presence, raw `content`
 *  (the authoritative candidate), and the parsed projection (mirrors `FileView`). */
export interface FileView<T> {
  path: string;
  exists: boolean;
  content: string;
  parsed: T;
}

/** `GET /api/v1/config` — both policy files plus the masked chat key (mirrors
 *  `ConfigReadModel`, FR-UI-12/FR-CF-06). A pure read; loading mutates nothing. */
export interface ConfigReadModel {
  config: FileView<ParsedConfig>;
  rules: FileView<ParsedRules>;
  chat_key: MaskedSecret;
}

/** The outcome of a validated atomic `POST /config/save` (mirrors
 *  `ConfigWriteOutcome`). */
export interface ConfigWriteOutcome {
  file: "config" | "rules";
  path: string;
  bytes_written: number;
  /** Only ever `true` for a `rules.toml` save (the provenance stamp, BR-35). */
  provenance_stamped: boolean;
}

/** The masked outcome of a `POST /config/secret` write (mirrors
 *  `SecretWriteOutcome`) — the new key state, presence + last-4 only. */
export interface SecretWriteOutcome {
  path: string;
  chat_key: MaskedSecret;
}

/** The outcome of an explicit `POST /config/apply` (mirrors the internally-tagged
 *  `ConfigApplyOutcome`): a `config.toml` reconcile or a `rules.toml` re-eval. */
export type ConfigApplyOutcome =
  | {
      action: "reconciled";
      reconciled_files: number;
      full_index: boolean;
      unresolved_refs: number;
      files_failed: string[];
      warnings: string[];
    }
  | {
      action: "reevaluated";
      signal: number | null;
      violations: number;
      freshness: string;
      warnings: string[];
    };

// ── Deep verify (S-206/S-207, CR-052, FR-UI-25, FR-GV-19, ADR-46) ─────────────
// Typed mirrors of the `logos-core/src/models/quality.rs` deep-verify read-model,
// wired through the one intent-guarded read-model POST (`/api/v1/verify`).

/** The fast structural checkpoint embedded in a {@link VerifyReport} (mirrors
 *  `DoctorReport`, FR-GV-18/FR-GV-20) — the same verdict `doctor` reports. */
export interface DoctorReport {
  /** `true` when the graph holds one node per `symbol_id`, no orphan rows,
   *  and no indexed file violates the current admission rules. */
  ok: boolean;
  node_count: number;
  /** Equals `node_count` when sound. */
  distinct_symbol_ids: number;
  /** Nodes leaked past the one-per-`symbol_id` invariant (Channel A, ADR-46). */
  duplicate_symbol_nodes: number;
  dangling_file_refs: number;
  dangling_edge_endpoints: number;
  orphan_shingles: number;
  /** Indexed files the current admission rules would reject — gitignored,
   *  under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded
   *  (S-215, FR-GV-20). The exact count; never truncated. */
  unadmitted_files: number;
  /** A capped, lexically-ordered sample of `unadmitted_files` paths. */
  unadmitted_sample: string[];
  /** One line per detected fault; empty when `ok`. */
  faults: string[];
  message: string;
}

/** One store's whole-graph census for the deep-verify diff (mirrors `VerifyCensus`). */
export interface VerifyCensus {
  files: number;
  nodes: number;
  edges: number;
}

/** The on-demand deep-`verify` verdict (mirrors `VerifyReport`, CR-052, FR-GV-19,
 *  NFR-RA-06, ADR-46): the live graph diffed against a throwaway shadow reindex.
 *  The count deltas are `live − reindex`: a positive `node_delta` (with
 *  `leaked_symbols`) is the drift signature — stale rows the live store leaked;
 *  `orphaned_symbols` are reindex-only symbols the live graph is missing. */
export interface VerifyReport {
  /** `true` when the live graph matches a fresh reindex — the `CONSISTENT` verdict. */
  ok: boolean;
  live: VerifyCensus;
  reindex: VerifyCensus;
  /** `live.nodes − reindex.nodes`. */
  node_delta: number;
  /** `live.edges − reindex.edges`. */
  edge_delta: number;
  /** `live.files − reindex.files`. */
  file_delta: number;
  leaked_total: number;
  /** A deterministic, lexically-ordered, capped sample of `leaked_total`. */
  leaked_symbols: string[];
  orphaned_total: number;
  /** A deterministic, lexically-ordered, capped sample of `orphaned_total`. */
  orphaned_symbols: string[];
  structural: DoctorReport;
  message: string;
}
