//! Quality read-models — results from the governance and quality tools.
//!
//! Covers the quality/pipeline Engine methods (ADR-01):
//! `scan`, `gate`, `check_rules`, `evolution`, `dsm`, `doc_gaps`,
//! `health`, `session_start`, `session_end`, `rescan`

use std::collections::BTreeMap;

use serde::Serialize;

use crate::history::{DegradedReason, FileTemporal};
use crate::models::pipeline::RelationCoverage;

/// Full architecture-quality scan result (FR-QM-01..06, S-020).
///
/// The 0–10000 signal is the geometric-mean aggregate (ADR-12); `None` is the
/// empty-graph "n/a" sentinel, mirroring [`MetricSnapshot::aggregate_signal`].
/// Every scan reconciles first and stamps `freshness` (ADR-11, FR-RC-01/03).
#[derive(Debug, Default, Serialize)]
pub struct ScanResult {
    /// The 0–10000 quality signal (ADR-12); `None` = "n/a" (empty graph).
    pub signal: Option<u32>,
    /// The FR-RC-03 freshness line: `reconciled N files · HEAD <sha> · M
    /// unresolved refs`, prefixed `INCOMPLETE` on a partial reconcile
    /// (NFR-RA-11) or marked assumed-fresh under `--no-reconcile` (FR-RC-04).
    pub freshness: String,
    /// `rules.toml` violations found by this run (FR-GV-02).
    pub violations: Vec<Violation>,
    pub metrics: MetricSnapshot,
    /// Per-dimension worst-offender detail for the five CR-005 structural
    /// dimensions (FR-QM-09..13, CR-005): the top-N offending functions/containers
    /// per dimension, deterministically ordered and capped. Empty lists when a
    /// dimension has no offenders (or dropped out); the review-phase visibility
    /// the `scan` surface gains.
    pub worst_offenders: WorstOffenders,
    /// The **non-gated temporal tier** (CR-006, [FR-GH-07], [BR-26]): per-file
    /// churn / co-change / defect-heuristic columns, explicitly labeled as
    /// advisory so the two-tier boundary is visible, never implied
    /// ([NFR-CC-04]). Computed independently of the gated columns above, which
    /// stay byte-identical whether `history.db` is present or absent.
    ///
    /// [FR-GH-07]: ../../../docs/specs/requirements/FR-GH-07.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub temporal: TemporalTier,
    /// Degradations (reconcile skips, unreadable files) — never an error.
    pub warnings: Vec<String>,
}

/// The non-gated temporal tier rendered in scan detail ([FR-GH-07]).
///
/// Carries the values from the git-history analytics tier (CR-006), kept
/// strictly separate from the gated quality columns: a `gated: false` marker
/// and a tier label make the boundary explicit ([NFR-CC-04], [BR-26]), and the
/// defect column is labeled a **heuristic** ([FR-GH-05]). When the tier is
/// degraded (non-git / `git` absent / shallow) or unavailable, `files` is empty
/// and `notice` explains why — `n/a`, never fabricated ([FR-GH-08],
/// [NFR-RA-05]).
///
/// [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
/// [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Default, Serialize)]
pub struct TemporalTier {
    /// The tier label — advisory, never gated ([BR-26], [NFR-CC-04]).
    pub tier: &'static str,
    /// Always `false`: the temporal tier never moves the gate ([BR-26]).
    pub gated: bool,
    /// The mandatory heuristic label on each file's `defect_commits`
    /// ([FR-GH-05]).
    pub defect_label: &'static str,
    /// The HEAD the temporal values were computed at; `None` when degraded.
    pub head_sha: Option<String>,
    /// The effective `[history]` config hash ([FR-GH-09]); `None` when degraded.
    pub config_hash: Option<String>,
    /// `Some` when the tier degraded instead of computing ([FR-GH-08]).
    pub degraded: Option<DegradedReason>,
    /// A one-line notice (degraded reason and/or first-mine), or `None`.
    pub notice: Option<String>,
    /// Per-file temporal columns, canonical path order; a file absent here has
    /// no in-window history → `n/a` ([FR-GH-03]).
    pub files: Vec<FileTemporal>,
}

/// Per-dimension worst-offender lists for the five CR-005 structural dimensions
/// (CR-005 §3.2 review-phase visibility): the top-N offenders per dimension,
/// each list deterministically ordered (by offending severity, then node id) and
/// capped ([NFR-RA-06]). A dimension with no offenders — or one that dropped out
/// of the aggregate (Cohesion/Focus with no construct) — carries an empty list.
///
/// The lists explain *which* code drives a low dimension score, so a reviewer can
/// act on the signal; they never enter the aggregate or the gate (report detail
/// only, exactly as `doc_gaps` is advisory).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Default, Serialize)]
pub struct WorstOffenders {
    /// Deeply-nested production functions (`max_nesting_depth ≥ T_nest`),
    /// deepest first (FR-QM-09).
    pub nesting: Vec<Offender>,
    /// Production brain methods (CC ∧ LOC ∧ nesting thresholds all met), highest
    /// complexity first (FR-QM-10).
    pub conciseness: Vec<Offender>,
    /// Low-cohesion production classes (LCOM4 ≥ 2), most fragmented first
    /// (FR-QM-11).
    pub cohesion: Vec<Offender>,
    /// God class-like containers (methods ≥ `T_m` ∨ span ≥ `T_span`), most
    /// methods first (FR-QM-12).
    pub focus: Vec<Offender>,
    /// Production functions in a near-clone group (`clone_group IS NOT NULL`),
    /// grouped by clone-group id (FR-QM-13).
    pub uniqueness: Vec<Offender>,
}

/// One worst-offender entry: the offending symbol and a deterministic,
/// human-readable severity descriptor (CR-005 review-phase visibility).
///
/// `detail` encodes the offending magnitude (e.g. `"nesting depth 6"`,
/// `"LCOM4 4"`, `"23 methods · span 540"`) derived from integer facts, so it is
/// byte-identical across runs and the four CI targets ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Offender {
    /// The offending symbol's name.
    pub name: String,
    /// The defining file, when the node is bound to one.
    pub file: String,
    /// 1-based declaration start line, when recorded.
    pub line: Option<i64>,
    /// A deterministic descriptor of the offending magnitude.
    pub detail: String,
}

/// A single architecture-rule violation (FR-GV-02).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// The rule key: a constraint name (`max_cc`), `layer-ordering`,
    /// `boundary:<from>-><to>`, `forbidden_import:<from>-><to>`,
    /// `require_tested:<paths>`, or `require_documented:<paths>`.
    pub rule: String,
    /// `"constraint"`, `"layer"`, or `"boundary"` — the `violations` table
    /// `rule_type` discriminator (SRS §5.1). The CR-002/CR-003 families reuse
    /// these without a migration: a `forbidden_import` is a `boundary`, a
    /// `require_tested` or `require_documented` gap is a `constraint`; the `rule`
    /// key disambiguates.
    pub rule_type: String,
    /// `"error"` or `"warning"`; `check` exits 1 on any error (FR-GV-03).
    /// All `rules.toml` violations are errors in v1 — checked-in policy is
    /// binding (ratified 2026-06-06).
    pub severity: String,
    /// The offending file (empty for a project-wide violation, e.g.
    /// `max_cycles`).
    pub file: String,
    /// The offending node's storage id, when the violation points at one
    /// (per-function constraints) — the `violations.node_id` column.
    pub node_id: Option<i64>,
    pub message: String,
}

/// Snapshot of the five quality metrics — raw + normalized per metric, the
/// counts the run scored, and the aggregate signal (S-018, FR-QM-01..07,
/// ADR-12).
///
/// Field order is the canonical dimension order the aggregate reduces in
/// (ADR-08, FR-QM-14): the five original metrics (modularity, acyclicity, depth,
/// equality, redundancy) then the five CR-005 structural dimensions (nesting,
/// conciseness, cohesion, focus, uniqueness).
///
/// The original five keep the ADR-12 **zero short-circuit** (a hard `0`
/// collapses the signal — anti-gaming). The new five are **floored at 0.01**
/// (FR-QM-14): they drag the signal but never alone collapse it. Cohesion and
/// Focus are [`Option`]: `None` is the **applicability drop-out** (ADR-21) — the
/// construct does not exist in the repo (no classes / no class-like
/// containers), the snapshot persists NULL + a `false` applicability flag, and
/// the dimension drops out of the geometric-mean denominator (a class-less repo
/// gets a deterministic 9-dimension mean, FR-QM-11/12/14, UAT-QM-10).
#[derive(Debug, Default, Serialize)]
pub struct MetricSnapshot {
    /// Newman Q on the directory partition (FR-QM-01).
    pub modularity: MetricValue,
    /// Cycle count via `tarjan_scc` (FR-QM-02).
    pub acyclicity: MetricValue,
    /// Longest path over the condensation (FR-QM-03).
    pub depth: MetricValue,
    /// `1 − Gini` of per-function cyclomatic complexity (FR-QM-04).
    pub equality: MetricValue,
    /// `1 − dead/duplicate function ratio` (FR-QM-05).
    pub redundancy: MetricValue,
    /// `1 − deep-nesting ratio`, floored at 0.01 (FR-QM-09, CR-005): production
    /// functions with `max_nesting_depth ≥ T_nest` over production functions.
    pub nesting: MetricValue,
    /// `1 − brain-method ratio`, floored at 0.01 (FR-QM-10, CR-005): a brain
    /// method meets all of CC ≥ `T_cc` ∧ LOC ≥ `T_loc` ∧ nesting ≥ `T_bn`.
    pub conciseness: MetricValue,
    /// Mean of `1/LCOM4` over production classes, floored at 0.01 (FR-QM-11,
    /// CR-005); `None` = **n/a drop-out** (no applicable classes, ADR-21).
    pub cohesion: Option<MetricValue>,
    /// `1 − god-container ratio` over class-like containers, floored at 0.01
    /// (FR-QM-12, CR-005); `None` = **n/a drop-out** (no class-like containers).
    pub focus: Option<MetricValue>,
    /// `1 − near-clone ratio`, floored at 0.01 (FR-QM-13, CR-005): production
    /// functions in any near-clone group (`clone_group IS NOT NULL`) over
    /// production functions.
    pub uniqueness: MetricValue,
    /// The hash of the effective detection-threshold set this run scored under
    /// (FR-QM-14, BR-25, ADR-21) — persisted in every snapshot so a tuning change
    /// is visible and gate-gated (the announced auto-re-baseline, FR-GV-10).
    /// A property of the run configuration, not the graph, so it is recorded on
    /// every snapshot including the empty-graph sentinel.
    pub thresholds_hash: String,
    /// Vertices in the metric graph the run scored.
    pub node_count: u64,
    /// Edges in the metric graph the run scored.
    pub edge_count: u64,
    /// Production function/method nodes considered by Equality/Redundancy —
    /// `is_test=true` functions are excluded from the scope (FR-QM-08, BR-18).
    pub function_count: u64,
    /// Test function/method nodes excluded from the production scope
    /// (FR-QM-08): the "N test functions excluded from metrics" count, surfaced
    /// for transparency and persisted on the snapshot (FR-QM-07, NFR-CC-04).
    pub test_function_count: u64,
    /// The empty-graph honesty flag (`node_count == 0`, ADR-12).
    pub empty: bool,
    /// The rounded 0–10000 signal (ADR-08); `None` serialises as `null` — the
    /// "n/a" sentinel for an empty graph, never a misleading ~8033 (ADR-12,
    /// NFR-CC-04). `Some(0)` is the real zero short-circuit.
    pub aggregate_signal: Option<u32>,
}

/// One metric's raw + normalized pair (FR-QM-07).
///
/// `raw` is the pre-normalization quantity (Q, cycle count, depth, Gini,
/// redundant ratio) so evolution can show *which* dimension moved; `normalized`
/// is the [0,1] value entering the geometric mean.
#[derive(Debug, Default, Serialize)]
pub struct MetricValue {
    pub raw: f64,
    pub normalized: f64,
}

/// Gate check result for CI integration (FR-GV-04/05, BR-10).
///
/// The gate fails iff `current < baseline − epsilon` (aggregate-regression
/// only, DL-04); per-metric regressions are detail, never an independent
/// failure. No baseline → informational pass.
#[derive(Debug, Default, Serialize)]
pub struct GateResult {
    pub passed: bool,
    /// `true` when this run upserted the baseline (`gate --save` /
    /// `session_start`, FR-GV-04).
    pub saved: bool,
    /// The fresh 0–10000 signal; `None` = "n/a" (empty graph, ADR-12).
    pub signal: Option<u32>,
    /// The baseline signal compared against, when one existed.
    pub baseline_signal: Option<u32>,
    /// Test function/method nodes excluded from the production-scope metrics
    /// (FR-QM-08): the "N test functions excluded from metrics" count this gate
    /// scored, surfaced alongside the signal (FR-QM-07, NFR-CC-04).
    pub test_function_count: u64,
    /// The optional explicit floor (`gate --threshold`); failing it also
    /// fails the gate.
    pub threshold: Option<u32>,
    /// The float-noise tolerance on the integer signal (BR-10: ≈1.0).
    pub epsilon: f64,
    /// Which metrics moved down vs the baseline — reported as detail (BR-10).
    pub regressions: Vec<MetricRegression>,
    /// Structural-integrity faults folded in from the fast `doctor` check
    /// (CR-052, [FR-GV-18], [NFR-RA-13]): any non-empty entry hard-fails the
    /// gate (`passed = false`) independent of the metric signal — a corrupted
    /// graph blocks the session even when the signal holds the baseline
    /// ([FR-GV-05]). Empty on a structurally sound graph.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub structural_faults: Vec<String>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
}

/// One metric's regression vs the baseline (FR-GV-05 detail reporting).
#[derive(Debug, Default, Serialize)]
pub struct MetricRegression {
    /// Canonical metric name (modularity, acyclicity, depth, equality,
    /// redundancy).
    pub metric: String,
    /// The baseline's normalized [0,1] value.
    pub baseline: f64,
    /// This run's normalized [0,1] value.
    pub current: f64,
    /// `current − baseline` (negative = regressed).
    pub delta: f64,
}

/// Architecture-rules compliance report (FR-GV-02).
#[derive(Debug, Default, Serialize)]
pub struct RulesReport {
    /// `false` when any violation has `severity == "error"` — the `check`
    /// exit-1 discriminator (FR-GV-03).
    pub passed: bool,
    /// Active rules evaluated: set constraints + layer ordering (when layers
    /// are declared) + one per boundary + one per forbidden-import + one per
    /// require-tested contract + one per require-documented contract.
    pub checked_rules: u32,
    /// `true` when a `rules.toml` contract was loaded; `false` when none exists
    /// (the default `<root>/.logos/rules.toml` is absent and an empty contract
    /// was evaluated). The honest "no contract authored yet" signal a surface
    /// needs to tell an empty result apart from an absent one — `checked_rules`
    /// cannot, since the CR-005 structural budgets always evaluate (NFR-CC-04).
    pub rules_present: bool,
    pub violations: Vec<Violation>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
}

/// Signal evolution over stored snapshots (FR-GV-06).
///
/// Reports history — it is not an aggregate evaluation and does not
/// reconcile (BR-03 lists the reconciling runs; `evolution` is not one).
#[derive(Debug, Default, Serialize)]
pub struct EvolutionReport {
    /// The window actually applied (default 30).
    pub limit: u32,
    /// The most recent snapshots, oldest first, with per-metric deltas.
    pub snapshots: Vec<EvolutionPoint>,
    /// Degradations (e.g. no snapshots recorded yet) — never an error.
    pub warnings: Vec<String>,
}

/// One snapshot in the evolution series, with deltas vs its predecessor.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvolutionPoint {
    /// The `metric_snapshots` row id (append-only series order).
    pub snapshot_id: i64,
    /// Unix-seconds timestamp the snapshot was recorded at.
    pub created_at: i64,
    /// Optional VCS commit pin (FR-GV-09).
    pub commit_sha: Option<String>,
    /// The 0–10000 signal; `None` = "n/a" (empty graph, ADR-12).
    pub signal: Option<u32>,
    /// Signed signal movement vs the previous snapshot in the series; `None`
    /// for the first point or when either signal is "n/a".
    pub signal_delta: Option<i64>,
    /// Per-metric normalized values and movement (FR-GV-06 trend detail).
    pub metric_deltas: Vec<MetricDelta>,
}

/// One metric's value and movement at one evolution point.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MetricDelta {
    /// Canonical metric name.
    pub metric: String,
    /// This snapshot's normalized [0,1] value.
    pub normalized: f64,
    /// `normalized − previous.normalized`; `None` for the first point.
    pub delta: Option<f64>,
}

/// Dependency structure matrix (FR-GV-07).
///
/// `matrix[i][j]` counts dependency edges from `rows[i]` to `rows[j]`; rows
/// are ordered by layer order then name, so forward (downward) dependencies
/// sit below the diagonal and back-edges above.
#[derive(Debug, Default, Serialize)]
pub struct DsmReport {
    /// `"module"` (default) or `"file"`.
    pub granularity: String,
    /// Row/column labels in matrix order.
    pub rows: Vec<DsmRow>,
    /// The square matrix: `matrix[i][j]` = count of dep edges `i → j`.
    pub matrix: Vec<Vec<u32>>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
}

/// One row (= column) of the DSM.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DsmRow {
    /// The aggregate's label: a module key or a file path.
    pub name: String,
    /// The `[[layers]]` band ordering the row, when one matched (file-backed
    /// aggregates only; pure module keys are unassigned).
    pub layer: Option<String>,
}

/// Documentation-gap analysis result (FR-GV-14).
///
/// The scope is the *public API*: only `exported` Function/Method symbols are
/// considered. A symbol is "documented" if any [`DocSection`](crate::model::NodeKind::DocSection)
/// resolves a [`DocReference`](crate::model::EdgeKind::DocReference) to it
/// (FR-DG-04) — reference presence in the doc graph, never documentation
/// quality (the `caveat`).
#[derive(Debug, Default, Serialize)]
pub struct DocGapsReport {
    /// Exported functions/methods referenced by no `DocSection`, sorted by file
    /// then name, truncated to `limit`.
    pub undocumented: Vec<DocGap>,
    /// Exported function/method nodes considered (the public-API scope).
    pub total_functions: u64,
    /// Of those, the count referenced by at least one `DocSection`.
    pub documented_functions: u64,
    /// Rounded 0–10000 documentation signal (ADR-08 integer posture, AR-03);
    /// `None` when there are no exported functions to document ("n/a", NFR-CC-04).
    pub documentation_ratio: Option<u32>,
    /// The cap applied to `undocumented` (default 50).
    pub limit: u32,
    /// `true` when more gaps existed than `limit` allowed to list.
    pub truncated: bool,
    /// The mandatory honesty caveat — always emitted.
    pub caveat: String,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    /// Degradations — never an error.
    pub warnings: Vec<String>,
}

/// One undocumented exported function/method (FR-GV-14).
#[derive(Debug, Default, Serialize)]
pub struct DocGap {
    pub name: String,
    pub file: String,
    /// 1-based declaration start line, when recorded.
    pub line: Option<i64>,
}

/// System health check — ARCHITECTURE health: DB integrity, schema version,
/// graph coherence (S-020). For INDEX freshness see the navigation `status`.
#[derive(Debug, Default, Serialize)]
pub struct HealthInfo {
    /// `true` when the store opens, the schema is current, and the FTS index
    /// is coherent.
    pub ok: bool,
    pub db_path: String,
    pub db_size_bytes: u64,
    /// The store's `PRAGMA user_version` (FR-DB-04).
    pub schema_version: i64,
    /// FTS5 external-content coherence (a desync is a Correctness fault,
    /// ADR-14 — surfaced here, loud).
    pub fts_ok: bool,
    /// Graph structural integrity (CR-052, [FR-GV-18], [NFR-RA-13]): `true` when
    /// the node store holds one node per `symbol_id` and no orphan rows. Drift
    /// is a Correctness fault — surfaced here and hard-failing the gate.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub structural_ok: bool,
    /// One line per detected structural fault (empty when `structural_ok`) —
    /// the `doctor` verdict folded into `health` (CR-052, [FR-GV-18]).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    pub structural_faults: Vec<String>,
    /// Indexed source files.
    pub files: u64,
    /// Graph vertices.
    pub nodes: u64,
    /// Graph relationships.
    pub edges: u64,
    /// Reference-ledger rows not currently bound to an edge.
    pub unresolved_refs: u64,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
}

/// The `doctor` structural-integrity verdict (CR-052, [FR-GV-18], [NFR-RA-13],
/// [ADR-46]): the fast always-on guard that asserts one node per `symbol_id`
/// and zero orphan rows in O(a handful of indexed queries), extended by S-215
/// ([FR-GV-20], [ADR-48]) with the always-on admission tripwire.
///
/// The read-model twin of [`StructuralReport`](crate::graph_store::StructuralReport):
/// the same census counts plus the derived `ok` verdict, human-readable
/// `faults`, and a summary `message`. `doctor` exits 1 on `ok == false`, and the
/// same verdict hard-fails `session_end` ([FR-GV-05]) and `check_rules`
/// ([FR-GV-02]) independent of the metric signal.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
#[derive(Debug, Default, Serialize)]
pub struct DoctorReport {
    /// `true` when the graph holds one node per `symbol_id`, no orphan rows,
    /// and no indexed file violates the current admission rules.
    pub ok: bool,
    /// `COUNT(*) FROM nodes`.
    pub node_count: u64,
    /// `COUNT(DISTINCT symbol_id) FROM nodes` — equals `node_count` when sound.
    pub distinct_symbol_ids: u64,
    /// Nodes leaked past the one-per-`symbol_id` invariant (Channel A, ADR-46).
    pub duplicate_symbol_nodes: u64,
    /// Nodes whose non-NULL `file_id` references a missing file.
    pub dangling_file_refs: u64,
    /// Edges whose `source`/`target` references a missing node.
    pub dangling_edge_endpoints: u64,
    /// Shingles whose `node_id` references a missing node.
    pub orphan_shingles: u64,
    /// Indexed files the *current* admission rules would reject — gitignored,
    /// under a nested `.git` boundary, in `ignored_dirs`, or glob-excluded
    /// (S-215, [FR-GV-20]). The exact count; never truncated.
    ///
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    pub unadmitted_files: u64,
    /// A capped, lexically-ordered sample of [`unadmitted_files`](Self::unadmitted_files)
    /// paths ([NFR-RA-06]) — bounded so the read-model stays a fixed size even
    /// on a badly drifted graph; the count above stays exact.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub unadmitted_sample: Vec<String>,
    /// One line per detected fault (empty when `ok`).
    pub faults: Vec<String>,
    /// Documentation directory-symlinks that exist under the documentation-
    /// include set but ended up unindexed ([FR-IX-11]) — a git-ignored symlink
    /// with no sanctioned bypass, or one whose target escapes containment. Purely
    /// diagnostic: this does **not** affect [`ok`](Self::ok) or the exit status,
    /// it only names the dropped path(s) and reason so a silent doc-drop is
    /// visible ([CR-071]).
    ///
    /// [FR-IX-11]: ../../../docs/specs/requirements/FR-IX-11.md
    #[serde(default)]
    pub doc_symlink_warnings: Vec<String>,
    pub message: String,
}

/// One store's whole-graph census for the deep [`VerifyReport`] diff (CR-052,
/// [FR-GV-19], [NFR-RA-06]): the row counts read from a read-only connection. A
/// report carries two — the live graph and the fresh shadow reindex that defines
/// the equivalence target ([NFR-RA-06]).
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VerifyCensus {
    /// `COUNT(*) FROM files` — indexed source files.
    pub files: u64,
    /// `COUNT(*) FROM nodes` — graph vertices.
    pub nodes: u64,
    /// `COUNT(*) FROM edges` — graph relationships.
    pub edges: u64,
}

/// The on-demand **deep-`verify`** verdict (CR-052, [FR-GV-19], [NFR-RA-06],
/// [ADR-46]): the live graph diffed against a throwaway shadow store reindexed
/// via the always-purge [`index`](../../../docs/specs/requirements/FR-IX-01.md)
/// path. The fresh reindex defines ground truth ([NFR-RA-06]); the diff catches
/// the **Channel-B orphans** — files the live store retains but a fresh index
/// would drop — that the fast structural [`DoctorReport`] cannot see, and embeds
/// that fast check ([FR-GV-18]) as `structural`.
///
/// The count deltas are `live − reindex`: a positive `node_delta` (with
/// `leaked_symbols`) is the drift signature — stale rows the live store leaked;
/// `orphaned_symbols` are reindex-only symbols the live graph is missing. The
/// symbol samples are lexically ordered and capped for a bounded read-model
/// ([NFR-RA-06]); the `*_total` counts are exact.
///
/// `verify` exits 1 on `ok == false` on the CLI ([FR-GV-19]); the MCP tool and
/// the web read-model ([FR-UI-25]) serialize this report verbatim.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-UI-25]: ../../../docs/specs/requirements/FR-UI-25.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
#[derive(Debug, Default, Serialize)]
pub struct VerifyReport {
    /// `true` when the live graph matches a fresh reindex: zero count deltas, an
    /// empty symbol-set diff, and `structural.ok` ([FR-GV-19]).
    pub ok: bool,
    /// The live graph census, read from a read-only connection — the live store
    /// is never mutated by `verify` ([FR-GV-19]).
    pub live: VerifyCensus,
    /// The fresh shadow-reindex census — the equivalence target ([NFR-RA-06]).
    pub reindex: VerifyCensus,
    /// `live.nodes − reindex.nodes`: a positive value is a live surplus (the
    /// leak signature), negative a live deficit. Zero on a sound graph.
    pub node_delta: i64,
    /// `live.edges − reindex.edges`.
    pub edge_delta: i64,
    /// `live.files − reindex.files`.
    pub file_delta: i64,
    /// Symbols present in the live graph but absent from a fresh reindex — the
    /// Channel-B leak ([ADR-46]): stale nodes the live store retains.
    pub leaked_total: u64,
    /// A deterministic, lexically-ordered, capped sample of `leaked_total`
    /// ([NFR-RA-06]).
    pub leaked_symbols: Vec<String>,
    /// Symbols present in a fresh reindex but absent from the live graph — the
    /// live graph under-counts (a missed insertion or an over-eager purge).
    pub orphaned_total: u64,
    /// A deterministic, lexically-ordered, capped sample of `orphaned_total`
    /// ([NFR-RA-06]).
    pub orphaned_symbols: Vec<String>,
    /// The embedded fast structural check on the live graph ([FR-GV-18]) — the
    /// same verdict `doctor` reports, folded into the deep check.
    pub structural: DoctorReport,
    pub message: String,
}

/// Session lifecycle info — `session_start` (FR-GV-04).
///
/// `session_start` is the MCP spelling of `gate --save`: it computes a fresh
/// snapshot and upserts the baseline; `session_end` returns a [`GateResult`].
#[derive(Debug, Default, Serialize)]
pub struct SessionInfo {
    /// The baseline snapshot's row id, as the session handle.
    pub session_id: String,
    /// Unix-seconds timestamp the session (baseline) was recorded at.
    pub started_at: i64,
    /// The signal recorded as the baseline; `None` = "n/a" (empty graph).
    pub signal: Option<u32>,
    /// The FR-RC-03 freshness line.
    pub freshness: String,
    pub message: String,
}

/// Observability stats — usage/perf telemetry from `telemetry.db`
/// (FR-OB-04, NFR-OO-03).
#[derive(Debug, Default, Serialize)]
pub struct StatsInfo {
    /// The reporting window in days (FR-OB-04: default 7).
    pub window_days: u32,
    pub calls_total: u64,
    /// Per-`(surface, tool)` usage breakdown, sorted by surface then tool.
    pub calls_by_tool: Vec<ToolUsage>,
    pub latency_p50_ms: u64,
    pub latency_p95_ms: u64,
    pub latency_p99_ms: u64,
    /// Estimated ad-hoc file reads avoided by navigation calls — an estimate,
    /// honestly labeled (NFR-CC-04; constants ratified per SRS OQ-01).
    pub reads_saved_estimate: u64,
    /// `reads_saved_estimate` × net tokens per avoided read — the headline
    /// dogfood metric (NFR-OO-03).
    pub tokens_saved_estimate: u64,
    /// Per-relation-class cross-artifact binding counts read live from the graph
    /// ledger (CR-011, [FR-OB-04], [FR-CG-11]), keyed by the relation's payload
    /// token (`proto-import`, `route`, `type-name`, …). Empty for a repository
    /// with no artifact wiring, or when the graph store cannot be read (the
    /// telemetry surface degrades to a warning, never an error). A `BTreeMap` so
    /// the report order is deterministic ([NFR-RA-06]).
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub artifact_bindings: BTreeMap<String, RelationCoverage>,
    /// Per-UTC-day activity across the window, **oldest day first**
    /// ([FR-OB-04]): raw events grouped by `date(at)` merged with any
    /// `daily_rollup` days the window reaches back into (the same dual source
    /// as `calls_by_tool`, so aged-out days still contribute). Empty when the
    /// window recorded nothing.
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    pub activity_by_day: Vec<DailyActivity>,
    /// Dev-vs-`main` usage split ([FR-OB-08]): at most two buckets — `"dev"`, the
    /// cumulative sum of every non-`main` origin (each a worktree branch), and
    /// `"main"`, the primary checkout plus legacy NULL rows (pre-v2), folded via
    /// `COALESCE(origin,'main')`; sorted so `"dev"` precedes `"main"`. Raw events
    /// only — `daily_rollup` carries no `origin` column, so this breakdown
    /// deliberately omits rolled-up days rather than fabricate an attribution for
    /// them ([NFR-CC-04]); consequently its call sum can be less than
    /// `calls_total` over a window old enough to reach aged-out rollups.
    ///
    /// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub calls_by_origin: Vec<OriginUsage>,
    /// Degradations (e.g. no telemetry recorded yet), never an error.
    pub warnings: Vec<String>,
}

/// Usage of one tool on one surface within the stats window (FR-OB-04).
#[derive(Debug, Default, Serialize)]
pub struct ToolUsage {
    /// `"cli"` or `"mcp"`.
    pub surface: String,
    /// Engine method or pipeline pass name.
    pub tool: String,
    pub calls: u64,
    pub ok_calls: u64,
}

/// One UTC day's activity in the stats window ([FR-OB-04]): total calls and the
/// successful subset, keyed by the `'YYYY-MM-DD'` calendar day.
///
/// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
#[derive(Debug, Default, Serialize)]
pub struct DailyActivity {
    /// The UTC calendar day, `'YYYY-MM-DD'`.
    pub day: String,
    pub calls: u64,
    pub ok_calls: u64,
}

/// Calls attributed to one dev-vs-`main` bucket ([FR-OB-08]): `"main"` for the
/// primary checkout (and legacy NULL rows), or `"dev"` for all worktree branches
/// combined.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
#[derive(Debug, Default, Serialize)]
pub struct OriginUsage {
    /// The dev-vs-`main` bucket — `"dev"` (all worktree branches) or `"main"`.
    pub origin: String,
    pub calls: u64,
    pub ok_calls: u64,
}

/// Languages registered in the plugin substrate (FR-PL-06).
///
/// Lists the grammars that loaded successfully plus any that were skipped at
/// load due to an ABI mismatch, so `logos languages` makes a degraded grammar
/// set visible rather than silent (FR-PL-03, UAT-PL-02).
#[derive(Debug, Default, Serialize)]
pub struct LanguagesInfo {
    pub languages: Vec<LanguageDescriptor>,
    /// Grammars skipped at load (ABI mismatch). Empty in the healthy case.
    pub skipped: Vec<SkippedLanguage>,
}

/// Descriptor for one registered language/grammar (FR-PL-06).
#[derive(Debug, Default, Serialize)]
pub struct LanguageDescriptor {
    pub name: String,
    /// File extensions this grammar claims (without the leading dot).
    pub extensions: Vec<String>,
    /// Basename claims for an extensionless artifact format (CR-010, FR-CG-01),
    /// e.g. `["Dockerfile"]`. Empty for code/doc plugins. Surfaced so
    /// `logos languages` lists an artifact plugin's filename claims alongside its
    /// extensions ([FR-PL-06] as modified).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filenames: Vec<String>,
    /// Whether this is a config/artifact-class plugin (CR-010, FR-CG-01) — `true`
    /// marks the third plugin class beside code and documentation. Surfaced so
    /// `logos languages` distinguishes artifact plugins.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub artifact: bool,
    /// Module path separator joining symbol segments (`::`, `.`, `/`).
    pub module_separator: String,
    /// Extraction capabilities this grammar supports (e.g. `["symbols"]`).
    pub capabilities: Vec<String>,
    /// The tree-sitter ABI version the grammar was loaded at.
    pub abi_version: u32,
    /// Capabilities whose active query is sourced from an on-disk override
    /// rather than the embedded default (FR-PL-04). Empty when none.
    pub overridden_capabilities: Vec<String>,
}

/// A grammar skipped at load, with the reason (FR-PL-03).
#[derive(Debug, Default, Serialize)]
pub struct SkippedLanguage {
    pub name: String,
    pub reason: String,
}
