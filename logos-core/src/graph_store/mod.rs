//! The canonical SQLite graph store ([graph-store component], [ADR-05]).
//!
//! `graph_store` owns `logos.db`: the schema, the FTS5 search index, the
//! forward-only migrations, the per-connection WAL pragma contract, and the
//! prepared-statement point queries. It is the system-of-record every later
//! component ([execution-runtime], [graph-hydration], [navigation-service])
//! builds on.
//!
//! # Connection contract ([FR-DB-02], [NFR-RA-10])
//!
//! Every connection — opened via [`SqliteGraphStore::open`] or
//! [`SqliteGraphStore::open_in_memory`] — sets `journal_mode=WAL`,
//! `foreign_keys=ON` (it defaults *OFF* — the [ADR-05] footgun), and
//! `synchronous=NORMAL`, plus a 5-second busy-timeout so writers serialize and
//! then fail gracefully rather than blocking readers ([NFR-RA-10]).
//!
//! # The read seam ([`GraphStore`])
//!
//! The [`GraphStore`] trait is the point-query interface consumers depend on
//! ([navigation-service], [resolution-engine], [pipeline-orchestrator]):
//! `node` / `callers` / `callees` / `search`. Every query is a **prepared
//! statement with bound parameters** — never string interpolation. That is the
//! injection boundary: scanned source names reach SQLite only as bound values
//! ([NFR-SE-02]).
//!
//! # Crash-safety ([NFR-RA-07])
//!
//! Multi-row writes go through [`SqliteGraphStore::write_batch`], which wraps
//! the work in one transaction: it commits only if the closure returns `Ok`,
//! and rolls back wholesale on any error — so an interrupted index/sync batch
//! never leaves partial state.
//!
//! [graph-store component]: ../../../docs/specs/architecture/components/graph-store.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
//! [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
//! [navigation-service]: ../../../docs/specs/architecture/components/navigation-service.md
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
//! [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md

mod migrate;
mod schema;

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};
use crate::models::navigation::LanguageCount;

/// The `project_metadata` key under which the admission-config fingerprint is
/// stored (CR-004, [ADR-20], [FR-SY-07]).
///
/// The single durable record of the configuration in force at the last
/// reconciliation — the pipeline compares
/// [`Config::admission_fingerprint`](crate::config::Config::admission_fingerprint)
/// against this value to gate the config-narrowing purge.
///
/// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
/// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
pub const CONFIG_FINGERPRINT_KEY: &str = "config_fingerprint";

/// The `project_metadata` key under which the persisted monotonic graph
/// revision is stored (CR-027, [ADR-32], [FR-SY-09]).
///
/// The durable, cross-process successor to the in-memory-only
/// [`SyncStamp`](crate::hydrate::SyncStamp): advanced by one at the post-`index`/
/// `sync` point whenever the graph changes (see
/// [`BatchWriter::advance_graph_revision`]) and read back via
/// [`GraphStore::graph_revision`]. A later process or surface compares it to
/// answer "has the graph changed since revision N?" without re-deriving state —
/// the native wiki tier's cache key and the agent-tier freshness comparator.
///
/// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
/// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
pub const GRAPH_REVISION_KEY: &str = "graph_revision";

/// A row read back from the `nodes` table, mapped to model types.
///
/// `kind` is recovered via [`NodeKind::try_from`] and `symbol` via
/// [`LogosSymbol::parse`]; a value that fails either conversion is a corrupt
/// store — surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The canonical SCIP symbol identity.
    pub symbol: LogosSymbol,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name (the FTS-indexed column).
    pub name: String,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// 1-based start line of the declaration, when recorded ([FR-NV-04]).
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    pub start_line: Option<i64>,
    /// 1-based end line of the declaration, when recorded.
    pub end_line: Option<i64>,
}

/// An edge read back from the `edges` table, mapped to model types.
///
/// The whole-graph counterpart to [`NodeRow`]: where the point queries
/// (`callers`/`callees`) return the *nodes* adjacent to one vertex, [`EdgeRow`]
/// is one relationship in the bulk stream [`GraphStore::all_edges`] feeds to
/// petgraph hydration ([graph-hydration], [ADR-05]). `kind` is recovered via
/// [`EdgeKind::try_from`]; an out-of-range discriminant is a corrupt store,
/// surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
/// [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EdgeRow {
    /// The source node (the `e.source` endpoint).
    pub source: NodeId,
    /// The target node (the `e.target` endpoint).
    pub target: NodeId,
    /// The relationship kind.
    pub kind: EdgeKind,
}

/// The fields needed to insert a new node.
///
/// `symbol` must already be a resolved [`LogosSymbol`] id (see
/// [`SqliteGraphStore::upsert_symbol`]); `kind` is a typed [`NodeKind`] so the
/// caller cannot pass an out-of-range discriminant past the type system before
/// the `CHECK` constraint even runs.
#[derive(Debug, Clone, Copy)]
pub struct NewNode<'a> {
    /// FK into `symbols(id)`.
    pub symbol_id: i64,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name.
    pub name: &'a str,
    /// Optional FK into `files(id)`.
    pub file_id: Option<i64>,
    /// Optional 1-based start line.
    pub start_line: Option<i64>,
    /// Optional 1-based end line.
    pub end_line: Option<i64>,
    /// `true` for an annotation-materialised policy node
    /// ([`NodeKind::Layer`]/[`NodeKind::Boundary`]) that is cleared and rebuilt
    /// each annotation run (S-014, [FR-AN-03]). `false` for every extracted node.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub derived: bool,
    /// Declaration visibility captured by Pass 1 — the exported-is-live
    /// dead-code root set ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub exported: bool,
    /// Cyclomatic complexity, for `Function`/`Method` nodes ([FR-AN-04]).
    ///
    /// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
    pub cyclomatic_complexity: Option<i64>,
    /// Physical line count of the definition, for `Function`/`Method` nodes.
    pub line_count: Option<i64>,
    /// The normalised AST-shape fingerprint duplicate detection groups by
    /// ([FR-AN-02]); `Function`/`Method` nodes only.
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub fingerprint: Option<&'a str>,
    /// Extraction-time test-marker evidence captured by Pass 1 ([FR-EX-06],
    /// S-027): `true` exactly when this function/method carries a
    /// language-native test marker. The persisted **input** to the unified
    /// `is_test` annotation ([FR-AN-05]) — never inferred, never on a
    /// non-callable.
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub test_evidence: bool,
    /// The FTS-indexed `nodes.body` text. Two producers set it: a `DocSection`'s
    /// own prose beneath its heading, excluding nested sub-sections ([FR-DG-05],
    /// S-037); and a config typed-anchor's **payload subtype** (S-065, [CR-010],
    /// [FR-CG-03]) — e.g. `"object"`/`"interface"` for a `GqlType`. `None` for
    /// every code node and for any node with no searchable body / no payload, so
    /// the column (and its `nodes_fts` posting) stays `NULL`. Note: do **not**
    /// assume `body.is_some()` implies `DocSection` — config anchors set it too.
    ///
    /// [FR-DG-05]: ../../../docs/specs/requirements/FR-DG-05.md
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
    pub body: Option<&'a str>,
    /// The per-function maximum block-structure nesting depth captured by Pass 1
    /// (CR-005, [FR-EX-07]); `Function`/`Method` nodes only, `None` otherwise.
    /// The input to the Nesting and Conciseness dimensions ([FR-QM-09]/
    /// [FR-QM-10]).
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    pub max_nesting_depth: Option<i64>,
}

impl<'a> NewNode<'a> {
    /// A plain extracted node with no annotation payload: not derived, not
    /// exported, no metrics, no fingerprint. The common starting point — set
    /// the annotation fields the caller actually knows.
    pub fn plain(symbol_id: i64, kind: NodeKind, name: &'a str) -> Self {
        NewNode {
            symbol_id,
            kind,
            name,
            file_id: None,
            start_line: None,
            end_line: None,
            derived: false,
            exported: false,
            cyclomatic_complexity: None,
            line_count: None,
            fingerprint: None,
            test_evidence: false,
            body: None,
            max_nesting_depth: None,
        }
    }
}

/// The annotation-pass snapshot of one node — the columns Pass 3 computes over
/// (S-014, [FR-AN-01..03]), read in `id` order for a deterministic run
/// ([NFR-RA-06]).
///
/// Deliberately a separate row type from [`NodeRow`]: the hydration/navigation
/// read path does not pay for the annotation columns, and the annotation pass
/// does not depend on the point-query projection.
///
/// [FR-AN-01..03]: ../../../docs/specs/requirements/FR-AN-01.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationNodeRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The node ontology kind.
    pub kind: NodeKind,
    /// The human-facing name (matched against `[semantics].entry_points`).
    pub name: String,
    /// Declaration visibility — the exported-is-live root set ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub exported: bool,
    /// `true` for a policy node a previous annotation run materialised.
    pub derived: bool,
    /// The AST-shape fingerprint, for `Function`/`Method` nodes ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub fingerprint: Option<String>,
    /// Extraction-time test-marker evidence captured by Pass 1 ([FR-EX-06]) —
    /// the persisted input to the unified `is_test` verdict Pass 3 computes
    /// ([FR-AN-05]).
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub test_evidence: bool,
    /// The defining file's id, if the node is bound to one.
    pub file_id: Option<i64>,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// The dead-code verdict of the last annotation run; `None` = not yet
    /// annotated / not a dead-code candidate ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub is_dead: Option<bool>,
    /// The duplicate verdict of the last annotation run; `None` = not yet
    /// annotated / carries no fingerprint ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub is_duplicate: Option<bool>,
    /// The unified test verdict of the last annotation run ([FR-AN-05]) — a
    /// definite boolean (`false` before the first run), never the tri-state
    /// `NULL` the dead/duplicate verdicts carry.
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    pub is_test: bool,
    /// The `[[layers]]` band of the defining file, when one matched
    /// ([FR-AN-03]).
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub layer_membership: Option<String>,
    /// The stable near-clone group id of the last annotation run — the minimum
    /// node id of the function's near-clone component, or `None` when it belongs
    /// to no near-clone group ([FR-AN-06], CR-005). The input the Uniqueness
    /// dimension consumes ([FR-QM-13]).
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub clone_group: Option<NodeId>,
}

/// One indexed file with the content hash recorded at its last (re-)index.
///
/// The [pipeline-orchestrator] reads these back to drive incremental-sync dirty
/// detection: a candidate file whose freshly computed blake3 hash equals the
/// stored `content_hash` is unchanged and is skipped without re-extraction
/// ([FR-SY-03]).
///
/// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
/// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileRecord {
    /// The file row's id — the `unresolved_refs.file_id` FK target (S-011).
    pub id: i64,
    /// The project-relative path key (matches `FileInput::path`).
    pub path: String,
    /// The content hash stored at the last index, if one was recorded.
    pub content_hash: Option<String>,
}

/// Whole-store row counts — the cheap index-health snapshot behind the
/// navigation `status` tool ([FR-NV-07]).
///
/// One prepared statement of `COUNT(*)` subselects: no table scan of row
/// *content*, no discovery walk — safe inside the navigation freshness
/// contract ([FR-RC-05]).
///
/// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
/// [FR-RC-05]: ../../../docs/specs/requirements/FR-RC-05.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StoreCounts {
    /// Rows in `files` — indexed source files.
    pub files: u64,
    /// Rows in `nodes` — graph vertices.
    pub nodes: u64,
    /// Rows in `edges` — graph relationships.
    pub edges: u64,
    /// Rows in `unresolved_refs` — the whole reference ledger (S-011).
    pub refs_total: u64,
    /// Ledger rows currently bound to an edge (`resolved = 1`).
    pub refs_resolved: u64,
}

/// A cross-file edge captured before a synced file's nodes are deleted, so it
/// can be re-attached after re-extraction ([ADR-10] capture-before-delete).
///
/// Endpoints are recorded by their stable [`LogosSymbol`] string rather than by
/// rowid: deleting and re-inserting a file's nodes assigns fresh rowids, but the
/// canonical-ordinal symbol of an unchanged declaration is invariant ([ADR-07]),
/// so the edge rebinds by symbol after the file is re-extracted.
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEdge {
    /// The source endpoint's symbol (a node in another file — untouched by the sync).
    pub source_symbol: String,
    /// The target endpoint's symbol (a node in the synced file — re-created on re-extract).
    pub target_symbol: String,
    /// The edge kind discriminant ([`EdgeKind::as_i32`]).
    pub kind: i32,
}

/// One row of the `unresolved_refs` reference ledger (S-011, [ADR-10]),
/// mapped to model types.
///
/// `form` is recovered via [`RefForm::try_from`] and `kind` via
/// [`EdgeKind::try_from`]; a value failing either conversion is a corrupt
/// store, surfaced as an error advising a rebuild ([NFR-RA-08]).
///
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
/// [NFR-RA-08]: ../../../docs/specs/requirements/NFR-RA-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnresolvedRefRow {
    /// The ledger row id.
    pub id: i64,
    /// The file whose (re-)extraction produced this row, if still indexed.
    pub file_id: Option<i64>,
    /// The referencing declaration's stable symbol string.
    pub source_symbol: String,
    /// The reference target text, interpreted per `form`.
    pub target: String,
    /// The in-scope name an import binds (`use a::b` → `b`, `use a::b as c`
    /// → `c`); `None` for non-import forms.
    pub alias: Option<String>,
    /// How `target` is to be interpreted by the resolution pass.
    pub form: RefForm,
    /// The edge kind a successful binding produces.
    pub kind: EdgeKind,
    /// 1-based source line of the reference, when known.
    pub line: Option<i64>,
    /// `true` once the resolution pass has bound this ref to an edge.
    pub resolved: bool,
    /// The cross-artifact relation class ([`ArtifactRelation`] wire token) for an
    /// `ArtifactRef`/`ArtifactBinding` ref (CR-011, [FR-CG-07]); `None` for every
    /// code/doc/access ref. Carried so the resolution pass can label the bound
    /// edge and per-relation-class coverage can group the ledger ([FR-CG-11]).
    ///
    /// [ArtifactRelation]: crate::model::ArtifactRelation
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    pub payload: Option<String>,
}

/// One function/method node's metric inputs for the quality metrics engine
/// (S-018, [FR-QM-04], [FR-QM-05]).
///
/// The minimal per-function slice the Equality (Gini of cyclomatic complexity)
/// and Redundancy (dead/duplicate ratio) metrics need — captured by Pass 1
/// (complexity) and Pass 3 (verdicts). `None` keeps the tri-state honesty of
/// the native columns: "not computed", never a silent zero ([NFR-CC-04]).
///
/// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
/// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetricRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// Per-function cyclomatic complexity from Pass 1; `None` = not computed.
    pub cyclomatic_complexity: Option<i64>,
    /// The dead-code verdict of the last annotation run ([FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub is_dead: Option<bool>,
    /// The duplicate verdict of the last annotation run ([FR-AN-02]).
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub is_duplicate: Option<bool>,
    /// Physical line count of the definition from Pass 1; `None` = not computed.
    /// A brain-method input ([FR-QM-10]).
    ///
    /// [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
    pub line_count: Option<i64>,
    /// Per-function maximum block-structure nesting depth from Pass 1 (CR-005,
    /// [FR-EX-07]); `None` = not computed / un-re-extracted. The Nesting input
    /// ([FR-QM-09]) and a brain-method input ([FR-QM-10]).
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    /// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
    pub max_nesting_depth: Option<i64>,
    /// The stable near-clone group id of the last annotation run (CR-005,
    /// [FR-AN-06]) — the minimum node id of the function's near-clone component,
    /// or `None` when it belongs to no near-clone group. The Uniqueness input
    /// ([FR-QM-13]): a function is near-cloned iff this is non-`None`.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub clone_group: Option<NodeId>,
}

/// One persisted `metric_snapshots` row (S-018, [FR-QM-07]).
///
/// Raw + normalized per metric, the counts the run scored, the `empty` flag,
/// and the rounded aggregate signal — `None` is the [ADR-12] "n/a" sentinel
/// (empty graph), distinct from a real `0` (zero short-circuit).
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricSnapshotRow {
    /// The snapshot row id (append-only series order).
    pub id: i64,
    /// Unix-seconds timestamp the snapshot was recorded at.
    pub created_at: i64,
    /// Optional VCS commit pin for the snapshot.
    pub commit_sha: Option<String>,
    /// Vertices in the metric graph the run scored.
    pub node_count: i64,
    /// Edges in the metric graph the run scored.
    pub edge_count: i64,
    /// Production function/method nodes considered by Equality/Redundancy
    /// (`is_test=true` excluded, [FR-QM-08]).
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope
    /// ([FR-QM-08], [FR-QM-07]): the persisted "N test functions excluded"
    /// count. `0` on snapshots written before migration 7.
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    pub test_function_count: i64,
    /// The metrics-semantics version the snapshot was scored under
    /// ([FR-GV-10]): the gate auto-re-baselines when this differs from the
    /// current [`METRIC_SEMANTICS_VERSION`](crate::metrics::METRIC_SEMANTICS_VERSION).
    /// `1` (test-inclusive) on snapshots written before migration 7.
    ///
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub metric_version: i64,
    /// The [ADR-12] empty-graph flag (`node_count == 0`).
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub empty: bool,
    /// Raw + normalized per-metric values, in canonical metric order:
    /// modularity, acyclicity, depth, equality, redundancy ([ADR-08]).
    ///
    /// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
    pub modularity_raw: f64,
    /// Normalized modularity `(Q+0.5)/1.5` clamped to [0,1] ([FR-QM-01]).
    ///
    /// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
    pub modularity_normalized: f64,
    /// Cycle count — multi-node SCCs (`len > 1`) only; self-recursion
    /// excluded ([FR-QM-02], metric-semantics v4).
    ///
    /// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
    pub acyclicity_raw: f64,
    /// Normalized acyclicity `1/(1+cycles)`.
    pub acyclicity_normalized: f64,
    /// Longest path (vertex count) over the condensation ([FR-QM-03]).
    ///
    /// [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
    pub depth_raw: f64,
    /// Normalized depth `1/(1+depth/8)`.
    pub depth_normalized: f64,
    /// Gini coefficient of per-function cyclomatic complexity ([FR-QM-04]).
    ///
    /// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
    pub equality_raw: f64,
    /// Normalized equality `1 − Gini`.
    pub equality_normalized: f64,
    /// Redundant-function ratio (dead or duplicate, counted once)
    /// ([FR-QM-05]).
    ///
    /// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
    pub redundancy_raw: f64,
    /// Normalized redundancy `1 − ratio`.
    pub redundancy_normalized: f64,
    /// The effective detection-threshold set hash the snapshot scored under
    /// (CR-005, [FR-QM-14], [BR-25]); `None` on a pre-v3 snapshot. The gate
    /// auto-re-baselines when this differs from the baseline ([FR-GV-10]), wired
    /// in S-045. The five new dimensions' raw+normalized pairs and applicability
    /// flags are persisted ([FR-QM-07]) but not surfaced on this read model — the
    /// fresh-compute [`MetricSnapshot`](crate::models::quality::MetricSnapshot)
    /// carries them for `scan`; this row backs the gate/evolution series.
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub thresholds_hash: Option<String>,
    /// The rounded 0–10000 signal; `None` = "n/a" (empty graph, [ADR-12]).
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub aggregate_signal: Option<i64>,
}

/// The most-recent persisted `metric_snapshots` row with the **full** CR-005
/// dimension set — every persisted dimension (all ten) plus the two
/// applicability flags ([FR-QM-09]..[FR-QM-14]).
///
/// Unlike [`MetricSnapshotRow`] — which deliberately surfaces only the original
/// five for the gate/evolution series — this read model carries the structural
/// dimensions so the read-only `latest_metrics` accessor ([ADR-28], [CR-018],
/// S-082) can return the last persisted snapshot's whole breakdown to the web
/// dashboard **without re-computing one**: a dashboard GET reflects the last
/// `scan`, it never triggers a new evaluate-and-persist. The `*_normalized`
/// pairs are `None` only on a pre-v3 snapshot (scored before the structural
/// dimensions existed) or — for `cohesion`/`focus` — when the dimension dropped
/// out of the mean (no applicable construct, the [`MetricSnapshot`] n/a sentinel).
///
/// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
/// [`MetricSnapshot`]: crate::models::quality::MetricSnapshot
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatestMetricSnapshot {
    /// Vertices in the metric graph the run scored.
    pub node_count: i64,
    /// Edges in the metric graph the run scored.
    pub edge_count: i64,
    /// Production function/method nodes considered ([FR-QM-08]).
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope ([FR-QM-08]).
    pub test_function_count: i64,
    /// The [ADR-12] empty-graph flag (`node_count == 0`).
    pub empty: bool,
    /// The effective detection-threshold set hash the snapshot scored under
    /// ([FR-QM-14]); `None` on a pre-v3 snapshot.
    pub thresholds_hash: Option<String>,
    /// The rounded 0–10000 signal; `None` = "n/a" (empty graph, [ADR-12]).
    pub aggregate_signal: Option<i64>,
    pub modularity_raw: f64,
    pub modularity_normalized: f64,
    pub acyclicity_raw: f64,
    pub acyclicity_normalized: f64,
    pub depth_raw: f64,
    pub depth_normalized: f64,
    pub equality_raw: f64,
    pub equality_normalized: f64,
    pub redundancy_raw: f64,
    pub redundancy_normalized: f64,
    /// Structural dimensions (CR-005). `None` on a pre-v3 snapshot.
    pub nesting_raw: Option<f64>,
    pub nesting_normalized: Option<f64>,
    pub conciseness_raw: Option<f64>,
    pub conciseness_normalized: Option<f64>,
    /// Cohesion / Focus value columns are `None` exactly when their
    /// `*_applicable` flag is `Some(false)` — the applicability drop-out.
    pub cohesion_raw: Option<f64>,
    pub cohesion_normalized: Option<f64>,
    pub cohesion_applicable: Option<bool>,
    pub focus_raw: Option<f64>,
    pub focus_normalized: Option<f64>,
    pub focus_applicable: Option<bool>,
    pub uniqueness_raw: Option<f64>,
    pub uniqueness_normalized: Option<f64>,
}

/// The fields needed to insert a `metric_snapshots` row (S-018, [FR-QM-07]).
///
/// Same shape as [`MetricSnapshotRow`] minus the assigned `id`. The table is
/// append-only: there is deliberately no update primitive.
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
#[derive(Debug, Clone)]
pub struct NewMetricSnapshot<'a> {
    /// Unix-seconds timestamp to record.
    pub created_at: i64,
    /// Optional VCS commit pin.
    pub commit_sha: Option<&'a str>,
    /// Vertices in the metric graph.
    pub node_count: i64,
    /// Edges in the metric graph.
    pub edge_count: i64,
    /// Production function/method nodes considered ([FR-QM-08]).
    ///
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    pub function_count: i64,
    /// Test function/method nodes excluded from the production scope
    /// ([FR-QM-08], [FR-QM-07]).
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    pub test_function_count: i64,
    /// The metrics-semantics version this snapshot was scored under
    /// ([FR-GV-10]) — the gate's re-baseline trigger.
    ///
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    pub metric_version: i64,
    /// The [ADR-12] empty-graph flag.
    ///
    /// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
    pub empty: bool,
    /// Raw Newman Q.
    pub modularity_raw: f64,
    /// Normalized modularity.
    pub modularity_normalized: f64,
    /// Raw cycle count.
    pub acyclicity_raw: f64,
    /// Normalized acyclicity.
    pub acyclicity_normalized: f64,
    /// Raw condensed longest-path depth.
    pub depth_raw: f64,
    /// Normalized depth.
    pub depth_normalized: f64,
    /// Raw Gini coefficient.
    pub equality_raw: f64,
    /// Normalized equality.
    pub equality_normalized: f64,
    /// Raw redundant-function ratio.
    pub redundancy_raw: f64,
    /// Normalized redundancy.
    pub redundancy_normalized: f64,
    /// Raw deep-nesting ratio (CR-005, [FR-QM-09]); `None` persists `NULL` on a
    /// pre-v3 snapshot that scored only the original five.
    ///
    /// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
    pub nesting_raw: Option<f64>,
    /// Normalized Nesting `(1 − ratio)` floored at 0.01 ([FR-QM-09]).
    pub nesting_normalized: Option<f64>,
    /// Raw brain-method ratio (CR-005, [FR-QM-10]).
    ///
    /// [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
    pub conciseness_raw: Option<f64>,
    /// Normalized Conciseness `(1 − ratio)` floored at 0.01 ([FR-QM-10]).
    pub conciseness_normalized: Option<f64>,
    /// Raw mean `1/LCOM4` over production classes (CR-005, [FR-QM-11]); `None`
    /// when Cohesion dropped out of the mean (no applicable classes) or on a
    /// pre-v3 snapshot.
    ///
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    pub cohesion_raw: Option<f64>,
    /// Normalized Cohesion (the mean `1/LCOM4`) floored at 0.01 ([FR-QM-11]);
    /// `None` exactly when [`cohesion_applicable`](Self::cohesion_applicable) is
    /// `Some(false)`.
    pub cohesion_normalized: Option<f64>,
    /// `Some(true)` when Cohesion applied to the mean, `Some(false)` when it
    /// dropped out (no applicable classes, [FR-QM-11]/[FR-QM-14]); `None` on a
    /// pre-v3 snapshot.
    pub cohesion_applicable: Option<bool>,
    /// Raw god-container ratio (CR-005, [FR-QM-12]); `None` when Focus dropped
    /// out (no class-like containers) or on a pre-v3 snapshot.
    ///
    /// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
    pub focus_raw: Option<f64>,
    /// Normalized Focus `(1 − ratio)` floored at 0.01 ([FR-QM-12]); `None`
    /// exactly when [`focus_applicable`](Self::focus_applicable) is `Some(false)`.
    pub focus_normalized: Option<f64>,
    /// `Some(true)` when Focus applied, `Some(false)` when it dropped out (no
    /// class-like containers, [FR-QM-12]/[FR-QM-14]); `None` on a pre-v3 snapshot.
    pub focus_applicable: Option<bool>,
    /// Raw near-clone ratio (CR-005, [FR-QM-13]).
    ///
    /// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
    pub uniqueness_raw: Option<f64>,
    /// Normalized Uniqueness `(1 − ratio)` floored at 0.01 ([FR-QM-13]).
    pub uniqueness_normalized: Option<f64>,
    /// The effective detection-threshold set hash this run scored under (CR-005,
    /// [FR-QM-14], [BR-25]); `None` on a pre-v3 snapshot. A baseline mismatch
    /// triggers the announced auto-re-baseline ([FR-GV-10]).
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    pub thresholds_hash: Option<&'a str>,
    /// The rounded signal; `None` persists SQL `NULL` (the "n/a" sentinel).
    pub aggregate_signal: Option<i64>,
}

/// One function/method node's constraint inputs for the governance evaluator
/// (S-020, [FR-GV-02]): the per-function point-query slice behind `max_cc`
/// and `max_fn_lines`, carrying the name/file/line needed for an actionable
/// violation message.
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraintRow {
    /// The storage-local rowid handle.
    pub id: NodeId,
    /// The human-facing name.
    pub name: String,
    /// The defining file's path, if the node is bound to one.
    pub file_path: Option<String>,
    /// 1-based start line of the declaration, when recorded.
    pub start_line: Option<i64>,
    /// Per-function cyclomatic complexity from Pass 1; `None` = not computed.
    pub cyclomatic_complexity: Option<i64>,
    /// Per-function line count from Pass 1; `None` = not computed.
    pub line_count: Option<i64>,
}

/// The store-integrity slice behind the `health` tool (S-020): the schema
/// version, readable from the RO pool. The FTS coherence half lives on
/// [`BatchWriter::fts_integrity_check`] — FTS5's `'integrity-check'` command
/// is an INSERT, so it needs the writer connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreHealth {
    /// The `PRAGMA user_version` ([FR-DB-04]).
    ///
    /// [FR-DB-04]: ../../../docs/specs/requirements/FR-DB-04.md
    pub schema_version: i64,
}

/// The fast **structural-integrity** census behind `doctor` ([FR-GV-18],
/// [NFR-RA-13], [ADR-46]): the invariant `node_count == distinct(symbol_id)`
/// plus the three orphan-row counters (dangling `file_id`, dangling edge
/// endpoints, orphan shingles).
///
/// Computed by [`structural_report`] in O(a handful of indexed queries) — every
/// counter is a `COUNT` over a primary-key anti-join or the `symbol_id`
/// distinct, so it never scans row content and stays inside the point-query
/// latency budget ([NFR-PE-01]). A healthy graph reports every counter zero
/// ([`StructuralReport::is_ok`]); any non-zero counter is a structural fault the
/// gate hard-fails on, independent of the metric signal ([FR-GV-02],
/// [FR-GV-05]).
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StructuralReport {
    /// `COUNT(*) FROM nodes` — total graph vertices.
    pub node_count: u64,
    /// `COUNT(DISTINCT symbol_id) FROM nodes` — the one-node-per-symbol target.
    pub distinct_symbol_ids: u64,
    /// `node_count − distinct_symbol_ids`: nodes leaked past the
    /// one-per-`symbol_id` invariant (Channel A, [ADR-46]). Zero on a healthy
    /// graph — the `UNIQUE(symbol_id)` schema constraint makes it structurally
    /// unreachable at write time; a non-zero value is a regression that bypassed
    /// the constraint.
    pub duplicate_symbol_nodes: u64,
    /// Nodes whose non-NULL `file_id` references a `files` row that is gone
    /// (the `ON DELETE SET NULL` FK should nullify these — a survivor is drift).
    pub dangling_file_refs: u64,
    /// Edges whose `source` or `target` references a missing `nodes` row (the
    /// `ON DELETE CASCADE` FK should reap these — a survivor is drift).
    pub dangling_edge_endpoints: u64,
    /// Shingles whose `node_id` references a missing `nodes` row (the
    /// `ON DELETE CASCADE` FK should reap these — a survivor is drift).
    pub orphan_shingles: u64,
}

impl StructuralReport {
    /// `true` when the graph holds exactly one node per `symbol_id` and carries
    /// no orphan rows — the [NFR-RA-13] invariant.
    ///
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    pub fn is_ok(&self) -> bool {
        self.duplicate_symbol_nodes == 0
            && self.dangling_file_refs == 0
            && self.dangling_edge_endpoints == 0
            && self.orphan_shingles == 0
    }

    /// One human-readable fault line per non-zero counter, in a deterministic
    /// order ([NFR-RA-06]) — empty when [`is_ok`](Self::is_ok). Each names the
    /// structural fault so the gate can report *which* invariant broke, even
    /// when the metric signal is unchanged ([FR-GV-18]).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        if self.duplicate_symbol_nodes > 0 {
            faults.push(format!(
                "{} duplicate-symbol node(s): {} nodes for {} distinct symbol_id \
                 (expected one node per symbol_id, NFR-RA-13)",
                self.duplicate_symbol_nodes, self.node_count, self.distinct_symbol_ids
            ));
        }
        if self.dangling_file_refs > 0 {
            faults.push(format!(
                "{} node(s) with a dangling file_id (references a missing files row)",
                self.dangling_file_refs
            ));
        }
        if self.dangling_edge_endpoints > 0 {
            faults.push(format!(
                "{} edge(s) with a dangling endpoint (source or target references a missing node)",
                self.dangling_edge_endpoints
            ));
        }
        if self.orphan_shingles > 0 {
            faults.push(format!(
                "{} orphan shingle(s) (node_id references a missing node)",
                self.orphan_shingles
            ));
        }
        faults
    }
}

/// Run the fast structural-integrity census ([`StructuralReport`], [FR-GV-18],
/// [NFR-RA-13]) against `conn`.
///
/// Four indexed reads: the `node_count`/`distinct(symbol_id)` pair in one row,
/// then three primary-key anti-joins (dangling `file_id`, dangling edge
/// endpoints, orphan shingles). No table-content scan — it stays inside the
/// point-query budget ([NFR-PE-01]) and is cheap enough to assert in debug
/// builds after every `index`/`sync`.
///
/// Free-standing over `&Connection` so both the read seam
/// ([`GraphStore::structural_check`]) and the writer connection can call it, and
/// so a hand-built dirty fixture can be checked directly in tests.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
pub(crate) fn structural_report(conn: &Connection) -> Result<StructuralReport> {
    let (node_count, distinct_symbol_ids) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT symbol_id) FROM nodes",
            [],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
        )
        .context("counting nodes vs distinct symbol_id")?;

    // Anti-joins keyed on primary keys (files.id, nodes.id): SQLite serves each
    // NOT EXISTS as an indexed lookup, so these never scan row content.
    let dangling_file_refs = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes n \
             WHERE n.file_id IS NOT NULL \
               AND NOT EXISTS (SELECT 1 FROM files f WHERE f.id = n.file_id)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting dangling file_id references")? as u64;

    let dangling_edge_endpoints = conn
        .query_row(
            "SELECT COUNT(*) FROM edges e \
             WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.source) \
                OR NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.target)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting dangling edge endpoints")? as u64;

    let orphan_shingles = conn
        .query_row(
            "SELECT COUNT(*) FROM shingles sh \
             WHERE NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = sh.node_id)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .context("counting orphan shingles")? as u64;

    Ok(StructuralReport {
        node_count,
        distinct_symbol_ids,
        duplicate_symbol_nodes: node_count.saturating_sub(distinct_symbol_ids),
        dangling_file_refs,
        dangling_edge_endpoints,
        orphan_shingles,
    })
}

/// The singleton `rules_cache` row ([FR-GV-01], S-020): the blake3 hash of
/// `rules.toml` and its parsed JSON, so an unchanged contract skips the TOML
/// parse + validation on re-run.
///
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesCacheRow {
    /// blake3 hex hash of the `rules.toml` content the cache was built from.
    pub rules_hash: String,
    /// The parsed [`Rules`](crate::config::Rules) as canonical JSON.
    pub parsed_json: String,
}

/// One persisted `violations` row (S-020, [FR-GV-02], SRS §5.1).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViolationRow {
    /// The row id (insertion order within the run).
    pub id: i64,
    /// The metric snapshot persisted by the same run (`scan`), if any.
    pub snapshot_id: Option<i64>,
    /// `"constraint"`, `"layer"`, or `"boundary"`.
    pub rule_type: String,
    /// The rule key (e.g. `max_cc`, `layer-ordering`, `boundary:a->b`).
    pub rule_key: String,
    /// The offending node, when the violation points at one.
    pub node_id: Option<i64>,
    /// The offending file, when the violation points at one.
    pub file: Option<String>,
    pub message: String,
    /// `"error"` or `"warning"` ([FR-GV-03]).
    ///
    /// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
    pub severity: String,
}

/// The fields needed to insert a `violations` row (S-020, [FR-GV-02]).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
#[derive(Debug, Clone)]
pub struct NewViolation<'a> {
    /// The metric snapshot persisted by the same run, if any.
    pub snapshot_id: Option<i64>,
    /// `"constraint"`, `"layer"`, or `"boundary"`.
    pub rule_type: &'a str,
    /// The rule key.
    pub rule_key: &'a str,
    /// The offending node, when known.
    pub node_id: Option<i64>,
    /// The offending file, when known.
    pub file: Option<&'a str>,
    pub message: &'a str,
    /// `"error"` or `"warning"`.
    pub severity: &'a str,
    /// Unix-seconds timestamp of the run.
    pub created_at: i64,
}

/// The fields needed to insert a reference-ledger row (S-011).
///
/// Insertion is idempotent over `(source_symbol, target, form, kind)` — the
/// ledger's UNIQUE rule absorbs a function calling the same path twice.
#[derive(Debug, Clone, Copy)]
pub struct NewUnresolvedRef<'a> {
    /// FK into `files(id)` — the file whose extraction produced the ref.
    pub file_id: Option<i64>,
    /// The referencing declaration's stable symbol string.
    pub source_symbol: &'a str,
    /// The reference target text, interpreted per `form`.
    pub target: &'a str,
    /// The in-scope name an import binds; `None` for non-import forms.
    pub alias: Option<&'a str>,
    /// How `target` is to be interpreted.
    pub form: RefForm,
    /// The edge kind a successful binding produces.
    pub kind: EdgeKind,
    /// 1-based source line of the reference, when known.
    pub line: Option<i64>,
    /// The cross-artifact relation class ([`ArtifactRelation`](crate::model::ArtifactRelation)
    /// wire token) for an `ArtifactRef`/`ArtifactBinding` ref (CR-011); `None` for
    /// every code/doc/access ref.
    pub payload: Option<&'a str>,
}

/// The point-query read interface over the code graph.
///
/// Implemented by [`SqliteGraphStore`]; this is the seam navigation/resolution
/// components depend on rather than the concrete store ([graph-store
/// component]).
///
/// [graph-store component]: ../../../docs/specs/architecture/components/graph-store.md
pub trait GraphStore {
    /// Fetch a single node by id, or `None` if no such node exists.
    fn node(&self, id: NodeId) -> Result<Option<NodeRow>>;

    /// All nodes that *call* `id` — the sources of inbound `calls` edges
    /// (backed by `idx_edges_target_kind`).
    fn callers(&self, id: NodeId) -> Result<Vec<NodeRow>>;

    /// All nodes that `id` *calls* — the targets of outbound `calls` edges
    /// (backed by `idx_edges_source_kind`).
    fn callees(&self, id: NodeId) -> Result<Vec<NodeRow>>;

    /// Full-text search over node names, optionally filtered by `kind`,
    /// returning up to `limit` rows ranked best-match-first.
    ///
    /// `query` is bound as an FTS5 MATCH parameter — never interpolated. An
    /// empty or whitespace-only `query` returns no rows (`Ok(vec![])`).
    fn search(&self, query: &str, kind: Option<NodeKind>, limit: i64) -> Result<Vec<NodeRow>>;

    /// Fetch the node carrying the exact canonical `symbol` string, or `None`.
    ///
    /// The navigation symbol-argument resolver's first strategy ([FR-NV-04]):
    /// an agent that already holds a canonical [`LogosSymbol`] (from a prior
    /// result) round-trips it exactly. Deterministic when a symbol ever maps
    /// to more than one node (lowest `id` wins, same rule as
    /// [`BatchWriter::node_id_for_symbol`]).
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    fn node_by_symbol(&self, symbol: &str) -> Result<Option<NodeRow>>;

    /// All nodes whose human-facing `name` equals `name` exactly, ordered by
    /// `id`.
    ///
    /// The navigation symbol-argument resolver's second strategy: `callers
    /// parse_config` should hit the function named `parse_config` without the
    /// caller knowing canonical symbol syntax ([FR-NV-05]).
    ///
    /// [FR-NV-05]: ../../../docs/specs/requirements/FR-NV-05.md
    fn nodes_by_name(&self, name: &str) -> Result<Vec<NodeRow>>;

    /// Every node `name` defined in the file at `path` (empty when the file is
    /// absent or carries no nodes).
    ///
    /// The CR-015 incremental resolver reads this for each changed or removed
    /// file *before* persist replaces it, building the set of symbol names a
    /// sync adds or removes. Those "dirty" names decide which otherwise-untouched
    /// reference rows must be re-bound; the rest provably keep their binding, so
    /// the sync skips them instead of re-resolving the whole ledger — same result
    /// as a full re-bind ([FR-RS-03]), a fraction of the work. It must run
    /// pre-persist because the re-extract deletes the very nodes it returns.
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    fn node_names_for_path(&self, path: &str) -> Result<Vec<String>>;

    /// Every inbound edge of **any** kind: `(edge kind, source node)` pairs,
    /// ordered by `(kind, source id)`.
    ///
    /// The immediate-edges half of the `node` read-model ([FR-NV-04]) —
    /// [`callers`](GraphStore::callers) is the `Calls`-only special case.
    ///
    /// [FR-NV-04]: ../../../docs/specs/requirements/FR-NV-04.md
    fn neighbours_in(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>>;

    /// Every outbound edge of **any** kind: `(edge kind, target node)` pairs,
    /// ordered by `(kind, target id)`.
    fn neighbours_out(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>>;

    /// Whole-store row counts for the `status` health snapshot ([FR-NV-07]).
    ///
    /// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
    fn counts(&self) -> Result<StoreCounts>;

    /// The per-project **language composition** ([FR-UI-10]): one entry per
    /// language **present** in the graph, with its node count and the number of
    /// distinct files that contributed those nodes, ordered deterministically
    /// (node count descending, then language ascending — [NFR-RA-06]).
    ///
    /// Presence is read from the `nodes ⋈ files` join, so a language appears
    /// **iff** it has at least one indexed node: a registered-but-unused grammar
    /// is absent, a file tagged with a language but carrying no nodes never
    /// fabricates an entry, and a node whose `file_id` was nulled (its file
    /// deleted) contributes to no language. Every count is therefore a graph
    /// fact ([NFR-RA-05]). An empty graph yields an empty vector.
    ///
    /// [FR-UI-10]: ../../../docs/specs/requirements/FR-UI-10.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn language_composition(&self) -> Result<Vec<LanguageCount>>;

    /// Best-effort "did you mean" name suggestions for `text` ([FR-NV-09]).
    ///
    /// Tries an FTS5 prefix match on the first search token, then falls back
    /// to a substring `LIKE` over node names. Returns distinct names, at most
    /// `limit`. Never errors on odd input — an unparsable FTS query simply
    /// yields the fallback path ([FR-NV-09] graceful contract).
    ///
    /// [FR-NV-09]: ../../../docs/specs/requirements/FR-NV-09.md
    fn suggest(&self, text: &str, limit: i64) -> Result<Vec<String>>;

    /// Stream **every** node in the graph, ordered by `id`.
    ///
    /// This is the whole-graph read [graph-hydration] uses to materialise a
    /// petgraph view ([ADR-05]); the point queries above answer single-vertex
    /// questions. The deterministic `id` ordering is what lets hydration assign
    /// stable vertex indices so graph algorithms are reproducible ([NFR-RA-06]).
    ///
    /// [graph-hydration]: ../../../docs/specs/architecture/components/graph-hydration.md
    /// [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn all_nodes(&self) -> Result<Vec<NodeRow>>;

    /// Stream **every** edge in the graph, ordered by `(source, target, kind)`.
    ///
    /// The relationship half of the hydration read: combined with
    /// [`all_nodes`](GraphStore::all_nodes) it is everything petgraph needs to
    /// build a view. Hydration filters out [`EdgeKind::Contains`] for the
    /// dependency views ([FR-DB-06]); the store returns every kind so the caller
    /// decides. The deterministic ordering keeps the built graph reproducible
    /// ([NFR-RA-06]).
    ///
    /// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn all_edges(&self) -> Result<Vec<EdgeRow>>;

    /// The source node of every **dispatch live-root marker** — a `RoutesTo`
    /// self-edge (`source == target`) planted by the dispatch pass ([CR-043],
    /// [`crate::resolve::dispatch`]).
    ///
    /// A targeted read so the dispatch pass stays change-proportional on the
    /// sync hot path ([NFR-PE-03]): markers are a tiny set (one per
    /// framework-dispatched method), so this is O(markers), never the
    /// whole-graph [`all_edges`](GraphStore::all_edges) scan. No genuine
    /// framework route is a self-edge, so the `source == target` filter selects
    /// exactly the markers.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn dispatch_markers(&self) -> Result<Vec<NodeId>>;

    /// The `Function`/`Method` nodes defined in the given project-relative file
    /// `paths`, ordered by `id`.
    ///
    /// The change-proportional node read the dispatch pass uses on a sync
    /// ([NFR-PE-03], [`crate::resolve::dispatch`]): only the changed files' nodes
    /// are materialised, never the whole-graph [`all_nodes`](GraphStore::all_nodes)
    /// scan. An empty `paths` yields an empty result.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn callable_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>>;

    /// The subset of `node_ids` that carry a **dispatch live-root marker** — a
    /// `RoutesTo` self-edge (`source == target`, [`crate::resolve::dispatch`]).
    ///
    /// Index-served via `idx_edges_source_kind` (the `source IN (…)` + `kind`
    /// predicate), so the dispatch pass can read its existing markers
    /// change-proportionally on a sync — only for the changed files' nodes,
    /// never the whole-graph scan [`dispatch_markers`](GraphStore::dispatch_markers)
    /// does on a full index ([NFR-PE-03]). An empty `node_ids` yields nothing.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn markers_for_nodes(&self, node_ids: &[NodeId]) -> Result<Vec<NodeId>>;

    /// Every indexed file with the content hash recorded at its last index.
    ///
    /// The pipeline reads this once at the start of a sync to dirty-detect the
    /// candidate set ([FR-SY-03]) and to decide whether an auto-index is needed
    /// (an empty result means the graph has never been indexed, [FR-IX-07]).
    ///
    /// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
    /// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
    fn indexed_files(&self) -> Result<Vec<FileRecord>>;

    /// Stream the **entire** reference ledger, ordered by `id` (S-011).
    ///
    /// The resolution pass reads this as part of its immutable snapshot: every
    /// row is re-evaluated on every index/sync ([FR-RS-03] retry contract),
    /// and import rows — bound or not — are the durable per-file scope facts
    /// (alias/glob maps) name binding builds on. The `id` ordering keeps the
    /// pass deterministic ([NFR-RA-06]).
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn unresolved_refs(&self) -> Result<Vec<UnresolvedRefRow>>;

    /// Stream every node with its **annotation-pass** columns, ordered by `id`
    /// (S-014).
    ///
    /// The whole-graph read the annotation engine snapshots before computing
    /// dead-code, duplicates, and layer membership ([FR-AN-01..03]); the
    /// deterministic ordering keeps the pass reproducible ([NFR-RA-06]).
    ///
    /// [FR-AN-01..03]: ../../../docs/specs/requirements/FR-AN-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn annotation_nodes(&self) -> Result<Vec<AnnotationNodeRow>>;

    /// Stream the inverted near-clone shingle index ([FR-EX-09], the migration-10
    /// `shingles` table) as `(node_id, hash)` rows in deterministic
    /// `(node_id, hash)` order (S-043).
    ///
    /// The id-ordered iteration the near-clone clustering sub-pass requires
    /// ([FR-AN-06]): a node's shingle rows are contiguous and ascending, so the
    /// pass reads each function's set in one pass and builds the inverted index
    /// deterministically ([NFR-RA-06]). Each `hash` is the platform-independent
    /// `u64` fingerprint read back from its lossless signed-`i64` storage with
    /// the inverse cast.
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn shingle_index(&self) -> Result<Vec<(NodeId, u64)>>;

    /// Cheap existence check: does the graph carry **any** framework footprint —
    /// a promoted `route`/`component` node, or an `unresolved_refs` target naming
    /// one of `detector_prefixes` (canonical `::`-joined detector strings, the
    /// per-plugin `framework_detectors`)?
    ///
    /// The incremental [`framework`](crate::resolve::framework) gate (S-024-HF):
    /// the framework-promotion pass is a pure, idempotent function of the
    /// candidate files (those whose ledger names a detector) and the bound graph.
    /// When the post-resolve graph has neither a promoted node nor a detector ref,
    /// **no** promotion is possible, so an incremental `sync` skips the
    /// whole-graph framework snapshot entirely — the framework-free fast path the
    /// [NFR-PE-03] single-file-sync budget needs. A full [`index`](crate::pipeline::index)
    /// never calls this (it always runs the whole-graph pass). The match mirrors
    /// [`matches_detector`](crate::resolve::framework): a target equals a detector
    /// or extends it by whole `::` segments. Returns `false` for an empty prefix
    /// list with no promoted nodes.
    ///
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    fn has_framework_footprint(&self, detector_prefixes: &[String]) -> Result<bool>;

    /// The ids of every node the annotation pass marked `is_test = 1`, ordered
    /// by `id` ([FR-AN-05]).
    ///
    /// The single source of truth the `[[require_tested]]` contract ([FR-GV-13])
    /// reads to seed its reachability BFS — by reading the persisted column
    /// rather than re-deriving test status, the contract and the annotation can
    /// never disagree about what a test is (CR-001 CRA-01). The `id` ordering keeps
    /// the consumer deterministic ([NFR-RA-06]).
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn test_node_ids(&self) -> Result<Vec<NodeId>>;

    /// Stream every **non-derived** function/method node's metric inputs,
    /// ordered by `id` (S-018).
    ///
    /// The per-function slice the metrics engine's Equality and Redundancy
    /// metrics read ([FR-QM-04], [FR-QM-05]); derived policy nodes are never
    /// functions but are excluded defensively. The `id` ordering keeps the
    /// metric reduction canonical ([ADR-08], [NFR-RA-06]).
    ///
    /// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
    /// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
    /// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn function_metrics(&self) -> Result<Vec<FunctionMetricRow>>;

    /// Stream every persisted metric snapshot, ordered by `id` (S-018).
    ///
    /// The append-only series behind [FR-QM-07] verification and the
    /// `evolution` reader (S-020).
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    fn metric_snapshots(&self) -> Result<Vec<MetricSnapshotRow>>;

    /// The metric snapshot the `baseline` row for `scope` points at, or
    /// `None` when no baseline has been saved (S-020, [FR-GV-04]).
    ///
    /// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
    fn baseline_snapshot(&self, scope: &str) -> Result<Option<MetricSnapshotRow>>;

    /// The most-recent persisted snapshot's **full** dimension breakdown, or
    /// `None` when no snapshot has ever been recorded (a never-`scan`-ned store).
    ///
    /// A pure read of the last `metric_snapshots` row — it computes and persists
    /// nothing. Backs the read-only `latest_metrics` accessor the web dashboard
    /// reads through so a GET reflects the last `scan` and never writes ([ADR-28],
    /// [CR-018], S-082). Distinct from [`metric_snapshots`](Self::metric_snapshots)
    /// (the whole gate/evolution series, original five dimensions only).
    ///
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
    fn latest_metric_snapshot(&self) -> Result<Option<LatestMetricSnapshot>>;

    /// The singleton `rules_cache` row, or `None` when no `rules.toml` parse
    /// has been cached yet (S-020, [FR-GV-01]).
    ///
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    fn rules_cache(&self) -> Result<Option<RulesCacheRow>>;

    /// Stream every **non-derived** function/method node's constraint inputs,
    /// ordered by `id` (S-020, [FR-GV-02]): the per-function point queries
    /// behind `max_cc` / `max_fn_lines`, with name/file/line for actionable
    /// violation messages.
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    fn function_constraint_rows(&self) -> Result<Vec<FunctionConstraintRow>>;

    /// The persisted `violations` of the last `check_rules` run, ordered by
    /// `id` (S-020, [FR-GV-02] idempotence verification).
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    fn violations(&self) -> Result<Vec<ViolationRow>>;

    /// The store-integrity slice behind the `health` tool (S-020): schema
    /// version + FTS5 coherence in one read.
    fn store_health(&self) -> Result<StoreHealth>;

    /// The fast **structural-integrity** census behind `doctor` ([FR-GV-18],
    /// [NFR-RA-13], [ADR-46]): asserts `node_count == distinct(symbol_id)` and
    /// zero orphan rows in O(a handful of indexed queries). Readable from the RO
    /// pool, so `session_end` / `check_rules` fold the verdict in via
    /// [`Runtime::submit_read`](crate::runtime::Runtime::submit_read) and
    /// hard-fail on drift.
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    fn structural_check(&self) -> Result<StructuralReport>;

    /// Read a durable `project_metadata` value by key, or `None` when the key
    /// has never been written (CR-004, [ADR-20], the migration-15 kv table).
    ///
    /// The pipeline reads [`CONFIG_FINGERPRINT_KEY`] here to detect that the
    /// admission-relevant configuration changed since the last reconciliation
    /// and purge now-unadmitted files exactly once per change ([FR-SY-07]). A
    /// `None` (fresh DB, or a store upgraded across migration 15) reads as "no
    /// fingerprint recorded yet" and arms the first reconciliation.
    ///
    /// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    fn project_metadata(&self, key: &str) -> Result<Option<String>>;

    /// The current persisted monotonic graph revision (CR-027, [ADR-32],
    /// [FR-SY-09]), or `0` when no `index`/`sync` has advanced it yet.
    ///
    /// The durable, cross-process successor to the in-memory
    /// [`SyncStamp`](crate::hydrate::SyncStamp): a second process opening the same
    /// `logos.db` reads the identical value, and the status read-model
    /// ([RP-02]) exposes it. A fresh database (or one upgraded across migration
    /// 15 before the first advance) reads as `0` — "no graph yet". A stored value
    /// that fails to parse is likewise treated as `0` rather than erroring: the
    /// revision is a freshness hint, never a correctness gate.
    ///
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [RP-02]: ../../../docs/specs/requirements/RP-02.md
    fn graph_revision(&self) -> Result<u64> {
        Ok(self
            .project_metadata(GRAPH_REVISION_KEY)?
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0))
    }
}

/// The canonical SQLite-backed graph store — sole writer of `logos.db`.
pub struct SqliteGraphStore {
    conn: Connection,
}

impl SqliteGraphStore {
    /// Open (creating if absent) the database at `path`, apply the connection
    /// contract, and run any pending migrations.
    ///
    /// The result is a single, copyable `logos.db` file ([NFR-DM-03]): WAL
    /// sidecars (`-wal`/`-shm`) are checkpointed back into it on close.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened, the pragmas cannot be
    /// set, or a migration fails.
    ///
    /// [NFR-DM-03]: ../../../docs/specs/requirements/NFR-DM-03.md
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .with_context(|| format!("opening graph store at {}", path.display()))?;
        Self::from_connection(conn)
    }

    /// Open an ephemeral in-memory database (tests, transient analyses).
    ///
    /// The pragma contract is still applied; `journal_mode` reports `memory`
    /// rather than `wal` since WAL is meaningful only for on-disk files.
    ///
    /// # Errors
    /// Returns an error if the connection cannot be configured or migrated.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory graph store")?;
        Self::from_connection(conn)
    }

    /// Open a **read-only** connection over an already-migrated `logos.db`.
    ///
    /// This is the connection the WAL reader pool ([execution-runtime], [ADR-02])
    /// hands out: under WAL, a read-only connection serves a consistent snapshot
    /// and is **never blocked by the single writer** ([NFR-PE-01], [NFR-RA-10]).
    /// Unlike [`open`](Self::open) it does **not** migrate — a read-only
    /// connection cannot write — so the writer must have opened (and migrated)
    /// the database first. The connection contract is enforced read-side via
    /// `query_only = ON`, which rejects any accidental write at the SQL layer.
    ///
    /// # Threading
    /// The connection is opened with `SQLITE_OPEN_NO_MUTEX`, so it must be
    /// accessed by **at most one thread at a time**. The
    /// [`ReaderPool`](crate::runtime) is the supported way to get concurrent
    /// read-only access — it hands each connection to one thread at a time. Do
    /// not share a single `open_readonly` store across threads directly.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened read-only, the read-side
    /// pragmas cannot be set, or the database is not yet migrated to the latest
    /// schema version (open the writer first).
    ///
    /// [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    /// [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // READ_ONLY (never create), NO_MUTEX (each connection is used by exactly
        // one thread at a time — the pool moves it, never shares it), URI off to
        // mirror a plain file open.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening read-only graph store at {}", path.display()))?;
        configure_readonly_connection(&conn)?;

        // A read-only connection cannot migrate; assert the writer already did.
        let version = migrate::current_version(&conn)?;
        let latest = migrate::latest_version();
        if version != latest {
            return Err(anyhow!(
                "read-only graph store at {} is at schema v{version}, expected v{latest} — \
                 the writer must open (and migrate) the database before the reader pool attaches",
                path.display()
            ));
        }
        Ok(Self { conn })
    }

    /// Apply the per-connection contract then migrate to the latest schema.
    fn from_connection(mut conn: Connection) -> Result<Self> {
        configure_connection(&conn)?;
        migrate::apply_migrations(&mut conn)?;
        Ok(Self { conn })
    }

    /// The schema version recorded in `PRAGMA user_version`.
    ///
    /// # Errors
    /// Returns an error if the pragma cannot be read.
    pub fn schema_version(&self) -> Result<i64> {
        migrate::current_version(&self.conn)
    }

    /// Verify the **writer** connection pragma contract is in force ([FR-DB-02]).
    ///
    /// A cheap health check for a store opened through [`open`](Self::open) /
    /// [`open_in_memory`](Self::open_in_memory) (the writer path): confirms
    /// `foreign_keys` is ON, `synchronous` is NORMAL, `journal_mode` is WAL (or
    /// `memory` for in-memory stores), and the writer bulk-load pragmas (CR-057)
    /// are set — `cache_size`, `temp_store = MEMORY`, and, for an on-disk store, a
    /// non-zero `mmap_size` (memory-mapped I/O is inert on `:memory:`, so it reads
    /// back `0` there). It is **not** valid against a read-only pool connection
    /// ([`open_readonly`](Self::open_readonly)), which deliberately omits the
    /// bulk-load pragmas — it would report them missing.
    ///
    /// # Errors
    /// Returns an error naming the first pragma found out of contract.
    ///
    /// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
    pub fn verify_connection_contract(&self) -> Result<()> {
        let foreign_keys: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .context("reading PRAGMA foreign_keys")?;
        if foreign_keys != 1 {
            return Err(anyhow!(
                "foreign_keys is {foreign_keys}, expected 1 (ON) — FR-DB-02 violated"
            ));
        }

        let synchronous: i64 = self
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .context("reading PRAGMA synchronous")?;
        if synchronous != 1 {
            return Err(anyhow!(
                "synchronous is {synchronous}, expected 1 (NORMAL) — FR-DB-02 violated"
            ));
        }

        let journal_mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .context("reading PRAGMA journal_mode")?;
        if journal_mode != "wal" && journal_mode != "memory" {
            return Err(anyhow!(
                "journal_mode is {journal_mode:?}, expected \"wal\" — FR-DB-02 violated"
            ));
        }

        // Writer bulk-load pragmas (FR-DB-02, CR-057). `cache_size` reads back
        // the negative KiB value we set; `temp_store` reads back MEMORY (2).
        let cache_size: i64 = self
            .conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .context("reading PRAGMA cache_size")?;
        if cache_size != WRITER_CACHE_SIZE_KIB {
            return Err(anyhow!(
                "cache_size is {cache_size}, expected {WRITER_CACHE_SIZE_KIB} — \
                 the bulk-load contract (FR-DB-02, CR-057) is not in force"
            ));
        }
        let temp_store: i64 = self
            .conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .context("reading PRAGMA temp_store")?;
        if temp_store != TEMP_STORE_MEMORY {
            return Err(anyhow!(
                "temp_store is {temp_store}, expected {TEMP_STORE_MEMORY} (MEMORY) — \
                 the bulk-load contract (FR-DB-02, CR-057) is not in force"
            ));
        }
        // mmap I/O is meaningful only for an on-disk file; `:memory:` reports 0.
        if journal_mode == "wal" {
            let mmap_size: i64 = self
                .conn
                .query_row("PRAGMA mmap_size", [], |r| r.get(0))
                .context("reading PRAGMA mmap_size")?;
            if mmap_size != WRITER_MMAP_SIZE_BYTES {
                return Err(anyhow!(
                    "mmap_size is {mmap_size}, expected {WRITER_MMAP_SIZE_BYTES} — \
                     the bulk-load contract (FR-DB-02, CR-057) is not in force"
                ));
            }
        }

        Ok(())
    }

    /// Run the FTS5 external-content integrity check ([FR-DB-03], [NFR-RA-09]).
    ///
    /// Asserts the `nodes_fts` inverted index is consistent with the `nodes`
    /// content table. A desync (e.g. a missed `'delete'` trigger row) makes
    /// SQLite return `SQLITE_CORRUPT_VTAB`, surfaced here as an error.
    ///
    /// # Errors
    /// Returns an error if the FTS index has desynced.
    ///
    /// [FR-DB-03]: ../../../docs/specs/requirements/FR-DB-03.md
    /// [NFR-RA-09]: ../../../docs/specs/requirements/NFR-RA-09.md
    pub fn fts_integrity_check(&self) -> Result<()> {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .context("FTS5 integrity check failed — index desynced (NFR-RA-09)")
    }

    /// Checkpoint the WAL back into the main database file.
    ///
    /// Folds `-wal` frames into `logos.db` (TRUNCATE mode empties the WAL), so
    /// the single file is self-contained and copyable ([NFR-DM-03]). A no-op
    /// for in-memory stores.
    ///
    /// # Errors
    /// Returns an error if the checkpoint statement fails.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("checkpointing WAL into the main database file")
    }

    // ── Writers (autocommit) ─────────────────────────────────────────────────

    /// Insert a file row, returning its id.
    ///
    /// # Errors
    /// Returns an error on a constraint violation (e.g. duplicate `path`) or
    /// I/O failure.
    pub fn insert_file(
        &self,
        path: &str,
        language: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        insert_file(&self.conn, path, language, content_hash)
    }

    /// Insert the symbol if new (idempotent), returning its id.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn upsert_symbol(&self, symbol: &LogosSymbol) -> Result<i64> {
        upsert_symbol(&self.conn, symbol)
    }

    /// Insert a node, or — if a node for `node.symbol_id` already exists —
    /// update it in place ([FR-SY-10], [ADR-46]). Idempotent over
    /// `symbol_id`: re-extracting the same symbol never accumulates a second
    /// row, it folds into the existing one.
    ///
    /// # Errors
    /// Returns an error if `kind` is out of range (the `CHECK` constraint
    /// fires), a foreign key is unsatisfied, or I/O fails.
    ///
    /// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    pub fn insert_node(&self, node: &NewNode<'_>) -> Result<NodeId> {
        insert_node(&self.conn, node)
    }

    /// Insert an edge.
    ///
    /// # Errors
    /// Returns an error if `(source, target, kind)` already exists (the
    /// uniqueness rule rejects it), an endpoint node is missing, or I/O fails.
    pub fn insert_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
        insert_edge(&self.conn, source, target, kind)
    }

    /// Delete a node by id (cascading to its edges and firing the FTS
    /// `'delete'` trigger). Returns the number of rows removed (0 or 1).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_node(&self, id: NodeId) -> Result<usize> {
        delete_node(&self.conn, id)
    }

    // ── Atomic batch ─────────────────────────────────────────────────────────

    /// Run `f` inside a single transaction, committing only if it returns `Ok`.
    ///
    /// This is the crash-safety primitive ([NFR-RA-07]): the closure receives a
    /// [`BatchWriter`] and performs many writes; if any step returns `Err`, the
    /// transaction is dropped un-committed and SQLite rolls the whole batch back
    /// — no partial index/sync state survives an interruption.
    ///
    /// # Errors
    /// Returns the closure's error (after rollback), or an error if the
    /// transaction cannot be opened or committed.
    ///
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    pub fn write_batch<T>(&mut self, f: impl FnOnce(&BatchWriter<'_>) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction().context("opening write batch")?;
        let out = {
            let writer = BatchWriter { conn: &tx };
            f(&writer)?
        };
        tx.commit().context("committing write batch")?;
        Ok(out)
    }
}

impl GraphStore for SqliteGraphStore {
    fn node(&self, id: NodeId) -> Result<Option<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.id = ?1",
        )?;
        let raw = stmt
            .query_row([id.get()], map_raw_row)
            .optional()
            .context("querying node by id")?;
        raw.map(raw_to_node).transpose()
    }

    fn callers(&self, id: NodeId) -> Result<Vec<NodeRow>> {
        self.adjacent(id, Direction::Callers)
    }

    fn callees(&self, id: NodeId) -> Result<Vec<NodeRow>> {
        self.adjacent(id, Direction::Callees)
    }

    fn search(&self, query: &str, kind: Option<NodeKind>, limit: i64) -> Result<Vec<NodeRow>> {
        // An empty/whitespace query is a well-defined no-op rather than an
        // opaque FTS5 `syntax error` (which a blank search box would otherwise
        // hit). `query` and the kind filter are BOUND parameters — the FTS
        // MATCH text never enters the SQL string (NFR-SE-02 injection boundary).
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes_fts \
             JOIN nodes n ON n.id = nodes_fts.rowid \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE nodes_fts MATCH ?1 \
               AND (?2 IS NULL OR n.kind = ?2) \
             ORDER BY rank \
             LIMIT ?3",
        )?;
        let kind_filter: Option<i32> = kind.map(NodeKind::as_i32);
        let raws = stmt
            .query_map(rusqlite::params![query, kind_filter, limit], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting search results")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn node_by_symbol(&self, symbol: &str) -> Result<Option<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE s.symbol = ?1 \
             ORDER BY n.id \
             LIMIT 1",
        )?;
        let raw = stmt
            .query_row([symbol], map_raw_row)
            .optional()
            .context("querying node by canonical symbol")?;
        raw.map(raw_to_node).transpose()
    }

    fn nodes_by_name(&self, name: &str) -> Result<Vec<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.name = ?1 \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([name], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting nodes by name")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn node_names_for_path(&self, path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.name FROM nodes n \
             JOIN files f ON f.id = n.file_id \
             WHERE f.path = ?1",
        )?;
        let names = stmt
            .query_map([path], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting node names for path")?;
        Ok(names)
    }

    fn neighbours_in(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>> {
        self.neighbours(id, SQL_NEIGHBOURS_IN)
    }

    fn neighbours_out(&self, id: NodeId) -> Result<Vec<(EdgeKind, NodeRow)>> {
        self.neighbours(id, SQL_NEIGHBOURS_OUT)
    }

    fn counts(&self) -> Result<StoreCounts> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT (SELECT COUNT(*) FROM files), \
                    (SELECT COUNT(*) FROM nodes), \
                    (SELECT COUNT(*) FROM edges), \
                    (SELECT COUNT(*) FROM unresolved_refs), \
                    (SELECT COUNT(*) FROM unresolved_refs WHERE resolved = 1)",
        )?;
        stmt.query_row([], |row| {
            Ok(StoreCounts {
                files: row.get::<_, i64>(0)? as u64,
                nodes: row.get::<_, i64>(1)? as u64,
                edges: row.get::<_, i64>(2)? as u64,
                refs_total: row.get::<_, i64>(3)? as u64,
                refs_resolved: row.get::<_, i64>(4)? as u64,
            })
        })
        .context("reading store counts")
    }

    fn language_composition(&self) -> Result<Vec<LanguageCount>> {
        // Drive presence off the node⋈file join (not the `files` table): a
        // language appears iff it has ≥1 indexed node ([FR-UI-10]). The inner
        // JOIN drops nodes whose `file_id` is NULL (file deleted, FK SET NULL)
        // and files with no nodes; `f.language IS NOT NULL` drops un-tagged
        // files. `COUNT(DISTINCT n.file_id)` is the contributing-file count.
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.language, COUNT(n.id), COUNT(DISTINCT n.file_id) \
             FROM nodes n JOIN files f ON f.id = n.file_id \
             WHERE f.language IS NOT NULL \
             GROUP BY f.language",
        )?;
        let mut composition = stmt
            .query_map([], |row| {
                Ok(LanguageCount {
                    language: row.get::<_, String>(0)?,
                    nodes: row.get::<_, i64>(1)? as u64,
                    files: row.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading language composition")?;
        // Deterministic order, sorted in Rust so it is independent of SQLite's
        // collation and identical across the four target platforms ([NFR-RA-06]):
        // node count descending (the Dashboard sizes the Languages card by
        // magnitude), language name ascending as the stable tie-break.
        composition.sort_by(|a, b| {
            b.nodes
                .cmp(&a.nodes)
                .then_with(|| a.language.cmp(&b.language))
        });
        Ok(composition)
    }

    fn suggest(&self, text: &str, limit: i64) -> Result<Vec<String>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        // Strategy 1: FTS5 prefix match on the first identifier-ish token.
        // The token is embedded in a quoted FTS string ("tok"*), itself a
        // BOUND parameter — user text never reaches the SQL string (NFR-SE-02).
        if let Some(token) = first_identifier_token(text) {
            let mut stmt = self.conn.prepare_cached(
                "SELECT DISTINCT n.name \
                 FROM nodes_fts \
                 JOIN nodes n ON n.id = nodes_fts.rowid \
                 WHERE nodes_fts MATCH ?1 \
                 ORDER BY rank \
                 LIMIT ?2",
            )?;
            let fts_query = format!("\"{}\"*", token.replace('"', "\"\""));
            // An FTS syntax error on hostile input is not a failure of the
            // graceful contract — fall through to the LIKE strategy.
            let names: Vec<String> = stmt
                .query_map(rusqlite::params![fts_query, limit], |r| {
                    r.get::<_, String>(0)
                })
                .map(|rows| rows.filter_map(std::result::Result::ok).collect())
                .unwrap_or_default();
            if !names.is_empty() {
                return Ok(names);
            }
        }
        // Strategy 2: substring LIKE over names, wildcards escaped so the
        // user text matches literally.
        let escaped = text
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        if escaped.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT name FROM nodes \
             WHERE name LIKE '%' || ?1 || '%' ESCAPE '\\' \
             ORDER BY name \
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![escaped, limit], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting LIKE suggestions")?;
        Ok(rows)
    }

    fn all_nodes(&self) -> Result<Vec<NodeRow>> {
        // ORDER BY n.id is load-bearing, not cosmetic: hydration assigns petgraph
        // vertex indices in iteration order, so a stable order makes graph
        // algorithms reproducible (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             LEFT JOIN files f ON f.id = n.file_id \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([], map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting all nodes for hydration")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn all_edges(&self) -> Result<Vec<EdgeRow>> {
        // Deterministic order for reproducible hydration (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, target, kind FROM edges ORDER BY source, target, kind",
        )?;
        let raws = stmt
            .query_map([], |row| {
                let source: i64 = row.get(0)?;
                let target: i64 = row.get(1)?;
                let kind: i32 = row.get(2)?;
                Ok((source, target, kind))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting all edges for hydration")?;
        raws.into_iter()
            .map(|(source, target, kind)| {
                let kind = EdgeKind::try_from(kind).map_err(|e| {
                    anyhow!("corrupt edge kind {kind} for edge {source}->{target}: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok(EdgeRow {
                    source: NodeId(source),
                    target: NodeId(target),
                    kind,
                })
            })
            .collect()
    }

    fn dispatch_markers(&self) -> Result<Vec<NodeId>> {
        // Only the dispatch live-root markers (RoutesTo self-edges) — a tiny set,
        // so the dispatch pass never pays the whole-graph `all_edges` scan on the
        // sync hot path (NFR-PE-03). `source` order keeps the result deterministic
        // (NFR-RA-06).
        let kind = EdgeKind::RoutesTo.as_i32();
        let mut stmt = self.conn.prepare_cached(
            "SELECT source FROM edges WHERE kind = ?1 AND source = target ORDER BY source",
        )?;
        let ids = stmt
            .query_map([kind], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting dispatch live-root markers")?;
        Ok(ids.into_iter().map(NodeId).collect())
    }

    fn callable_nodes_in_files(&self, paths: &[String]) -> Result<Vec<NodeRow>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // Dynamic `IN (?,?,…)` over the (small) changed-file set; `prepare` not
        // `prepare_cached` because the placeholder count varies. `kind IN (7,8)`
        // is Function/Method (the only nodes that carry a dispatch marker).
        let placeholders = vec!["?"; paths.len()].join(",");
        let func = NodeKind::Function.as_i32();
        let method = NodeKind::Method.as_i32();
        let sql = format!(
            "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
             FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id \
             JOIN files f ON f.id = n.file_id \
             WHERE f.path IN ({placeholders}) AND n.kind IN ({func}, {method}) \
             ORDER BY n.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raws = stmt
            .query_map(rusqlite::params_from_iter(paths.iter()), map_raw_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting callable nodes in files")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    fn markers_for_nodes(&self, node_ids: &[NodeId]) -> Result<Vec<NodeId>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        // `source IN (…) AND kind = … AND source = target`: the `source IN` +
        // kind predicate is served by `idx_edges_source_kind`, so this is
        // O(changed nodes), not the whole-edge scan (NFR-PE-03). `prepare` (not
        // cached) because the placeholder count varies with the changed set.
        let kind = EdgeKind::RoutesTo.as_i32();
        let placeholders = vec!["?"; node_ids.len()].join(",");
        let sql = format!(
            "SELECT source FROM edges \
             WHERE kind = {kind} AND source = target AND source IN ({placeholders}) \
             ORDER BY source"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt
            .query_map(rusqlite::params_from_iter(node_ids.iter().map(|n| n.0)), |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting dispatch markers for nodes")?;
        Ok(ids.into_iter().map(NodeId).collect())
    }

    fn indexed_files(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, path, content_hash FROM files ORDER BY path")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    content_hash: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("listing indexed files")?;
        Ok(rows)
    }

    fn project_metadata(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .context("reading project_metadata value")
    }

    fn unresolved_refs(&self) -> Result<Vec<UnresolvedRefRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload \
             FROM unresolved_refs ORDER BY id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i32>(5)?,
                    row.get::<_, i32>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the reference ledger")?;
        raws.into_iter()
            .map(
                |(id, file_id, source_symbol, target, alias, form, kind, line, resolved, payload)| {
                    let form = RefForm::try_from(form).map_err(|e| {
                        anyhow!("corrupt ref form {form} for ref {id}: {e}; rebuild advised (NFR-RA-08)")
                    })?;
                    let kind = EdgeKind::try_from(kind).map_err(|e| {
                        anyhow!("corrupt ref kind {kind} for ref {id}: {e}; rebuild advised (NFR-RA-08)")
                    })?;
                    Ok(UnresolvedRefRow {
                        id,
                        file_id,
                        source_symbol,
                        target,
                        alias,
                        form,
                        kind,
                        line,
                        resolved: resolved != 0,
                        payload,
                    })
                },
            )
            .collect()
    }

    fn annotation_nodes(&self) -> Result<Vec<AnnotationNodeRow>> {
        // ORDER BY n.id keeps the annotation pass deterministic (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, n.kind, n.name, n.exported, n.derived, n.fingerprint, \
                    n.test_evidence, n.file_id, f.path, n.is_dead, n.is_duplicate, \
                    n.is_test, n.layer_membership, n.clone_group \
             FROM nodes n \
             LEFT JOIN files f ON f.id = n.file_id \
             ORDER BY n.id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting nodes for the annotation pass")?;
        raws.into_iter()
            .map(
                |(id, kind, name, exported, derived, fingerprint, test_evidence, file_id, file_path, dead, dup, test, layer, clone_group)| {
                    let kind = NodeKind::try_from(kind).map_err(|e| {
                        anyhow!(
                            "corrupt node kind {kind} for node {id}: {e}; rebuild advised (NFR-RA-08)"
                        )
                    })?;
                    Ok(AnnotationNodeRow {
                        id: NodeId(id),
                        kind,
                        name,
                        exported: exported != 0,
                        derived: derived != 0,
                        fingerprint,
                        test_evidence: test_evidence != 0,
                        file_id,
                        file_path,
                        is_dead: dead.map(|v| v != 0),
                        is_duplicate: dup.map(|v| v != 0),
                        is_test: test != 0,
                        layer_membership: layer,
                        clone_group: clone_group.map(NodeId),
                    })
                },
            )
            .collect()
    }

    fn shingle_index(&self) -> Result<Vec<(NodeId, u64)>> {
        // ORDER BY (node_id, hash) is the deterministic id-ordered iteration the
        // near-clone clustering pass requires (FR-AN-06, NFR-RA-06): a function's
        // rows are contiguous and ascending. `hash as u64` is the inverse of the
        // lossless u64→i64 store cast (the shingles table holds the u64 as the
        // equivalent signed INTEGER).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT node_id, hash FROM shingles ORDER BY node_id, hash")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((NodeId(row.get::<_, i64>(0)?), row.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the shingle index for near-clone clustering")?;
        Ok(rows)
    }

    fn has_framework_footprint(&self, detector_prefixes: &[String]) -> Result<bool> {
        // 1) A promoted route/component node already in the graph? Its presence
        //    alone forces the full reconcile (a demotion may be due).
        let route = crate::model::NodeKind::Route.as_i32();
        let component = crate::model::NodeKind::Component.as_i32();
        let has_promoted: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE kind = ?1 OR kind = ?2)",
                rusqlite::params![route, component],
                |row| row.get::<_, i64>(0),
            )
            .context("checking for promoted framework nodes")?
            != 0;
        if has_promoted || detector_prefixes.is_empty() {
            return Ok(has_promoted);
        }

        // 2) Any **unresolved** ledger ref naming a detector? `target = prefix` or
        //    it extends the prefix by whole `::` segments — the `matches_detector`
        //    rule in SQL. Canonical detector strings carry no GLOB metacharacters
        //    (`* ? [`), so `prefix::*` matches exactly the "extends by a segment"
        //    case. The `resolved = 0` scope is both an optimisation and a faithful
        //    mirror of the framework pass's own candidacy fingerprint: a detector
        //    names an *external* package the graph never indexes, so a detector ref
        //    can never bind and always survives as `resolved = 0` (see the
        //    framework module's "Ledger-gated candidacy" — the surviving ledger
        //    *is* the framework fingerprint). The cheap integer `resolved = 0`
        //    filter restricts the per-row GLOB work to the small unresolved tail
        //    instead of the whole ledger. EXISTS returns on the first hit; a
        //    framework-free graph answers `false` after one scan.
        let mut clauses: Vec<&str> = Vec::with_capacity(detector_prefixes.len());
        let mut params: Vec<rusqlite::types::Value> =
            Vec::with_capacity(detector_prefixes.len() * 2);
        for prefix in detector_prefixes {
            clauses.push("target = ? OR target GLOB ?");
            params.push(rusqlite::types::Value::Text(prefix.clone()));
            params.push(rusqlite::types::Value::Text(format!("{prefix}::*")));
        }
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM unresolved_refs WHERE resolved = 0 AND ({}))",
            clauses.join(" OR ")
        );
        let has_ref: bool = self
            .conn
            .query_row(&sql, rusqlite::params_from_iter(params), |row| {
                row.get::<_, i64>(0)
            })
            .context("checking for framework-detector references")?
            != 0;
        Ok(has_ref)
    }

    fn test_node_ids(&self) -> Result<Vec<NodeId>> {
        // ORDER BY id keeps the consumer (require_tested BFS) deterministic
        // (NFR-RA-06). Reading the persisted verdict — not re-deriving it —
        // is what guarantees reachability ≡ the annotation (CR-001 CRA-01).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM nodes WHERE is_test = 1 ORDER BY id")?;
        let ids = stmt
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting test node ids")?;
        Ok(ids)
    }

    fn function_metrics(&self) -> Result<Vec<FunctionMetricRow>> {
        // ORDER BY id keeps the metric reduction canonical (ADR-08, NFR-RA-06).
        // The CR-005 dimensions add line_count (Conciseness), max_nesting_depth
        // (Nesting/Conciseness), and clone_group (Uniqueness) to the slice.
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, cyclomatic_complexity, is_dead, is_duplicate, \
                    line_count, max_nesting_depth, clone_group \
             FROM nodes WHERE kind IN (?1, ?2) AND derived = 0 \
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map(
                [NodeKind::Function.as_i32(), NodeKind::Method.as_i32()],
                |row| {
                    Ok(FunctionMetricRow {
                        id: NodeId(row.get(0)?),
                        cyclomatic_complexity: row.get(1)?,
                        is_dead: row.get::<_, Option<i64>>(2)?.map(|v| v != 0),
                        is_duplicate: row.get::<_, Option<i64>>(3)?.map(|v| v != 0),
                        line_count: row.get(4)?,
                        max_nesting_depth: row.get(5)?,
                        clone_group: row.get::<_, Option<i64>>(6)?.map(NodeId),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting function rows for the metrics engine")?;
        Ok(rows)
    }

    fn metric_snapshots(&self) -> Result<Vec<MetricSnapshotRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, commit_sha, node_count, edge_count, function_count, \
                    test_function_count, metric_version, empty, \
                    modularity_raw, modularity_normalized, \
                    acyclicity_raw, acyclicity_normalized, \
                    depth_raw, depth_normalized, \
                    equality_raw, equality_normalized, \
                    redundancy_raw, redundancy_normalized, \
                    thresholds_hash, aggregate_signal \
             FROM metric_snapshots ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(MetricSnapshotRow {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    commit_sha: row.get(2)?,
                    node_count: row.get(3)?,
                    edge_count: row.get(4)?,
                    function_count: row.get(5)?,
                    test_function_count: row.get(6)?,
                    metric_version: row.get(7)?,
                    empty: row.get::<_, i64>(8)? != 0,
                    modularity_raw: row.get(9)?,
                    modularity_normalized: row.get(10)?,
                    acyclicity_raw: row.get(11)?,
                    acyclicity_normalized: row.get(12)?,
                    depth_raw: row.get(13)?,
                    depth_normalized: row.get(14)?,
                    equality_raw: row.get(15)?,
                    equality_normalized: row.get(16)?,
                    redundancy_raw: row.get(17)?,
                    redundancy_normalized: row.get(18)?,
                    thresholds_hash: row.get(19)?,
                    aggregate_signal: row.get(20)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting metric snapshots")?;
        Ok(rows)
    }

    fn baseline_snapshot(&self, scope: &str) -> Result<Option<MetricSnapshotRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.created_at, m.commit_sha, m.node_count, m.edge_count, \
                    m.function_count, m.test_function_count, m.metric_version, m.empty, \
                    m.modularity_raw, m.modularity_normalized, \
                    m.acyclicity_raw, m.acyclicity_normalized, \
                    m.depth_raw, m.depth_normalized, \
                    m.equality_raw, m.equality_normalized, \
                    m.redundancy_raw, m.redundancy_normalized, \
                    m.thresholds_hash, m.aggregate_signal \
             FROM baseline b \
             JOIN metric_snapshots m ON m.id = b.snapshot_id \
             WHERE b.scope = ?1",
        )?;
        stmt.query_row([scope], |row| {
            Ok(MetricSnapshotRow {
                id: row.get(0)?,
                created_at: row.get(1)?,
                commit_sha: row.get(2)?,
                node_count: row.get(3)?,
                edge_count: row.get(4)?,
                function_count: row.get(5)?,
                test_function_count: row.get(6)?,
                metric_version: row.get(7)?,
                empty: row.get::<_, i64>(8)? != 0,
                modularity_raw: row.get(9)?,
                modularity_normalized: row.get(10)?,
                acyclicity_raw: row.get(11)?,
                acyclicity_normalized: row.get(12)?,
                depth_raw: row.get(13)?,
                depth_normalized: row.get(14)?,
                equality_raw: row.get(15)?,
                equality_normalized: row.get(16)?,
                redundancy_raw: row.get(17)?,
                redundancy_normalized: row.get(18)?,
                thresholds_hash: row.get(19)?,
                aggregate_signal: row.get(20)?,
            })
        })
        .optional()
        .context("querying the gate baseline")
    }

    fn latest_metric_snapshot(&self) -> Result<Option<LatestMetricSnapshot>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT node_count, edge_count, function_count, test_function_count, empty, \
                    thresholds_hash, aggregate_signal, \
                    modularity_raw, modularity_normalized, \
                    acyclicity_raw, acyclicity_normalized, \
                    depth_raw, depth_normalized, \
                    equality_raw, equality_normalized, \
                    redundancy_raw, redundancy_normalized, \
                    nesting_raw, nesting_normalized, \
                    conciseness_raw, conciseness_normalized, \
                    cohesion_raw, cohesion_normalized, cohesion_applicable, \
                    focus_raw, focus_normalized, focus_applicable, \
                    uniqueness_raw, uniqueness_normalized \
             FROM metric_snapshots ORDER BY id DESC LIMIT 1",
        )?;
        stmt.query_row([], |row| {
            // `*_applicable` is stored as INTEGER 0/1/NULL → Option<bool>.
            let opt_bool = |idx: usize| -> rusqlite::Result<Option<bool>> {
                Ok(row.get::<_, Option<i64>>(idx)?.map(|n| n != 0))
            };
            Ok(LatestMetricSnapshot {
                node_count: row.get(0)?,
                edge_count: row.get(1)?,
                function_count: row.get(2)?,
                test_function_count: row.get(3)?,
                empty: row.get::<_, i64>(4)? != 0,
                thresholds_hash: row.get(5)?,
                aggregate_signal: row.get(6)?,
                modularity_raw: row.get(7)?,
                modularity_normalized: row.get(8)?,
                acyclicity_raw: row.get(9)?,
                acyclicity_normalized: row.get(10)?,
                depth_raw: row.get(11)?,
                depth_normalized: row.get(12)?,
                equality_raw: row.get(13)?,
                equality_normalized: row.get(14)?,
                redundancy_raw: row.get(15)?,
                redundancy_normalized: row.get(16)?,
                nesting_raw: row.get(17)?,
                nesting_normalized: row.get(18)?,
                conciseness_raw: row.get(19)?,
                conciseness_normalized: row.get(20)?,
                cohesion_raw: row.get(21)?,
                cohesion_normalized: row.get(22)?,
                cohesion_applicable: opt_bool(23)?,
                focus_raw: row.get(24)?,
                focus_normalized: row.get(25)?,
                focus_applicable: opt_bool(26)?,
                uniqueness_raw: row.get(27)?,
                uniqueness_normalized: row.get(28)?,
            })
        })
        .optional()
        .context("querying the latest metric snapshot")
    }

    fn rules_cache(&self) -> Result<Option<RulesCacheRow>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT rules_hash, parsed_json FROM rules_cache WHERE id = 1")?;
        stmt.query_row([], |row| {
            Ok(RulesCacheRow {
                rules_hash: row.get(0)?,
                parsed_json: row.get(1)?,
            })
        })
        .optional()
        .context("querying the rules cache")
    }

    fn function_constraint_rows(&self) -> Result<Vec<FunctionConstraintRow>> {
        // ORDER BY id keeps violation order deterministic (NFR-RA-06).
        let mut stmt = self.conn.prepare_cached(
            "SELECT n.id, n.name, f.path, n.start_line, \
                    n.cyclomatic_complexity, n.line_count \
             FROM nodes n \
             LEFT JOIN files f ON f.id = n.file_id \
             WHERE n.kind IN (?1, ?2) AND n.derived = 0 \
             ORDER BY n.id",
        )?;
        let rows = stmt
            .query_map(
                [NodeKind::Function.as_i32(), NodeKind::Method.as_i32()],
                |row| {
                    Ok(FunctionConstraintRow {
                        id: NodeId(row.get(0)?),
                        name: row.get(1)?,
                        file_path: row.get(2)?,
                        start_line: row.get(3)?,
                        cyclomatic_complexity: row.get(4)?,
                        line_count: row.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting function rows for the rules evaluator")?;
        Ok(rows)
    }

    fn violations(&self) -> Result<Vec<ViolationRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, snapshot_id, rule_type, rule_key, node_id, file, message, severity \
             FROM violations ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ViolationRow {
                    id: row.get(0)?,
                    snapshot_id: row.get(1)?,
                    rule_type: row.get(2)?,
                    rule_key: row.get(3)?,
                    node_id: row.get(4)?,
                    file: row.get(5)?,
                    message: row.get(6)?,
                    severity: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting persisted violations")?;
        Ok(rows)
    }

    fn store_health(&self) -> Result<StoreHealth> {
        Ok(StoreHealth {
            schema_version: self.schema_version()?,
        })
    }

    fn structural_check(&self) -> Result<StructuralReport> {
        structural_report(&self.conn)
    }
}

/// Which side of a `calls` edge to pivot on for an adjacency query.
///
/// A closed enum (not a `&str`) so the SQL is selected from two fixed `const`
/// statements — the type system makes string interpolation of an endpoint
/// impossible by construction, keeping the injection boundary structural rather
/// than conventional ([NFR-SE-02]).
#[derive(Debug, Clone, Copy)]
enum Direction {
    /// Inbound `calls` edges: match on `target`, project the `source` node.
    Callers,
    /// Outbound `calls` edges: match on `source`, project the `target` node.
    Callees,
}

/// `callers` of a node: pivot on `e.target`, return the `e.source` node.
const SQL_CALLERS: &str =
    "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.source \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.target = ?1 AND e.kind = ?2 \
     ORDER BY n.id";

/// `callees` of a node: pivot on `e.source`, return the `e.target` node.
const SQL_CALLEES: &str =
    "SELECT n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.target \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.source = ?1 AND e.kind = ?2 \
     ORDER BY n.id";

/// `neighbours_in`: every inbound edge of any kind, projecting the source node
/// plus the edge kind (first column).
const SQL_NEIGHBOURS_IN: &str =
    "SELECT e.kind, n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.source \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.target = ?1 \
     ORDER BY e.kind, n.id";

/// `neighbours_out`: every outbound edge of any kind, projecting the target
/// node plus the edge kind (first column).
const SQL_NEIGHBOURS_OUT: &str =
    "SELECT e.kind, n.id, s.symbol, n.kind, n.name, f.path, n.start_line, n.end_line \
     FROM edges e \
     JOIN nodes n ON n.id = e.target \
     JOIN symbols s ON s.id = n.symbol_id \
     LEFT JOIN files f ON f.id = n.file_id \
     WHERE e.source = ?1 \
     ORDER BY e.kind, n.id";

impl SqliteGraphStore {
    /// Shared body of [`GraphStore::callers`] / [`GraphStore::callees`].
    ///
    /// Selects one of two fixed `const` statements by [`Direction`]; `id` and
    /// the `calls` discriminant are bound parameters. No SQL is built at
    /// runtime — there is no interpolation surface.
    fn adjacent(&self, id: NodeId, direction: Direction) -> Result<Vec<NodeRow>> {
        let sql = match direction {
            Direction::Callers => SQL_CALLERS,
            Direction::Callees => SQL_CALLEES,
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let raws = stmt
            .query_map(
                rusqlite::params![id.get(), EdgeKind::Calls.as_i32()],
                map_raw_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting adjacency results")?;
        raws.into_iter().map(raw_to_node).collect()
    }

    /// Shared body of [`GraphStore::neighbours_in`] /
    /// [`GraphStore::neighbours_out`]: one of two fixed `const` statements,
    /// `id` bound — same no-interpolation posture as [`adjacent`](Self::adjacent).
    fn neighbours(&self, id: NodeId, sql: &'static str) -> Result<Vec<(EdgeKind, NodeRow)>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let raws = stmt
            .query_map([id.get()], |row| {
                let edge_kind: i32 = row.get(0)?;
                Ok((
                    edge_kind,
                    (
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ),
                ))
            })?
            .collect::<rusqlite::Result<Vec<(i32, RawNodeRow)>>>()
            .context("collecting neighbour edges")?;
        raws.into_iter()
            .map(|(edge_kind, raw)| {
                let kind = EdgeKind::try_from(edge_kind).map_err(|e| {
                    anyhow!("corrupt edge kind {edge_kind} on a neighbour edge: {e}; rebuild advised (NFR-RA-08)")
                })?;
                Ok((kind, raw_to_node(raw)?))
            })
            .collect()
    }
}

/// The first identifier-ish token of a free-text query — the FTS5 prefix-match
/// seed for [`GraphStore::suggest`]. `None` when the text holds no
/// alphanumeric/underscore run.
fn first_identifier_token(text: &str) -> Option<&str> {
    let start = text.find(|c: char| c.is_alphanumeric() || c == '_')?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// A writer scoped to an open transaction (see [`SqliteGraphStore::write_batch`]).
///
/// Exposes the same insert primitives as the store, but every call participates
/// in the surrounding transaction, so the whole batch commits or rolls back as
/// a unit ([NFR-RA-07]).
pub struct BatchWriter<'a> {
    conn: &'a Connection,
}

impl BatchWriter<'_> {
    /// Read an integer `PRAGMA` value from the **live writer connection** this
    /// batch runs on ([FR-DB-02]).
    ///
    /// The writer's RW connection lives inside the single-writer actor thread
    /// ([ADR-02]) and is never handed out, so this is the only seam that can
    /// inspect it directly — the CR-057 bulk-load-pragma verification submits a
    /// no-op write batch and reads `cache_size` / `mmap_size` / `temp_store`
    /// through here. Intended for **value-read** pragmas only: `name` must be a
    /// trusted literal in query form (`PRAGMA name`, no `= value`) — it is
    /// interpolated into SQL (pragma names cannot be bound as parameters), and
    /// this method never appends a value, so it does not itself write. `pub(crate)`
    /// because it is a verification seam, not part of the public write API.
    ///
    /// # Errors
    /// Returns an error if the pragma name is unknown or the read fails.
    ///
    /// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    #[cfg(test)]
    pub(crate) fn pragma_i64(&self, name: &str) -> Result<i64> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .with_context(|| format!("reading PRAGMA {name} from the writer connection"))
    }

    /// Insert a file row within the batch; see [`SqliteGraphStore::insert_file`].
    ///
    /// # Errors
    /// Returns an error on a constraint violation or I/O failure.
    pub fn insert_file(
        &self,
        path: &str,
        language: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<i64> {
        insert_file(self.conn, path, language, content_hash)
    }

    /// Insert the symbol if new within the batch; see
    /// [`SqliteGraphStore::upsert_symbol`].
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn upsert_symbol(&self, symbol: &LogosSymbol) -> Result<i64> {
        upsert_symbol(self.conn, symbol)
    }

    /// Insert or upsert a node within the batch; see
    /// [`SqliteGraphStore::insert_node`].
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    pub fn insert_node(&self, node: &NewNode<'_>) -> Result<NodeId> {
        insert_node(self.conn, node)
    }

    /// Insert an edge within the batch; see [`SqliteGraphStore::insert_edge`].
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    pub fn insert_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
        insert_edge(self.conn, source, target, kind)
    }

    /// Persist a function node's winnowed near-clone shingle set (CR-005,
    /// [FR-EX-09]) into the inverted `shingles` index. Idempotent over the
    /// `(node_id, hash)` primary key, so a re-emitted hash is a no-op; an empty
    /// set (a body below the token floor) writes nothing.
    ///
    /// The `u64` fingerprints are bit-cast to the equivalent signed `i64` for
    /// SQLite's INTEGER column — a total, lossless round-trip
    /// ([`i64::from_ne_bytes`]/[`u64::to_ne_bytes`] would also serve; the cast is
    /// the same reinterpretation) read back by the clustering pass with the
    /// inverse cast (S-043).
    ///
    /// # Errors
    /// Returns an error if a constraint fires or I/O fails.
    ///
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    pub fn insert_shingles(&self, node_id: NodeId, hashes: &[u64]) -> Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO shingles (node_id, hash) VALUES (?1, ?2) \
             ON CONFLICT(node_id, hash) DO NOTHING",
        )?;
        for &hash in hashes {
            stmt.execute(rusqlite::params![node_id.get(), hash as i64])
                .context("inserting shingle fingerprint")?;
        }
        Ok(())
    }

    // ── Incremental-sync primitives ([ADR-10]) ───────────────────────────────

    /// The id of the file row for `path`, or `None` if the file is not indexed.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn file_id(&self, path: &str) -> Result<Option<i64>> {
        self.conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .optional()
            .context("looking up file id by path")
    }

    /// Update an existing file row's `language`/`content_hash` in place, keeping
    /// its id stable so the row's identity survives a re-extract.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn update_file(
        &self,
        file_id: i64,
        language: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE files SET language = ?2, content_hash = ?3 WHERE id = ?1",
                rusqlite::params![file_id, language, content_hash],
            )
            .context("updating file row")?;
        Ok(())
    }

    /// Upsert a durable `project_metadata` key/value pair (CR-004, [ADR-20], the
    /// migration-15 kv table).
    ///
    /// Used to record [`CONFIG_FINGERPRINT_KEY`] after a config-narrowing purge,
    /// so the purge runs at most once per config change ([FR-SY-07]). The
    /// `ON CONFLICT … DO UPDATE` makes a re-record idempotent — the key is the
    /// primary key, so each project carries one row per fact.
    ///
    /// [ADR-20]: ../../../docs/specs/architecture/decisions/ADR-20.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn set_project_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO project_metadata (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )
            .context("recording project_metadata value")?;
        Ok(())
    }

    /// Advance the persisted monotonic graph revision by one and return the new
    /// value (CR-027, [ADR-32], [FR-SY-09]).
    ///
    /// Read-modify-write **within the surrounding transaction**: the current
    /// revision is read from `project_metadata` and re-written incremented in the
    /// same batch ([NFR-RA-07]). Because every write rides the single-writer
    /// actor ([NFR-RA-10]), no concurrent writer can interleave between the read
    /// and the upsert, so the counter is strictly monotonic without a separate
    /// lock. A never-written revision reads as `0`, so the first advance returns
    /// `1`; an unparseable stored value is likewise treated as `0` (the read
    /// counterpart in [`GraphStore::graph_revision`] makes the same choice).
    ///
    /// Persisting the revision is the single-row write [NFR-PE-03] budgets for on
    /// the existing post-`sync` write path. The caller advances it once per
    /// completed `index` and once per `sync` that mutated the graph (the
    /// pipeline gates the call) — never on a no-op `sync` or a read-only read.
    ///
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    /// [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
    ///
    /// # Errors
    /// Returns an error on I/O failure reading or writing the revision row.
    pub fn advance_graph_revision(&self) -> Result<u64> {
        let current: u64 = self
            .conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [GRAPH_REVISION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("reading current graph revision")?
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(0);
        // saturating_add keeps the type total: a u64 counter advanced once per
        // pipeline run will never realistically overflow, but saturating beats a
        // debug panic and stays clippy-clean.
        let next = current.saturating_add(1);
        // Reuse the shared upsert rather than repeating its SQL: both run on the
        // same transaction connection, so atomicity is unchanged (the single-writer
        // actor forbids any interleaving between the read above and this write), and
        // the revision can never drift from how every other metadata key is written.
        self.set_project_metadata(GRAPH_REVISION_KEY, &next.to_string())?;
        Ok(next)
    }

    /// Delete every node bound to `file_id`, returning the count removed.
    ///
    /// Each deleted node cascades to its incident edges (both `source` and
    /// `target` are `ON DELETE CASCADE`) and fires the FTS `'delete'` trigger.
    /// This is the destructive half of an incremental re-extract — call
    /// [`inbound_cross_file_edges`](Self::inbound_cross_file_edges) **first** to
    /// preserve links pointing into this file ([ADR-10]).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_nodes_for_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM nodes WHERE file_id = ?1", [file_id])
            .context("deleting nodes for file")
    }

    /// Delete a file row by id, returning the count removed (0 or 1).
    ///
    /// Any nodes still bound to it have their `file_id` set to `NULL`
    /// (`ON DELETE SET NULL`); a re-extract deletes those nodes first via
    /// [`delete_nodes_for_file`](Self::delete_nodes_for_file), so this is used
    /// for true removals (a file deleted from disk).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM files WHERE id = ?1", [file_id])
            .context("deleting file row")
    }

    /// Capture the cross-file edges that point **into** `file_id` — edges whose
    /// target node lives in this file but whose source node does not ([ADR-10]).
    ///
    /// Endpoints are returned as symbol strings so the edges can be re-attached
    /// after the file is re-extracted (the target node's rowid changes, but its
    /// canonical symbol does not). Intra-file edges (source also in this file)
    /// are excluded — extraction re-emits those. A source node with a `NULL`
    /// `file_id` is treated as cross-file and therefore captured.
    ///
    /// **Derived edges are never captured** (`derived = 0` filter): the
    /// annotation pass owns and recomputes them every run (S-014, [FR-AN-03]),
    /// so capturing one would launder it into the ledger as a permanently
    /// retried, resolution-rebound `derived = 0` edge that `clear_derived`
    /// could never own — a governance flag outliving the rule that produced it.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn inbound_cross_file_edges(&self, file_id: i64) -> Result<Vec<CapturedEdge>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ss.symbol, ts.symbol, e.kind \
             FROM edges e \
             JOIN nodes sn ON sn.id = e.source \
             JOIN nodes tn ON tn.id = e.target \
             JOIN symbols ss ON ss.id = sn.symbol_id \
             JOIN symbols ts ON ts.id = tn.symbol_id \
             WHERE tn.file_id = ?1 AND (sn.file_id IS NULL OR sn.file_id <> ?1) \
               AND e.derived = 0",
        )?;
        let rows = stmt
            .query_map([file_id], |row| {
                Ok(CapturedEdge {
                    source_symbol: row.get(0)?,
                    target_symbol: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("capturing inbound cross-file edges")?;
        Ok(rows)
    }

    // ── Reference-ledger primitives (S-011, [ADR-10]) ────────────────────────

    /// Insert a reference-ledger row, idempotently over the
    /// `(source_symbol, target, form, kind)` uniqueness rule.
    ///
    /// A duplicate (the same function calling the same path twice, or a
    /// captured edge re-captured on a later sync) is a no-op — the first row's
    /// `line` is kept.
    ///
    /// # Errors
    /// Returns an error if a CHECK/FK constraint fires or I/O fails.
    pub fn insert_unresolved_ref(&self, r: &NewUnresolvedRef<'_>) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO unresolved_refs \
                 (file_id, source_symbol, target, alias, form, kind, line, payload) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(source_symbol, target, form, kind) DO NOTHING",
                rusqlite::params![
                    r.file_id,
                    r.source_symbol,
                    r.target,
                    r.alias,
                    r.form.as_i32(),
                    r.kind.as_i32(),
                    r.line,
                    r.payload,
                ],
            )
            .context("inserting unresolved ref")?;
        Ok(())
    }

    /// Delete every reference-ledger row produced by `file_id`, returning the
    /// count removed.
    ///
    /// The replace-wholesale half of a re-extract: stale refs from the file's
    /// previous version never linger, fresh extraction re-inserts the current
    /// set.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_unresolved_refs_for_file(&self, file_id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM unresolved_refs WHERE file_id = ?1", [file_id])
            .context("deleting unresolved refs for file")
    }

    /// Record a reference-ledger row's binding state after a resolution run.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn mark_ref_resolved(&self, ref_id: i64, resolved: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE unresolved_refs SET resolved = ?2 WHERE id = ?1",
                rusqlite::params![ref_id, i64::from(resolved)],
            )
            .context("updating ref resolved state")?;
        Ok(())
    }

    /// Insert an edge unless `(source, target, kind)` already exists; returns
    /// `true` when a row was actually inserted.
    ///
    /// The resolution pass uses this (rather than [`insert_edge`]) because two
    /// distinct ledger rows can legitimately bind to the same edge — e.g. a
    /// captured exact-symbol ref and the re-extracted textual ref for the same
    /// call ([ADR-10]).
    ///
    /// [`insert_edge`]: Self::insert_edge
    /// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    pub fn insert_edge_if_absent(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<bool> {
        self.insert_edge_with_payload_if_absent(source, target, kind, None)
    }

    /// Insert an edge with a relation `payload` unless `(source, target, kind)`
    /// already exists; returns `true` when a row was actually inserted.
    ///
    /// The payload variant of [`insert_edge_if_absent`](Self::insert_edge_if_absent):
    /// the resolution pass writes the cross-artifact relation class
    /// ([`ArtifactRelation`](crate::model::ArtifactRelation) wire token) onto an
    /// `ArtifactRef`/`ArtifactBinding` edge so navigation can surface *which*
    /// relation a binding expresses (CR-011, [FR-CG-07], [FR-CG-11]); a code/doc/
    /// access binding passes `None`. `payload` is **not** part of the UNIQUE key,
    /// so a re-resolved edge keeps its first-written payload — idempotent like the
    /// plain insert.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    pub fn insert_edge_with_payload_if_absent(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        payload: Option<&str>,
    ) -> Result<bool> {
        let inserted = self
            .conn
            .execute(
                "INSERT INTO edges (source, target, kind, payload) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(source, target, kind) DO NOTHING",
                rusqlite::params![source.get(), target.get(), kind.as_i32(), payload],
            )
            .context("inserting edge with payload if absent")?;
        Ok(inserted > 0)
    }

    // ── Annotation primitives (S-014, [FR-AN-01..04]) ────────────────────────

    /// Clear every derived policy node and derived edge, returning
    /// `(nodes_removed, edges_removed)`.
    ///
    /// The idempotency primitive of the annotation pass ([FR-AN-03]): each run
    /// starts from a clean slate and re-materialises the policy graph, so a
    /// re-run never accumulates stale `forbidden_dependency` flags or layer
    /// nodes. Derived nodes cascade their incident edges and fire the FTS
    /// `'delete'` trigger like any other node delete.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn clear_derived(&self) -> Result<(usize, usize)> {
        // Edges first: a standalone derived edge between two extracted nodes
        // (the forbidden_dependency flag) would not be cascaded by the node
        // delete below.
        let edges = self
            .conn
            .execute("DELETE FROM edges WHERE derived = 1", [])
            .context("clearing derived edges")?;
        let nodes = self
            .conn
            .execute("DELETE FROM nodes WHERE derived = 1", [])
            .context("clearing derived nodes")?;
        Ok((nodes, edges))
    }

    /// Write one node's Pass-3 annotation results to its native columns
    /// ([FR-AN-04]): `is_dead`, `is_duplicate`, `is_test`, `layer_membership`,
    /// and the near-clone `clone_group` ([FR-AN-06], CR-005).
    ///
    /// `None` records the honest "not applicable / not computed" `NULL`,
    /// distinct from `false`, for the tri-state verdicts. `is_test` is a
    /// definite boolean ([FR-AN-05]) — positive-evidence classification, so it
    /// is always `0` or `1`, never `NULL`. `clone_group` is the stable clone-group
    /// identifier (the minimum node id of the function's near-clone component) or
    /// `None` when the function is in no near-clone group ([FR-AN-06]); writing it
    /// for every annotated node — `None` included — clears stale membership from a
    /// prior run, keeping the pass idempotent ([NFR-RA-06]).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn set_node_annotations(
        &self,
        id: NodeId,
        is_dead: Option<bool>,
        is_duplicate: Option<bool>,
        is_test: bool,
        layer_membership: Option<&str>,
        clone_group: Option<NodeId>,
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE nodes SET is_dead = ?2, is_duplicate = ?3, is_test = ?4, \
                              layer_membership = ?5, clone_group = ?6 \
             WHERE id = ?1",
        )?;
        stmt.execute(rusqlite::params![
            id.get(),
            is_dead.map(i64::from),
            is_duplicate.map(i64::from),
            i64::from(is_test),
            layer_membership,
            clone_group.map(NodeId::get),
        ])
        .context("writing node annotations")?;
        Ok(())
    }

    /// Insert a **derived** edge (`derived = 1`) unless `(source, target,
    /// kind)` already exists; returns `true` when a row was inserted.
    ///
    /// The materialisation primitive for `forbidden_dependency` flags
    /// ([FR-AN-03]). Conflict-tolerant because two boundaries can legitimately
    /// condemn the same underlying dependency. The conflict arm **upgrades**
    /// `derived` to `1` rather than doing nothing — defence in depth: should a
    /// same-`(source, target, kind)` row ever exist un-flagged (e.g. a
    /// historical capture from before
    /// [`inbound_cross_file_edges`](Self::inbound_cross_file_edges) excluded
    /// derived edges), the flag is repaired so `clear_derived` owns it again.
    ///
    /// # Errors
    /// Returns an error if an endpoint node is missing or I/O fails.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn insert_derived_edge(
        &self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<bool> {
        // `WHERE edges.derived = 0` keeps the duplicate-materialisation case a
        // true no-op (affected-rows 0) while still repairing a stranded flag.
        let inserted = self
            .conn
            .execute(
                "INSERT INTO edges (source, target, kind, derived) VALUES (?1, ?2, ?3, 1) \
                 ON CONFLICT(source, target, kind) DO UPDATE SET derived = 1 \
                 WHERE edges.derived = 0",
                rusqlite::params![source.get(), target.get(), kind.as_i32()],
            )
            .context("inserting derived edge")?;
        Ok(inserted > 0)
    }

    /// Record a file's `[[layers]]` band ([FR-AN-03]); `None` clears it (the
    /// file matches no layer glob under the current `rules.toml`).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
    pub fn set_file_layer(&self, file_id: i64, layer: Option<&str>) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached("UPDATE files SET layer = ?2 WHERE id = ?1")?;
        stmt.execute(rusqlite::params![file_id, layer])
            .context("writing file layer")?;
        Ok(())
    }

    /// Resolve a symbol string to the id of the node that carries it, or `None`.
    ///
    /// Used to rebind a captured cross-file edge: the source endpoint persists
    /// across the sync, and the target endpoint is looked up by its (stable)
    /// symbol after re-extraction.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn node_id_for_symbol(&self, symbol: &str) -> Result<Option<NodeId>> {
        self.conn
            .query_row(
                // `ORDER BY n.id` makes the pick deterministic if a symbol ever maps
                // to more than one node (symbols are unique per node by construction
                // today, ADR-07, but the ordering removes any reliance on that).
                "SELECT n.id FROM nodes n \
                 JOIN symbols s ON s.id = n.symbol_id \
                 WHERE s.symbol = ?1 \
                 ORDER BY n.id \
                 LIMIT 1",
                [symbol],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .context("resolving node id by symbol")
            .map(|opt| opt.map(NodeId))
    }

    // ── Framework-promotion primitives (S-012, [FR-FW-01]..[FR-FW-04]) ───────

    /// Delete a node by id within the batch (cascading to its incident edges
    /// and firing the FTS `'delete'` trigger); see
    /// [`SqliteGraphStore::delete_node`].
    ///
    /// The framework pass uses this to retire a promoted `route`/`component`
    /// node whose source pattern no longer exists (the reconcile sweep).
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_node(&self, id: NodeId) -> Result<usize> {
        delete_node(self.conn, id)
    }

    /// Delete one `(source, target, kind)` edge, returning the count removed
    /// (0 or 1).
    ///
    /// The framework pass uses this to retire a stale edge incident to a
    /// promoted node (e.g. a route whose handler wiring changed) without
    /// churning the node ids themselves.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    pub fn delete_edge(&self, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM edges WHERE source = ?1 AND target = ?2 AND kind = ?3",
                rusqlite::params![source.get(), target.get(), kind.as_i32()],
            )
            .context("deleting edge")
    }

    // ── Metric-snapshot primitives (S-018, [FR-QM-07]) ───────────────────────

    /// Append one `metric_snapshots` row, returning its id.
    ///
    /// The single write the metrics engine performs ([FR-QM-07]). The table is
    /// append-only by contract — past snapshots are never mutated, so this is
    /// deliberately the only snapshot primitive.
    ///
    /// # Errors
    /// Returns an error if a CHECK constraint fires or I/O fails.
    ///
    /// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
    pub fn insert_metric_snapshot(&self, snapshot: &NewMetricSnapshot<'_>) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO metric_snapshots \
                 (created_at, commit_sha, node_count, edge_count, function_count, \
                  test_function_count, metric_version, empty, \
                  modularity_raw, modularity_normalized, \
                  acyclicity_raw, acyclicity_normalized, \
                  depth_raw, depth_normalized, \
                  equality_raw, equality_normalized, \
                  redundancy_raw, redundancy_normalized, \
                  nesting_raw, nesting_normalized, \
                  conciseness_raw, conciseness_normalized, \
                  cohesion_raw, cohesion_normalized, cohesion_applicable, \
                  focus_raw, focus_normalized, focus_applicable, \
                  uniqueness_raw, uniqueness_normalized, \
                  thresholds_hash, aggregate_signal) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, \
                         ?30, ?31, ?32)",
                rusqlite::params![
                    snapshot.created_at,
                    snapshot.commit_sha,
                    snapshot.node_count,
                    snapshot.edge_count,
                    snapshot.function_count,
                    snapshot.test_function_count,
                    snapshot.metric_version,
                    i64::from(snapshot.empty),
                    snapshot.modularity_raw,
                    snapshot.modularity_normalized,
                    snapshot.acyclicity_raw,
                    snapshot.acyclicity_normalized,
                    snapshot.depth_raw,
                    snapshot.depth_normalized,
                    snapshot.equality_raw,
                    snapshot.equality_normalized,
                    snapshot.redundancy_raw,
                    snapshot.redundancy_normalized,
                    snapshot.nesting_raw,
                    snapshot.nesting_normalized,
                    snapshot.conciseness_raw,
                    snapshot.conciseness_normalized,
                    snapshot.cohesion_raw,
                    snapshot.cohesion_normalized,
                    snapshot.cohesion_applicable.map(i64::from),
                    snapshot.focus_raw,
                    snapshot.focus_normalized,
                    snapshot.focus_applicable.map(i64::from),
                    snapshot.uniqueness_raw,
                    snapshot.uniqueness_normalized,
                    snapshot.thresholds_hash,
                    snapshot.aggregate_signal,
                ],
            )
            .context("inserting metric snapshot")?;
        Ok(self.conn.last_insert_rowid())
    }

    // ── Governance primitives (S-020, [FR-GV-01..05]) ────────────────────────

    /// Upsert the gate baseline for `scope` ([FR-GV-04]): re-running
    /// `session_start` / `gate --save` overwrites the previous anchor.
    ///
    /// # Errors
    /// Returns an error if `snapshot_id` does not exist or I/O fails.
    ///
    /// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
    pub fn upsert_baseline(&self, scope: &str, snapshot_id: i64, created_at: i64) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO baseline (scope, snapshot_id, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(scope) DO UPDATE SET snapshot_id = ?2, created_at = ?3",
                rusqlite::params![scope, snapshot_id, created_at],
            )
            .context("upserting the gate baseline")?;
        Ok(())
    }

    /// Replace the persisted violation set wholesale ([FR-GV-02], SRS §5.1
    /// "written per check_rules run") — the same clear-then-rematerialise
    /// idempotence as the derived policy graph (BR-12).
    ///
    /// # Errors
    /// Returns an error on a constraint violation or I/O failure.
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    pub fn replace_violations(&self, violations: &[NewViolation<'_>]) -> Result<()> {
        self.conn
            .execute("DELETE FROM violations", [])
            .context("clearing the previous violation set")?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO violations \
             (snapshot_id, rule_type, rule_key, node_id, file, message, severity, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for v in violations {
            stmt.execute(rusqlite::params![
                v.snapshot_id,
                v.rule_type,
                v.rule_key,
                v.node_id,
                v.file,
                v.message,
                v.severity,
                v.created_at,
            ])
            .context("inserting a violation row")?;
        }
        Ok(())
    }

    /// Run the FTS5 external-content integrity check on the writer
    /// connection (S-020 `health`): the `'integrity-check'` special command
    /// is an INSERT, so the read-only pool cannot issue it. A desync is the
    /// [NFR-RA-09] Correctness fault the `health` tool exists to surface.
    ///
    /// # Errors
    /// Returns an error when the FTS index disagrees with `nodes`.
    ///
    /// [NFR-RA-09]: ../../../docs/specs/requirements/NFR-RA-09.md
    pub fn fts_integrity_check(&self) -> Result<()> {
        self.conn
            .execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('integrity-check');")
            .context("FTS5 integrity check failed — index desynced (NFR-RA-09)")
    }

    /// Write the singleton `rules_cache` row ([FR-GV-01]): called only when
    /// the `rules.toml` content hash changed.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    ///
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    pub fn set_rules_cache(
        &self,
        rules_hash: &str,
        parsed_json: &str,
        updated_at: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO rules_cache (id, rules_hash, parsed_json, updated_at) \
                 VALUES (1, ?1, ?2, ?3) \
                 ON CONFLICT(id) DO UPDATE SET rules_hash = ?1, parsed_json = ?2, updated_at = ?3",
                rusqlite::params![rules_hash, parsed_json, updated_at],
            )
            .context("writing the rules cache")?;
        Ok(())
    }
}

// ── Row-writer primitives (work on any &Connection: a store conn or a tx) ─────

fn insert_file(
    conn: &Connection,
    path: &str,
    language: Option<&str>,
    content_hash: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO files (path, language, content_hash) VALUES (?1, ?2, ?3)",
        rusqlite::params![path, language, content_hash],
    )
    .context("inserting file")?;
    Ok(conn.last_insert_rowid())
}

fn upsert_symbol(conn: &Connection, symbol: &LogosSymbol) -> Result<i64> {
    // Idempotent: the UNIQUE(symbol) index makes the second insert a no-op, then
    // we read the (existing or freshly inserted) id back.
    conn.execute(
        "INSERT INTO symbols (symbol) VALUES (?1) ON CONFLICT(symbol) DO NOTHING",
        [symbol.as_str()],
    )
    .context("upserting symbol")?;
    conn.query_row(
        "SELECT id FROM symbols WHERE symbol = ?1",
        [symbol.as_str()],
        |row| row.get(0),
    )
    .context("reading symbol id")
}

/// Idempotent over `symbol_id` ([FR-SY-10], [NFR-RA-13], [ADR-46]): the
/// `UNIQUE(symbol_id)` constraint (S-201) makes a second node for an
/// already-present symbol a conflict rather than a duplicate row, and the
/// `DO UPDATE` folds the fresh extraction fields into the existing row in
/// place. The per-file delete-then-insert ([FR-SY-01]) is the primary
/// replace-in-place mechanism for a re-extracted file; this upsert is the
/// backstop that keeps `insert_node` itself safe to call again for a symbol
/// that was never deleted (e.g. two nodes folding to the same symbol within
/// one persist, or a caller that reuses an existing symbol without a prior
/// delete) — re-extraction never accumulates a second node.
///
/// `last_insert_rowid()` only reflects the most recent INSERT, not an UPDATE,
/// so it cannot be trusted on the `DO UPDATE` path (SQLite docs). The id is
/// instead read back explicitly via `symbol_id`, mirroring the
/// [`upsert_symbol`] idiom.
///
/// The `SET` list covers exactly [`NewNode`]'s extraction-owned columns.
/// Annotation-owned columns (`is_dead`, `is_duplicate`, `layer_membership`,
/// `is_test`, `clone_group`) are deliberately absent, so a re-extraction never
/// wipes a prior annotation pass's verdict out from under it.
///
/// [FR-SY-01]: ../../../docs/specs/requirements/FR-SY-01.md
/// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
fn insert_node(conn: &Connection, node: &NewNode<'_>) -> Result<NodeId> {
    conn.execute(
        "INSERT INTO nodes (symbol_id, kind, name, file_id, start_line, end_line, \
                            derived, exported, cyclomatic_complexity, line_count, fingerprint, \
                            test_evidence, body, max_nesting_depth) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT(symbol_id) DO UPDATE SET \
             kind = excluded.kind, \
             name = excluded.name, \
             file_id = excluded.file_id, \
             start_line = excluded.start_line, \
             end_line = excluded.end_line, \
             derived = excluded.derived, \
             exported = excluded.exported, \
             cyclomatic_complexity = excluded.cyclomatic_complexity, \
             line_count = excluded.line_count, \
             fingerprint = excluded.fingerprint, \
             test_evidence = excluded.test_evidence, \
             body = excluded.body, \
             max_nesting_depth = excluded.max_nesting_depth",
        rusqlite::params![
            node.symbol_id,
            node.kind.as_i32(),
            node.name,
            node.file_id,
            node.start_line,
            node.end_line,
            i64::from(node.derived),
            i64::from(node.exported),
            node.cyclomatic_complexity,
            node.line_count,
            node.fingerprint,
            i64::from(node.test_evidence),
            node.body,
            node.max_nesting_depth,
        ],
    )
    .context("upserting node")?;
    conn.query_row(
        "SELECT id FROM nodes WHERE symbol_id = ?1",
        [node.symbol_id],
        |row| row.get(0),
    )
    .map(NodeId)
    .context("reading node id back by symbol_id")
}

fn insert_edge(conn: &Connection, source: NodeId, target: NodeId, kind: EdgeKind) -> Result<()> {
    conn.execute(
        "INSERT INTO edges (source, target, kind) VALUES (?1, ?2, ?3)",
        rusqlite::params![source.get(), target.get(), kind.as_i32()],
    )
    .context("inserting edge")?;
    Ok(())
}

fn delete_node(conn: &Connection, id: NodeId) -> Result<usize> {
    conn.execute("DELETE FROM nodes WHERE id = ?1", [id.get()])
        .context("deleting node")
}

/// The raw column tuple read from a node-shaped row, before model conversion.
type RawNodeRow = (
    i64,
    String,
    i32,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// Map a SQL row to the raw column tuple. Total over the SQL types, so it stays
/// inside `rusqlite::Result`; the fallible model conversion happens in
/// [`raw_to_node`].
fn map_raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawNodeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

/// Convert a raw row to a [`NodeRow`], validating the stored symbol and kind.
///
/// A discriminant or symbol that fails to map back is an integrity error — the
/// store this binary wrote should never contain one — surfaced with guidance to
/// rebuild ([NFR-RA-08]).
fn raw_to_node(
    (id, symbol, kind, name, file_path, start_line, end_line): RawNodeRow,
) -> Result<NodeRow> {
    let symbol = LogosSymbol::parse(&symbol).with_context(|| {
        format!("corrupt symbol stored for node {id}; rebuild advised (NFR-RA-08)")
    })?;
    let kind = NodeKind::try_from(kind).map_err(|e| {
        anyhow!("corrupt node kind {kind} for node {id}: {e}; rebuild advised (NFR-RA-08)")
    })?;
    Ok(NodeRow {
        id: NodeId(id),
        symbol,
        kind,
        name,
        file_path,
        start_line,
        end_line,
    })
}

/// Writer page cache: `-65536` = 64 MiB (negative = KiB, SQLite convention).
/// Larger than the 2 MiB default so the insert-heavy Pass-1 persist and the
/// whole-graph reads across passes 2–5 spill to disk far less; bounded well
/// under the ≤1 GB indexing ceiling ([NFR-PE-06]) — one writer connection, so
/// this 64 MiB is paid once, not per reader (CR-057).
const WRITER_CACHE_SIZE_KIB: i64 = -65_536;

/// Memory-mapped I/O window for the writer: 256 MiB. Serves whole-graph page
/// reads/inserts straight from the mapped file instead of `read`/`write`
/// syscalls + buffer copies, cutting page churn ([FR-DB-02]). The mapping is
/// file-backed and reclaimable (it does not pin 256 MiB of anonymous RSS), and
/// the on-disk store stays far below this window within the ~100k-LOC envelope,
/// so it holds under [NFR-PE-06]. Inert on `:memory:` stores (no file to map).
const WRITER_MMAP_SIZE_BYTES: i64 = 268_435_456;

/// `temp_store = MEMORY` (2): SQLite keeps transient b-trees (index builds,
/// sorters) in RAM rather than spilling temp files during the bulk load.
const TEMP_STORE_MEMORY: i64 = 2;

/// Apply the per-connection pragma contract ([FR-DB-02], [NFR-RA-10]).
///
/// `execute_batch` runs the base pragmas and discards the row `journal_mode`
/// returns. `foreign_keys` must be set here, on every connection, before any
/// transaction — it defaults OFF (the [ADR-05] footgun).
///
/// This is the **writer** connection's configuration path ([`open`](SqliteGraphStore::open)
/// / [`open_in_memory`](SqliteGraphStore::open_in_memory) → [`from_connection`](SqliteGraphStore::from_connection);
/// readers use [`configure_readonly_connection`]). The writer additionally sets
/// the bulk-load pragmas ([FR-DB-02], CR-057) — a larger page cache, a
/// memory-mapped I/O window, and in-memory temp storage — to cut page churn on
/// the cold-index write path and the whole-graph reads, bounded by the ≤1 GB
/// indexing RSS ceiling ([NFR-PE-06]). They are graph-output-neutral
/// ([NFR-RA-06]): pragmas change how bytes move to/from disk, never what is
/// stored or read back.
fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;",
    )
    .context("applying the per-connection pragma contract (FR-DB-02)")?;
    // Bulk-load pragmas on the writer connection (FR-DB-02, CR-057), each
    // bounded under NFR-PE-06 (see the constants' docs). Applied via
    // `pragma_update` (not `execute_batch`) so a bad value surfaces as an error
    // rather than a silently-ignored statement.
    conn.pragma_update(None, "cache_size", WRITER_CACHE_SIZE_KIB)
        .context("setting the writer cache_size bulk-load pragma (FR-DB-02)")?;
    conn.pragma_update(None, "mmap_size", WRITER_MMAP_SIZE_BYTES)
        .context("setting the writer mmap_size bulk-load pragma (FR-DB-02)")?;
    conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
        .context("setting the writer temp_store bulk-load pragma (FR-DB-02)")?;
    // Writers serialize via this timeout, then fail gracefully rather than
    // blocking readers (NFR-RA-10).
    conn.busy_timeout(Duration::from_secs(5))
        .context("setting busy_timeout")?;
    Ok(())
}

/// Apply the **read-only** per-connection contract for a WAL reader ([ADR-02]).
///
/// Deliberately omits `PRAGMA journal_mode = WAL`: switching journal mode takes
/// a write lock a read-only connection does not hold, and the file is already in
/// WAL (the writer set it on first open). `foreign_keys`/`synchronous` are
/// connection-level settings that touch no data, and `query_only = ON` makes the
/// read-only contract structural — any stray write on a pool connection fails at
/// the SQL layer rather than silently contending with the writer.
///
/// The bulk-load pragmas (CR-057) are **not** applied here: [FR-DB-02] scopes
/// them to the writer connection, and the [S-225] cold-index benchmark showed a
/// read-side memory-map window yields no measurable pass-2–5 read-time reduction
/// — those reads hit just-persisted, OS-page-cache-hot pages and are dominated
/// by CPU-side row deserialization, which `mmap` does not touch. Keeping the
/// readers on the base contract holds the pool strictly within [NFR-PE-06] (no
/// per-reader heap cache) and keeps the code aligned with the requirement's
/// writer-only wording.
///
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
/// [FR-DB-02]: ../../../docs/specs/requirements/FR-DB-02.md
/// [NFR-PE-06]: ../../../docs/specs/requirements/NFR-PE-06.md
/// [S-225]: ../../../docs/planning/journal.md#s-225-per-phase-index-instrumentation-and-repeatable-cold-index-benchmark
fn configure_readonly_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\n\
         PRAGMA synchronous = NORMAL;\n\
         PRAGMA query_only = ON;",
    )
    .context("applying the read-only connection contract (FR-DB-02)")?;
    // Match the writer's grace window so a checkpoint-induced brief contention
    // resolves rather than erroring instantly (NFR-RA-10).
    conn.busy_timeout(Duration::from_secs(5))
        .context("setting busy_timeout")?;
    Ok(())
}

// ── Worktree seeding (S-021, ADR-15) ────────────────────────────────────────

/// Copy a primary checkout's canonical store to `dst_db` as a worktree seed —
/// the graph-store "seed/copy" library seam consumed by the api-facade
/// ([ADR-15], [FR-WT-03]).
///
/// Copies the single-file DB (the [NFR-DM-03] copyability contract) **plus**
/// its `-wal` sidecar when present, so commits a live primary has not yet
/// checkpointed replay when the copy first opens. The `-shm` index is never
/// copied (SQLite rebuilds it), and nothing else in the source `.logos/`
/// travels — derived sibling state like `telemetry.db` stays put
/// ([NFR-DM-04]).
///
/// Best-effort against a live writer: the db+wal pair is copied at one moment;
/// a checkpoint racing the copy can at worst lose the very latest commits,
/// which the caller's diff-reconcile / reconcile backstop recovers ([ADR-11]).
///
/// # Errors
/// Returns an error if the destination directory cannot be created or either
/// file copy fails. On error the destination db+wal pair is removed — a failed
/// seed leaves NO partial artifacts a later fresh store could mispair with.
/// The caller treats a failed seed as "no seed" and falls back to a fresh
/// store + full index.
///
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-15]: ../../../docs/specs/architecture/decisions/ADR-15.md
/// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
/// [NFR-DM-03]: ../../../docs/specs/requirements/NFR-DM-03.md
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
pub fn seed_copy(src_db: &Path, dst_db: &Path) -> Result<()> {
    let result = try_seed_copy(src_db, dst_db);
    if result.is_err() {
        // The pair is written together and cleaned together: stranding a
        // partial `logos.db` OR an orphan `-wal` would let a later fresh
        // store pair with foreign state.
        let _ = std::fs::remove_file(dst_db);
        let _ = std::fs::remove_file(wal_sidecar(dst_db));
    }
    result
}

/// Fallible body of [`seed_copy`]; the wrapper owns failure cleanup.
fn try_seed_copy(src_db: &Path, dst_db: &Path) -> Result<()> {
    if let Some(parent) = dst_db.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating the seed destination {}", parent.display()))?;
    }
    std::fs::copy(src_db, dst_db).with_context(|| {
        format!(
            "seeding {} from the primary store {}",
            dst_db.display(),
            src_db.display()
        )
    })?;

    let src_wal = wal_sidecar(src_db);
    let dst_wal = wal_sidecar(dst_db);
    if src_wal.is_file() {
        std::fs::copy(&src_wal, &dst_wal)
            .with_context(|| format!("copying the WAL sidecar {}", src_wal.display()))?;
    } else {
        // Never pair the fresh copy with a stale leftover WAL.
        let _ = std::fs::remove_file(&dst_wal);
    }
    Ok(())
}

/// The `-wal` sidecar path for a database file (`logos.db` → `logos.db-wal`).
fn wal_sidecar(db: &Path) -> std::path::PathBuf {
    let mut os = db.as_os_str().to_os_string();
    os.push("-wal");
    std::path::PathBuf::from(os)
}

#[cfg(test)]
mod tests;
