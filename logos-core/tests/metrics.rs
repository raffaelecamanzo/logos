//! Integration tests for the quality metrics engine's persistence half
//! (S-018 / FR-QM-07, ADR-12, NFR-RA-06), exercised end-to-end: real
//! extraction + resolution + annotation through [`Engine::index`], hydration
//! through the Engine cache, then [`logos_core::metrics::snapshot`] against
//! the live runtime.
//!
//! Coverage by acceptance criterion:
//! - a snapshot row persists raw + normalized values for all five metrics,
//!   the counts, the `empty` flag, and the optional commit sha (FR-QM-07);
//! - an empty graph persists `empty = 1` with a NULL signal and surfaces
//!   "n/a" (FR-QM-06, UAT-QM-06, ADR-12);
//! - repeated runs on the same graph yield the identical integer signal
//!   (NFR-RA-06) and append — never mutate — the series.

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::graph_store::MetricSnapshotRow;
use logos_core::metrics;
use logos_core::models::quality::MetricSnapshot;
use logos_core::{Engine, Granularity, Runtime};
use tempfile::TempDir;

/// Count the documentation-kind nodes the store currently holds — used by the
/// FR-DG-06 fitness test to prove documentation was actually ingested (so a
/// byte-identical result reflects real exclusion, not a no-op).
#[cfg(feature = "lang-markdown")]
fn documentation_node_count(rt: &Runtime) -> usize {
    rt.submit_read(|store| store.all_nodes())
        .expect("read runs")
        .into_iter()
        .filter(|n| n.kind.is_documentation())
        .count()
}

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Assert two snapshots are byte-identical across every persisted metric value
/// and the aggregate signal — the NFR-RA-06 / FR-QM-08 invariance contract.
fn assert_metrics_byte_identical(a: &MetricSnapshot, b: &MetricSnapshot) {
    assert_eq!(
        a.aggregate_signal, b.aggregate_signal,
        "the aggregate signal must be byte-identical (FR-QM-08)"
    );
    for (name, x, y) in [
        ("modularity", &a.modularity, &b.modularity),
        ("acyclicity", &a.acyclicity, &b.acyclicity),
        ("depth", &a.depth, &b.depth),
        ("equality", &a.equality, &b.equality),
        ("redundancy", &a.redundancy, &b.redundancy),
    ] {
        assert_eq!(x.raw.to_bits(), y.raw.to_bits(), "{name} raw drifted");
        assert_eq!(
            x.normalized.to_bits(),
            y.normalized.to_bits(),
            "{name} normalized drifted"
        );
    }
    assert_eq!(a.node_count, b.node_count, "production node_count drifted");
    assert_eq!(a.edge_count, b.edge_count, "production edge_count drifted");
    assert_eq!(
        a.function_count, b.function_count,
        "production function_count drifted"
    );
}

/// The persisted snapshot series, via the public read seam.
fn rows(rt: &Runtime) -> Vec<MetricSnapshotRow> {
    rt.submit_read(|store| store.metric_snapshots())
        .expect("read runs")
}

// ── FR-QM-07: a snapshot persists raw + normalized + counts + sha ────────────

#[test]
fn snapshot_persists_raw_normalized_counts_and_sha() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
fn never_called() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().unwrap();
    let (id, model) = metrics::snapshot(rt, &view, Some("abc1234"), metrics::Thresholds::default())
        .expect("snapshot runs");

    let all = rows(rt);
    assert_eq!(all.len(), 1, "exactly one appended row");
    let row = &all[0];
    assert_eq!(row.id, id, "the returned id is the persisted row");
    assert_eq!(row.commit_sha.as_deref(), Some("abc1234"), "sha persists");
    assert!(!row.empty, "an indexed fixture is not the empty sentinel");
    assert!(row.node_count > 0 && row.function_count > 0);
    assert!(row.created_at > 0, "a real timestamp was recorded");

    // Raw + normalized present and sane for all five metrics (FR-QM-07).
    for (name, raw, normalized) in [
        ("modularity", row.modularity_raw, row.modularity_normalized),
        ("acyclicity", row.acyclicity_raw, row.acyclicity_normalized),
        ("depth", row.depth_raw, row.depth_normalized),
        ("equality", row.equality_raw, row.equality_normalized),
        ("redundancy", row.redundancy_raw, row.redundancy_normalized),
    ] {
        assert!(raw.is_finite(), "{name} raw is a real value");
        assert!(
            (0.0..=1.0).contains(&normalized),
            "{name} normalized is in [0,1], got {normalized}"
        );
    }

    // The stored signal is the read-model's signal, in range.
    let signal = row.aggregate_signal.expect("non-empty graph has a signal");
    assert_eq!(Some(signal), model.aggregate_signal.map(i64::from));
    assert!((0..=10_000).contains(&signal));

    // The row mirrors the read-model values byte-for-byte (same f64 bits).
    assert_eq!(row.modularity_raw.to_bits(), model.modularity.raw.to_bits());
    assert_eq!(
        row.equality_normalized.to_bits(),
        model.equality.normalized.to_bits()
    );
}

// ── FR-QM-06 / UAT-QM-06 / ADR-12: the empty graph persists "n/a" ────────────

#[test]
fn empty_graph_snapshot_persists_the_na_sentinel() {
    let tmp = TempDir::new().unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("an empty store hydrates an empty view");
    let rt = engine.runtime().unwrap();
    let (_, model) =
        metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("snapshot runs");

    assert!(model.empty, "node_count == 0 → empty (ADR-12)");
    assert_eq!(
        model.aggregate_signal, None,
        "the read-model surfaces n/a, never ~8033 (UAT-QM-06)"
    );

    let all = rows(rt);
    assert_eq!(all.len(), 1);
    assert!(all[0].empty, "empty = 1 persists");
    assert_eq!(
        all[0].aggregate_signal, None,
        "aggregate_signal is SQL NULL for the sentinel (FR-QM-07, ADR-12)"
    );
    assert_eq!(all[0].commit_sha, None, "absent sha persists as NULL");
}

// ── NFR-RA-06: identical graph → identical signal; series appends ────────────

#[test]
fn repeated_snapshots_yield_identical_signals_and_append() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn entry() {
    one();
    two();
}
fn one() {}
fn two() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("view hydrates");
    let rt = engine.runtime().unwrap();

    let (first_id, first) =
        metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("first run");
    let (second_id, second) =
        metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("second run");

    assert_eq!(
        first.aggregate_signal, second.aggregate_signal,
        "identical input → identical integer signal (NFR-RA-06, ADR-08)"
    );
    assert!(second_id > first_id, "the series appends, never mutates");

    let all = rows(rt);
    assert_eq!(all.len(), 2, "both rows are retained (append-only)");
    assert_eq!(all[0].aggregate_signal, all[1].aggregate_signal);
}

// ── FR-QM-08 / UAT-QM-07: the signal is invariant under adding/removing tests ─

/// The 2026-06-07 manual-test scenario, end-to-end and inverted: indexing a
/// production fixture, then adding three structurally identical test functions
/// that call the deepest production function, leaves every normalized metric and
/// the aggregate signal byte-identical — because the persisted `is_test` column
/// (FR-AN-05) excludes them from the production scope (FR-QM-08). Removing them
/// restores the same values; `test_function_count` tracks 0 → 3 → 0
/// (UAT-QM-07). This drives the *real* extraction → annotation → metrics path,
/// proving the persisted column — not a recomputation — gates the scope.
#[test]
fn signal_is_byte_identical_across_adding_and_removing_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn level_one() -> u32 {
    level_two() + 1
}
pub fn level_two() -> u32 {
    level_three() + 1
}
pub fn level_three() -> u32 {
    7
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let indexed = engine.index();
    assert!(indexed.warnings.is_empty(), "{:?}", indexed.warnings);

    // Measurement 1 — production only.
    let base = engine.scan(true).expect("scan runs").metrics;
    assert!(!base.empty && base.aggregate_signal.is_some());
    assert_eq!(base.test_function_count, 0, "no tests indexed yet");
    let production_functions = base.function_count;

    // Add three structurally identical tests under `tests/` (is_test by path
    // convention, FR-AN-05). Each calls the deepest production function and is a
    // duplicate of the others — the depth-extending and redundancy-lowering
    // shapes that broke the signal before CR-001.
    write(
        tmp.path(),
        "tests/invariance.rs",
        "\
#[test]
fn case_one() {
    assert_eq!(level_three(), 7);
}
#[test]
fn case_two() {
    assert_eq!(level_three(), 7);
}
#[test]
fn case_three() {
    assert_eq!(level_three(), 7);
}
",
    );
    let with_tests = engine.scan(true).expect("scan runs").metrics;
    assert_eq!(
        with_tests.test_function_count, 3,
        "all three tests are excluded from the scope and counted (FR-QM-07)"
    );
    assert_eq!(
        with_tests.function_count, production_functions,
        "the production denominator is unchanged by the added tests"
    );
    assert_metrics_byte_identical(&base, &with_tests);

    // Remove the tests and re-score: the count and every metric return to the
    // production baseline (UAT-QM-07: 0 → 3 → 0).
    fs::remove_file(tmp.path().join("tests/invariance.rs")).unwrap();
    let removed = engine.scan(true).expect("scan runs").metrics;
    assert_eq!(removed.test_function_count, 0, "the count returns to zero");
    assert_metrics_byte_identical(&base, &removed);
}

// ── FR-QM-07 / NFR-CC-04: the excluded count persists on the snapshot ────────

/// A scan over a fixture mixing production and test code persists
/// `test_function_count` on the `metric_snapshots` row (FR-QM-07) and reports it
/// on the read-model — the "N test functions excluded from metrics" transparency
/// line (NFR-CC-04).
#[test]
fn snapshot_persists_the_excluded_test_function_count() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn produce() -> u32 {
    7
}
",
    );
    write(
        tmp.path(),
        "tests/cover.rs",
        "\
#[test]
fn covers_produce() {
    assert_eq!(produce(), 7);
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let rt = engine.runtime().unwrap();

    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("view hydrates");
    let (id, model) =
        metrics::snapshot(rt, &view, None, metrics::Thresholds::default()).expect("snapshot runs");
    assert_eq!(
        model.test_function_count, 1,
        "the one test function is excluded and counted"
    );

    // The count is persisted on the append-only ledger row (FR-QM-07).
    let row = rows(rt)
        .into_iter()
        .find(|r| r.id == id)
        .expect("row present");
    assert_eq!(
        row.test_function_count, 1,
        "test_function_count persists on metric_snapshots (FR-QM-07)"
    );
    assert_eq!(
        row.metric_version,
        metrics::METRIC_SEMANTICS_VERSION,
        "the snapshot is stamped with the current metrics-semantics version"
    );
}

// ── FR-DG-06 / UAT-DG-04: the signal is invariant under adding/removing docs ─

/// The documentation analogue of the `is_test` invariance fixture, and the CI
/// fitness test FR-DG-06 / NFR-RA-06 demand: index a code-only project and
/// measure the aggregate signal; add documentation — including a doc that
/// *references code* (a resolved doc→code edge) — and re-scan; the signal and
/// every normalized metric are byte-identical, because graph hydration scopes
/// metrics, cycle detection, and DSM to the code subgraph ([FR-DG-06],
/// [ADR-19]). The `dsm` report lists no documentation file, and removing the
/// docs restores the same values — adding documentation can never move the
/// quality signal ([UAT-DG-04]).
///
/// Gated on `lang-markdown`: without the grammar no doc nodes are ingested and
/// the assertion would be vacuous, so the test only runs where it is meaningful.
#[cfg(feature = "lang-markdown")]
#[test]
fn signal_is_byte_identical_across_adding_and_removing_documentation() {
    let tmp = TempDir::new().unwrap();
    // A small but real dependency structure across two directories so every
    // metric (modularity's partition, acyclicity, depth, equality, redundancy)
    // has something to measure.
    write(
        tmp.path(),
        "src/api.rs",
        "\
pub fn entry() -> u32 {
    core::compute() + helper()
}
pub fn helper() -> u32 {
    7
}
",
    );
    write(
        tmp.path(),
        "src/core.rs",
        "\
pub fn compute() -> u32 {
    leaf() + 1
}
fn leaf() -> u32 {
    41
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let rt = engine.runtime().unwrap();

    // Measurement 1 — code only.
    let base = engine.scan(true).expect("scan runs").metrics;
    assert!(
        !base.empty && base.aggregate_signal.is_some(),
        "the code fixture yields a real signal"
    );
    assert_eq!(documentation_node_count(rt), 0, "no docs ingested yet");
    let base_dsm = engine
        .dsm(Some(logos_core::governance::DsmGranularity::File), true)
        .expect("dsm runs");

    // Add documentation: a doc tree, a doc→doc link, and — the adversarial case
    // FR-DG-06 names explicitly — a doc that *references code* (`compute`/`entry`
    // as code spans, an explicit path to a source file), which the resolver may
    // bind as doc→code edges. None of it may perturb the code subgraph.
    write(
        tmp.path(),
        "docs/guide.md",
        "\
# Guide

## Overview

The entry point is `entry`, which calls `compute` in `src/core.rs`.
See the [reference](reference.md) for details.
",
    );
    write(
        tmp.path(),
        "docs/reference.md",
        "\
# Reference

## Compute

`compute` returns the leaf value plus one.
",
    );
    write(tmp.path(), "README.md", "# Project\n\nEntry is `entry`.\n");

    let with_docs = engine.scan(true).expect("scan runs").metrics;
    assert!(
        documentation_node_count(rt) > 0,
        "documentation was ingested — the invariance assertion is meaningful"
    );
    assert_metrics_byte_identical(&base, &with_docs);

    // ── ADR-34 / S-113 metric-neutrality guard ──────────────────────────────
    // The presentation-only visualization view (the one graph_elements() now
    // feeds the canvas) *keeps* the documentation layer and its cross-layer
    // edges. Hydrating and reading it must surface that content, yet leave
    // scan/aggregate-signal/cycles byte-identical to the code-only baseline —
    // proof the new view never feeds a metric/algorithm path ([FR-DG-06],
    // [FR-QM-08], [UAT-DG-04]).
    let vis = engine
        .hydrate(Granularity::Visualization)
        .expect("visualization view hydrates");
    let vis_doc_nodes = vis
        .graph()
        .node_weights()
        .filter(|v| v.kind.is_some_and(|k| k.is_documentation()))
        .count();
    assert!(
        vis_doc_nodes > 0,
        "the visualization view surfaces the doc layer — the guard is meaningful, not vacuous"
    );
    let vis_doc_edges = vis
        .graph()
        .edge_weights()
        .filter(|e| e.kind.is_some_and(|k| k.is_documentation()))
        .count();
    assert!(
        vis_doc_edges > 0,
        "the visualization view surfaces the cross-layer doc edges the code subgraph drops"
    );
    // Re-scan AFTER touching the visualization view: still byte-identical to the
    // code baseline (cycles included via acyclicity) — the view moved no metric.
    let after_vis = engine.scan(true).expect("scan runs").metrics;
    assert_metrics_byte_identical(&base, &after_vis);

    // ── S-122 / FR-UI-15 metric-neutrality guard ────────────────────────────
    // Exercise `graph_elements` with the new server-side re-budgeting `layers` /
    // `edge_types` filters present — every combination (a layer subset, an edge-type
    // subset, and the empty "hide everything" filter). These read the presentation-
    // only visualization view exactly like the unfiltered call, so re-scanning after
    // them must STILL be byte-identical to the code baseline: the filters narrow only
    // the presentation snapshot, never a metric/algorithm path (FR-QM-08, ADR-34).
    use logos_core::model::EdgeKind;
    use logos_core::models::navigation::{GraphGranularity, GraphLayer};
    let _ = engine.graph_elements(None, None, Some(&[GraphLayer::Code]), None, None, false);
    let _ = engine.graph_elements(None, None, None, Some(&[EdgeKind::Calls, EdgeKind::Imports]), None, false);
    let _ = engine.graph_elements(
        None,
        None,
        Some(&[GraphLayer::Code, GraphLayer::Doc]),
        Some(&[EdgeKind::Calls]),
        None,
        false,
    );
    let _ = engine.graph_elements(None, None, Some(&[]), Some(&[]), None, false);
    let after_filtered = engine.scan(true).expect("scan runs").metrics;
    assert_metrics_byte_identical(&base, &after_filtered);

    // ── S-124 / FR-UI-15 / ADR-36 metric-neutrality guard ───────────────────
    // Exercise `graph_elements` at every semantic cluster-zoom tier — module-rollup,
    // file-rollup, and the symbol/visualization view, with and without the
    // re-budgeting filters. These read the EXISTING rollup / visualization hydration
    // views for presentation only; none is on a metric/cycle/DSM/dead-code path, so
    // re-scanning after them must STILL be byte-identical to the code baseline: a
    // cluster tier reads a rollup view, it never moves the aggregate signal
    // (FR-QM-08, FR-DG-06, ADR-34, ADR-36).
    for tier in [
        GraphGranularity::Module,
        GraphGranularity::File,
        GraphGranularity::Symbol,
    ] {
        let _ = engine.graph_elements(None, None, None, None, Some(tier), false);
        let _ = engine.graph_elements(None, Some(3), Some(&[GraphLayer::Code]), None, Some(tier), false);
    }
    let after_tiers = engine.scan(true).expect("scan runs").metrics;
    assert_metrics_byte_identical(&base, &after_tiers);

    // ── S-128 / FR-UI-16 / ADR-37 metric-neutrality guard ───────────────────
    // Exercise `graph_elements` with the documentation-intent overlay ON — whole-graph
    // and filtered. The overlay reads the SAME presentation-only visualization view
    // (it only admits already-bound governing-doc nodes adjacent to the kept code, via
    // existing DocReference/TracesTo edges); it adds no edge kind, binding, query verb,
    // or metric path, so re-scanning after it must STILL be byte-identical to the code
    // baseline — the intent overlay is presentation-only (FR-DG-06, FR-QM-08, ADR-34,
    // ADR-37, NFR-RA-05).
    let _ = engine.graph_elements(None, None, None, None, None, true);
    let _ = engine.graph_elements(None, Some(3), Some(&[GraphLayer::Code, GraphLayer::Doc]), None, None, true);
    let after_intent = engine.scan(true).expect("scan runs").metrics;
    assert_metrics_byte_identical(&base, &after_intent);

    // The `dsm` graph-algorithm consumer is likewise unmoved after reading the rollup
    // views: its dimension matches the code-only baseline — proof the module/file
    // rollups feed no DSM/metric path (ADR-34, FR-QM-08).
    let after_tiers_dsm = engine
        .dsm(Some(logos_core::governance::DsmGranularity::File), true)
        .expect("dsm runs");
    assert_eq!(
        base_dsm.rows.len(),
        after_tiers_dsm.rows.len(),
        "reading the rollup views for the cluster-zoom tiers moved the DSM dimension",
    );

    // The `dsm` report (a graph-algorithm consumer) lists no documentation file.
    let with_docs_dsm = engine
        .dsm(Some(logos_core::governance::DsmGranularity::File), true)
        .expect("dsm runs");
    assert!(
        with_docs_dsm.rows.iter().all(|r| !r.name.ends_with(".md")),
        "a documentation file appeared in the DSM: {:?}",
        with_docs_dsm.rows
    );
    assert_eq!(
        base_dsm.rows.len(),
        with_docs_dsm.rows.len(),
        "documentation changed the DSM dimension"
    );

    // Remove the docs and re-score: the signal returns to the code baseline
    // (UAT-DG-04: the round trip is byte-identical in both directions).
    fs::remove_dir_all(tmp.path().join("docs")).unwrap();
    fs::remove_file(tmp.path().join("README.md")).unwrap();
    let removed = engine.scan(true).expect("scan runs").metrics;
    assert_eq!(
        documentation_node_count(rt),
        0,
        "the docs are gone from the graph"
    );
    assert_metrics_byte_identical(&base, &removed);
}
