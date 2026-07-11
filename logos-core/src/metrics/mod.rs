//! The quality metrics engine ([metrics-engine component], S-018/S-044,
//! [FR-QM-01]..[FR-QM-14], [ADR-08], [ADR-12], [ADR-21]).
//!
//! Computes **ten** quality dimensions over a hydrated dependency view and the
//! production-scope node/edge snapshot, and combines them into the deterministic
//! 0–10000 integer signal. The five original macro-structural metrics:
//!
//! 1. **Modularity** ([FR-QM-01]) — Newman's Q under the **directory
//!    partition**: every vertex belongs to the directory of its defining
//!    file, so intra-directory dependency edges count as community-internal
//!    (the rolled-up "module self-loops" the SRS §7.4 trap warns about — drop
//!    them and Q ≈ 0 always). Normalized `(Q+0.5)/1.5` clamped to [0,1];
//!    `m == 0` → `1/3`.
//! 2. **Acyclicity** ([FR-QM-02], [ADR-30]) — count of `tarjan_scc`
//!    components with `len > 1` (multi-node mutual-recursion cycles only; a
//!    singleton self-loop / self-recursion is **not** counted — metric-
//!    semantics v4, [CR-022]); normalized `1/(1+cycles)`. **The same SCC set
//!    feeds Depth's condensation** so the gate and the `max_cycles` rule can
//!    never disagree.
//! 3. **Depth** ([FR-QM-03]) — longest path (vertex count) over the SCC
//!    condensation: a whole cycle collapses to one layer, so a pure tangle
//!    scores depth 1. Normalized `1/(1+depth/8)`.
//! 4. **Equality** ([FR-QM-04]) — `1 − Gini` of per-function cyclomatic
//!    complexity; `n==0`, `n==1`, or `Σx==0` → 1.0.
//! 5. **Redundancy** ([FR-QM-05]) — `1 − redundant/total` where a function is
//!    redundant if dead **or** duplicate (counted once even when both).
//!
//! …and the five CR-005 micro-structural dimensions, each floored at 0.01 and
//! computed over the production scope (see [`extended`]):
//!
//! 6. **Nesting** ([FR-QM-09]) — `1 − deep-nesting ratio`.
//! 7. **Conciseness** ([FR-QM-10]) — `1 − brain-method ratio`.
//! 8. **Cohesion** ([FR-QM-11]) — mean of `1/LCOM4` over classes; **n/a
//!    drop-out** when no class exists.
//! 9. **Focus** ([FR-QM-12]) — `1 − god-container ratio`; **n/a drop-out** when
//!    no class-like container exists.
//! 10. **Uniqueness** ([FR-QM-13]) — `1 − near-clone ratio`.
//!
//! # Aggregation (metric-semantics v4, [FR-QM-06], [FR-QM-14], [ADR-12], [ADR-21])
//!
//! `signal = exp((Σ ln nᵢ)/k) · 10000` over the **applicable** dimensions in
//! **canonical order** (the five original metrics, then nesting, conciseness,
//! cohesion, focus, uniqueness), rounded to an integer ([ADR-08]). `k` is the
//! count of applicable dimensions (10, or 9/8 when Cohesion/Focus drop out).
//! Three guards:
//!
//! - **Zero short-circuit (original five only)** — if any *original* `nᵢ == 0.0`
//!   the signal is `0` *before* any `ln` runs (a hard systemic pathology
//!   collapses the score; anti-gaming, [ADR-21]). The new five are floored, never
//!   `0`, so they drag but never alone collapse the signal.
//! - **Applicability drop-out** — Cohesion/Focus with no construct store NULL +
//!   a `false` flag and drop out of the denominator ([ADR-21]); a class-less repo
//!   gets a deterministic 9-dimension mean ([UAT-QM-10]).
//! - **Empty-graph sentinel** — `node_count == 0` stores `empty = 1`,
//!   `aggregate_signal = NULL`, and surfaces as `"n/a"` rather than the
//!   misleading ~8033 a naive mean of the guard values would produce
//!   ([NFR-CC-04]). Unchanged by CR-005.
//!
//! Every snapshot additionally persists the **effective-thresholds hash**
//! ([FR-QM-14], [BR-25]): a tuning change to the detection thresholds is visible
//! and triggers the announced gate auto-re-baseline ([FR-GV-10]).
//!
//! # Determinism ([ADR-08], [NFR-RA-06], [AA-03])
//!
//! Every order-sensitive reduction runs in a fixed order: vertices and edges
//! arrive in the hydrated view's deterministic index order, community sums
//! iterate a `BTreeMap`, complexity values are sorted ascending, and the
//! log-space sum walks the canonical metric order. Equality, Redundancy,
//! Acyclicity, and Depth accumulate in **exact integer arithmetic** with one
//! trailing f64 division, so only Modularity's community sum and the final
//! `ln`/`exp` are float reductions at all — and the rounded integer absorbs
//! sub-unit residue. Cross-target byte-identity of the stored signal is the
//! [AA-03] assumption; its proof lands with the S-025 CI matrix.
//!
//! # Inputs
//!
//! [`compute`] is **pure** — it takes the hydrated [`GraphView`] (the
//! `ExcludeContains` dependency view, [FR-DB-06]) the original five run on, the
//! node snapshot the view was built from (for the directory partition and the
//! class-like containers Cohesion/Focus read), the **whole** edge set (the
//! `Contains`/`Accesses`/`Calls` edges Cohesion/Focus need, absent from the
//! dependency view), the per-function metric rows, the `is_test` set, and the
//! effective [`Thresholds`]. Derived governance artifacts are excluded up front:
//! `Layer`/`Boundary` policy vertices and `ForbiddenDependency` edges are flags
//! the annotation pass re-materialises each run, not dependencies — the same
//! derived-filter posture the annotation engine itself takes.
//!
//! [`snapshot`] orchestrates a full run: one reader-pool read, the pure
//! compute, and one append-only `metric_snapshots` write ([FR-QM-07]) —
//! mirroring the snapshot → compute → commit shape of the resolution and
//! annotation passes.
//!
//! [metrics-engine component]: ../../../docs/specs/architecture/components/metrics-engine.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
//! [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
//! [ADR-30]: ../../../docs/specs/architecture/decisions/ADR-30.md
//! [CR-022]: ../../../docs/requests/CR-022-acyclicity-self-recursion-exclusion.md
//! [AA-03]: ../../../docs/specs/architecture.md#24-assumptions
//! [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
//! [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
//! [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
//! [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
//! [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
//! [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
//! [FR-QM-06]: ../../../docs/specs/requirements/FR-QM-06.md
//! [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
//! [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
//! [FR-QM-10]: ../../../docs/specs/requirements/FR-QM-10.md
//! [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
//! [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
//! [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
//! [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
//! [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
//! [UAT-QM-10]: ../../../docs/specs/requirements/UAT-QM-10.md
//! [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::graph_store::{EdgeRow, FunctionMetricRow, NewMetricSnapshot, NodeRow};
use crate::hydrate::GraphView;
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::quality::{MetricSnapshot, MetricValue, Offender, WorstOffenders};
use crate::runtime::Runtime;

mod extended;
use extended::ContainerIndex;
pub use extended::{GodContainer, Thresholds};

/// The per-dimension worst-offender list cap ([`worst_offenders`], CR-005
/// review-phase visibility): the top-N offenders kept per dimension, capping the
/// `scan` report surface ([NFR-RA-06] determinism, bounded output).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub const WORST_OFFENDER_CAP: usize = 10;

#[cfg(test)]
mod tests;

/// The directory community of a vertex bound to no file ([FR-QM-01]'s
/// partition needs *every* vertex in exactly one community).
///
/// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
const UNBOUND_DIR: &str = "<unbound>";

/// The five original metrics whose hard `0` short-circuits the signal ([ADR-12]
/// zero short-circuit, scoped to the original five by [ADR-21]).
///
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
const ORIGINAL_METRIC_COUNT: usize = 5;

/// The metrics-semantics version stamped on every snapshot ([FR-GV-10]).
///
/// Bumped whenever a change alters *what the signal measures* (not how it is
/// computed), invalidating stored baselines. A baseline recorded under a
/// different version is incomparable: the gate auto-re-baselines against the
/// fresh snapshot and passes informationally instead of failing against an
/// incomparable anchor ([FR-GV-10], [UAT-GV-06]).
///
/// - **v1** — the original test-inclusive scope (S-018): every function/method
///   entered the metric numerators, denominators, and Depth's condensation.
/// - **v2** — the production scope ([FR-QM-08], [CR-001], [ADR-18]): `is_test`
///   functions are excluded. Pre-upgrade snapshots read as v1 (the migration-7
///   `metric_version DEFAULT 1`), so the first post-upgrade gate re-baselines.
/// - **v3** — the extended ten-dimension signal ([CR-005], [FR-QM-14],
///   [ADR-21]): Nesting, Conciseness, Cohesion, Focus, and Uniqueness join the
///   aggregate as the applicable-dimension geometric mean (floors on the new
///   five, the zero short-circuit kept on the original five, applicability
///   drop-out for Cohesion/Focus). A v2 baseline is incomparable to a v3 run, so
///   the first post-upgrade gate auto-re-baselines ([FR-GV-10]) — exactly as the
///   v1→v2 bump did. A `rules.toml` threshold edit is *also* visible and gated,
///   through the effective-thresholds hash ([FR-QM-14], [BR-25]), wired in
///   [S-045].
/// - **v4** — Acyclicity excludes self-recursion ([CR-022], [ADR-30], S-090):
///   only `tarjan_scc` components with `len > 1` (mutual recursion between
///   distinct units) count as cycles; a singleton self-loop now contributes
///   zero. The `max_cycles` rule and the DSM read the same narrowed
///   `acyclicity.raw`, so a v3 baseline is incomparable and the first
///   post-upgrade gate auto-re-baselines ([FR-GV-10]) — exactly as the prior
///   semantics bumps did.
///
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [UAT-GV-06]: ../../../docs/specs/requirements/UAT-GV-06.md
/// [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
/// [CR-005]: ../../../docs/requests/CR-005-extended-structural-metrics.md
/// [CR-022]: ../../../docs/requests/CR-022-acyclicity-self-recursion-exclusion.md
/// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
/// [ADR-30]: ../../../docs/specs/architecture/decisions/ADR-30.md
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
/// [S-045]: ../../../docs/planning/journal.md#s-045-metric-thresholds-budgets-and-worst-offender-reporting
pub const METRIC_SEMANTICS_VERSION: i64 = 4;

/// Compute the five metrics and the aggregate signal over the **production
/// scope** of a hydrated dependency view — pure, no I/O ([FR-QM-01]..[FR-QM-06],
/// [FR-QM-08]).
///
/// `view` should be the `ExcludeContains` dependency view ([FR-DB-06]);
/// `nodes` is the node snapshot the view was hydrated from (supplies the
/// directory partition); `functions` is the per-function metric slice
/// ([`GraphStore::function_metrics`](crate::graph_store::GraphStore::function_metrics));
/// `test_ids` is the persisted `is_test` node set
/// ([`GraphStore::test_node_ids`](crate::graph_store::GraphStore::test_node_ids)) —
/// the single source of truth, never re-derived here ([FR-AN-05], [CR-001]).
///
/// Per [FR-QM-08]/[BR-18], `is_test` nodes are excluded from **every** metric:
/// dropped from the metric graph (so Modularity, Acyclicity, and Depth's
/// condensation see only the production subgraph) and filtered out of the
/// function slice (so Equality's Gini and Redundancy's ratio count production
/// numerators and denominators only). The count of excluded test functions is
/// reported as `test_function_count` ([FR-QM-07], [NFR-CC-04]). There is no
/// configuration to re-include tests ([BR-18]).
///
/// Same inputs always produce the same output, bit for bit — all reductions
/// run in canonical order ([ADR-08], [NFR-RA-06]).
///
/// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
/// [FR-QM-06]: ../../../docs/specs/requirements/FR-QM-06.md
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
pub fn compute(
    view: &GraphView,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
    test_ids: &HashSet<NodeId>,
    thresholds: Thresholds,
) -> MetricSnapshot {
    // Production scope (FR-QM-08): the metric graph drops is_test vertices and
    // their incident edges (Modularity/Acyclicity/Depth see only production),
    // exactly as it already drops derived Layer/Boundary policy vertices.
    let (graph, dirs) = metric_graph(view, nodes, test_ids);

    // Equality and Redundancy count production functions only — test rows are
    // excluded from both numerators and denominators (FR-QM-08, BR-18). The
    // CR-005 ratio dimensions (Nesting/Conciseness/Uniqueness) share this scope.
    let production: Vec<&FunctionMetricRow> = functions
        .iter()
        .filter(|f| !test_ids.contains(&f.id))
        .collect();
    let test_function_count = (functions.len() - production.len()) as u64;

    // Canonical metric order (ADR-08): modularity, acyclicity, depth,
    // equality, redundancy.
    let modularity = modularity(&graph, &dirs);
    // One SCC run feeds both Acyclicity and Depth (FR-QM-02: gate and rule —
    // and the two metrics — agree on the same cycle set).
    let sccs = tarjan_scc(&graph);
    let acyclicity = acyclicity(&sccs);
    let depth = depth(&graph, &sccs);
    let equality = equality(&production);
    let redundancy = redundancy(&production);

    // The five CR-005 structural dimensions (FR-QM-09..13), in canonical order.
    // Cohesion/Focus read class-like containers from the raw node/edge snapshot
    // (Contains/Accesses/Calls are absent from the ExcludeContains view); both
    // can drop out (None) when their construct is absent (ADR-21).
    let nesting = extended::nesting(&production, &thresholds);
    let conciseness = extended::conciseness(&production, &thresholds);
    let uniqueness = extended::uniqueness(&production);
    let containers = ContainerIndex::build(nodes, edges, test_ids);
    let cohesion = containers.cohesion();
    let focus = containers.focus(&thresholds);

    let empty = graph.node_count() == 0;
    let aggregate_signal = if empty {
        // The ADR-12 empty-graph sentinel: never a misleading number. Unchanged
        // by CR-005 — an empty production graph is still "n/a".
        None
    } else {
        Some(aggregate(
            [
                modularity.normalized,
                acyclicity.normalized,
                depth.normalized,
                equality.normalized,
                redundancy.normalized,
            ],
            &applicable_new_dimensions(&nesting, &conciseness, &cohesion, &focus, &uniqueness),
        ))
    };

    MetricSnapshot {
        modularity,
        acyclicity,
        depth,
        equality,
        redundancy,
        nesting,
        conciseness,
        cohesion,
        focus,
        uniqueness,
        thresholds_hash: thresholds.hash(),
        node_count: graph.node_count() as u64,
        edge_count: graph.edge_count() as u64,
        function_count: production.len() as u64,
        test_function_count,
        empty,
        aggregate_signal,
    }
}

/// The normalized values of the **applicable** new dimensions, in canonical
/// order (nesting, conciseness, cohesion, focus, uniqueness) — Cohesion and
/// Focus contribute only when applicable, so a class-less repo yields four (or
/// fewer) entries and the aggregate denominator shrinks accordingly ([FR-QM-14],
/// [ADR-21] applicability drop-out).
///
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
fn applicable_new_dimensions(
    nesting: &MetricValue,
    conciseness: &MetricValue,
    cohesion: &Option<MetricValue>,
    focus: &Option<MetricValue>,
    uniqueness: &MetricValue,
) -> Vec<f64> {
    let mut dims = vec![nesting.normalized, conciseness.normalized];
    if let Some(c) = cohesion {
        dims.push(c.normalized);
    }
    if let Some(f) = focus {
        dims.push(f.normalized);
    }
    dims.push(uniqueness.normalized);
    dims
}

/// Run a full metrics pass: snapshot the store, [`compute`], and append one
/// `metric_snapshots` row ([FR-QM-07]).
///
/// `view` must be hydrated from the store's current state (the caller — the
/// governance engine's reconcile-then-score, S-020 — owns that freshness
/// contract). `thresholds` is the effective CR-005 detection-threshold set the
/// caller composed from `rules.toml` ([BR-25]); its hash is persisted on the
/// row. Returns the persisted row id and the read-model.
///
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the batch
/// rolls back wholesale, [NFR-RA-07]).
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn snapshot(
    runtime: &Runtime,
    view: &GraphView,
    commit_sha: Option<&str>,
    thresholds: Thresholds,
) -> Result<(i64, MetricSnapshot)> {
    let (nodes, edges, functions, test_ids) = runtime
        .submit_read(|store| {
            Ok((
                store.all_nodes()?,
                store.all_edges()?,
                store.function_metrics()?,
                store.test_node_ids()?,
            ))
        })
        .context("reading the metrics snapshot inputs")?;

    // Read the persisted is_test verdict — the single source of truth the
    // annotation pass computes (FR-AN-05, CR-001); never re-derived here.
    let test_ids: HashSet<NodeId> = test_ids.into_iter().collect();
    // The effective CR-005 detection thresholds (BR-25): the governance engine
    // composes the documented defaults with the rules.toml [metric_thresholds]
    // overrides and passes the result here — the single seam S-044 left for
    // S-045. The persisted thresholds_hash follows automatically.
    let computed = compute(view, &nodes, &edges, &functions, &test_ids, thresholds);

    // created_at is bookkeeping, not part of the deterministic signal
    // (golden tests pin aggregate_signal, never the timestamp — ADR-08).
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Copy the persisted fields out so the write closure is 'static.
    let sha = commit_sha.map(str::to_owned);
    let row = OwnedSnapshotFields::from_model(&computed, created_at);
    let hash = computed.thresholds_hash.clone();
    let id = runtime
        .submit_write(move |writer| {
            writer.insert_metric_snapshot(&NewMetricSnapshot {
                created_at: row.created_at,
                commit_sha: sha.as_deref(),
                node_count: row.node_count,
                edge_count: row.edge_count,
                function_count: row.function_count,
                test_function_count: row.test_function_count,
                metric_version: METRIC_SEMANTICS_VERSION,
                empty: row.empty,
                modularity_raw: row.modularity.0,
                modularity_normalized: row.modularity.1,
                acyclicity_raw: row.acyclicity.0,
                acyclicity_normalized: row.acyclicity.1,
                depth_raw: row.depth.0,
                depth_normalized: row.depth.1,
                equality_raw: row.equality.0,
                equality_normalized: row.equality.1,
                redundancy_raw: row.redundancy.0,
                redundancy_normalized: row.redundancy.1,
                nesting_raw: Some(row.nesting.0),
                nesting_normalized: Some(row.nesting.1),
                conciseness_raw: Some(row.conciseness.0),
                conciseness_normalized: Some(row.conciseness.1),
                // Cohesion/Focus persist NULL value + applicable=false when they
                // dropped out of the mean (ADR-21 applicability drop-out).
                cohesion_raw: row.cohesion.map(|c| c.0),
                cohesion_normalized: row.cohesion.map(|c| c.1),
                cohesion_applicable: Some(row.cohesion.is_some()),
                focus_raw: row.focus.map(|f| f.0),
                focus_normalized: row.focus.map(|f| f.1),
                focus_applicable: Some(row.focus.is_some()),
                uniqueness_raw: Some(row.uniqueness.0),
                uniqueness_normalized: Some(row.uniqueness.1),
                thresholds_hash: Some(hash.as_str()),
                aggregate_signal: row.aggregate_signal,
            })
        })
        .context("persisting the metric snapshot")?;

    Ok((id, computed))
}

/// The class-like containers over the god thresholds ([FR-QM-12]), in node-id
/// order — pure, no I/O.
///
/// Backs the `no_god_containers` budget ([FR-GV-11] ext., [UAT-GV-08]) in the
/// governance evaluator: it counts the *same* containers Focus counts as god, so
/// the budget and the dimension can never disagree. The caller enriches each
/// [`GodContainer::id`] to a name/file via the node set for the violation
/// message; the list is already deterministic (the container index is built from
/// the id-ordered node set, [NFR-RA-06]).
///
/// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [UAT-GV-08]: ../../../docs/specs/requirements/UAT-GV-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub fn god_containers(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    test_ids: &HashSet<NodeId>,
    thresholds: Thresholds,
) -> Vec<GodContainer> {
    ContainerIndex::build(nodes, edges, test_ids).god_containers(&thresholds)
}

/// The per-dimension worst-offender lists for the five CR-005 structural
/// dimensions ([FR-QM-09]..[FR-QM-13]) — pure, no I/O (CR-005 §3.2 review-phase
/// visibility).
///
/// Each list is **production-scoped** (test functions/containers excluded, so it
/// agrees with the dimension it explains, [FR-QM-08]), ordered by offending
/// severity then node id, and capped at `cap` ([NFR-RA-06] determinism, bounded
/// output). The lists are report detail only — they never enter the aggregate or
/// the gate (exactly as `doc_gaps` is advisory). `functions` carries
/// the per-function facts; `nodes` supplies the name/file/line each offender
/// reports (the metric rows omit them).
///
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub fn worst_offenders(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
    test_ids: &HashSet<NodeId>,
    thresholds: Thresholds,
    cap: usize,
) -> WorstOffenders {
    let by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
    // Production scope (FR-QM-08): the offender lists explain the production-scope
    // dimensions, so they exclude is_test functions just as the dimensions do.
    let production: Vec<&FunctionMetricRow> = functions
        .iter()
        .filter(|f| !test_ids.contains(&f.id))
        .collect();

    // Enrich a node id + its severity descriptor into an Offender; an id absent
    // from the node set (never expected) is dropped rather than fabricated.
    let offender = |id: NodeId, detail: String| -> Option<Offender> {
        by_id.get(&id).map(|n| Offender {
            name: n.name.clone(),
            file: n.file_path.clone().unwrap_or_default(),
            line: n.start_line,
            detail,
        })
    };

    // Nesting (FR-QM-09): deeply-nested functions, deepest first then id asc.
    let mut nesting: Vec<(NodeId, i64)> = production
        .iter()
        .filter_map(|f| {
            f.max_nesting_depth
                .filter(|&d| d >= thresholds.nest)
                .map(|d| (f.id, d))
        })
        .collect();
    nesting.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let nesting = nesting
        .into_iter()
        .take(cap)
        .filter_map(|(id, depth)| offender(id, format!("nesting depth {depth}")))
        .collect();

    // Conciseness (FR-QM-10): brain methods (all three thresholds), highest CC
    // first, then LOC, then id asc.
    let mut conciseness: Vec<(NodeId, i64, i64, i64)> = production
        .iter()
        .filter_map(|f| {
            let (cc, loc, nest) = (
                f.cyclomatic_complexity?,
                f.line_count?,
                f.max_nesting_depth?,
            );
            (cc >= thresholds.brain_cc
                && loc >= thresholds.brain_loc
                && nest >= thresholds.brain_nest)
                .then_some((f.id, cc, loc, nest))
        })
        .collect();
    conciseness.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let conciseness = conciseness
        .into_iter()
        .take(cap)
        .filter_map(|(id, cc, loc, nest)| {
            offender(id, format!("CC {cc} · LOC {loc} · nesting {nest}"))
        })
        .collect();

    // Uniqueness (FR-QM-13): near-clone production functions, grouped by clone
    // group id then id asc (a stable, group-clustered listing).
    let mut uniqueness: Vec<(NodeId, i64)> = production
        .iter()
        .filter_map(|f| f.clone_group.map(|g| (f.id, g.get())))
        .collect();
    uniqueness.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let uniqueness = uniqueness
        .into_iter()
        .take(cap)
        .filter_map(|(id, group)| offender(id, format!("clone group #{group}")))
        .collect();

    // Cohesion/Focus read the class-like container index (FR-QM-11/12).
    let containers = ContainerIndex::build(nodes, edges, test_ids);

    // Cohesion (FR-QM-11): low-cohesion classes (LCOM4 ≥ 2), most fragmented
    // first then id asc.
    let mut cohesion = containers.low_cohesion_classes();
    cohesion.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let cohesion = cohesion
        .into_iter()
        .take(cap)
        .filter_map(|(id, lcom4)| offender(id, format!("LCOM4 {lcom4}")))
        .collect();

    // Focus (FR-QM-12): god containers, most methods first then widest span then
    // id asc.
    let mut focus = containers.god_containers(&thresholds);
    focus.sort_by(|a, b| {
        b.method_count
            .cmp(&a.method_count)
            .then(b.span.cmp(&a.span))
            .then(a.id.cmp(&b.id))
    });
    let focus = focus
        .into_iter()
        .take(cap)
        .filter_map(|g| {
            offender(
                g.id,
                format!("{} methods · span {}", g.method_count, g.span),
            )
        })
        .collect();

    WorstOffenders {
        nesting,
        conciseness,
        cohesion,
        focus,
        uniqueness,
    }
}

/// The `Copy`-able snapshot fields, detached from the read-model so the write
/// closure can own them across the `'static` writer-actor boundary. The
/// non-`Copy` `thresholds_hash` is moved into the closure separately.
#[derive(Clone, Copy)]
struct OwnedSnapshotFields {
    created_at: i64,
    node_count: i64,
    edge_count: i64,
    function_count: i64,
    test_function_count: i64,
    empty: bool,
    modularity: (f64, f64),
    acyclicity: (f64, f64),
    depth: (f64, f64),
    equality: (f64, f64),
    redundancy: (f64, f64),
    nesting: (f64, f64),
    conciseness: (f64, f64),
    /// `None` = Cohesion dropped out of the mean (no applicable classes, ADR-21).
    cohesion: Option<(f64, f64)>,
    /// `None` = Focus dropped out of the mean (no class-like containers).
    focus: Option<(f64, f64)>,
    uniqueness: (f64, f64),
    aggregate_signal: Option<i64>,
}

impl OwnedSnapshotFields {
    fn from_model(snapshot: &MetricSnapshot, created_at: i64) -> Self {
        let pair = |v: &MetricValue| (v.raw, v.normalized);
        Self {
            created_at,
            node_count: snapshot.node_count as i64,
            edge_count: snapshot.edge_count as i64,
            function_count: snapshot.function_count as i64,
            test_function_count: snapshot.test_function_count as i64,
            empty: snapshot.empty,
            modularity: pair(&snapshot.modularity),
            acyclicity: pair(&snapshot.acyclicity),
            depth: pair(&snapshot.depth),
            equality: pair(&snapshot.equality),
            redundancy: pair(&snapshot.redundancy),
            nesting: pair(&snapshot.nesting),
            conciseness: pair(&snapshot.conciseness),
            cohesion: snapshot.cohesion.as_ref().map(pair),
            focus: snapshot.focus.as_ref().map(pair),
            uniqueness: pair(&snapshot.uniqueness),
            aggregate_signal: snapshot.aggregate_signal.map(i64::from),
        }
    }
}

/// Build the metric graph: the hydrated view minus derived governance
/// artifacts and test code, plus each vertex's directory community.
///
/// Filters `Layer`/`Boundary` policy vertices and `ForbiddenDependency`
/// edges — annotation-materialised flags, not dependencies — and the
/// `is_test` vertices in `test_ids` (the production scope, [FR-QM-08]/[BR-18]).
/// Dropping a vertex also drops its incident edges (they are skipped when an
/// endpoint is not in `kept`), so the production subgraph the metrics score is
/// closed. Surviving vertices keep the view's deterministic index order, edges
/// the view's emission order, so the rebuilt indices are reproducible
/// ([NFR-RA-06]).
///
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn metric_graph(
    view: &GraphView,
    nodes: &[NodeRow],
    test_ids: &HashSet<NodeId>,
) -> (DiGraph<(), ()>, Vec<String>) {
    let file_of: HashMap<NodeId, &str> = nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();

    let source = view.graph();
    let mut graph = DiGraph::<(), ()>::with_capacity(source.node_count(), source.edge_count());
    let mut dirs: Vec<String> = Vec::with_capacity(source.node_count());
    let mut kept: HashMap<NodeIndex, NodeIndex> = HashMap::with_capacity(source.node_count());

    for index in source.node_indices() {
        let vertex = &source[index];
        if matches!(vertex.kind, Some(NodeKind::Layer | NodeKind::Boundary)) {
            continue; // derived policy vertex — not part of the code graph
        }
        // Promoted broker vertices (S-256, CR-061) are **markers**, not code: the code
        // they mark — the publishing/subscribing method — is already a vertex here, so
        // counting them too would measure the model rather than the source.
        //
        // For a `Topic` this is not merely tidy, it is load-bearing. A topic is a
        // repo-scoped identity with **no file** ([FR-WS-11]), so `file_of` misses it and
        // it lands in the `UNBOUND_DIR` community. Every `Publishes`/`Subscribes` edge
        // would then run from its producer's directory into `<unbound>` — external to
        // both communities, contributing to `degree` but never to `internal` — so
        // modularity would fall for **any** repo that indexes a broker topic, purely as
        // an artifact of how we model it. The user's real coupling (publisher → topic →
        // subscriber) is not a call edge and was never in this graph to begin with;
        // adding the model of it must not move the gated signal ([NFR-RA-06], and the
        // CR-061 invariant that these features never alter a member's gated verdict).
        //
        // [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
        // [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
        if matches!(
            vertex.kind,
            Some(NodeKind::Topic | NodeKind::Producer | NodeKind::Consumer)
        ) {
            continue;
        }
        if vertex.node_id.is_some_and(|id| test_ids.contains(&id)) {
            continue; // is_test vertex — excluded from the production scope (FR-QM-08)
        }
        let dir = vertex
            .node_id
            .and_then(|id| file_of.get(&id))
            .map_or(UNBOUND_DIR.to_string(), |path| directory_of(path));
        kept.insert(index, graph.add_node(()));
        dirs.push(dir);
    }

    for edge in source.edge_references() {
        if edge.weight().kind == Some(EdgeKind::ForbiddenDependency) {
            continue; // derived governance flag — not a dependency
        }
        let (Some(&src), Some(&dst)) = (kept.get(&edge.source()), kept.get(&edge.target())) else {
            continue; // incident to a filtered policy vertex
        };
        graph.add_edge(src, dst, ());
    }

    (graph, dirs)
}

/// The directory component of a project-relative path; `""` for a root-level
/// file. Splits on either separator so the partition is OS-independent.
fn directory_of(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(idx) => path[..idx].to_string(),
        None => String::new(),
    }
}

/// Modularity — Newman's Q under the directory partition ([FR-QM-01]).
///
/// `Q = Σ_c [ e_c/m − (d_c/2m)² ]` where `e_c` counts edges internal to
/// community `c` (this is where intra-directory edges are *retained* — the
/// rolled-up self-loop mandate) and `d_c` sums vertex degrees. Community sums
/// iterate a `BTreeMap` so the float reduction is canonical ([ADR-08]).
///
/// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
fn modularity(graph: &DiGraph<(), ()>, dirs: &[String]) -> MetricValue {
    let m = graph.edge_count();
    if m == 0 {
        // FR-QM-01: m == 0 → Q = 0 → normalized (0+0.5)/1.5 = 1/3 ≈ 0.333.
        return MetricValue {
            raw: 0.0,
            normalized: 0.5 / 1.5,
        };
    }

    // Exact integer tallies per community, in deterministic edge order.
    let mut communities: BTreeMap<&str, (u64, u64)> = BTreeMap::new(); // (internal, degree)
    for edge in graph.edge_references() {
        let src_dir = dirs[edge.source().index()].as_str();
        let dst_dir = dirs[edge.target().index()].as_str();
        communities.entry(src_dir).or_default().1 += 1;
        communities.entry(dst_dir).or_default().1 += 1;
        if src_dir == dst_dir {
            communities.entry(src_dir).or_default().0 += 1;
        }
    }

    // The only float summation in the engine: canonical BTreeMap key order.
    let m = m as f64;
    let mut q = 0.0_f64;
    for &(internal, degree) in communities.values() {
        let fraction = degree as f64 / (2.0 * m);
        q += internal as f64 / m - fraction * fraction;
    }

    MetricValue {
        raw: q,
        normalized: ((q + 0.5) / 1.5).clamp(0.0, 1.0),
    }
}

/// Acyclicity — cycle count from the shared SCC set ([FR-QM-02], [ADR-30]).
///
/// A cycle is an SCC with `len > 1` — mutual recursion between two or more
/// distinct units. A singleton vertex with a self-loop (self-recursion) is a
/// unit depending on **itself**, not a dependency cycle *between* units, so it
/// contributes zero ([CR-022] / [ADR-30], metric-semantics v4). A self-loop
/// *inside* a multi-vertex SCC is still counted once — the SCC is the unit the
/// `max_cycles` rule and the DSM consume, and they read this single
/// `acyclicity.raw` source so they can never disagree.
///
/// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
/// [ADR-30]: ../../../docs/specs/architecture/decisions/ADR-30.md
/// [CR-022]: ../../../docs/requests/CR-022-acyclicity-self-recursion-exclusion.md
fn acyclicity(sccs: &[Vec<NodeIndex>]) -> MetricValue {
    let cycles = sccs.iter().filter(|scc| scc.len() > 1).count() as u64;
    MetricValue {
        raw: cycles as f64,
        normalized: 1.0 / (1.0 + cycles as f64),
    }
}

/// Depth — longest path (in vertices) over the SCC condensation ([FR-QM-03]).
///
/// Reuses the Acyclicity SCC set: each SCC condenses to one vertex, so a whole
/// cycle collapses to depth 1. `tarjan_scc` returns components in reverse
/// topological order — every condensed successor of component `i` sits at an
/// index `< i` — so a single forward sweep is the longest-path DP. All
/// arithmetic is integral; only the final normalization divides in f64.
///
/// [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
fn depth(graph: &DiGraph<(), ()>, sccs: &[Vec<NodeIndex>]) -> MetricValue {
    let mut scc_of = vec![0_usize; graph.node_count()];
    for (component, members) in sccs.iter().enumerate() {
        for &vertex in members {
            scc_of[vertex.index()] = component;
        }
    }

    // Condensed successor sets (BTreeSet: deduplicated, deterministic).
    let mut successors: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); sccs.len()];
    for edge in graph.edge_references() {
        let (src, dst) = (scc_of[edge.source().index()], scc_of[edge.target().index()]);
        if src != dst {
            debug_assert!(dst < src, "tarjan_scc is reverse-topological");
            successors[src].insert(dst);
        }
    }

    // Longest-path DP in the reverse-topological component order.
    let mut longest = vec![0_u64; sccs.len()];
    for component in 0..sccs.len() {
        let best = successors[component]
            .iter()
            .map(|&succ| longest[succ])
            .max()
            .unwrap_or(0);
        longest[component] = 1 + best;
    }
    let depth = longest.iter().copied().max().unwrap_or(0);

    MetricValue {
        raw: depth as f64,
        normalized: 1.0 / (1.0 + depth as f64 / 8.0),
    }
}

/// Equality — `1 − Gini` of per-function cyclomatic complexity ([FR-QM-04]).
///
/// Sorted-array Gini with exact integer accumulation:
/// `G = 2·Σ(i·xᵢ) / (n·Σx) − (n+1)/n` over ascending `xᵢ`, 1-based `i`.
/// Functions whose complexity was never computed (`NULL`) are excluded rather
/// than coerced to 0 — `NULL` means "not computed", and a phantom zero would
/// *inflate* inequality ([NFR-CC-04] honesty).
///
/// [FR-QM-04]: ../../../docs/specs/requirements/FR-QM-04.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn equality(functions: &[&FunctionMetricRow]) -> MetricValue {
    let mut complexities: Vec<i64> = functions
        .iter()
        .filter_map(|f| f.cyclomatic_complexity)
        .collect();
    complexities.sort_unstable();

    let n = complexities.len() as i64;
    let total: i64 = complexities.iter().sum();
    if n <= 1 || total == 0 {
        // FR-QM-04 guards: nothing to distribute → perfectly equal.
        return MetricValue {
            raw: 0.0,
            normalized: 1.0,
        };
    }

    let weighted: i64 = complexities
        .iter()
        .enumerate()
        .map(|(i, &x)| (i as i64 + 1) * x)
        .sum();
    let gini =
        ((2 * weighted) as f64 / (n * total) as f64 - (n + 1) as f64 / n as f64).clamp(0.0, 1.0);

    MetricValue {
        raw: gini,
        normalized: 1.0 - gini,
    }
}

/// Redundancy — `1 − redundant/total` over function/method nodes
/// ([FR-QM-05]).
///
/// A function is redundant if it is dead **or** duplicate; each row is one
/// distinct node, so a function that is both is counted exactly once.
///
/// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
fn redundancy(functions: &[&FunctionMetricRow]) -> MetricValue {
    let total = functions.len();
    if total == 0 {
        return MetricValue {
            raw: 0.0,
            normalized: 1.0,
        };
    }
    let redundant = functions
        .iter()
        .filter(|f| f.is_dead == Some(true) || f.is_duplicate == Some(true))
        .count();
    let ratio = redundant as f64 / total as f64;
    MetricValue {
        raw: ratio,
        normalized: 1.0 - ratio,
    }
}

/// The applicable-dimension geometric-mean aggregate, rounded to the 0–10000
/// integer signal (metric-semantics v4, [FR-QM-06], [FR-QM-14], [ADR-12],
/// [ADR-21], [ADR-08]).
///
/// `original` are the five original metrics in canonical order; `new_dims` are
/// the **applicable** new dimensions in canonical order (Cohesion/Focus omitted
/// when they dropped out, [ADR-21]). The reduction:
///
/// 1. **Zero short-circuit (original five only, [ADR-21]).** If any *original*
///    metric is `0.0`, the signal is `0` — a hard zero in a systemic-pathology
///    metric collapses the score *and* keeps `ln(0) = −∞` out of the reduction.
///    The new five are floored at [`extended::DIMENSION_FLOOR`], never `0`, so
///    they never trigger this — they drag but never alone collapse the signal.
/// 2. **Geometric mean over the applicable dimensions.** Sum `ln nᵢ` over the
///    original five then the applicable new dims, in canonical order, and divide
///    by the *count of applicable dimensions* (10, or 9/8 when Cohesion/Focus
///    drop out) — a deterministic, honest n-dimension mean ([FR-QM-14],
///    [UAT-QM-10]).
///
/// The empty-graph sentinel is handled by the caller ([ADR-12]); this function
/// is only reached for a non-empty production graph.
///
/// [FR-QM-06]: ../../../docs/specs/requirements/FR-QM-06.md
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
fn aggregate(original: [f64; ORIGINAL_METRIC_COUNT], new_dims: &[f64]) -> u32 {
    // Zero short-circuit scoped to the original five (ADR-21): a floored new
    // dimension can never be 0, so only an original hard zero collapses here.
    if original.contains(&0.0) {
        return 0;
    }
    // Log-space sum in canonical order: original five, then the applicable new
    // dimensions. The denominator is the number of dimensions that actually
    // entered (applicability drop-out, FR-QM-14).
    let ln_sum: f64 = original.iter().chain(new_dims.iter()).map(|n| n.ln()).sum();
    let count = (ORIGINAL_METRIC_COUNT + new_dims.len()) as f64;
    let signal = ((ln_sum / count).exp() * 10_000.0).round();
    // exp of a non-positive mean never exceeds 1.0 (and the floored inputs keep it
    // well above 0), but clamp both ends defensively so the persisted CHECK
    // (0..=10000) can never fire on float residue.
    signal.clamp(0.0, 10_000.0) as u32
}
