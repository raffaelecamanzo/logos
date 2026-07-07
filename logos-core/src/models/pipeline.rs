//! Pipeline read-models — results from index and sync operations.
//!
//! Used by the two pipeline Engine methods (ADR-01): `index`, `sync`.

use std::collections::BTreeMap;

use serde::Serialize;

/// Bound/unresolved counts for one cross-artifact relation class (CR-011,
/// [FR-CG-11], [FR-RS-04]).
///
/// The honest per-relation breakdown of the artifact wiring: how many references
/// of a given relation (`proto-import`, `route`, `type-name`, …) are bound to an
/// edge versus still sitting in the ledger. Never a floor, only a measurement
/// ([NFR-CC-04]).
///
/// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RelationCoverage {
    /// References of this relation class currently bound to an edge.
    pub bound: u64,
    /// References of this relation class persisted for retry — never fabricated
    /// ([NFR-RA-05]).
    pub unresolved: u64,
}

/// Resolution-pass coverage/confidence read-model (S-011, FR-RS-04).
///
/// The honest-signal carrier (NFR-RA-11, NFR-CC-04): wherever resolution data
/// is consumed, the bound-ratio and the surviving unresolved count ride along
/// — heuristic binding is never presented as ground truth.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ResolutionStats {
    /// Every reference in the ledger (calls, method calls, imports, captures).
    pub refs_total: u64,
    /// References currently bound to an edge.
    pub refs_resolved: u64,
    /// References that could not be bound — persisted and retried each sync
    /// (FR-RS-03, NFR-RA-05).
    pub refs_unresolved: u64,
    /// Edges actually inserted by this run (`0` for a pure coverage read).
    pub edges_created: u64,
    /// The bound-ratio `refs_resolved / refs_total` (1.0 for an empty ledger).
    pub coverage: f64,
    /// Per-relation-class bound/unresolved counts for the cross-artifact
    /// references (CR-011, [FR-CG-11], [FR-RS-04]), keyed by the relation's
    /// payload token. Empty for a repository with no artifact wiring; a
    /// `BTreeMap` so the surface is deterministic across runs ([NFR-RA-06]).
    ///
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    /// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub by_relation: BTreeMap<String, RelationCoverage>,
}

/// Framework-promotion read-model (S-012, FR-FW-01..04).
///
/// `duration_ms` is the honest cost signal for the ≤30s index budget
/// (NFR-PE-02): the wall-clock the framework pass added to this run, surfaced
/// so the OQ-07 budget question is answered with measured data, not guesses.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct FrameworkStats {
    /// Files whose reference ledger names a known framework crate — the only
    /// files the pass ever parses (FR-FW-04: a plain library scans zero).
    pub files_scanned: u64,
    /// `route` nodes currently promoted (the post-reconcile total).
    pub routes: u64,
    /// `component` nodes currently promoted (the post-reconcile total).
    pub components: u64,
    /// Wall-clock cost of the whole pass for this run (OQ-07 evidence).
    pub duration_ms: u64,
}

/// Framework-dispatch live-rooting read-model (CR-043, ADR-39, Pass 2¾).
///
/// The honest cost + coverage signal for the dispatch pass that live-roots
/// framework-dispatched methods (trait-impl dispatch, `#[tool]` tool dispatch)
/// so the dead-code pass no longer mis-reports them dead ([FR-RS-03],
/// [FR-AN-01]). `duration_ms` keeps the ≤30s budget question answered with
/// measured wall-clock ([NFR-PE-02]) exactly as [`FrameworkStats`] does.
///
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct DispatchStats {
    /// `.rs` files (re)scanned this run — every Rust file on a full index, only
    /// the changed Rust files on an incremental sync.
    pub files_scanned: u64,
    /// Framework-dispatched methods recognised in the scanned files (the
    /// post-reconcile total of live-root markers in those files).
    pub entries: u64,
    /// Live-root marker edges inserted this run.
    pub markers_added: u64,
    /// Live-root marker edges retired this run (a method that is no longer a
    /// dispatch entry).
    pub markers_removed: u64,
    /// Wall-clock cost of the whole pass for this run (OQ-07 evidence).
    pub duration_ms: u64,
}

/// Annotation-pass (Pass 3) read-model (S-014, FR-AN-01..05) — the
/// dead/duplicate/test counts the annotation engine surfaces per run (the
/// component's observability contract; telemetry persistence lands in S-019).
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct AnnotationStats {
    /// Nodes whose annotation columns were (re-)written this run.
    pub nodes_annotated: u64,
    /// Function/method nodes flagged `is_dead = true` (FR-AN-01).
    pub dead: u64,
    /// Function/method nodes flagged `is_duplicate = true` (FR-AN-02).
    pub duplicates: u64,
    /// Function/method nodes that belong to a near-clone group — those with a
    /// non-NULL `clone_group` (FR-AN-06, CR-005). Distinct from `duplicates`:
    /// near-clone membership (Jaccard shingle similarity) never touches the
    /// exact-AST-shape duplicate verdict.
    pub clones: u64,
    /// Distinct near-clone groups formed this run — connected components of the
    /// clone-pair graph (FR-AN-06).
    pub clone_groups: u64,
    /// Nodes classified `is_test = true` — the unified test annotation
    /// (FR-AN-05, CR-001), the single detection source for `test_gaps`,
    /// dead-code roots, and the metrics scope filter.
    pub tests: u64,
    /// `layer` policy nodes materialised from `rules.toml` (FR-AN-03).
    pub layer_nodes: u64,
    /// `boundary` policy nodes materialised from `rules.toml` (FR-AN-03).
    pub boundary_nodes: u64,
    /// Derived `forbidden_dependency` edges flagged this run (FR-AN-03).
    pub forbidden_edges: u64,
}

/// Per-phase wall-clock breakdown of a full `index`, in milliseconds
/// ([FR-OB-06], [CR-057]).
///
/// The eight phases the [pipeline-orchestrator] runs in sequence
/// (`discover → load → extract → persist → resolve → framework → dispatch →
/// annotate`), each timed exactly once through the single `tracing` emission
/// seam ([FR-OB-01], [NFR-OO-01]) — not a parallel bespoke timing path. Their
/// sum reconciles with [`IndexResult::duration_ms`] within measurement noise
/// (the small unattributed remainder is the reconcile/bookkeeping between
/// passes: purge, fingerprint, revision advance). This is the evidence that
/// attributes cold-index time against the ≤30 s budget ([NFR-PE-02]) and the
/// gate every [CR-057](../../../docs/requests/CR-057-indexing-performance-optimization.md)
/// optimization quotes a before/after from.
///
/// [FR-OB-06]: ../../../docs/specs/requirements/FR-OB-06.md
/// [FR-OB-01]: ../../../docs/specs/requirements/FR-OB-01.md
/// [NFR-OO-01]: ../../../docs/specs/requirements/NFR-OO-01.md
/// [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
/// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PhaseDurations {
    /// Discovery walk — the gitignore-aware candidate enumeration ([FR-IX-02]).
    pub discover_ms: u64,
    /// File load — read + blake3 hash of every candidate.
    pub load_ms: u64,
    /// Pass 1 extraction — parallel tree-sitter parse on the worker pool
    /// ([FR-IX-03]).
    pub extract_ms: u64,
    /// Pass 1 persistence — the write-batch loop through the single writer
    /// ([ADR-02], [NFR-RA-07]).
    pub persist_ms: u64,
    /// Pass 2 resolution — binding the reference ledger ([FR-RS-03]); includes
    /// the focused re-resolve for newly-promoted route/component nodes (CR-017).
    pub resolve_ms: u64,
    /// Framework promotion pass ([FR-FW-01]..04); mirrors
    /// [`FrameworkStats::duration_ms`].
    pub framework_ms: u64,
    /// Framework-dispatch live-rooting pass (CR-043, [ADR-39]); mirrors
    /// [`DispatchStats::duration_ms`].
    pub dispatch_ms: u64,
    /// Pass 3 annotation — dead-code/duplicate/clone/layer verdicts
    /// ([FR-AN-01]..06).
    pub annotate_ms: u64,
}

/// Result of a full index run (FR-IX-01..06).
#[derive(Debug, Default, Serialize)]
pub struct IndexResult {
    pub files_indexed: u64,
    pub nodes_created: u64,
    pub edges_created: u64,
    /// Pass-2 coverage/confidence (S-011, FR-RS-04).
    pub resolution: ResolutionStats,
    /// Framework route/component promotion (S-012, FR-FW-01..04).
    pub framework: FrameworkStats,
    /// Framework-dispatch live-rooting (CR-043, ADR-39, FR-RS-03).
    pub dispatch: DispatchStats,
    /// Pass-3 annotation counts (S-014, FR-AN-01..04).
    pub annotation: AnnotationStats,
    pub duration_ms: u64,
    /// Per-phase wall-clock breakdown ([FR-OB-06], CR-057) — the eight timed
    /// pipeline phases whose sum reconciles with `duration_ms` within
    /// measurement noise. Sourced from the single `tracing` seam ([FR-OB-01]).
    ///
    /// [FR-OB-06]: ../../../docs/specs/requirements/FR-OB-06.md
    /// [FR-OB-01]: ../../../docs/specs/requirements/FR-OB-01.md
    pub phases: PhaseDurations,
    pub warnings: Vec<String>,
    /// Files that could not be read/extracted this run (unreadable or
    /// non-UTF-8). The per-file failure set the governance engine stamps an
    /// `INCOMPLETE` freshness line from (NFR-RA-11, ADR-11, S-020); each is
    /// also described in `warnings`.
    pub files_failed: Vec<String>,
}

/// Result of an incremental sync run (FR-SY-01..06).
#[derive(Debug, Default, Serialize)]
pub struct SyncResult {
    pub files_added: u64,
    pub files_modified: u64,
    pub files_removed: u64,
    /// Pass-2 coverage/confidence after the retry sweep (S-011, FR-RS-03/04).
    pub resolution: ResolutionStats,
    /// Framework route/component promotion after the reconcile (S-012).
    pub framework: FrameworkStats,
    /// Framework-dispatch live-rooting after the reconcile (CR-043, ADR-39).
    pub dispatch: DispatchStats,
    /// Pass-3 annotation counts after the re-annotate (S-014, FR-AN-01..04).
    pub annotation: AnnotationStats,
    pub duration_ms: u64,
    pub warnings: Vec<String>,
    /// Files that could not be read/extracted this run — the `INCOMPLETE`
    /// input (NFR-RA-11, S-020); each is also described in `warnings`.
    pub files_failed: Vec<String>,
}

/// Action taken on one `init`-managed target (S-023, FR-IN-01..04).
///
/// The wire vocabulary of the DL-07 posture: `init` is idempotent and
/// non-clobbering, so every step reports *exactly* what it did — including
/// the deliberate refusals (`Skipped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InitAction {
    /// The target did not exist and was generated.
    Created,
    /// Only the managed portion was regenerated; user content outside the
    /// markers (or outside the managed key) was preserved.
    Updated,
    /// Already in the desired state — nothing was written.
    Unchanged,
    /// Deliberately not touched (non-clobber, DL-07); `detail` says why.
    Skipped,
}

/// One step of the `init` bootstrap (S-023, FR-IN-01..04).
#[derive(Debug, Clone, Serialize)]
pub struct InitStep {
    /// Root-relative target this step manages (e.g. `.logos/config.toml`).
    pub target: String,
    /// What happened to the target.
    pub action: InitAction,
    /// Human-readable note (skip reason); empty when self-evident.
    pub detail: String,
}

/// Result of the `init` bootstrapping command.
#[derive(Debug, Default, Serialize)]
pub struct InitResult {
    pub logos_dir: String,
    pub db_path: String,
    pub message: String,
    /// Per-target outcomes for every step this invocation ran (S-023,
    /// FR-IN-01..04).
    pub steps: Vec<InitStep>,
}
