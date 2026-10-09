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
//!    `m == 0` → `1/3`. **Not applicable** below [`MODULARITY_MIN_EDGES`]
//!    edges (m = 0 included, [CR-156], metric-semantics v6): the computed pair
//!    is kept, but the dimension drops out of the aggregate.
//! 2. **Acyclicity** ([FR-QM-02], [ADR-61], extends [ADR-30]) — count of
//!    `tarjan_scc` components with `len > 1` **whose members span more than one
//!    directory** (a cross-module dependency cycle); a singleton self-loop /
//!    self-recursion (metric-semantics v4, [CR-022]) and a multi-node SCC
//!    confined to a single directory (metric-semantics v5, [CR-087]) both count
//!    zero; normalized `1/(1+cycles)`. The narrowed count is the single source
//!    the DSM and the `max_cycles` rule read, so the gate and the rule can never
//!    disagree.
//! 3. **Depth** ([FR-QM-03], [ADR-62], [CR-088]) — longest path (vertex count)
//!    over the SCC condensation of the **module-rollup graph** (the same
//!    `directory_of` partition Modularity uses, metric-semantics v5): symbols
//!    roll up to their directory, so a long call chain inside a single directory
//!    scores depth 1 and a dependency cycle among directories collapses to one
//!    layer. It measures architectural layering, not intra-file call length.
//!    Normalized `1/(1+depth/8)`.
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
//! # Aggregation (metric-semantics v6, [FR-QM-06], [FR-QM-14], [ADR-12], [ADR-21])
//!
//! `signal = exp((Σ ln nᵢ)/k) · 10000` over the **applicable** dimensions in
//! **canonical order** (the five original metrics, then nesting, conciseness,
//! cohesion, focus, uniqueness), rounded to an integer ([ADR-08]). `k` is the
//! count of applicable dimensions (10, or fewer as Modularity, Cohesion and
//! Focus drop out). Three guards:
//!
//! - **Zero short-circuit (applicable original five only)** — if any
//!   *applicable original* `nᵢ == 0.0` the signal is `0` *before* any `ln` runs
//!   (a hard systemic pathology collapses the score; anti-gaming, [ADR-21]). The
//!   new five are floored, never `0`, so they drag but never alone collapse the
//!   signal.
//! - **Applicability drop-out** — Cohesion/Focus with no construct store NULL +
//!   a `false` flag and drop out of the denominator ([ADR-21]); a class-less repo
//!   gets a deterministic 9-dimension mean ([UAT-QM-10]). Modularity on a graph
//!   with fewer than [`MODULARITY_MIN_EDGES`] edges drops out the same way — out
//!   of the denominator *and* out of the short-circuit — but keeps its computed
//!   values beside a `false` `modularity_applicable` flag ([CR-156]).
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
//! [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
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
use crate::models::quality::{
    MetricSnapshot, MetricValue, ModularityNotApplicable, Offender, WorstOffenders,
};
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

/// The fewest edges Modularity's graph needs for Modularity to apply
/// ([CR-156], metric-semantics v6).
///
/// Below it Newman's Q has no community structure to measure: a library whose
/// one to four edges all cross between two directories scores Q = −0.5, which
/// normalizes to exactly 0 and — through the [ADR-12] short-circuit — zeroed
/// the whole signal of an ordinary model → enum layering. Such a graph drops
/// Modularity out of the aggregate instead ([ADR-21] rule 2). A fixed constant
/// of the metric semantics, deliberately not a `rules.toml` knob: a tunable
/// threshold on the one dimension that can zero the signal is a gaming surface.
///
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
pub const MODULARITY_MIN_EDGES: u64 = 5;

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
/// - **v5** — the metric semantics move to the **module-rollup graph** for both
///   cross-module dimensions ([CR-087]/[CR-088], [ADR-61]/[ADR-62], S-298/S-299);
///   a v4 baseline is incomparable, so the first post-upgrade gate
///   auto-re-baselines ([FR-GV-10]):
///   - *Acyclicity* counts only cross-module cycles: a multi-node `tarjan_scc`
///     component counts only when its members span **more than one directory**
///     (the `directory_of` partition Modularity uses); an SCC confined to a
///     single directory — idiomatic intra-module mutual recursion — now
///     contributes zero, extending the v4 self-recursion narrowing to the module
///     boundary. The narrowed count is the single source the `max_cycles` rule
///     and the DSM read.
///   - *Depth* is measured over the SCC condensation of that same module-rollup
///     graph rather than the symbol-level call graph: a long call chain confined
///     to one directory rolls up to a single module vertex (depth 1) instead of
///     reporting intra-file call length as architectural layering.
/// - **v6** — Modularity is **not applicable** on a graph with fewer than
///   [`MODULARITY_MIN_EDGES`] edges ([CR-156], [ADR-21] rule 2): it keeps its
///   computed pair on the snapshot but leaves both the geometric mean and the
///   [ADR-12] zero short-circuit, so a small library whose one to four edges all
///   cross two directories no longer scores 0. An edgeless non-empty graph drops
///   it too (the neutral 1/3 no longer enters the mean). Every graph with
///   m ≥ 5 scores byte-identically to v5; a v5 baseline is still incomparable, so
///   the first post-upgrade gate auto-re-baselines ([FR-GV-10]).
/// - **v7** — the structural metrics stop counting declarative code ([CR-163],
///   S-501/S-502); a v6 baseline is incomparable, so the first post-upgrade gate
///   auto-re-baselines ([FR-GV-10]) — one re-baseline for the whole CR:
///   - *The exact-duplicate floor* ([FR-AN-02], S-501): a function is
///     `is_duplicate` only when it has a body ([FR-EX-11]) **and** at least
///     `duplicate_min_tokens` (default 50) normalized tokens, so Redundancy and
///     the `max_duplicates` budget stop counting four-line constant overrides
///     and bodyless declarations as copy-paste.
///   - *Bodied LCOM4* ([FR-QM-11]): Cohesion's LCOM4 counts the components of a
///     class's **bodied** methods; a bodyless declaration is no longer its own
///     component (it still links the bodied methods that call it), and a class
///     with no bodied method is unscoreable.
///   - *Bodied Focus* ([FR-QM-12]): the god predicate counts **bodied**
///     methods (`bodied ≥ T_m ∨ span ≥ T_span`), so a MapStruct-style mapper of
///     abstract declarations is no longer a god container.
///
///   A `has_body` fact still `NULL` (an upgraded store not yet re-extracted)
///   reads as bodied, never bodyless. The Uniqueness offender list's
///   order-by-mass (CR-163 §3.2 E) is report order only and needs no bump.
///
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
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
/// [CR-087]: ../../../docs/requests/CR-087-acyclicity-cross-module-cycle-boundary.md
/// [CR-088]: ../../../docs/requests/CR-088-depth-module-granularity.md
/// [ADR-61]: ../../../docs/specs/architecture/decisions/ADR-61.md
/// [ADR-62]: ../../../docs/specs/architecture/decisions/ADR-62.md
/// [CR-163]: ../../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
/// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
/// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
/// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
/// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
pub const METRIC_SEMANTICS_VERSION: i64 = 7;

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
    let MetricGraph { graph, dirs, .. } = metric_graph(view, nodes, test_ids);

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
    // CR-156: below MODULARITY_MIN_EDGES the graph has no community structure to
    // measure, so Modularity drops out (ADR-21 rule 2) — its computed pair stays.
    let modularity_not_applicable = ModularityNotApplicable::for_edges(graph.edge_count() as u64);
    // Acyclicity counts cross-module SCCs (>1 directory, ADR-61) over the
    // symbol-level SCC set — the single narrowed source the DSM and the
    // `max_cycles` rule read. Depth measures architectural layering over its own
    // module-rollup condensation (CR-088, ADR-62), reading the same `dirs`
    // partition Modularity uses; the two dimensions no longer share an SCC set.
    let sccs = tarjan_scc(&graph);
    let acyclicity = acyclicity(&sccs, &dirs);
    let depth = depth(&graph, &dirs);
    let equality = equality(&production);
    let redundancy = redundancy(&production);

    // The five CR-005 structural dimensions (FR-QM-09..13), in canonical order.
    // Cohesion/Focus read class-like containers from the raw node/edge snapshot
    // (Contains/Accesses/Calls are absent from the ExcludeContains view); both
    // can drop out (None) when their construct is absent (ADR-21).
    let nesting = extended::nesting(&production, &thresholds);
    let conciseness = extended::conciseness(&production, &thresholds);
    let uniqueness = extended::uniqueness(&production);
    let containers = ContainerIndex::build(nodes, edges, functions, test_ids);
    let cohesion = containers.cohesion();
    let focus = containers.focus(&thresholds);

    let empty = graph.node_count() == 0;
    let aggregate_signal = if empty {
        // The ADR-12 empty-graph sentinel: never a misleading number. Unchanged
        // by CR-005 — an empty production graph is still "n/a".
        None
    } else {
        Some(aggregate(
            &applicable_original_dimensions(
                modularity_not_applicable.is_none().then_some(&modularity),
                &acyclicity,
                &depth,
                &equality,
                &redundancy,
            ),
            &applicable_new_dimensions(&nesting, &conciseness, &cohesion, &focus, &uniqueness),
        ))
    };

    MetricSnapshot {
        modularity,
        modularity_not_applicable,
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

/// The normalized values of the **applicable** original dimensions, in canonical
/// order (modularity, acyclicity, depth, equality, redundancy) — Modularity
/// contributes only when applicable ([CR-156]), exactly as Cohesion/Focus do in
/// [`applicable_new_dimensions`]. Acyclicity, Depth, Equality and Redundancy
/// always apply. What this returns is both the set the geometric mean spans and
/// the set the [ADR-12] zero short-circuit inspects, so a not-applicable
/// Modularity leaves both by one mechanism. `modularity` is `None` when it is
/// not applicable, the same `Option` shape the sibling takes for Cohesion/Focus.
///
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
fn applicable_original_dimensions(
    modularity: Option<&MetricValue>,
    acyclicity: &MetricValue,
    depth: &MetricValue,
    equality: &MetricValue,
    redundancy: &MetricValue,
) -> Vec<f64> {
    let mut dims = Vec::with_capacity(5);
    if let Some(m) = modularity {
        dims.push(m.normalized);
    }
    dims.extend([acyclicity, depth, equality, redundancy].map(|v| v.normalized));
    dims
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

/// What [`snapshot`] persisted: the appended row's id, its read-model, and the
/// worst-offender lists written with it ([FR-QM-15]).
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
#[derive(Debug)]
pub struct RecordedSnapshot {
    /// The appended `metric_snapshots` row id.
    pub id: i64,
    /// The metric read-model the row persists.
    pub metrics: MetricSnapshot,
    /// The per-dimension lists persisted with the row, from the same read the
    /// metrics were computed on; always [`recorded`](WorstOffenders::recorded).
    pub worst_offenders: WorstOffenders,
}

/// Run a full metrics pass: snapshot the store, [`compute`] the metrics and
/// the [`worst_offenders`] from that one read, and append one
/// `metric_snapshots` row with its offender lists ([FR-QM-07], [FR-QM-15]).
///
/// `view` must be hydrated from the store's current state (the caller — the
/// governance engine's reconcile-then-score, S-020 — owns that freshness
/// contract). `thresholds` is the effective CR-005 detection-threshold set the
/// caller composed from `rules.toml` ([BR-25]); its hash is persisted on the
/// row, and the offender lists are computed under the same set. This is the
/// one snapshot-append path, so every caller — `scan`, `gate`,
/// `session_start`, `session_end` — persists the lists it computed.
///
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the batch
/// rolls back wholesale, [NFR-RA-07]).
///
/// [FR-QM-07]: ../../../docs/specs/requirements/FR-QM-07.md
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn snapshot(
    runtime: &Runtime,
    view: &GraphView,
    commit_sha: Option<&str>,
    thresholds: Thresholds,
) -> Result<RecordedSnapshot> {
    let inputs = read_inputs(runtime)?;
    let computed = compute(
        view,
        &inputs.nodes,
        &inputs.edges,
        &inputs.functions,
        &inputs.test_ids,
        thresholds,
    );
    let offenders = worst_offenders(
        view,
        &inputs.nodes,
        &inputs.edges,
        &inputs.functions,
        &inputs.test_ids,
        thresholds,
        WORST_OFFENDER_CAP,
    );

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
    // The lists ride into the write closure and back out, so the one batch
    // that appends the row also appends them (NFR-RA-07) without a copy.
    let (id, worst_offenders) = runtime
        .submit_write(move |writer| {
            let id = writer.insert_metric_snapshot(
                &NewMetricSnapshot {
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
                    // Modularity keeps its computed pair even when it is not
                    // applicable (CR-156); only the flag records the drop-out.
                    modularity_applicable: Some(row.modularity_applicable),
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
                },
                &offenders,
            )?;
            Ok((id, offenders))
        })
        .context("persisting the metric snapshot")?;

    Ok(RecordedSnapshot {
        id,
        metrics: computed,
        worst_offenders,
    })
}

/// The store facts every metrics pass scores: nodes, edges, per-function
/// metrics and the persisted `is_test` verdict — read in one `submit_read`.
struct SnapshotInputs {
    nodes: Vec<NodeRow>,
    edges: Vec<EdgeRow>,
    functions: Vec<FunctionMetricRow>,
    test_ids: HashSet<NodeId>,
}

fn read_inputs(runtime: &Runtime) -> Result<SnapshotInputs> {
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
    Ok(SnapshotInputs {
        nodes,
        edges,
        functions,
        test_ids: test_ids.into_iter().collect(),
    })
}

/// Compute the metric snapshot **without persisting it** — the read-only twin of
/// [`snapshot`] ([CR-095]).
///
/// [`snapshot`] is the [FR-GV-09] path: "every gate writes a snapshot, saved or
/// compared". That is right for `scan`/`gate`, which exist to record the signal
/// — and wrong for the **report tier**, which only wants to *show* it. A hook
/// that fires at every session boundary would otherwise take the graph-DB write
/// lock and append a row to the [FR-GV-06] `evolution` series on every
/// `/clear`, drowning the signal history in session-open noise and causing the
/// very lock contention the readout was written to tolerate.
///
/// So this returns the same freshly-computed [`MetricSnapshot`] [`snapshot`]
/// would have persisted, with **no write**: the deterministic `compute` half is
/// shared verbatim, so the two paths can never disagree on the signal.
///
/// # Errors
/// Returns an error if the snapshot input read fails.
///
/// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
/// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
pub fn compute_snapshot(
    runtime: &Runtime,
    view: &GraphView,
    thresholds: Thresholds,
) -> Result<MetricSnapshot> {
    let inputs = read_inputs(runtime)?;
    // The effective CR-005 detection thresholds (BR-25): the governance engine
    // composes the documented defaults with the rules.toml [metric_thresholds]
    // overrides and passes the result here — the single seam S-044 left for
    // S-045. The persisted thresholds_hash follows automatically.
    Ok(compute(
        view,
        &inputs.nodes,
        &inputs.edges,
        &inputs.functions,
        &inputs.test_ids,
        thresholds,
    ))
}

/// The class-like containers over the god thresholds ([FR-QM-12]), in node-id
/// order — pure, no I/O.
///
/// Backs the `no_god_containers` budget ([FR-GV-11] ext., [UAT-GV-08]) in the
/// governance evaluator: it counts the *same* containers Focus counts as god, so
/// the budget and the dimension can never disagree — including over which
/// methods count: `functions` supplies the `has_body` fact, so the budget counts
/// bodied methods exactly as Focus does (metric-semantics v7). The caller
/// enriches each [`GodContainer::id`] to a name/file via the node set for the
/// violation message; the list is already deterministic (the container index is
/// built from the id-ordered node set, [NFR-RA-06]).
///
/// [FR-QM-12]: ../../../docs/specs/requirements/FR-QM-12.md
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [UAT-GV-08]: ../../../docs/specs/requirements/UAT-GV-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub fn god_containers(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
    test_ids: &HashSet<NodeId>,
    thresholds: Thresholds,
) -> Vec<GodContainer> {
    ContainerIndex::build(nodes, edges, functions, test_ids).god_containers(&thresholds)
}

/// The per-dimension worst-offender lists for nine dimensions — the five CR-005
/// structural dimensions ([FR-QM-09]..[FR-QM-13], CR-005 §3.2 review-phase
/// visibility) and Acyclicity, Depth, Equality and Redundancy ([CR-209]) — pure,
/// no I/O.
///
/// Each list is **production-scoped** (test functions/containers excluded, so it
/// agrees with the dimension it explains, [FR-QM-08]), ordered by offending
/// severity then a stable tie-break, and capped at `cap` ([NFR-RA-06]
/// determinism, bounded output). The lists are report detail only — they never
/// enter the aggregate or the gate (exactly as `doc_gaps` is advisory).
/// `functions` carries the per-function facts; `nodes` supplies the
/// name/file/line each offender reports (the metric rows omit them); `view` is
/// the dependency view [`compute`] scores, whose production metric graph
/// Acyclicity and Depth explain.
///
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
///
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [FR-QM-09]: ../../../docs/specs/requirements/FR-QM-09.md
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub fn worst_offenders(
    view: &GraphView,
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

    // Uniqueness (FR-QM-13, CR-163 E): near-clone production functions, ranked
    // by their group's duplicated mass — members × mean line count — descending,
    // then group id, then member id, so the largest copy-paste surfaces first and
    // a group's members stay adjacent.
    let uniqueness = clone_groups_by_mass(&production)
        .into_iter()
        .flat_map(|g| {
            let detail = g.detail();
            g.members.into_iter().map(move |id| (id, detail.clone()))
        })
        .take(cap)
        .filter_map(|(id, detail)| offender(id, detail))
        .collect();

    // Cohesion/Focus read the class-like container index (FR-QM-11/12) over
    // bodied methods (metric-semantics v7).
    let containers = ContainerIndex::build(nodes, edges, functions, test_ids);

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

    // The CR-209 lists. Acyclicity and Depth explain the production metric
    // graph `compute` scores, rebuilt here from the same view and node set.
    let metric = metric_graph(view, nodes, test_ids);

    // Acyclicity (FR-QM-02): each cycle `acyclicity` counts, most members first,
    // then its lowest member id — which also names the row.
    let sccs = tarjan_scc(&metric.graph);
    let mut cycles: Vec<(usize, NodeId, BTreeSet<&str>)> = cross_module_cycles(&sccs, &metric.dirs)
        .filter_map(|scc| {
            let named_by = scc.iter().filter_map(|v| metric.ids[v.index()]).min()?;
            let dirs = scc.iter().map(|v| metric.dirs[v.index()].as_str()).collect();
            Some((scc.len(), named_by, dirs))
        })
        .collect();
    cycles.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let acyclicity = cycles
        .into_iter()
        .take(cap)
        .filter_map(|(size, id, dirs)| {
            // The count is stated, so the directories past the named few are
            // an ellipsis rather than a second count.
            let more = if dirs.len() > NAMED_DIRECTORIES { ", …" } else { "" };
            offender(
                id,
                format!(
                    "{size} symbols across {} directories: {}{more}",
                    dirs.len(),
                    named_directories(&dirs)
                ),
            )
        })
        .collect();

    // Depth (FR-QM-03): the chain heads of the module-rollup condensation; a
    // chain names directories, not a symbol, so it has no file or line.
    let depth = depth_chains(&metric.graph, &metric.dirs, cap)
        .into_iter()
        .map(|(head, detail)| Offender {
            name: head,
            file: String::new(),
            line: None,
            detail,
        })
        .collect();

    // Equality (FR-QM-04): the functions above the mean complexity — the ones
    // that raise the Gini — highest first, then id asc. Compared as
    // `cc · n > Σcc`, exact; an even spread lists none, as it scores 1.
    let measured: Vec<(NodeId, i64)> = production
        .iter()
        .filter_map(|f| f.cyclomatic_complexity.map(|cc| (f.id, cc)))
        .collect();
    let (count, total) = (
        measured.len() as i64,
        measured.iter().map(|&(_, cc)| cc).sum::<i64>(),
    );
    let mut equality: Vec<(NodeId, i64)> = measured
        .into_iter()
        .filter(|&(_, cc)| cc * count > total)
        .collect();
    equality.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let equality = equality
        .into_iter()
        .take(cap)
        .filter_map(|(id, cc)| offender(id, format!("complexity {cc}")))
        .collect();

    // Redundancy (FR-QM-05): the dead or duplicate functions `redundancy`
    // counts, most lines first (an unrecorded count last), then id asc. Only a
    // `Some(true)` verdict counts, so a language without a reachability verdict
    // (`is_dead = NULL`) is never listed as dead.
    let mut redundancy: Vec<(NodeId, Option<i64>, bool, bool)> = production
        .iter()
        .filter(|f| is_redundant(f))
        .map(|f| {
            (
                f.id,
                f.line_count,
                f.is_dead == Some(true),
                f.is_duplicate == Some(true),
            )
        })
        .collect();
    redundancy.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let redundancy = redundancy
        .into_iter()
        .take(cap)
        .filter_map(|(id, _, dead, duplicate)| {
            let detail = match (dead, duplicate) {
                (true, true) => "dead, duplicate",
                (true, false) => "dead",
                _ => "duplicate",
            };
            offender(id, detail.to_string())
        })
        .collect();

    WorstOffenders {
        // Computed here, so these are the lists — possibly all empty — and not
        // the "not recorded" default (FR-QM-15).
        recorded: true,
        nesting,
        conciseness,
        cohesion,
        focus,
        uniqueness,
        acyclicity,
        depth,
        equality,
        redundancy,
        unrecorded: Vec::new(),
    }
}

/// The [CR-209] lists a recorded snapshot holds no row for although its own
/// scores say the list is not empty — so the snapshot was written before the
/// list existed, and a reader must show it as not recorded rather than as
/// "none flagged" ([NFR-CC-04]). `offenders_recorded` admits only `1`, so the
/// store cannot mark these snapshots itself without a migration.
///
/// Exact, never a guess, because each list is empty precisely when its
/// dimension scores clean ([`worst_offenders`]): no cross-directory cycle
/// (Acyclicity 0), no chain of two layers (Depth ≤ 1), no function above the
/// mean complexity (an even spread, whose Gini computes to exactly 0.0: both
/// terms are the same correctly-rounded `(n+1)/n`), no dead or duplicate
/// function (Redundancy 0). A snapshot this version writes therefore never
/// reports a list here.
///
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn unrecorded_lists(
    metrics: &MetricSnapshot,
    offenders: &WorstOffenders,
) -> Vec<&'static str> {
    if !offenders.recorded {
        return Vec::new(); // every list is already "not recorded"
    }
    [
        ("acyclicity", &offenders.acyclicity, metrics.acyclicity.raw > 0.0),
        ("depth", &offenders.depth, metrics.depth.raw >= 2.0),
        ("equality", &offenders.equality, metrics.equality.raw > 0.0),
        ("redundancy", &offenders.redundancy, metrics.redundancy.raw > 0.0),
    ]
    .into_iter()
    .filter(|(_, list, scored)| list.is_empty() && *scored)
    .map(|(dimension, _, _)| dimension)
    .collect()
}

/// One near-clone group's production members and its duplicated mass — the unit
/// the Uniqueness offender list ranks ([FR-QM-13], [CR-163] §3.2 E).
///
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [CR-163]: ../../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
#[derive(Debug)]
struct CloneGroupMass {
    /// The stable group id (the component's minimum node id, [FR-AN-06]).
    ///
    /// [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
    group: i64,
    /// The group's production members, id ascending.
    members: Vec<NodeId>,
    /// The summed line count of the members whose line count is recorded.
    lines: i64,
    /// How many members have a recorded line count — the mean's denominator.
    measured: i64,
}

impl CloneGroupMass {
    /// `members × mean line count` as an exact fraction `(numerator,
    /// denominator)`. The mean spans the members whose line count is recorded,
    /// never a fabricated zero for the rest ([NFR-CC-04]); a group with none
    /// recorded has mass 0. Kept rational so the ranking is integer-exact and
    /// byte-identical across targets ([NFR-RA-06]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn mass(&self) -> (i128, i128) {
        (
            self.members.len() as i128 * i128::from(self.lines),
            i128::from(self.measured.max(1)),
        )
    }

    /// The offender descriptor: `clone group #G · N members × L lines`, `L` the
    /// mean recorded line count rounded half up (display only — the ranking
    /// reads the exact [`mass`](Self::mass)). A group with no recorded line
    /// count states its members alone rather than a fabricated `0 lines`.
    fn detail(&self) -> String {
        let members = self.members.len();
        if self.measured == 0 {
            return format!("clone group #{} · {members} members", self.group);
        }
        let mean = (2 * self.lines + self.measured) / (2 * self.measured);
        format!(
            "clone group #{} · {members} members × {mean} lines",
            self.group
        )
    }
}

/// The production near-clone groups ordered by duplicated mass (`members × mean
/// line count`) descending, then group id ascending; each group's members are id
/// ascending ([FR-QM-13], [CR-163] §3.2 E). A (2 × 30 lines) group therefore
/// precedes a (6 × 4 lines) group although it has fewer members. Masses compare
/// by cross-multiplication, exact and deterministic ([NFR-RA-06]).
///
/// [FR-QM-13]: ../../../docs/specs/requirements/FR-QM-13.md
/// [CR-163]: ../../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn clone_groups_by_mass(production: &[&FunctionMetricRow]) -> Vec<CloneGroupMass> {
    let mut by_group: BTreeMap<i64, CloneGroupMass> = BTreeMap::new();
    for f in production {
        let Some(group) = f.clone_group else { continue };
        let g = by_group.entry(group.get()).or_insert_with(|| CloneGroupMass {
            group: group.get(),
            members: Vec::new(),
            lines: 0,
            measured: 0,
        });
        g.members.push(f.id);
        if let Some(lines) = f.line_count {
            g.lines += lines;
            g.measured += 1;
        }
    }
    let mut groups: Vec<CloneGroupMass> = by_group.into_values().collect();
    for g in &mut groups {
        g.members.sort_unstable();
    }
    groups.sort_by(|a, b| {
        let ((an, ad), (bn, bd)) = (a.mass(), b.mass());
        (bn * ad).cmp(&(an * bd)).then(a.group.cmp(&b.group))
    });
    groups
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
    /// `false` = Modularity dropped out of the mean (fewer than
    /// [`MODULARITY_MIN_EDGES`] edges, CR-156); the pair above is kept.
    modularity_applicable: bool,
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
            modularity_applicable: snapshot.modularity_not_applicable.is_none(),
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

/// The production metric graph [`compute`] scores and [`worst_offenders`]
/// explains, with two per-vertex facts indexed by vertex index: the directory
/// community and the node id.
struct MetricGraph {
    graph: DiGraph<(), ()>,
    /// Each vertex's directory community ([`directory_of`], or [`UNBOUND_DIR`]).
    dirs: Vec<String>,
    /// Each vertex's node id — what an offender row is named by ([CR-209]).
    /// `None` only for a rollup view's aggregate vertex, which the metrics'
    /// symbol-level view never holds.
    ///
    /// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
    ids: Vec<Option<NodeId>>,
}

/// Build the metric graph: the hydrated view minus derived governance
/// artifacts and test code, plus each vertex's directory community and node id.
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
) -> MetricGraph {
    let file_of: HashMap<NodeId, &str> = nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();

    let source = view.graph();
    let mut graph = DiGraph::<(), ()>::with_capacity(source.node_count(), source.edge_count());
    let mut dirs: Vec<String> = Vec::with_capacity(source.node_count());
    let mut ids: Vec<Option<NodeId>> = Vec::with_capacity(source.node_count());
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
        ids.push(vertex.node_id);
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

    MetricGraph { graph, dirs, ids }
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
/// Always computes the pair, at every size: whether it *applies* is decided
/// beside it in [`compute`] ([`MODULARITY_MIN_EDGES`], [CR-156]), so a
/// not-applicable Modularity still persists the values it would have scored.
///
/// [FR-QM-01]: ../../../docs/specs/requirements/FR-QM-01.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
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

/// Acyclicity — cross-module cycle count from the shared SCC set ([FR-QM-02],
/// [ADR-61], extends [ADR-30]).
///
/// A cycle is an SCC with `len > 1` **whose members span more than one
/// directory** — a dependency cycle *between distinct modules* (the
/// `directory_of` partition [`modularity`] already uses, threaded in as `dirs`).
/// Two narrowings compose here:
///
/// - A singleton vertex with a self-loop (self-recursion) is a unit depending on
///   **itself**, so it contributes zero ([CR-022] / [ADR-30], metric-semantics
///   v4).
/// - A multi-node SCC confined to a **single directory** (idiomatic intra-module
///   mutual recursion — recursive-descent walkers, name resolvers) is control
///   flow, not a layering tangle, so it too contributes zero (metric-semantics
///   v5, [CR-087] / [ADR-61]).
///
/// A self-loop *inside* a counted cross-module SCC is still counted once — the
/// SCC is the unit the `max_cycles` rule and the DSM consume, and they read this
/// single (narrowed) `acyclicity.raw` source so they can never disagree.
///
/// `dirs` is indexed by `graph` vertex index, exactly as the [`tarjan_scc`]
/// `NodeIndex`es in `sccs` are, so `dirs[vertex.index()]` is the vertex's
/// directory.
///
/// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
/// [ADR-30]: ../../../docs/specs/architecture/decisions/ADR-30.md
/// [ADR-61]: ../../../docs/specs/architecture/decisions/ADR-61.md
/// [CR-022]: ../../../docs/requests/CR-022-acyclicity-self-recursion-exclusion.md
/// [CR-087]: ../../../docs/requests/CR-087-acyclicity-cross-module-cycle-boundary.md
fn acyclicity(sccs: &[Vec<NodeIndex>], dirs: &[String]) -> MetricValue {
    let cycles = cross_module_cycles(sccs, dirs).count() as u64;
    MetricValue {
        raw: cycles as f64,
        normalized: 1.0 / (1.0 + cycles as f64),
    }
}

/// The SCCs [`acyclicity`] counts as cycles: more than one member, spanning more
/// than one directory ([ADR-61]). The one filter both the count and the
/// Acyclicity offender list ([`worst_offenders`], [CR-209]) read, so the list
/// names exactly the cycles the dimension scores.
///
/// [ADR-61]: ../../../docs/specs/architecture/decisions/ADR-61.md
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
fn cross_module_cycles<'a>(
    sccs: &'a [Vec<NodeIndex>],
    dirs: &'a [String],
) -> impl Iterator<Item = &'a Vec<NodeIndex>> {
    sccs.iter().filter(|scc| scc.len() > 1).filter(|scc| {
        // Spans >1 directory ⇔ some member's directory differs from the first
        // member's. Order-independent, allocation-free, deterministic.
        let first = dirs[scc[0].index()].as_str();
        scc.iter().any(|&vertex| dirs[vertex.index()].as_str() != first)
    })
}

/// Depth — longest path (in vertices) over the SCC condensation of the
/// **module-rollup graph** ([FR-QM-03], [ADR-62], [CR-088], metric-semantics v5).
///
/// Depth measures *architectural layering*, not intra-file call length: the
/// symbol graph is first rolled up to one vertex per directory — the same
/// `directory_of` partition [`modularity`] and [`acyclicity`] read, threaded in
/// as `dirs` — with cross-directory dependencies as the inter-module edges. The
/// longest path over that module graph's SCC condensation is the depth:
///
/// - a long call chain confined to one directory rolls up to a single module
///   vertex and scores depth 1 (the intra-file recursion CR-088 stops reporting
///   as layering);
/// - a dependency cycle among directories collapses to one condensed layer and
///   also scores 1;
/// - a linear chain of `L` directories scores `L`.
///
/// `tarjan_scc` returns components in reverse topological order, so a single
/// forward sweep over the condensed successor sets is the longest-path DP
/// ([`longest_condensed_path`]). All arithmetic is integral; only the final
/// normalization divides in f64.
///
/// [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
/// [ADR-62]: ../../../docs/specs/architecture/decisions/ADR-62.md
/// [CR-088]: ../../../docs/requests/CR-088-depth-module-granularity.md
fn depth(graph: &DiGraph<(), ()>, dirs: &[String]) -> MetricValue {
    let modules = module_rollup(graph, dirs);
    let sccs = tarjan_scc(&modules.graph);
    let depth = longest_condensed_path(&modules.graph, &sccs);
    MetricValue {
        raw: depth as f64,
        normalized: 1.0 / (1.0 + depth as f64 / 8.0),
    }
}

/// Roll the symbol-level metric graph up to its **module (directory) graph**:
/// one vertex per distinct directory in `dirs`, one edge per *cross-directory*
/// dependency ([ADR-62], [CR-088]). Intra-directory edges would be module
/// self-loops that cannot lengthen any path, so they are dropped; parallel
/// inter-module edges are deduplicated (the longest path is unchanged by them,
/// and the compact graph keeps the condensation bounded by the directory-pair
/// count rather than the symbol-edge count).
///
/// `dirs` is indexed by `graph` vertex index (as produced by [`metric_graph`]),
/// the same partition [`modularity`] reads — so Depth and Modularity share one
/// rollup by construction. Module vertices are created in first-appearance order
/// over the deterministic vertex order, so the rebuilt graph is reproducible
/// ([NFR-RA-06]). Each module vertex keeps its directory, which is what a Depth
/// offender row spells ([CR-209]).
///
/// [ADR-62]: ../../../docs/specs/architecture/decisions/ADR-62.md
/// [CR-088]: ../../../docs/requests/CR-088-depth-module-granularity.md
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn module_rollup<'a>(graph: &DiGraph<(), ()>, dirs: &'a [String]) -> ModuleGraph<'a> {
    let mut module_of: HashMap<&str, NodeIndex> = HashMap::with_capacity(dirs.len());
    let mut modules = DiGraph::<(), ()>::new();
    let mut module_dirs: Vec<&str> = Vec::new();
    for dir in dirs {
        module_of.entry(dir.as_str()).or_insert_with(|| {
            module_dirs.push(dir.as_str());
            modules.add_node(())
        });
    }

    let mut seen: HashSet<(NodeIndex, NodeIndex)> = HashSet::new();
    for edge in graph.edge_references() {
        let src = dirs[edge.source().index()].as_str();
        let dst = dirs[edge.target().index()].as_str();
        if src == dst {
            continue; // intra-directory edge — a module self-loop, no effect on depth
        }
        let (src, dst) = (module_of[src], module_of[dst]);
        if seen.insert((src, dst)) {
            modules.add_edge(src, dst, ());
        }
    }
    ModuleGraph {
        graph: modules,
        dirs: module_dirs,
    }
}

/// The module (directory) graph [`module_rollup`] builds, with each module
/// vertex's directory indexed by vertex index.
struct ModuleGraph<'a> {
    graph: DiGraph<(), ()>,
    dirs: Vec<&'a str>,
}

/// Longest path (in condensed vertices) over the SCC condensation of `graph`,
/// given its [`tarjan_scc`] component set. Each SCC collapses to one vertex, so a
/// whole cycle contributes a single layer. `tarjan_scc` yields components in
/// reverse topological order — every condensed successor of component `i` sits
/// at an index `< i` — so one forward sweep over the condensed successor sets is
/// the longest-path DP ([FR-QM-03]); the [`BTreeSet`] successor sets keep the
/// reduction deterministic ([ADR-08]).
///
/// [FR-QM-03]: ../../../docs/specs/requirements/FR-QM-03.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
fn longest_condensed_path(graph: &DiGraph<(), ()>, sccs: &[Vec<NodeIndex>]) -> u64 {
    condense(graph, sccs)
        .longest
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
}

/// The SCC condensation of a graph and its longest-path DP — the one
/// computation both the Depth value ([`longest_condensed_path`]) and the Depth
/// offender chains ([`depth_chains`]) read, so a listed chain is never longer
/// or shorter than the depth the dimension scores.
struct Condensation {
    /// Each component's condensed successors (deduplicated, deterministic).
    successors: Vec<BTreeSet<usize>>,
    /// The longest path, in condensed vertices, that starts at each component.
    longest: Vec<u64>,
}

/// Condense `graph` by its [`tarjan_scc`] component set `sccs` and run the
/// longest-path DP over it (see [`longest_condensed_path`] for why one forward
/// sweep suffices).
fn condense(graph: &DiGraph<(), ()>, sccs: &[Vec<NodeIndex>]) -> Condensation {
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
    Condensation {
        successors,
        longest,
    }
}

/// The Depth offender rows ([CR-209]): every chain of two or more layers in the
/// module-rollup condensation [`depth`] measures, one per **chain head** (a
/// layer nothing else depends on, so no listed chain is the tail of another),
/// as `(head, "dir1 → dir2 → … (L directories)")`.
///
/// Each head spells its longest chain, so the first row is the chain the Depth
/// value counts. Ranked by length descending, then head label ascending; at a
/// fork the chain follows the longer continuation, then the lower label. A
/// label is unique per layer, so both orders are total ([NFR-RA-06]). A layer
/// that is a directory cycle reads `{a, b}` and the count names layers rather
/// than directories ([`layer_label`]).
///
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn depth_chains(graph: &DiGraph<(), ()>, dirs: &[String], cap: usize) -> Vec<(String, String)> {
    let modules = module_rollup(graph, dirs);
    let sccs = tarjan_scc(&modules.graph);
    let Condensation {
        successors,
        longest,
    } = condense(&modules.graph, &sccs);
    let labels: Vec<String> = sccs
        .iter()
        .map(|members| {
            let dirs: BTreeSet<&str> = members.iter().map(|v| modules.dirs[v.index()]).collect();
            layer_label(&dirs)
        })
        .collect();

    let mut is_head = vec![true; sccs.len()];
    for &successor in successors.iter().flatten() {
        is_head[successor] = false;
    }
    let mut heads: Vec<usize> = (0..sccs.len())
        .filter(|&c| is_head[c] && longest[c] >= 2)
        .collect();
    let rank = |a: &usize, b: &usize| {
        longest[*b]
            .cmp(&longest[*a])
            .then(labels[*a].cmp(&labels[*b]))
    };
    heads.sort_by(rank);

    heads
        .into_iter()
        .take(cap)
        .map(|head| {
            let (mut chain, mut at) = (vec![head], head);
            while let Some(&next) = successors[at].iter().min_by(|a, b| rank(a, b)) {
                chain.push(next);
                at = next;
            }
            let unit = if chain.iter().all(|&c| sccs[c].len() == 1) {
                "directories"
            } else {
                "layers"
            };
            let spelled: Vec<&str> = chain.iter().map(|&c| labels[c].as_str()).collect();
            let detail = format!("{} ({} {unit})", spelled.join(" → "), chain.len());
            (labels[head].clone(), detail)
        })
        .collect()
}

/// How many directories an offender row names before it counts the rest: a
/// directory cycle on a real repository spans dozens, which would make a row
/// hundreds of characters long ([CR-209] §7).
///
/// [CR-209]: ../../../docs/requests/CR-209-health-records-the-worst-items-of-four-more-dimensions.md
const NAMED_DIRECTORIES: usize = 3;

/// A condensed layer as an offender row spells it: its one directory, or a
/// directory cycle's directories in braces — sorted, the first
/// [`NAMED_DIRECTORIES`] named and the rest counted (`{a, b, c +4 more}`). The
/// first name is unique to its layer, so the label is too.
fn layer_label(dirs: &BTreeSet<&str>) -> String {
    match dirs.len() {
        1 => named_directories(dirs),
        n if n <= NAMED_DIRECTORIES => format!("{{{}}}", named_directories(dirs)),
        n => format!(
            "{{{} +{} more}}",
            named_directories(dirs),
            n - NAMED_DIRECTORIES
        ),
    }
}

/// The first [`NAMED_DIRECTORIES`] of `dirs`, in order, comma-joined.
fn named_directories(dirs: &BTreeSet<&str>) -> String {
    let names: Vec<&str> = dirs
        .iter()
        .take(NAMED_DIRECTORIES)
        .map(|dir| directory_label(dir))
        .collect();
    names.join(", ")
}

/// A directory as an offender row names it: the project root reads `.` rather
/// than an empty name.
fn directory_label(dir: &str) -> &str {
    if dir.is_empty() {
        "."
    } else {
        dir
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

/// Whether a function counts as redundant: dead **or** duplicate, on a
/// `Some(true)` verdict only — a `NULL` verdict (a language with no
/// reachability verdict) is never redundant. The one predicate [`redundancy`]
/// counts and the Redundancy offender list ([`worst_offenders`]) lists, so the
/// list names exactly what the dimension scores.
fn is_redundant(f: &FunctionMetricRow) -> bool {
    f.is_dead == Some(true) || f.is_duplicate == Some(true)
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
    let redundant = functions.iter().filter(|f| is_redundant(f)).count();
    let ratio = redundant as f64 / total as f64;
    MetricValue {
        raw: ratio,
        normalized: 1.0 - ratio,
    }
}

/// The applicable-dimension geometric-mean aggregate, rounded to the 0–10000
/// integer signal (metric-semantics v6, [FR-QM-06], [FR-QM-14], [ADR-12],
/// [ADR-21], [ADR-08]).
///
/// `original` are the **applicable** original metrics in canonical order
/// (Modularity omitted when not applicable, [CR-156] —
/// [`applicable_original_dimensions`]); `new_dims` are the **applicable** new
/// dimensions in canonical order (Cohesion/Focus omitted when they dropped out,
/// [ADR-21]). The reduction:
///
/// 1. **Zero short-circuit (applicable original metrics only, [ADR-21]).** If
///    any applicable *original* metric is `0.0`, the signal is `0` — a hard zero
///    in a systemic-pathology metric collapses the score *and* keeps
///    `ln(0) = −∞` out of the reduction. A not-applicable Modularity is not in
///    `original`, so it cannot trigger this ([CR-156]). The new five are floored
///    at [`extended::DIMENSION_FLOOR`], never `0`, so they never trigger it —
///    they drag but never alone collapse the signal.
/// 2. **Geometric mean over the applicable dimensions.** Sum `ln nᵢ` over the
///    applicable original metrics then the applicable new dims, in canonical
///    order, and divide by the *count of applicable dimensions* (10, or fewer as
///    Modularity, Cohesion and Focus drop out) — a deterministic, honest
///    n-dimension mean ([FR-QM-14], [UAT-QM-10]).
///
/// The empty-graph sentinel is handled by the caller ([ADR-12]); this function
/// is only reached for a non-empty production graph.
///
/// [FR-QM-06]: ../../../docs/specs/requirements/FR-QM-06.md
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
/// [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md
/// [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
fn aggregate(original: &[f64], new_dims: &[f64]) -> u32 {
    // Zero short-circuit scoped to the applicable original metrics (ADR-21,
    // CR-156): a floored new dimension can never be 0, and a not-applicable
    // Modularity is not in `original`, so only an applicable original hard zero
    // collapses here.
    if original.contains(&0.0) {
        return 0;
    }
    // Log-space sum in canonical order: applicable originals, then the
    // applicable new dimensions. The denominator is the number of dimensions
    // that actually entered (applicability drop-out, FR-QM-14).
    let ln_sum: f64 = original.iter().chain(new_dims.iter()).map(|n| n.ln()).sum();
    let count = (original.len() + new_dims.len()) as f64;
    let signal = ((ln_sum / count).exp() * 10_000.0).round();
    // exp of a non-positive mean never exceeds 1.0 (and the floored inputs keep it
    // well above 0), but clamp both ends defensively so the persisted CHECK
    // (0..=10000) can never fire on float residue.
    signal.clamp(0.0, 10_000.0) as u32
}
