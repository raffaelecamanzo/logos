//! Unit tests for the quality metrics engine (S-018, [FR-QM-01]..[FR-QM-06],
//! [UAT-QM-01]..[UAT-QM-06], [ADR-08], [ADR-12]).
//!
//! All tests here are pure — hand-built [`NodeRow`]/[`EdgeRow`] snapshots fed
//! through [`build_view`] into [`compute`], no I/O. The persistence half
//! ([FR-QM-07]) is exercised end-to-end in `tests/metrics.rs`.

use std::collections::HashSet;

use crate::graph_store::{EdgeRow, FunctionMetricRow, NodeRow};
use crate::hydrate::{build_view, Granularity};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};

use super::compute;

// ── snapshot builders (the hydrate test conventions) ─────────────────────────

/// A node row with a synthetic local symbol and an optional defining file.
fn node(id: i64, name: &str, kind: NodeKind, file: Option<&str>) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("local symbol parses"),
        kind,
        name: name.to_string(),
        file_path: file.map(str::to_string),
        start_line: None,
        end_line: None,
    }
}

fn edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

/// A function metric row with complexity and verdicts; the CR-005 structural
/// inputs (line count, nesting depth, clone group) default to `None`/absent. Use
/// [`func_struct`] to set them for the new dimensions.
fn func(id: i64, cc: Option<i64>, dead: Option<bool>, dup: Option<bool>) -> FunctionMetricRow {
    FunctionMetricRow {
        id: NodeId(id),
        cyclomatic_complexity: cc,
        is_dead: dead,
        is_duplicate: dup,
        line_count: None,
        max_nesting_depth: None,
        clone_group: None,
    }
}

/// A function metric row carrying the CR-005 structural inputs the new
/// dimensions read: physical line count, maximum nesting depth, and near-clone
/// group membership ([FR-QM-09], [FR-QM-10], [FR-QM-13]).
fn func_struct(
    id: i64,
    cc: Option<i64>,
    line_count: Option<i64>,
    max_nesting_depth: Option<i64>,
    clone_group: Option<i64>,
) -> FunctionMetricRow {
    FunctionMetricRow {
        id: NodeId(id),
        cyclomatic_complexity: cc,
        is_dead: Some(false),
        is_duplicate: Some(false),
        line_count,
        max_nesting_depth,
        clone_group: clone_group.map(NodeId),
    }
}

/// Compute over a node/edge snapshot through the real `ExcludeContains` view,
/// with no test scope (the production-scope path is exercised by [`run_scoped`]).
fn run(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
) -> crate::models::quality::MetricSnapshot {
    run_scoped(nodes, edges, functions, &[])
}

/// Compute with an explicit `is_test` node set excluded from the production
/// scope ([FR-QM-08]).
///
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
fn run_scoped(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    functions: &[FunctionMetricRow],
    test_ids: &[i64],
) -> crate::models::quality::MetricSnapshot {
    let view = build_view(Granularity::ExcludeContains, nodes, edges);
    let test_ids: HashSet<NodeId> = test_ids.iter().map(|&id| NodeId(id)).collect();
    // The full edge set (Contains/Accesses/Calls) feeds the CR-005 Cohesion/Focus
    // dimensions; the ExcludeContains view feeds the original five.
    compute(
        &view,
        nodes,
        edges,
        functions,
        &test_ids,
        super::Thresholds::default(),
    )
}

// ── FR-QM-01 / UAT-QM-01: modularity distinguishes modular from tangled ──────

/// Two directories, all calls intra-directory → high Q; the same vertices with
/// every call crossing directories → low Q. And the modular fixture's Q must
/// not be ≈ 0 — intra-directory edges are retained as community-internal (the
/// SRS §7.4 self-loop trap).
#[test]
fn modularity_separates_modular_from_tangled() {
    let nodes = [
        node(1, "a1", NodeKind::Function, Some("alpha/a1.rs")),
        node(2, "a2", NodeKind::Function, Some("alpha/a2.rs")),
        node(3, "b1", NodeKind::Function, Some("beta/b1.rs")),
        node(4, "b2", NodeKind::Function, Some("beta/b2.rs")),
    ];
    let modular_edges = [
        edge(1, 2, EdgeKind::Calls), // alpha-internal
        edge(2, 1, EdgeKind::Calls), // alpha-internal
        edge(3, 4, EdgeKind::Calls), // beta-internal
        edge(4, 3, EdgeKind::Calls), // beta-internal
    ];
    let tangled_edges = [
        edge(1, 3, EdgeKind::Calls), // every edge crosses
        edge(3, 2, EdgeKind::Calls),
        edge(2, 4, EdgeKind::Calls),
        edge(4, 1, EdgeKind::Calls),
    ];

    let modular = run(&nodes, &modular_edges, &[]);
    let tangled = run(&nodes, &tangled_edges, &[]);

    assert!(
        modular.modularity.raw > 0.4,
        "intra-directory edges must be retained — Q ≈ 0 means the self-loop \
         trap fired (FR-QM-01), got {}",
        modular.modularity.raw
    );
    assert!(
        modular.modularity.normalized > tangled.modularity.normalized,
        "modular ({}) must outscore tangled ({}) (UAT-QM-01)",
        modular.modularity.normalized,
        tangled.modularity.normalized
    );
    // Fully tangled two-community graph: e_c = 0 for both → Q = −Σ(d/2m)² < 0.
    assert!(
        tangled.modularity.raw < 0.0,
        "a fully crossing graph has negative Q, got {}",
        tangled.modularity.raw
    );
}

/// `m == 0` → Q = 0 → normalized exactly 1/3 (FR-QM-01).
#[test]
fn modularity_with_no_edges_is_one_third() {
    let nodes = [node(1, "a", NodeKind::Function, Some("src/a.rs"))];
    let snap = run(&nodes, &[], &[]);
    assert_eq!(snap.modularity.raw, 0.0);
    assert_eq!(snap.modularity.normalized, 0.5 / 1.5);
}

/// A vertex bound to no file partitions into the `<unbound>` community rather
/// than being dropped (every vertex gets exactly one community).
#[test]
fn modularity_assigns_unbound_nodes_a_community() {
    let nodes = [
        node(1, "a", NodeKind::Function, None),
        node(2, "b", NodeKind::Function, None),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];
    let snap = run(&nodes, &edges, &[]);
    // One community holding the single edge: Q = 1/1 − (2/2)² = 0.
    assert_eq!(snap.modularity.raw, 0.0);
}

// ── FR-QM-02 / UAT-QM-02: acyclicity counts SCCs ─────────────────────────────

/// One two-vertex cycle → cycles = 1 → normalized exactly 1/2 (UAT-QM-02).
#[test]
fn one_cycle_scores_acyclicity_one_half() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls), edge(2, 1, EdgeKind::Calls)];
    let snap = run(&nodes, &edges, &[]);
    assert_eq!(snap.acyclicity.raw, 1.0, "one SCC of len 2 is one cycle");
    assert_eq!(snap.acyclicity.normalized, 0.5, "1/(1+1) (FR-QM-02)");
}

/// Self-recursion is **not** a dependency cycle: a singleton self-loop counts
/// 0, only multi-node SCCs count, and a self-loop *inside* a multi-node SCC is
/// counted once (the SCC is the unit). Metric-semantics v4 ([FR-QM-02],
/// [ADR-30], CR-022); a DAG still scores 1.0.
///
/// [ADR-30]: ../../../docs/specs/architecture/decisions/ADR-30.md
#[test]
fn self_recursion_excluded_only_multi_node_sccs_count() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
    ];

    // Case 1 — a singleton self-loop (self-recursion) plus a forward edge: the
    // self-loop's SCC is `{a}` (len 1), so it contributes 0 → the graph is a DAG.
    let with_self_loop = [edge(1, 1, EdgeKind::Calls), edge(1, 2, EdgeKind::Calls)];
    let snap = run(&nodes, &with_self_loop, &[]);
    assert_eq!(
        snap.acyclicity.raw, 0.0,
        "self-recursion is not a cycle (multi-node SCCs only, FR-QM-02 v4)"
    );
    assert_eq!(snap.acyclicity.normalized, 1.0);

    // Case 2 — a 2-node mutual-recursion SCC `{a, b}` contributes exactly 1.
    let mutual = [edge(1, 2, EdgeKind::Calls), edge(2, 1, EdgeKind::Calls)];
    let snap = run(&nodes, &mutual, &[]);
    assert_eq!(snap.acyclicity.raw, 1.0, "one multi-node SCC is one cycle");
    assert_eq!(snap.acyclicity.normalized, 0.5);

    // Case 3 — a self-loop *inside* a multi-node SCC is counted once, not twice:
    // `{a, b}` is mutual-recursive and `a` also self-recurses → still raw 1.
    let mutual_with_inner_self_loop = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 1, EdgeKind::Calls),
        edge(1, 1, EdgeKind::Calls),
    ];
    let snap = run(&nodes, &mutual_with_inner_self_loop, &[]);
    assert_eq!(
        snap.acyclicity.raw, 1.0,
        "a self-loop inside a multi-node SCC is counted once (the SCC is the unit)"
    );

    // A DAG still scores a perfect 1.0.
    let dag = [edge(1, 2, EdgeKind::Calls)];
    let snap = run(&nodes, &dag, &[]);
    assert_eq!(snap.acyclicity.raw, 0.0);
    assert_eq!(snap.acyclicity.normalized, 1.0, "a DAG has no cycles");
}

// ── FR-QM-03 / UAT-QM-03: depth over the condensation ────────────────────────

/// A linear chain of 3 vertices scores depth 3 → 1/(1+3/8) = 8/11.
#[test]
fn linear_chain_depth_is_its_vertex_count() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
        node(3, "c", NodeKind::Function, Some("src/c.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls), edge(2, 3, EdgeKind::Calls)];
    let snap = run(&nodes, &edges, &[]);
    assert_eq!(snap.depth.raw, 3.0, "a → b → c is three layers");
    assert_eq!(snap.depth.normalized, 1.0 / (1.0 + 3.0 / 8.0));
}

/// A pure cycle condenses to a single vertex → depth 1 (UAT-QM-03: the tangle
/// collapses to one layer, it does not fake depth).
#[test]
fn a_pure_cycle_collapses_to_depth_one() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
        node(3, "c", NodeKind::Function, Some("src/c.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 3, EdgeKind::Calls),
        edge(3, 1, EdgeKind::Calls),
    ];
    let snap = run(&nodes, &edges, &[]);
    assert_eq!(
        snap.depth.raw, 1.0,
        "the whole cycle is one condensed layer"
    );
}

/// A chain *through* a cycle: d → (a↔b) → c condenses to 3 layers.
#[test]
fn depth_chains_through_a_condensed_cycle() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
        node(3, "c", NodeKind::Function, Some("src/c.rs")),
        node(4, "d", NodeKind::Function, Some("src/d.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls), // a ↔ b: one SCC
        edge(2, 1, EdgeKind::Calls),
        edge(4, 1, EdgeKind::Calls), // d → SCC
        edge(2, 3, EdgeKind::Calls), // SCC → c
    ];
    let snap = run(&nodes, &edges, &[]);
    assert_eq!(
        snap.depth.raw, 3.0,
        "d → (ab) → c is three condensed layers"
    );
}

// ── FR-QM-04 / UAT-QM-04: equality tracks complexity concentration ──────────

/// One god-function among trivial ones scores lower equality than an even
/// spread (UAT-QM-04).
#[test]
fn concentrated_complexity_scores_lower_equality_than_even() {
    let concentrated = [
        func(1, Some(1), None, None),
        func(2, Some(1), None, None),
        func(3, Some(1), None, None),
        func(4, Some(37), None, None), // the god function
    ];
    let even = [
        func(1, Some(10), None, None),
        func(2, Some(10), None, None),
        func(3, Some(10), None, None),
        func(4, Some(10), None, None),
    ];
    let concentrated = run(&[], &[], &concentrated);
    let even = run(&[], &[], &even);

    assert!(
        concentrated.equality.normalized < even.equality.normalized,
        "concentration must lower equality: {} vs {}",
        concentrated.equality.normalized,
        even.equality.normalized
    );
    assert_eq!(
        even.equality.normalized, 1.0,
        "a perfectly even distribution has Gini 0"
    );
}

/// The FR-QM-04 guards: `n == 1`, `n == 0`, and `Σx == 0` all score 1.0.
#[test]
fn equality_guards_score_one() {
    let single = run(&[], &[], &[func(1, Some(9), None, None)]);
    assert_eq!(single.equality.normalized, 1.0, "n == 1 (UAT-QM-04 step 2)");

    let none = run(&[], &[], &[]);
    assert_eq!(none.equality.normalized, 1.0, "n == 0");

    let zeros = [func(1, Some(0), None, None), func(2, Some(0), None, None)];
    let zeros = run(&[], &[], &zeros);
    assert_eq!(zeros.equality.normalized, 1.0, "Σx == 0");
}

/// Functions with `NULL` complexity are excluded from the Gini, not coerced
/// to a phantom 0 (NFR-CC-04 honesty).
#[test]
fn null_complexity_is_excluded_not_zeroed() {
    // One real value + one NULL: with the NULL excluded this is the n == 1
    // guard (1.0); a coerced 0 would make it the maximally unequal pair.
    let rows = [func(1, Some(20), None, None), func(2, None, None, None)];
    let snap = run(&[], &[], &rows);
    assert_eq!(snap.equality.normalized, 1.0);
}

// ── FR-QM-05 / UAT-QM-05: redundancy counts a function once ──────────────────

/// A function that is BOTH dead and duplicate is counted once (UAT-QM-05).
#[test]
fn dead_and_duplicate_function_counts_once() {
    let rows = [
        func(1, Some(1), Some(true), Some(true)), // both — one redundant fn
        func(2, Some(1), Some(false), Some(false)),
        func(3, Some(1), Some(false), Some(false)),
        func(4, Some(1), Some(false), Some(false)),
    ];
    let snap = run(&[], &[], &rows);
    assert_eq!(
        snap.redundancy.raw, 0.25,
        "1 redundant of 4, not 2 of 4 (FR-QM-05 DISTINCT)"
    );
    assert_eq!(snap.redundancy.normalized, 0.75);
}

/// `total == 0` → 1.0; a clean repo scores 1.0; unannotated (`NULL`) verdicts
/// are not redundant.
#[test]
fn redundancy_guards_and_null_verdicts() {
    let none = run(&[], &[], &[]);
    assert_eq!(none.redundancy.normalized, 1.0, "total == 0 (FR-QM-05)");

    let clean = [func(1, Some(1), Some(false), Some(false))];
    assert_eq!(run(&[], &[], &clean).redundancy.normalized, 1.0);

    let unannotated = [func(1, Some(1), None, None)];
    assert_eq!(
        run(&[], &[], &unannotated).redundancy.normalized,
        1.0,
        "NULL verdicts mean 'not annotated', never 'redundant' (NFR-CC-04)"
    );
}

// ── FR-QM-06 / UAT-QM-06: aggregate guards ───────────────────────────────────

/// Any metric at exactly 0 collapses the signal to 0 (the zero short-circuit;
/// UAT-QM-06 step 2). All-dead functions drive redundancy to 0.0.
#[test]
fn zero_metric_short_circuits_the_signal_to_zero() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("src/a.rs")),
        node(2, "b", NodeKind::Function, Some("src/b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];
    let functions = [
        func(1, Some(1), Some(true), Some(false)),
        func(2, Some(1), Some(true), Some(false)),
    ];
    let snap = run(&nodes, &edges, &functions);
    assert_eq!(snap.redundancy.normalized, 0.0, "every function is dead");
    assert_eq!(
        snap.aggregate_signal,
        Some(0),
        "a hard zero collapses the signal (FR-QM-06, ADR-12)"
    );
    assert!(!snap.empty, "a zero signal is not the empty sentinel");
}

/// An empty graph returns the "n/a" sentinel — `empty = true`, signal `None` —
/// never the misleading ~8033 the guard values would average to (UAT-QM-06).
#[test]
fn empty_graph_is_na_not_8033() {
    let snap = run(&[], &[], &[]);
    assert!(snap.empty, "node_count == 0 sets the empty flag (ADR-12)");
    assert_eq!(
        snap.aggregate_signal, None,
        "an empty graph is n/a, never a number (FR-QM-06, NFR-CC-04)"
    );
    assert_eq!(snap.node_count, 0);
    assert_eq!(snap.function_count, 0);
}

/// A healthy graph scores in (0, 10000]; a perfect one would hit 10000.
#[test]
fn aggregate_signal_is_bounded() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("alpha/a.rs")),
        node(2, "b", NodeKind::Function, Some("alpha/b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];
    let functions = [
        func(1, Some(2), Some(false), Some(false)),
        func(2, Some(2), Some(false), Some(false)),
    ];
    let snap = run(&nodes, &edges, &functions);
    let signal = snap.aggregate_signal.expect("non-empty graph has a signal");
    assert!(signal > 0, "no metric is zero here");
    assert!(signal <= 10_000, "the signal is capped at 10000 (ADR-08)");
}

// ── NFR-RA-06 / ADR-08: determinism ──────────────────────────────────────────

/// The same input computed twice yields the identical snapshot, and the
/// rounded signal of a fixed fixture is pinned to an exact golden value —
/// byte-identical across runs (NFR-RA-06; the cross-target half is the S-025
/// CI matrix).
#[test]
fn identical_input_yields_an_identical_pinned_signal() {
    let nodes = [
        node(1, "a1", NodeKind::Function, Some("alpha/a.rs")),
        node(2, "a2", NodeKind::Function, Some("alpha/b.rs")),
        node(3, "b1", NodeKind::Function, Some("beta/c.rs")),
        node(4, "b2", NodeKind::Method, Some("beta/d.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 1, EdgeKind::Calls), // one alpha-internal cycle
        edge(3, 4, EdgeKind::Calls),
        edge(1, 3, EdgeKind::Imports), // one crossing
    ];
    let functions = [
        func(1, Some(3), Some(false), Some(false)),
        func(2, Some(5), Some(false), Some(true)), // one duplicate
        func(3, Some(2), Some(false), Some(false)),
        func(4, Some(2), Some(false), Some(false)),
    ];

    let first = run(&nodes, &edges, &functions);
    let second = run(&nodes, &edges, &functions);
    assert_eq!(first.aggregate_signal, second.aggregate_signal);
    assert_eq!(
        first.modularity.raw.to_bits(),
        second.modularity.raw.to_bits()
    );

    // The golden pin, hand-derived under metric-semantics v3 (CR-005, ADR-21):
    // the original five — modularity Q = 0.21875 (norm 23/48), cycles = 1
    // (norm 1/2), depth = 3 (norm 8/11), Gini(2,2,3,5) = 10/48 (norm 38/48),
    // redundancy ratio 1/4 (norm 3/4) — plus three always-applicable new
    // dimensions that are all a clean 1.0 here (no deep nesting, no brain
    // methods, no near-clones; depth/line/clone inputs all absent). Cohesion and
    // Focus drop out (the fixture has no Class/Struct container, ADR-21), so the
    // mean spans EIGHT dimensions:
    //   exp(ln(23/48·1/2·8/11·38/48·3/4·1·1·1)/8)·10000 ≈ 7531.
    // Recomputing this exact integer on every target is the ADR-08 fitness
    // function; a change here is a determinism regression, not test churn. The
    // value rose from the v2 five-dimension pin (6353) because the three 1.0
    // dimensions raise the geometric mean — a metric-semantics bump, not a
    // regression (the gate auto-re-baselines, FR-GV-10).
    assert_eq!(first.aggregate_signal, Some(7531));
    // The applicable-dimension provenance: Cohesion/Focus are n/a here.
    assert!(
        first.cohesion.is_none(),
        "no classes → Cohesion drops out (ADR-21)"
    );
    assert!(
        first.focus.is_none(),
        "no class-like containers → Focus drops out"
    );
    assert_eq!(first.nesting.normalized, 1.0);
    assert_eq!(first.uniqueness.normalized, 1.0);
}

// ── Derived-artifact hygiene ─────────────────────────────────────────────────

/// Derived policy vertices (`Layer`/`Boundary`) and `ForbiddenDependency`
/// edges are governance flags, not dependencies — adding them must not move
/// any metric or the signal.
#[test]
fn derived_policy_artifacts_do_not_move_the_signal() {
    let base_nodes = [
        node(1, "a", NodeKind::Function, Some("alpha/a.rs")),
        node(2, "b", NodeKind::Function, Some("beta/b.rs")),
    ];
    let base_edges = [edge(1, 2, EdgeKind::Calls)];
    let functions = [
        func(1, Some(2), Some(false), Some(false)),
        func(2, Some(3), Some(false), Some(false)),
    ];
    let base = run(&base_nodes, &base_edges, &functions);

    let with_policy_nodes = [
        node(1, "a", NodeKind::Function, Some("alpha/a.rs")),
        node(2, "b", NodeKind::Function, Some("beta/b.rs")),
        node(3, "core", NodeKind::Layer, None),
        node(4, "no-up", NodeKind::Boundary, None),
    ];
    let with_policy_edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(1, 2, EdgeKind::ForbiddenDependency), // the governance flag
    ];
    let flagged = run(&with_policy_nodes, &with_policy_edges, &functions);

    assert_eq!(
        base.aggregate_signal, flagged.aggregate_signal,
        "annotation-materialised artifacts must not change the score"
    );
    assert_eq!(base.node_count, flagged.node_count);
    assert_eq!(base.edge_count, flagged.edge_count);
}

// ── FR-QM-08 / UAT-QM-07: the production scope ───────────────────────────────

/// Adding structurally identical test functions that call the deepest
/// production function leaves every normalized metric, every raw value, and the
/// aggregate signal byte-identical — the production scope excludes them from the
/// graph (Modularity/Acyclicity/Depth), the Gini (Equality), and the redundant
/// ratio (Redundancy). The excluded count is reported, going 0 → 2 (the
/// 2026-06-07 manual-test scenario, inverted; FR-QM-08, UAT-QM-07).
#[test]
fn adding_test_functions_leaves_every_metric_byte_identical() {
    // Production: a depth-3 chain with varied complexity, no redundancy.
    let prod_nodes = [
        node(1, "alpha", NodeKind::Function, Some("src/alpha.rs")),
        node(2, "beta", NodeKind::Function, Some("src/beta.rs")),
        node(3, "deepest", NodeKind::Function, Some("src/deep.rs")),
    ];
    let prod_edges = [edge(1, 2, EdgeKind::Calls), edge(2, 3, EdgeKind::Calls)];
    let prod_funcs = [
        func(1, Some(3), Some(false), Some(false)),
        func(2, Some(5), Some(false), Some(false)),
        func(3, Some(2), Some(false), Some(false)),
    ];
    let base = run(&prod_nodes, &prod_edges, &prod_funcs);

    // Add two structurally identical tests (10, 11): each calls the deepest
    // production function (would extend Depth) and is a duplicate of the other
    // with trivial complexity (would lower Redundancy and skew Equality) — every
    // way a test could move the signal under the old test-inclusive scope.
    let scoped_nodes = [
        node(1, "alpha", NodeKind::Function, Some("src/alpha.rs")),
        node(2, "beta", NodeKind::Function, Some("src/beta.rs")),
        node(3, "deepest", NodeKind::Function, Some("src/deep.rs")),
        node(10, "test_one", NodeKind::Function, Some("tests/it.rs")),
        node(11, "test_two", NodeKind::Function, Some("tests/it.rs")),
    ];
    let scoped_edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 3, EdgeKind::Calls),
        edge(10, 3, EdgeKind::Calls), // test → deepest production fn
        edge(11, 3, EdgeKind::Calls),
    ];
    let scoped_funcs = [
        func(1, Some(3), Some(false), Some(false)),
        func(2, Some(5), Some(false), Some(false)),
        func(3, Some(2), Some(false), Some(false)),
        func(10, Some(1), Some(false), Some(true)), // duplicate pair
        func(11, Some(1), Some(false), Some(true)),
    ];
    let scoped = run_scoped(&scoped_nodes, &scoped_edges, &scoped_funcs, &[10, 11]);

    // The aggregate and every per-metric value are byte-identical (UAT-QM-07).
    assert_eq!(
        base.aggregate_signal, scoped.aggregate_signal,
        "adding tests must not move the aggregate signal (FR-QM-08)"
    );
    for (name, b, s) in [
        ("modularity", &base.modularity, &scoped.modularity),
        ("acyclicity", &base.acyclicity, &scoped.acyclicity),
        ("depth", &base.depth, &scoped.depth),
        ("equality", &base.equality, &scoped.equality),
        ("redundancy", &base.redundancy, &scoped.redundancy),
    ] {
        assert_eq!(
            b.raw.to_bits(),
            s.raw.to_bits(),
            "{name} raw must be byte-identical with tests excluded"
        );
        assert_eq!(
            b.normalized.to_bits(),
            s.normalized.to_bits(),
            "{name} normalized must be byte-identical with tests excluded"
        );
    }

    // The graph and function counts are the production scope, unchanged.
    assert_eq!(base.node_count, scoped.node_count, "test vertices dropped");
    assert_eq!(
        base.edge_count, scoped.edge_count,
        "edges incident to test vertices dropped with them"
    );
    assert_eq!(
        base.function_count, scoped.function_count,
        "Equality/Redundancy denominators are production-only"
    );

    // The excluded count is reported, going 0 → 2 (NFR-CC-04 transparency).
    assert_eq!(base.test_function_count, 0, "no tests in the baseline");
    assert_eq!(
        scoped.test_function_count, 2,
        "both tests excluded and counted"
    );
}

/// A dead or duplicate test function is excluded from the Redundancy numerator
/// *and* denominator: a project whose only "redundant" functions are tests
/// still scores a perfect Redundancy (FR-QM-08 — the perverse-incentive fix).
#[test]
fn dead_or_duplicate_test_functions_do_not_lower_redundancy() {
    let funcs = [
        func(1, Some(2), Some(false), Some(false)), // production, clean
        func(2, Some(3), Some(false), Some(false)), // production, clean
        func(10, Some(1), Some(true), Some(false)), // a dead test
    ];
    let scoped = run_scoped(&[], &[], &funcs, &[10]);
    assert_eq!(
        scoped.redundancy.normalized, 1.0,
        "a dead test must not register as production redundancy (FR-QM-08)"
    );
    assert_eq!(
        scoped.function_count, 2,
        "the test is out of the denominator"
    );
    assert_eq!(scoped.test_function_count, 1);
}

// ── CR-005 extended dimensions (FR-QM-09..14, UAT-QM-08..13, ADR-21) ──────────

/// A node carrying an explicit 1-based line span — the input to a container's
/// god-by-span check ([FR-QM-12]).
fn node_span(
    id: i64,
    name: &str,
    kind: NodeKind,
    file: Option<&str>,
    start: i64,
    end: i64,
) -> NodeRow {
    NodeRow {
        start_line: Some(start),
        end_line: Some(end),
        ..node(id, name, kind, file)
    }
}

// ── FR-QM-09 / UAT-QM-08: Nesting ────────────────────────────────────────────

/// A deeply nested fixture scores strictly lower than a flat one, and a fully
/// deep fixture floors at 0.01 rather than zeroing ([FR-QM-09], [UAT-QM-08]).
#[test]
fn nesting_lowers_with_depth_and_floors_at_001() {
    // Flat: every function below T_nest = 4.
    let flat = [
        func_struct(1, Some(1), Some(10), Some(0), None),
        func_struct(2, Some(1), Some(10), Some(1), None),
        func_struct(3, Some(1), Some(10), Some(2), None),
        func_struct(4, Some(1), Some(10), Some(3), None),
    ];
    // Half at/above the deep-nesting threshold.
    let deep = [
        func_struct(1, Some(1), Some(10), Some(0), None),
        func_struct(2, Some(1), Some(10), Some(1), None),
        func_struct(3, Some(1), Some(10), Some(4), None),
        func_struct(4, Some(1), Some(10), Some(6), None),
    ];

    let flat = run(&[], &[], &flat);
    let deep = run(&[], &[], &deep);
    assert_eq!(flat.nesting.normalized, 1.0, "no function ≥ T_nest → 1.0");
    assert_eq!(deep.nesting.raw, 0.5, "2 of 4 functions are deeply nested");
    assert_eq!(deep.nesting.normalized, 0.5);
    assert!(
        deep.nesting.normalized < flat.nesting.normalized,
        "the deep fixture scores strictly lower (UAT-QM-08)"
    );

    // All four deeply nested → ratio 1.0 → floored to 0.01, never 0 (FR-QM-09).
    let all_deep = [
        func_struct(1, Some(1), Some(10), Some(4), None),
        func_struct(2, Some(1), Some(10), Some(5), None),
    ];
    let all_deep = run(&[], &[], &all_deep);
    assert_eq!(all_deep.nesting.raw, 1.0);
    assert_eq!(
        all_deep.nesting.normalized, 0.01,
        "a fully deep fixture floors at 0.01 (FR-QM-09 floor, never 0)"
    );
}

/// Zero production functions → Nesting 1.0 (the FR-QM-09 degenerate guard), and
/// a function whose depth was never computed (`None`) is not counted deep.
#[test]
fn nesting_handles_zero_functions_and_uncomputed_depth() {
    let empty = run(&[], &[], &[]);
    assert_eq!(empty.nesting.normalized, 1.0, "zero functions → 1.0");

    // A None depth must not be coerced into a phantom deep function (NFR-CC-04).
    let unknown = [func_struct(1, Some(1), Some(10), None, None)];
    let unknown = run(&[], &[], &unknown);
    assert_eq!(unknown.nesting.raw, 0.0, "None depth is not deep");
    assert_eq!(
        unknown.nesting.normalized, 1.0,
        "and does not lower the score"
    );

    // A None-depth function is excluded from the numerator but STAYS in the
    // denominator: deep/total = 1/2, not 1/1 (FR-QM-09 — only the count of deep
    // functions is affected, not the population).
    let mixed = [
        func_struct(1, Some(1), Some(10), Some(5), None), // deep
        func_struct(2, Some(1), Some(10), None, None),    // depth unknown → not deep
    ];
    let mixed = run(&[], &[], &mixed);
    assert_eq!(
        mixed.nesting.raw, 0.5,
        "1 of 2 deep; None is not deep but remains in the denominator"
    );
}

// ── FR-QM-10 / UAT-QM-09: Conciseness (brain methods) ────────────────────────

/// A brain method requires all three thresholds simultaneously; a function
/// failing exactly one is not a brain method ([FR-QM-10], [UAT-QM-09]).
#[test]
fn conciseness_requires_all_three_brain_thresholds() {
    let funcs = [
        func_struct(1, Some(15), Some(100), Some(3), None), // brain: meets all three
        func_struct(2, Some(14), Some(100), Some(3), None), // fails CC
        func_struct(3, Some(15), Some(99), Some(3), None),  // fails LOC
        func_struct(4, Some(15), Some(100), Some(2), None), // fails nesting
    ];
    let r = run(&[], &[], &funcs);
    assert_eq!(
        r.conciseness.raw, 0.25,
        "exactly one of four is a brain method"
    );
    assert_eq!(r.conciseness.normalized, 0.75, "1 − 1/4 = 0.75 (UAT-QM-09)");
}

/// A function with any `None` brain-method input is never counted a brain method
/// (no fabricated metric, [NFR-CC-04]); zero production functions → Conciseness
/// 1.0 (the [FR-QM-10] degenerate guard).
#[test]
fn conciseness_excludes_none_inputs_and_handles_zero_functions() {
    let empty = run(&[], &[], &[]);
    assert_eq!(empty.conciseness.normalized, 1.0, "zero functions → 1.0");

    // None CC / None LOC / None nesting each disqualify a brain method even when
    // the other two are well over threshold.
    let unknowns = [
        func_struct(1, None, Some(200), Some(6), None), // None CC
        func_struct(2, Some(30), None, Some(6), None),  // None LOC
        func_struct(3, Some(30), Some(200), None, None), // None nesting
    ];
    let r = run(&[], &[], &unknowns);
    assert_eq!(
        r.conciseness.raw, 0.0,
        "a None brain-method input is never a brain method (NFR-CC-04)"
    );
    assert_eq!(r.conciseness.normalized, 1.0);
}

// ── FR-QM-13 / UAT-QM-12: Uniqueness ─────────────────────────────────────────

/// Uniqueness is `1 − near-clone ratio` over production functions: a function
/// in any clone group (non-NULL `clone_group`) counts ([FR-QM-13], [UAT-QM-12]).
#[test]
fn uniqueness_counts_near_clone_group_membership() {
    let funcs = [
        func_struct(1, Some(2), Some(20), Some(1), Some(1)), // clone group 1
        func_struct(2, Some(2), Some(20), Some(1), Some(1)), // clone group 1
        func_struct(3, Some(2), Some(20), Some(1), None),    // unique
    ];
    let r = run(&[], &[], &funcs);
    assert!(
        (r.uniqueness.raw - 2.0 / 3.0).abs() < 1e-12,
        "2 of 3 functions are near-clones"
    );
    assert!(
        (r.uniqueness.normalized - 1.0 / 3.0).abs() < 1e-12,
        "1 − 2/3 = 1/3 (UAT-QM-12)"
    );
    // Near-clone membership (clone_group) must NOT move Redundancy — exact-
    // duplicate semantics are independent (UAT-QM-12: "Redundancy is byte-
    // identical to its pre-clone value"). None of these are is_duplicate.
    assert_eq!(
        r.redundancy.normalized, 1.0,
        "clone_group membership leaves Redundancy untouched (UAT-QM-12, FR-QM-05)"
    );
}

// ── FR-QM-11 / UAT-QM-10: Cohesion (LCOM4) + n/a drop-out ─────────────────────

/// A class whose methods split into two disjoint field-access components scores
/// LCOM4 = 2 (cohesion 0.5); a fully cohesive class scores 1.0; the mean is over
/// both ([FR-QM-11], [UAT-QM-10]).
#[test]
fn cohesion_scores_lcom4_components_per_class() {
    // Cohesive class (id 1): methods 2,3 both access field 4 → one component.
    // Split class (id 10): method 11 → field 13, method 12 → field 14 → two.
    let nodes = [
        node(1, "Cohesive", NodeKind::Class, Some("a.java")),
        node(2, "m1", NodeKind::Method, Some("a.java")),
        node(3, "m2", NodeKind::Method, Some("a.java")),
        node(4, "f", NodeKind::Field, Some("a.java")),
        node(10, "Split", NodeKind::Class, Some("b.java")),
        node(11, "n1", NodeKind::Method, Some("b.java")),
        node(12, "n2", NodeKind::Method, Some("b.java")),
        node(13, "g1", NodeKind::Field, Some("b.java")),
        node(14, "g2", NodeKind::Field, Some("b.java")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
        edge(1, 4, EdgeKind::Contains),
        edge(2, 4, EdgeKind::Accesses), // both methods touch the same field
        edge(3, 4, EdgeKind::Accesses),
        edge(10, 11, EdgeKind::Contains),
        edge(10, 12, EdgeKind::Contains),
        edge(10, 13, EdgeKind::Contains),
        edge(10, 14, EdgeKind::Contains),
        edge(11, 13, EdgeKind::Accesses), // disjoint field accesses → 2 components
        edge(12, 14, EdgeKind::Accesses),
    ];
    let r = run(&nodes, &edges, &[]);
    let cohesion = r.cohesion.expect("classes exist → Cohesion applies");
    // mean(1/1, 1/2) = 0.75.
    assert!(
        (cohesion.raw - 0.75).abs() < 1e-12,
        "mean of cohesive 1.0 and split 0.5 = 0.75, got {}",
        cohesion.raw
    );
}

/// An intra-class call links two methods into one LCOM4 component even with no
/// shared field ([FR-QM-11]).
#[test]
fn cohesion_links_methods_via_intra_class_calls() {
    let nodes = [
        node(1, "C", NodeKind::Class, Some("a.java")),
        node(2, "m1", NodeKind::Method, Some("a.java")),
        node(3, "m2", NodeKind::Method, Some("a.java")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
        edge(2, 3, EdgeKind::Calls), // m1 calls m2 → one component
    ];
    let r = run(&nodes, &edges, &[]);
    let cohesion = r.cohesion.expect("a class exists");
    assert_eq!(
        cohesion.raw, 1.0,
        "the call collapses two methods into LCOM4 = 1"
    );
}

/// A class-less repo yields Cohesion n/a (None) and a deterministic mean over
/// the remaining dimensions ([FR-QM-11], [UAT-QM-10], [ADR-21] drop-out).
#[test]
fn cohesion_drops_out_when_no_class_exists() {
    // A pure-Rust-style fixture: structs, no classes.
    let nodes = [
        node(1, "S", NodeKind::Struct, Some("a.rs")),
        node(2, "m", NodeKind::Method, Some("a.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Contains)];
    let r = run(
        &nodes,
        &edges,
        &[func(2, Some(2), Some(false), Some(false))],
    );
    assert!(
        r.cohesion.is_none(),
        "no Class construct → Cohesion drops out of the mean (ADR-21)"
    );
    // Focus still applies — a struct is a class-like container.
    assert!(
        r.focus.is_some(),
        "a struct is a class-like container for Focus"
    );
    assert!(
        r.aggregate_signal.is_some(),
        "the aggregate is the deterministic mean of the remaining dimensions"
    );
}

/// Rust impl blocks / Go method-sets (extracted as `Struct`) are excluded from
/// Cohesion — only `Class` participates ([FR-QM-11] contested-construct rule).
#[test]
fn cohesion_excludes_struct_constructs() {
    let nodes = [
        node(1, "S", NodeKind::Struct, Some("a.rs")),
        node(2, "m1", NodeKind::Method, Some("a.rs")),
        node(3, "m2", NodeKind::Method, Some("a.rs")),
        node(4, "f", NodeKind::Field, Some("a.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
        edge(1, 4, EdgeKind::Contains),
        edge(2, 4, EdgeKind::Accesses),
    ];
    let r = run(&nodes, &edges, &[]);
    assert!(
        r.cohesion.is_none(),
        "a Struct does not participate in LCOM4 Cohesion (FR-QM-11)"
    );
}

/// A method-less class (LCOM4 undefined) is excluded from the cohesion mean — it
/// neither divides-by-zero nor inflates the mean with a phantom score; only the
/// scoreable class contributes ([FR-QM-11], [NFR-CC-04]).
#[test]
fn cohesion_excludes_method_less_classes_from_the_mean() {
    let nodes = [
        node(1, "Empty", NodeKind::Class, Some("a.java")), // field only, no methods
        node(2, "f", NodeKind::Field, Some("a.java")),
        node(10, "Full", NodeKind::Class, Some("b.java")),
        node(11, "m", NodeKind::Method, Some("b.java")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(10, 11, EdgeKind::Contains),
    ];
    let r = run(&nodes, &edges, &[]);
    let cohesion = r.cohesion.expect("one scoreable class exists");
    assert_eq!(
        cohesion.raw, 1.0,
        "only the one-method class scores (1/1); the method-less class is excluded"
    );
}

// ── FR-QM-12 / UAT-QM-11: Focus (god containers) ─────────────────────────────

/// A 25-method container is god; a 5-method container is not; Focus = 1 − 1/2
/// floored = 0.5, across Class/Struct alike ([FR-QM-12], [UAT-QM-11]).
#[test]
fn focus_flags_god_containers_by_method_count() {
    let mut nodes = vec![
        node(1, "God", NodeKind::Class, Some("a.java")),
        node(2, "Small", NodeKind::Class, Some("b.java")),
    ];
    let mut edges = Vec::new();
    // 25 methods under the god container (≥ T_m = 20).
    for m in 0..25 {
        let id = 100 + m;
        nodes.push(node(id, "gm", NodeKind::Method, Some("a.java")));
        edges.push(edge(1, id, EdgeKind::Contains));
    }
    // 5 methods under the small container.
    for m in 0..5 {
        let id = 200 + m;
        nodes.push(node(id, "sm", NodeKind::Method, Some("b.java")));
        edges.push(edge(2, id, EdgeKind::Contains));
    }
    let r = run(&nodes, &edges, &[]);
    let focus = r.focus.expect("containers exist → Focus applies");
    assert_eq!(focus.raw, 0.5, "1 of 2 containers is god");
    assert_eq!(focus.normalized, 0.5, "1 − 1/2 = 0.5 (UAT-QM-11)");
}

/// A container is also god when its line span ≥ T_span, even with no methods —
/// containers are enumerated from the node set, not from member edges
/// ([FR-QM-12]).
#[test]
fn focus_flags_god_containers_by_span() {
    let nodes = [
        node_span(1, "Wide", NodeKind::Struct, Some("a.rs"), 1, 600), // span 600 ≥ 500
        node_span(2, "Narrow", NodeKind::Struct, Some("b.rs"), 1, 20),
    ];
    let r = run(&nodes, &[], &[]);
    let focus = r.focus.expect("class-like containers exist");
    assert_eq!(
        focus.raw, 0.5,
        "the wide struct is god by span; the narrow one is not"
    );
}

/// Zero class-like containers → Focus n/a (None) and drop-out ([FR-QM-12],
/// [ADR-21]).
#[test]
fn focus_drops_out_when_no_container_exists() {
    let r = run(
        &[node(1, "f", NodeKind::Function, Some("a.rs"))],
        &[],
        &[func(1, Some(2), Some(false), Some(false))],
    );
    assert!(
        r.focus.is_none(),
        "no Class/Struct container → Focus drops out (ADR-21)"
    );
}

// ── FR-QM-14 / UAT-QM-13: extended aggregate (floors, drop-out, hash) ─────────

/// A new dimension forced to its floor drags but never zeroes the signal, while
/// an original-metric hard zero still yields 0 ([FR-QM-14], [UAT-QM-13]).
#[test]
fn floored_new_dimension_drags_but_original_zero_still_collapses() {
    // Heavily cloned production: every function near-cloned → Uniqueness floors.
    let nodes = [
        node(1, "a", NodeKind::Function, Some("x/a.rs")),
        node(2, "b", NodeKind::Function, Some("x/b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];
    let cloned = [
        func_struct(1, Some(2), Some(20), Some(1), Some(1)),
        func_struct(2, Some(2), Some(20), Some(1), Some(1)),
    ];
    let r = run(&nodes, &edges, &cloned);
    assert_eq!(
        r.uniqueness.normalized, 0.01,
        "all cloned → Uniqueness floors"
    );
    let signal = r.aggregate_signal.expect("non-empty graph");
    assert!(
        signal > 0,
        "a floored new dimension drags but never zeroes the signal (UAT-QM-13), got {signal}"
    );

    // An original-metric hard zero (force Redundancy to 0 via an all-redundant
    // production set, over a non-empty graph) still yields 0.
    let all_redundant = [
        func(1, Some(2), Some(true), Some(false)),
        func(2, Some(2), Some(false), Some(true)),
    ];
    let z = run(&nodes, &edges, &all_redundant);
    assert_eq!(
        z.redundancy.normalized, 0.0,
        "every production function redundant"
    );
    assert_eq!(
        z.aggregate_signal,
        Some(0),
        "an original-metric hard zero collapses the signal (ADR-12 short-circuit)"
    );
}

/// The empty-graph sentinel is unchanged by CR-005: no nodes → aggregate is
/// None ("n/a"), never a misleading number ([ADR-12], [FR-QM-14]).
#[test]
fn empty_graph_sentinel_is_unchanged_by_the_extended_set() {
    let r = run(&[], &[], &[]);
    assert!(r.empty, "no nodes → empty");
    assert_eq!(r.aggregate_signal, None, "empty graph stays n/a (ADR-12)");
}

/// Cohesion/Focus drop-out shrinks the aggregate denominator deterministically:
/// a class-less repo's signal is the mean over the eight applicable dimensions,
/// matching a direct geometric-mean computation ([FR-QM-14], [UAT-QM-10]).
#[test]
fn aggregate_denominator_follows_applicability_drop_out() {
    // No Class/Struct → Cohesion and Focus both n/a; the three other new dims
    // are all 1.0 (no nesting/brain/clone inputs), so the signal equals the
    // five-original geometric mean re-meaned over eight dimensions.
    let nodes = [
        node(1, "a", NodeKind::Function, Some("x/a.rs")),
        node(2, "b", NodeKind::Function, Some("x/b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];
    let funcs = [
        func(1, Some(2), Some(false), Some(false)),
        func(2, Some(3), Some(false), Some(false)),
    ];
    let r = run(&nodes, &edges, &funcs);
    assert!(
        r.cohesion.is_none() && r.focus.is_none(),
        "class-less → 8 dimensions"
    );

    // Recompute the expected signal directly from the five original normalized
    // values plus three 1.0 dimensions, over eight (NFR-RA-06 cross-check).
    let originals = [
        r.modularity.normalized,
        r.acyclicity.normalized,
        r.depth.normalized,
        r.equality.normalized,
        r.redundancy.normalized,
    ];
    let ln_sum: f64 = originals.iter().map(|n| n.ln()).sum::<f64>(); // +3·ln(1)=0
    let expected = ((ln_sum / 8.0).exp() * 10_000.0).round() as u32;
    assert_eq!(
        r.aggregate_signal,
        Some(expected),
        "8-dimension mean is deterministic"
    );
}

/// The exact nine-dimension path ([UAT-QM-13] "nine-dimension mean"): exactly one
/// of Cohesion/Focus drops out. A Struct (no Class) makes Focus apply while
/// Cohesion is n/a, so the mean spans NINE dimensions — cross-checked against a
/// direct geometric-mean computation over k = 9 ([FR-QM-14], [NFR-RA-06]).
#[test]
fn aggregate_nine_dimension_mean_when_only_cohesion_drops_out() {
    // A struct container (Focus applies) with one production method; no Class, so
    // Cohesion is n/a. Nesting/Conciseness/Uniqueness are all 1.0 (clean inputs).
    let nodes = [
        node(1, "S", NodeKind::Struct, Some("x/a.rs")),
        node(2, "m", NodeKind::Method, Some("x/a.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Contains)];
    let funcs = [func(2, Some(2), Some(false), Some(false))];
    let r = run(&nodes, &edges, &funcs);
    assert!(r.cohesion.is_none(), "no Class → Cohesion drops out");
    let focus = r.focus.expect("a Struct → Focus applies (k = 9)");

    // Direct k = 9 cross-check: original five + Focus + three 1.0 new dims.
    let dims = [
        r.modularity.normalized,
        r.acyclicity.normalized,
        r.depth.normalized,
        r.equality.normalized,
        r.redundancy.normalized,
        r.nesting.normalized,
        r.conciseness.normalized,
        focus.normalized,
        r.uniqueness.normalized,
    ];
    let ln_sum: f64 = dims.iter().map(|n| n.ln()).sum();
    let expected = ((ln_sum / 9.0).exp() * 10_000.0).round() as u32;
    assert_eq!(
        r.aggregate_signal,
        Some(expected),
        "the nine-dimension mean is deterministic (UAT-QM-13)"
    );
}

/// The effective-thresholds hash is non-empty, stable for equal thresholds, and
/// changes when a threshold changes — the provenance the gate re-baselines on
/// ([FR-QM-14], [BR-25]). A `T_nest` 4 → 5 edit also re-scores Nesting.
#[test]
fn thresholds_hash_is_stable_and_changes_with_tuning() {
    use std::collections::HashSet;

    let funcs = [
        func_struct(1, Some(1), Some(10), Some(4), None), // depth exactly 4
        func_struct(2, Some(1), Some(10), Some(1), None),
    ];
    let view = build_view(Granularity::ExcludeContains, &[], &[]);
    let no_tests: HashSet<NodeId> = HashSet::new();

    let default_t = super::Thresholds::default();
    let raised = super::Thresholds {
        nest: 5,
        ..default_t
    };

    let a = compute(&view, &[], &[], &funcs, &no_tests, default_t);
    let b = compute(&view, &[], &[], &funcs, &no_tests, default_t);
    let c = compute(&view, &[], &[], &funcs, &no_tests, raised);

    assert!(!a.thresholds_hash.is_empty(), "the hash is recorded");
    assert_eq!(
        a.thresholds_hash, b.thresholds_hash,
        "equal thresholds → equal hash"
    );
    assert_ne!(
        a.thresholds_hash, c.thresholds_hash,
        "a threshold edit changes the hash (FR-QM-14 re-baseline trigger)"
    );
    // T_nest 4 → 5: the depth-4 function is deep under default but not raised.
    assert_eq!(a.nesting.raw, 0.5, "1 of 2 deep at T_nest = 4");
    assert_eq!(
        c.nesting.raw, 0.0,
        "0 of 2 deep at T_nest = 5 (UAT-QM-13 edit)"
    );
}

/// CR-013 / FR-QM-14: the near-clone parameters join the hashed effective set,
/// but the **default-set hash stays byte-identical to the pre-CR-013 build** —
/// the clone keys are folded in append-on-divergence, so an untuned repo never
/// spuriously re-baselines on upgrade. Tuning either key moves the hash; setting
/// either key back to its documented default leaves the hash unchanged.
#[test]
fn clone_thresholds_join_the_hash_without_moving_the_default() {
    use super::Thresholds;

    // The exact pre-CR-013 canonical form: the six structural keys only.
    let pre_cr = "t_nest=4;t_cc=15;t_loc=100;t_bn=3;t_m=20;t_span=500";
    let expected = blake3::hash(pre_cr.as_bytes()).to_hex().to_string();

    let default = Thresholds::default();
    assert_eq!(
        default.hash(),
        expected,
        "the default-set hash is byte-identical to the pre-CR-013 build (FR-QM-14)"
    );

    // Setting the clone keys to their documented defaults is a no-op for the hash.
    let explicit_defaults = Thresholds {
        clone_similarity: 0.85,
        clone_min_tokens: 50,
        ..default
    };
    assert_eq!(
        explicit_defaults.hash(),
        expected,
        "the documented clone defaults do not move the hash"
    );

    // Tuning the similarity moves the hash (FR-GV-10 re-baseline trigger).
    let tuned_sim = Thresholds {
        clone_similarity: 0.9,
        ..default
    };
    assert_ne!(
        tuned_sim.hash(),
        expected,
        "a clone_similarity edit moves the effective-thresholds hash"
    );

    // Tuning the token floor moves the hash, distinctly from the similarity edit.
    let tuned_tokens = Thresholds {
        clone_min_tokens: 80,
        ..default
    };
    assert_ne!(
        tuned_tokens.hash(),
        expected,
        "a clone_min_tokens edit moves the effective-thresholds hash"
    );
    assert_ne!(
        tuned_sim.hash(),
        tuned_tokens.hash(),
        "the two near-clone keys are distinct in the canonical form"
    );
}

// ── UAT-QM-07: metric-neutrality across the extended set ─────────────────────

/// Adding test functions and test methods leaves all ten dimensions
/// byte-identical — the production scope governs the new five exactly as the
/// original five ([FR-QM-08], [UAT-QM-07]).
#[test]
fn extended_dimensions_are_invariant_under_added_tests() {
    // Baseline: one production class with two cohesive methods + a production
    // free function; some near-clones and nesting to make every dimension live.
    let nodes = [
        node(1, "C", NodeKind::Class, Some("a.java")),
        node(2, "m1", NodeKind::Method, Some("a.java")),
        node(3, "m2", NodeKind::Method, Some("a.java")),
        node(4, "f", NodeKind::Field, Some("a.java")),
        node(5, "free", NodeKind::Function, Some("a.java")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
        edge(1, 4, EdgeKind::Contains),
        edge(2, 4, EdgeKind::Accesses),
        edge(3, 4, EdgeKind::Accesses),
    ];
    let funcs = [
        func_struct(2, Some(2), Some(20), Some(2), Some(2)),
        func_struct(3, Some(2), Some(20), Some(2), Some(2)),
        func_struct(5, Some(2), Some(20), Some(1), None),
    ];
    let base = run(&nodes, &edges, &funcs);

    // Add structurally identical TEST functions and a TEST method inside the
    // class — all is_test, so all must be excluded from every dimension.
    let mut t_nodes = nodes.to_vec();
    t_nodes.push(node(6, "test_one", NodeKind::Function, Some("a_test.java")));
    t_nodes.push(node(7, "test_m", NodeKind::Method, Some("a.java")));
    let mut t_edges = edges.to_vec();
    t_edges.push(edge(1, 7, EdgeKind::Contains)); // a test method inside the class
    let mut t_funcs = funcs.to_vec();
    t_funcs.push(func_struct(6, Some(20), Some(200), Some(6), Some(2))); // brain+deep+clone
    t_funcs.push(func_struct(7, Some(20), Some(200), Some(6), Some(2)));
    let with_tests = run_scoped(&t_nodes, &t_edges, &t_funcs, &[6, 7]);

    // Every one of the ten dimensions is byte-identical (UAT-QM-07).
    let pair = |m: &crate::models::quality::MetricValue| (m.raw.to_bits(), m.normalized.to_bits());
    assert_eq!(pair(&base.modularity), pair(&with_tests.modularity));
    assert_eq!(pair(&base.acyclicity), pair(&with_tests.acyclicity));
    assert_eq!(pair(&base.depth), pair(&with_tests.depth));
    assert_eq!(pair(&base.equality), pair(&with_tests.equality));
    assert_eq!(pair(&base.redundancy), pair(&with_tests.redundancy));
    assert_eq!(pair(&base.nesting), pair(&with_tests.nesting));
    assert_eq!(pair(&base.conciseness), pair(&with_tests.conciseness));
    assert_eq!(
        base.cohesion.as_ref().map(pair),
        with_tests.cohesion.as_ref().map(pair),
        "a test method inside a class must not move Cohesion (UAT-QM-07)"
    );
    assert_eq!(
        base.focus.as_ref().map(pair),
        with_tests.focus.as_ref().map(pair)
    );
    assert_eq!(pair(&base.uniqueness), pair(&with_tests.uniqueness));
    assert_eq!(
        base.aggregate_signal, with_tests.aggregate_signal,
        "the whole ten-dimension signal is invariant under added tests (UAT-QM-07)"
    );
}

// ── CR-005 §3.2 / FR-QM-09..13: worst-offender reporting ─────────────────────

/// Nesting/Uniqueness worst-offenders rank by severity, cap at the requested N,
/// and exclude test-scoped functions ([FR-QM-08], [NFR-RA-06]). The descriptors
/// are deterministic strings derived from the integer facts.
#[test]
fn worst_offenders_rank_cap_and_scope_function_dimensions() {
    // Six deep functions (ids 1..6) at increasing depth, plus a deeper test fn
    // (id 99) that must be excluded by the production scope.
    let mut nodes: Vec<NodeRow> = (1..=6)
        .map(|id| {
            node(
                id,
                &format!("deep{id}"),
                NodeKind::Function,
                Some("src/a.rs"),
            )
        })
        .collect();
    nodes.push(node(
        99,
        "deep_test",
        NodeKind::Function,
        Some("tests/a.rs"),
    ));
    let mut functions: Vec<FunctionMetricRow> = (1..=6)
        .map(|id| func_struct(id, None, None, Some(4 + id), None)) // depth 5..10
        .collect();
    functions.push(func_struct(99, None, None, Some(99), None));

    let test_ids: HashSet<NodeId> = [NodeId(99)].into_iter().collect();
    let w = super::worst_offenders(
        &nodes,
        &[],
        &functions,
        &test_ids,
        super::Thresholds::default(),
        3,
    );

    // Top 3 by depth desc: ids 6 (depth 10), 5 (9), 4 (8); the test fn is absent.
    assert_eq!(w.nesting.len(), 3, "capped at 3");
    assert_eq!(w.nesting[0].name, "deep6");
    assert_eq!(w.nesting[0].detail, "nesting depth 10");
    assert_eq!(w.nesting[1].name, "deep5");
    assert_eq!(w.nesting[2].name, "deep4");
    assert!(
        w.nesting.iter().all(|o| o.name != "deep_test"),
        "test-scoped functions are excluded"
    );
}

/// Uniqueness offenders cluster by clone-group id then node id; Conciseness
/// offenders carry the three brain facts.
#[test]
fn worst_offenders_group_clones_and_describe_brain_methods() {
    let nodes = [
        node(1, "clone_b", NodeKind::Function, Some("src/a.rs")),
        node(2, "clone_a", NodeKind::Function, Some("src/a.rs")),
        node(3, "brainy", NodeKind::Function, Some("src/b.rs")),
    ];
    let functions = [
        func_struct(1, None, None, None, Some(1)), // clone group 1
        func_struct(2, None, None, None, Some(1)), // clone group 1
        func_struct(3, Some(20), Some(150), Some(4), None), // a brain method
    ];
    let w = super::worst_offenders(
        &nodes,
        &[],
        &functions,
        &HashSet::new(),
        super::Thresholds::default(),
        10,
    );

    assert_eq!(w.uniqueness.len(), 2);
    assert_eq!(
        w.uniqueness[0].name, "clone_b",
        "group asc, then node id asc"
    );
    assert_eq!(w.uniqueness[0].detail, "clone group #1");
    assert_eq!(w.conciseness.len(), 1);
    assert_eq!(w.conciseness[0].name, "brainy");
    assert_eq!(w.conciseness[0].detail, "CC 20 · LOC 150 · nesting 4");
}

/// Cohesion offenders are low-cohesion classes (LCOM4 ≥ 2); Focus offenders are
/// god containers, ranked by method count then span.
#[test]
fn worst_offenders_report_container_dimensions() {
    // A class (id 1) with two methods (2, 3) sharing no field → LCOM4 = 2.
    // A god-by-span class (id 10), method-less, spanning 600 lines.
    let nodes = [
        node(1, "Fragmented", NodeKind::Class, Some("src/a.rs")),
        node(2, "m1", NodeKind::Method, Some("src/a.rs")),
        node(3, "m2", NodeKind::Method, Some("src/a.rs")),
        node_span(10, "Huge", NodeKind::Class, Some("src/b.rs"), 1, 600),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
    ];
    let w = super::worst_offenders(
        &nodes,
        &edges,
        &[],
        &HashSet::new(),
        super::Thresholds::default(),
        10,
    );

    assert_eq!(
        w.cohesion.len(),
        1,
        "the two-component class is a cohesion offender"
    );
    assert_eq!(w.cohesion[0].name, "Fragmented");
    assert_eq!(w.cohesion[0].detail, "LCOM4 2");

    assert_eq!(w.focus.len(), 1, "the 600-line class is a god container");
    assert_eq!(w.focus[0].name, "Huge");
    assert_eq!(w.focus[0].detail, "0 methods · span 600");
}

/// The cap applies to every dimension, not just Nesting — a clone-heavy and a
/// god-container-heavy fixture each truncate to `cap` ([NFR-RA-06], CR-005 §3.2).
#[test]
fn worst_offenders_cap_applies_to_clone_and_container_dimensions() {
    // 12 near-clone production functions → Uniqueness truncates to the cap.
    let clone_nodes: Vec<NodeRow> = (1..=12)
        .map(|id| node(id, &format!("c{id}"), NodeKind::Function, Some("src/a.rs")))
        .collect();
    let clone_fns: Vec<FunctionMetricRow> = (1..=12)
        .map(|id| func_struct(id, None, None, None, Some(id)))
        .collect();
    let w = super::worst_offenders(
        &clone_nodes,
        &[],
        &clone_fns,
        &HashSet::new(),
        super::Thresholds::default(),
        3,
    );
    assert_eq!(w.uniqueness.len(), 3, "uniqueness capped at 3");

    // 12 god-by-span containers → Focus truncates to the cap.
    let god_nodes: Vec<NodeRow> = (1..=12)
        .map(|id| {
            node_span(
                id,
                &format!("G{id}"),
                NodeKind::Class,
                Some("src/b.rs"),
                1,
                600,
            )
        })
        .collect();
    let w2 = super::worst_offenders(
        &god_nodes,
        &[],
        &[],
        &HashSet::new(),
        super::Thresholds::default(),
        3,
    );
    assert_eq!(w2.focus.len(), 3, "focus capped at 3");
}

// ── S-256 / CR-061: the promoted broker vertices must not move the gated signal ──

/// **Regression for the S-256 signal move.** Promoting broker coupling to first-class
/// `Topic`/`Producer`/`Consumer` nodes must leave a repo's gated quality signal
/// **byte-identical**: the promoted vertices are markers of code that is *already* in
/// the graph (the publishing/subscribing method), so counting them would measure the
/// model rather than the source.
///
/// The `Topic` is the sharp case. It is a repo-scoped identity with **no file**
/// ([FR-WS-11]), so before the fix it fell into the `<unbound>` directory community and
/// every `Publishes`/`Subscribes` edge ran from its producer's directory into
/// `<unbound>` — external to both communities, contributing to `degree` but never to
/// `internal`. Modularity therefore *fell* for any repo that indexed a broker topic,
/// purely as an artifact of how the topic is modelled. This asserts the whole snapshot
/// is unchanged, not merely that it "did not crash".
///
/// [FR-WS-11]: ../../../../docs/specs/requirements/FR-WS-11.md
#[test]
fn promoted_broker_vertices_leave_the_metric_snapshot_byte_identical() {
    // A plain two-directory codebase with intra-directory calls (a well-modularised
    // fixture, so a modularity drop would be visible).
    let base_nodes = [
        node(1, "a1", NodeKind::Function, Some("src/a/one.rs")),
        node(2, "a2", NodeKind::Function, Some("src/a/two.rs")),
        node(3, "b1", NodeKind::Function, Some("src/b/one.rs")),
        node(4, "b2", NodeKind::Function, Some("src/b/two.rs")),
    ];
    let base_edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(3, 4, EdgeKind::Calls),
    ];
    let funcs = [
        func(1, Some(1), Some(false), Some(false)),
        func(2, Some(1), Some(false), Some(false)),
        func(3, Some(1), Some(false), Some(false)),
        func(4, Some(1), Some(false), Some(false)),
    ];

    let before = run_scoped(&base_nodes, &base_edges, &funcs, &[]);

    // Now promote a broker topic over exactly the same code: `a1` publishes to
    // `orders` and `b1` subscribes from it. The topic carries NO file (repo-scoped);
    // the producer/consumer are anchored in their declaring files.
    let mut nodes = base_nodes.to_vec();
    nodes.extend([
        node(10, "orders", NodeKind::Topic, None),
        node(11, "orders", NodeKind::Producer, Some("src/a/one.rs")),
        node(12, "orders", NodeKind::Consumer, Some("src/b/one.rs")),
    ]);
    let mut edges = base_edges.to_vec();
    edges.extend([
        edge(1, 11, EdgeKind::Contains),   // the publishing fn contains its producer
        edge(3, 12, EdgeKind::Contains),   // the subscribing fn contains its consumer
        edge(11, 10, EdgeKind::Publishes), // producer --publishes--> topic
        edge(12, 10, EdgeKind::Subscribes), // consumer --subscribes--> topic
    ]);

    let after = run_scoped(&nodes, &edges, &funcs, &[]);

    // Byte-identical, literally: the serialized snapshot IS the gated artifact, so
    // comparing it catches drift in any dimension — not only the modularity this
    // regressed on.
    let render = |s: &crate::models::quality::MetricSnapshot| {
        serde_json::to_string(s).expect("a metric snapshot serializes")
    };
    assert_eq!(
        render(&after),
        render(&before),
        "promoting a broker topic moved the gated signal — the file-less Topic vertex \
         is polluting the `<unbound>` community and depressing modularity"
    );
}
