//! Integration tests for the read-only graph-elements accessor
//! (`Engine::graph_elements`, S-084 / FR-UI-08, FR-DB-05, ADR-29), exercised
//! end-to-end through the [`Engine`] façade against a real temp-directory
//! fixture.
//!
//! Coverage by acceptance criterion:
//! - the accessor returns nodes+edges (each with layer and edge-type) from the
//!   hydrated graph, and every element traces to a graph field — none fabricated
//!   (FR-UI-08, NFR-RA-05);
//! - calling it once and repeatedly mutates no store (FR-UI-03, ADR-28) — the
//!   read-only-discipline fitness assertion;
//! - it honours the visible-element cap / level-of-detail bound for both
//!   whole-graph and seed-scoped modes, and reports the elided count so the
//!   frontend can show an honest "N more not shown" notice (NFR-CC-04);
//! - an unknown seed degrades to an honest empty snapshot with a warning
//!   (NFR-RA-05), never a fabricated node;
//! - the output is deterministic across repeated calls (NFR-RA-06).
//!
//! Gated on `lang-rust`: the fixture is a Rust crate, so the tests only run when
//! the Rust grammar is compiled in (matching the indexing tests' posture).
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::models::navigation::{GraphGranularity, GraphLayer};
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A fixture crate with a known call chain spanning several symbols:
/// `alpha → {run, beta}`, `beta → gamma`, `run` in its own file — enough nodes
/// and edges to exercise whole-graph, seed-scoping, and cap elision.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use crate::util::run;

pub fn alpha() {
    run();
    beta();
}
pub fn beta() {
    gamma();
}
pub fn gamma() {}
",
    );
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");
    // A standalone, unreferenced function in its own file: nothing calls, is
    // called by, imports, or references it, so it sits in a graph component
    // disconnected from the alpha/beta/gamma/run chain — the out-of-scope
    // element a seed-scoped snapshot must exclude.
    write(tmp.path(), "src/island.rs", "pub fn island() {}\n");
    tmp
}

/// An indexed long-lived engine over the fixture (hydration needs `start`).
fn indexed_engine(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

// ── FR-UI-08 / NFR-RA-05: nodes+edges with layer & edge-type, none fabricated ─

#[test]
fn whole_graph_returns_layered_nodes_and_typed_edges_that_trace_to_the_graph() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let elements = engine.graph_elements(None, None, None, None, None, false);
    assert!(elements.warnings.is_empty(), "{:?}", elements.warnings);
    assert_eq!(elements.seed, None, "whole-graph snapshot echoes no seed");

    // Nodes are present. This fixture is pure Rust (no doc/artifact files), so the
    // Visualization view graph_elements() now hydrates surfaces only code-layer
    // nodes here — a property of the code-only fixture, NOT of the view (which also
    // carries doc/artifact layers given non-code files; see
    // whole_graph_surfaces_the_doc_and_artifact_layers_layer_tagged). Each node
    // carries a kind — none fabricated (NFR-RA-05).
    assert!(elements.nodes.len() >= 4, "the call chain's functions are present");
    assert!(
        elements.nodes.iter().all(|n| n.layer == GraphLayer::Code),
        "the code-only fixture yields only code-layer nodes",
    );
    assert!(
        elements.nodes.iter().all(|n| !n.id.is_empty() && !n.label.is_empty()),
        "every node carries its symbol id and label from the graph",
    );

    // Edges are typed and both endpoints reference a rendered node id — the edge
    // traces to a graph field, never dangling (NFR-RA-05).
    assert!(!elements.edges.is_empty(), "the call chain yields dependency edges");
    let ids: std::collections::HashSet<&str> =
        elements.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &elements.edges {
        assert!(
            ids.contains(edge.source.as_str()) && ids.contains(edge.target.as_str()),
            "edge {edge:?} connects two rendered nodes",
        );
    }
    // The full graph fits under the default cap, so nothing is elided.
    assert_eq!(elements.elided_nodes, 0, "no elision when the graph fits the cap");
    assert_eq!(elements.elided_edges, 0);
    assert_eq!(elements.total_nodes as usize, elements.nodes.len());
    // CR-030/S-119 lowered the default visible-element cap to 250 so the canvas
    // opens sparser — pin the value so a silent change fails here (the `?cap=`
    // override and additive-expand still widen the rendered set on demand).
    assert_eq!(elements.cap, 250, "the default visible-element cap is the lowered 250");
}

// ── FR-UI-08 / ADR-34: the canvas receives the doc & artifact layers ─────────

/// After S-113 / [ADR-34] `graph_elements()` hydrates the presentation-only
/// visualization view, so the whole-graph snapshot now carries the **doc** and
/// **artifact** layers and their cross-layer edges — each layer-tagged — not just
/// the code subgraph. Before this change `graph_elements()` read the code-subgraph
/// view, so deselecting "code" on the canvas yielded an empty graph. Here we index
/// a fixture with a markdown doc (that links another doc and references code) and a
/// YAML config artifact, then assert all three layers reach the snapshot and a
/// layer-tagged `doc_reference` cross-layer edge is present.
///
/// Gated additionally on the markdown + YAML grammars: without them no doc/config
/// nodes are ingested and the layer assertions would be vacuous.
#[cfg(all(feature = "lang-markdown", feature = "lang-yaml"))]
#[test]
fn whole_graph_surfaces_the_doc_and_artifact_layers_layer_tagged() {
    use logos_core::model::EdgeKind;

    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn compute() -> u32 { 41 }
");
    // Two docs with an anchored doc->doc link (binds to the target DocSection —
    // the exactly-one-candidate case) plus a code span referencing `compute`.
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide

## Overview

The entry point is `compute` in `src/lib.rs`.
         See [the reference](reference.md#compute) for details.
",
    );
    write(
        tmp.path(),
        "docs/reference.md",
        "# Reference

## Compute

`compute` returns a constant.
",
    );
    // A YAML config artifact — its ConfigFile/ConfigSection nodes render in the
    // artifact layer.
    write(
        tmp.path(),
        "service.yaml",
        "service:
  name: demo
  port: 8080
",
    );

    let engine = indexed_engine(&tmp);
    let elements = engine.graph_elements(None, None, None, None, None, false);
    assert!(elements.warnings.is_empty(), "{:?}", elements.warnings);

    // All three presentation layers reach the canvas — the headline ADR-34 fix.
    assert!(
        elements.nodes.iter().any(|n| n.layer == GraphLayer::Code),
        "the code layer is present",
    );
    assert!(
        elements.nodes.iter().any(|n| n.layer == GraphLayer::Doc),
        "the doc layer reaches the canvas (was empty before ADR-34): {:?}",
        elements.nodes,
    );
    assert!(
        elements.nodes.iter().any(|n| n.layer == GraphLayer::Artifact),
        "the artifact layer reaches the canvas: {:?}",
        elements.nodes,
    );

    // Nothing fabricated: every edge connects two rendered nodes (NFR-RA-05).
    let layer_of: std::collections::HashMap<&str, GraphLayer> =
        elements.nodes.iter().map(|n| (n.id.as_str(), n.layer)).collect();
    for e in &elements.edges {
        assert!(
            layer_of.contains_key(e.source.as_str())
                && layer_of.contains_key(e.target.as_str()),
            "edge {e:?} connects two rendered nodes",
        );
    }

    // The load-bearing cross-layer edge: a `doc_reference` touching a Doc-layer
    // node — surfaced to the canvas only because the visualization view keeps it
    // (the code subgraph drops every doc edge).
    let has_doc_reference_edge = elements.edges.iter().any(|e| {
        e.edge_type == Some(EdgeKind::DocReference)
            && (layer_of.get(e.source.as_str()) == Some(&GraphLayer::Doc)
                || layer_of.get(e.target.as_str()) == Some(&GraphLayer::Doc))
    });
    assert!(
        has_doc_reference_edge,
        "a layer-tagged doc_reference cross-layer edge reaches the canvas: edges={:?}",
        elements.edges,
    );
}

// ── FR-UI-03 / ADR-28: calling once and repeatedly mutates no store ──────────

#[test]
fn repeated_calls_mutate_no_store() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let before = engine.status();
    // Hammer the accessor in whole-graph and seed-scoped modes.
    for _ in 0..5 {
        let _ = engine.graph_elements(None, None, None, None, None, false);
        let _ = engine.graph_elements(Some("alpha"), Some(2), None, None, None, false);
    }
    let after = engine.status();

    assert_eq!(
        (before.node_count, before.edge_count, before.file_count),
        (after.node_count, after.edge_count, after.file_count),
        "a read-only graph-elements call writes nothing to the store (ADR-28)",
    );
}

// ── NFR-CC-04: the cap / level-of-detail bound reports the elided remainder ──

#[test]
fn a_tight_cap_elides_the_remainder_and_reports_it() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.graph_elements(None, None, None, None, None, false);
    let capped = engine.graph_elements(None, Some(2), None, None, None, false);

    assert_eq!(capped.cap, 2);
    assert_eq!(capped.nodes.len(), 2, "the cap bounds the rendered node count");
    assert_eq!(
        capped.total_nodes, full.total_nodes,
        "the cap narrows what is rendered, not the honest in-scope total",
    );
    assert_eq!(
        capped.elided_nodes,
        full.total_nodes - 2,
        "the elided count is the exact remainder, never a silent truncation (NFR-CC-04)",
    );
    // Edges among only the 2 kept nodes are a subset; the elided-edge tally is
    // the honest remainder of the in-scope edges.
    assert_eq!(
        capped.elided_edges,
        capped.total_edges - capped.edges.len() as u32,
    );
}

// ── FR-UI-08: seed-scoped mode narrows to the seed's neighbourhood ───────────

#[test]
fn seed_scoped_mode_returns_the_seed_neighbourhood() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    // The seed is the canonical SCIP symbol string (what the web `/graph?seed=`
    // link carries), not the human name — resolve it via search, exactly as the
    // canvas's search-to-locate will.
    let hit = engine.search("gamma", None, None);
    let gamma = &hit.hits.iter().find(|h| h.name == "gamma").expect("gamma resolves").symbol;

    let whole = engine.graph_elements(None, None, None, None, None, false);
    let scoped = engine.graph_elements(Some(gamma), None, None, None, None, false);

    assert_eq!(scoped.seed.as_deref(), Some(gamma.as_str()), "the seed is echoed back");
    assert!(scoped.warnings.is_empty(), "{:?}", scoped.warnings);
    assert!(
        scoped.nodes.iter().any(|n| n.label == "gamma"),
        "the seed itself is in its neighbourhood",
    );
    // Seed-scoping must STRICTLY narrow the graph: `island` is a disconnected
    // component, so it is in the whole graph but never in gamma's neighbourhood.
    assert!(
        whole.nodes.iter().any(|n| n.label == "island"),
        "the whole graph includes the disconnected island",
    );
    assert!(
        scoped.total_nodes < whole.total_nodes,
        "seed-scoping strictly narrows the graph ({} vs {})",
        scoped.total_nodes,
        whole.total_nodes,
    );
    assert!(
        !scoped.nodes.iter().any(|n| n.label == "island"),
        "the disconnected island is excluded from gamma's neighbourhood",
    );
}

// ── NFR-CC-04: the cap / elided-count report holds in SEED-SCOPED mode too ───

#[test]
fn seed_scoped_cap_elides_the_remainder_and_reports_it() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let hit = engine.search("gamma", None, None);
    let gamma = &hit.hits.iter().find(|h| h.name == "gamma").expect("gamma resolves").symbol;

    let full = engine.graph_elements(Some(gamma), None, None, None, None, false);
    assert!(full.total_nodes >= 2, "gamma's neighbourhood spans several nodes");
    let capped = engine.graph_elements(Some(gamma), Some(1), None, None, None, false);

    assert_eq!(capped.seed.as_deref(), Some(gamma.as_str()));
    assert_eq!(capped.cap, 1);
    assert_eq!(capped.nodes.len(), 1, "the cap bounds the seed-scoped node count");
    assert_eq!(
        capped.total_nodes, full.total_nodes,
        "the cap narrows what is rendered, not the seed neighbourhood's honest total",
    );
    assert_eq!(
        capped.elided_nodes,
        full.total_nodes - 1,
        "the seed-scoped elided count is the exact remainder (NFR-CC-04)",
    );
    assert_eq!(capped.elided_edges, capped.total_edges - capped.edges.len() as u32);
}

// ── Issue 1: the seed survives the cap (proximity rank pins distance 0) ──────

#[test]
fn seed_scoped_cap_always_keeps_the_seed_node() {
    // Regression: focusing a node must keep the node itself even under a tight cap.
    // `reachable_from` of a node is often most of the graph, and the seed is
    // frequently low-degree (a leaf like `gamma`), so the prior global-degree LOD
    // rank could truncate the seed out of the cap — leaving the focus snapshot
    // indistinguishable from the whole graph and nothing for the canvas to ring or
    // center. The seeded LOD now ranks by BFS proximity, so the seed (distance 0)
    // is always retained. `gamma` is the lowest-degree node in its component, yet a
    // cap of 1 must keep it, not the component's highest-degree hub.
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let hit = engine.search("gamma", None, None);
    let gamma = &hit.hits.iter().find(|h| h.name == "gamma").expect("gamma resolves").symbol;

    let capped = engine.graph_elements(Some(gamma), Some(1), None, None, None, false);
    assert_eq!(capped.nodes.len(), 1, "the cap bounds the rendered set to one node");
    assert_eq!(
        capped.nodes[0].label, "gamma",
        "the seed itself survives the cap (proximity rank pins the distance-0 seed), \
         not the neighbourhood's highest-degree hub",
    );
}

// ── NFR-CC-04: cap = 0 honestly elides everything without underflow ──────────

#[test]
fn cap_zero_elides_everything_without_underflow() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.graph_elements(None, None, None, None, None, false);
    let zero = engine.graph_elements(None, Some(0), None, None, None, false);

    assert_eq!(zero.cap, 0);
    assert!(zero.nodes.is_empty(), "cap 0 renders no nodes");
    assert!(zero.edges.is_empty(), "cap 0 renders no edges");
    assert_eq!(zero.total_nodes, full.total_nodes, "the in-scope total is honest");
    assert_eq!(zero.elided_nodes, full.total_nodes, "every node is honestly elided");
    assert_eq!(zero.elided_edges, zero.total_edges, "every in-scope edge is elided");
}

// ── NFR-RA-05: an unknown seed is honest-empty, never fabricated ─────────────

#[test]
fn unknown_seed_yields_an_honest_empty_snapshot_with_a_warning() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let elements = engine.graph_elements(Some("does_not_exist"), None, None, None, None, false);
    assert_eq!(elements.seed.as_deref(), Some("does_not_exist"));
    assert!(elements.nodes.is_empty(), "no node is fabricated for an unknown seed");
    assert!(elements.edges.is_empty());
    assert!(
        elements.warnings.iter().any(|w| w.contains("unknown seed")),
        "the unknown seed is reported honestly: {:?}",
        elements.warnings,
    );
}

// ── NFR-RA-06: the snapshot is deterministic across repeated calls ───────────

#[test]
fn output_is_deterministic_across_calls() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let a = serde_json::to_value(engine.graph_elements(None, None, None, None, None, false)).unwrap();
    let b = serde_json::to_value(engine.graph_elements(None, None, None, None, None, false)).unwrap();
    assert_eq!(a, b, "same graph state ⇒ byte-identical elements (NFR-RA-06)");
}

// ── FR-UI-15 / S-122: the layer filter re-budgets BEFORE the cap ─────────────

/// A fixture mixing a code call-chain with a documentation file carrying many
/// headings. The `DocFile` accrues a high `Contains` degree (one edge per
/// section), so it is the single highest-degree vertex of the whole graph and is
/// kept under a tight cap — occupying a slot a code node would otherwise take.
/// That is exactly what the layer re-budget must reclaim.
#[cfg(feature = "lang-markdown")]
fn layered_rebudget_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Six code symbols: a module plus a five-function call chain.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn a() { b(); }
pub fn b() { c(); }
pub fn c() { d(); }
pub fn d() { e(); }
pub fn e() {}
",
    );
    // A doc file with six sibling headings: its DocFile node Contains all six, so
    // its degree (≥6) tops every code node (the module's Contains degree is 5, the
    // functions' call degree is ≤2) — guaranteeing a doc node sits in the unfiltered
    // top-cap and displaces a code node.
    write(
        tmp.path(),
        "docs/guide.md",
        "\
# Guide

## One

## Two

## Three

## Four

## Five

## Six
",
    );
    tmp
}

/// Deselecting a layer must **re-budget**, not merely shrink: the cap is re-spent
/// over the remaining layers, so a filtered fetch surfaces previously-elided nodes
/// of the remaining scope — it is *not* a strict subset of the unfiltered fetch
/// ([FR-UI-15], [NFR-CC-04]). Here a tight cap is partly spent on the high-degree
/// `DocFile` when unfiltered; filtering to the code layer reclaims that budget for
/// code nodes that were elided before.
#[cfg(feature = "lang-markdown")]
#[test]
fn layer_filter_refills_the_budget_with_previously_elided_nodes() {
    let tmp = layered_rebudget_fixture();
    let engine = indexed_engine(&tmp);

    const CAP: usize = 3;

    // Unfiltered at the tight cap: the doc layer is present and the high-degree
    // DocFile claims at least one of the three slots (the guard below proves it).
    let unfiltered = engine.graph_elements(None, Some(CAP), None, None, None, false);
    let code_in_unfiltered =
        unfiltered.nodes.iter().filter(|n| n.layer == GraphLayer::Code).count();
    let doc_in_unfiltered =
        unfiltered.nodes.iter().filter(|n| n.layer != GraphLayer::Code).count();
    assert_eq!(unfiltered.nodes.len(), CAP, "the cap bounds the unfiltered set");
    assert!(
        doc_in_unfiltered >= 1,
        "the high-degree DocFile occupies a slot unfiltered — the re-budget guard is meaningful: {:?}",
        unfiltered.nodes,
    );

    // Filter to the code layer at the SAME cap: the freed slot(s) backfill with
    // code nodes, so strictly more code nodes are shown than when unfiltered.
    let code_only =
        engine.graph_elements(None, Some(CAP), Some(&[GraphLayer::Code]), None, None, false);
    assert!(
        code_only.nodes.iter().all(|n| n.layer == GraphLayer::Code),
        "the layer filter returns only code-layer nodes: {:?}",
        code_only.nodes,
    );
    assert!(
        code_only.nodes.len() > code_in_unfiltered,
        "deselecting the doc/artifact layers backfills the freed budget with code nodes \
         ({} code shown filtered vs {} unfiltered) — re-budget, not mere shrink",
        code_only.nodes.len(),
        code_in_unfiltered,
    );

    // The elided count and in-scope total re-base on the FILTERED snapshot: the
    // honest denominator is now the code-only scope, not the whole graph (NFR-CC-04).
    assert!(
        code_only.total_nodes < unfiltered.total_nodes,
        "the in-scope total re-bases on the filtered scope ({} < {})",
        code_only.total_nodes,
        unfiltered.total_nodes,
    );
    assert_eq!(
        code_only.elided_nodes,
        code_only.total_nodes - code_only.nodes.len() as u32,
        "the elided count re-bases on the filtered snapshot's own total (NFR-CC-04)",
    );
}

/// Deselecting **every** layer is an honest empty graph, not a silent fall-back to
/// all layers — the user who unchecked all three boxes asked for nothing
/// ([FR-UI-15]).
#[test]
fn empty_layer_filter_yields_an_honest_empty_graph() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let none = engine.graph_elements(None, None, Some(&[]), None, None, false);
    assert!(none.nodes.is_empty(), "no layer selected ⇒ no nodes: {:?}", none.nodes);
    assert!(none.edges.is_empty(), "no nodes ⇒ no edges");
    assert_eq!(none.total_nodes, 0, "the filtered in-scope total is honestly zero");
    assert_eq!(none.elided_nodes, 0, "nothing in scope ⇒ nothing elided");

    // The same honest-empty contract holds at a rollup tier: a cluster is the code
    // layer by construction, so deselecting every layer (the empty allowed-set) still
    // matches nothing and empties the tier — not a silent fall-back to all layers
    // (S-124 / FR-UI-15, NFR-CC-04).
    let none_module =
        engine.graph_elements(None, None, Some(&[]), None, Some(GraphGranularity::Module), false);
    assert!(
        none_module.nodes.is_empty(),
        "no layer selected ⇒ no module clusters: {:?}",
        none_module.nodes,
    );
    assert_eq!(none_module.total_nodes, 0, "the rollup tier's filtered total is honestly zero");
    assert_eq!(none_module.elided_nodes, 0, "nothing in scope ⇒ nothing elided at the rollup tier");
}

// ── FR-UI-15 / S-122: the edge-type filter re-budgets the ranking ────────────

/// A fixture with two distinct degree hubs: a module whose `Contains` degree makes
/// it the unfiltered degree leader (modules never call), and a call target whose
/// `Calls` in-degree makes it the leader once the ranking counts only `calls`
/// edges. The single highest-degree node therefore *flips* under an `edge_types`
/// filter — the edge-type re-budget made visible.
fn edge_rebudget_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // A module containing five functions: its Contains degree (≥5) tops the graph
    // when every edge counts, but drops to zero once only `calls` edges count.
    write(
        tmp.path(),
        "src/big.rs",
        "\
pub fn b1() {}
pub fn b2() {}
pub fn b3() {}
pub fn b4() {}
pub fn b5() {}
",
    );
    // A call target reached by three callers: its `calls` in-degree (3) is the
    // highest once only `calls` edges count.
    write(
        tmp.path(),
        "src/call.rs",
        "\
pub fn target() {}
pub fn k1() { target(); }
pub fn k2() { target(); }
pub fn k3() { target(); }
",
    );
    write(tmp.path(), "src/lib.rs", "pub mod big;\npub mod call;\n");
    tmp
}

/// An `edge_types` filter must re-budget the degree ranking, not just hide edges:
/// the single kept node under a cap of 1 flips when only `calls` edges count,
/// because the unfiltered degree leader (a high-`Contains` module) has zero calls
/// ([FR-UI-15]). This proves the filter is applied *before* the degree-rank, so
/// deselecting an edge type admits previously-elided nodes of the remaining scope.
#[test]
fn edge_type_filter_rebudgets_the_degree_ranking() {
    let tmp = edge_rebudget_fixture();
    let engine = indexed_engine(&tmp);

    let unfiltered = engine.graph_elements(None, Some(1), None, None, None, false);
    let calls_only =
        engine.graph_elements(None, Some(1), None, Some(&[logos_core::model::EdgeKind::Calls]), None, false);

    assert_eq!(unfiltered.nodes.len(), 1, "cap 1 keeps a single node");
    assert_eq!(calls_only.nodes.len(), 1, "cap 1 keeps a single node under the filter too");
    assert_ne!(
        calls_only.nodes[0].id, unfiltered.nodes[0].id,
        "the kept node flips under an edge-type filter — the degree-rank re-budgets \
         over the remaining edge structure, admitting a previously-elided node",
    );
}

/// An `edge_types` filter restricts the rendered edges to the allowed types and
/// re-bases the in-scope edge total and the elided-edge tally on the filtered
/// snapshot ([FR-UI-15], [NFR-CC-04]).
#[test]
fn edge_type_filter_restricts_edges_and_rebases_the_tally() {
    use logos_core::model::EdgeKind;

    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.graph_elements(None, None, None, None, None, false);
    let calls_only = engine.graph_elements(None, None, None, Some(&[EdgeKind::Calls]), None, false);

    assert!(
        calls_only.edges.iter().all(|e| e.edge_type == Some(EdgeKind::Calls)),
        "only the allowed edge type is rendered: {:?}",
        calls_only.edges,
    );
    assert!(
        calls_only.total_edges < full.total_edges,
        "the in-scope edge total re-bases on the filtered edge set ({} < {})",
        calls_only.total_edges,
        full.total_edges,
    );
    assert_eq!(
        calls_only.elided_edges,
        calls_only.total_edges - calls_only.edges.len() as u32,
        "the elided-edge count re-bases on the filtered snapshot (NFR-CC-04)",
    );
    // The load-bearing distinction from a LAYER filter: an edge-type filter
    // re-ranks and re-selects but never removes nodes from the in-scope candidate
    // set, so `total_nodes` is invariant under it. (A regression that mistakenly
    // `retain`ed nodes by `edge_allowed` would change this — the guard catches it.)
    assert_eq!(
        calls_only.total_nodes, full.total_nodes,
        "an edge-type filter leaves the in-scope node count unchanged (re-ranks, never re-scopes nodes)",
    );
}

/// Deselecting **every** edge type hides every edge but leaves the node scope
/// intact — the honest empty-edge counterpart of the empty-layer case, and the
/// invariant that distinguishes an edge filter (re-ranks) from a layer filter
/// (re-scopes) ([FR-UI-15], [NFR-CC-04]).
#[test]
fn empty_edge_type_filter_yields_honest_empty_edges_but_keeps_the_nodes() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.graph_elements(None, None, None, None, None, false);
    let no_edges = engine.graph_elements(None, None, None, Some(&[]), None, false);

    assert!(no_edges.edges.is_empty(), "no edge type selected ⇒ no edges: {:?}", no_edges.edges);
    assert_eq!(no_edges.total_edges, 0, "the filtered in-scope edge total is honestly zero");
    assert_eq!(no_edges.elided_edges, 0, "no edge in scope ⇒ nothing elided");
    // Nodes are untouched by a pure edge filter — the scope is the same set.
    assert_eq!(
        no_edges.total_nodes, full.total_nodes,
        "an edge filter never removes nodes from scope",
    );
    assert_eq!(
        no_edges.nodes.len(),
        full.nodes.len(),
        "the same nodes render with every edge hidden (only edges vanish)",
    );
}

// ── FR-UI-15 / S-124 / ADR-36: the granularity parameter selects the tier ────

/// `granularity = None` is the **default symbol tier** — the visualization view —
/// and is byte-identical to an explicit `Some(Symbol)`: an unparameterized request
/// behaves exactly as it did before S-124 ([FR-UI-15], [ADR-34], [NFR-RA-06]).
#[test]
fn default_granularity_is_the_symbol_visualization_view() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let default = engine.graph_elements(None, None, None, None, None, false);
    let symbol = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Symbol), false);

    assert_eq!(default.granularity, GraphGranularity::Symbol, "the default tier is symbol");
    assert_eq!(
        serde_json::to_value(&default).unwrap(),
        serde_json::to_value(&symbol).unwrap(),
        "an unparameterized request equals an explicit symbol-tier request",
    );
    // Every symbol-tier vertex carries a kind and every edge a type — clusters are a
    // rollup-tier concept only (the guard the file/module tests below complement).
    assert!(default.nodes.iter().all(|n| n.kind.is_some()), "symbol-tier nodes carry a kind");
    assert!(
        default.edges.iter().all(|e| e.edge_type.is_some()),
        "symbol-tier edges carry a type",
    );
}

/// `granularity = Module` selects the **module-rollup view**
/// ([`Granularity::Module`]): vertices are module **clusters** (aggregates carrying
/// no per-node kind, serialised `null`) in the code layer, and edges are aggregated
/// dependencies (no per-edge type). The tier is echoed back and is strictly coarser
/// than the symbol tier — proof the parameter selects the existing rollup view, not
/// the visualization view ([FR-UI-15], [ADR-36], [FR-DB-05]).
#[test]
fn granularity_module_selects_the_module_rollup_cluster_view() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let symbol = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Symbol), false);
    let module = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Module), false);

    assert_eq!(module.granularity, GraphGranularity::Module, "the module tier is echoed back");
    assert!(!module.nodes.is_empty(), "the module rollup yields cluster vertices");
    // A rollup cluster is an aggregate: no per-node kind, code-layer by construction
    // (the rollup is the code subgraph — docs/artifacts excluded, FR-DG-06).
    assert!(
        module.nodes.iter().all(|n| n.kind.is_none()),
        "module-rollup cluster vertices carry no kind (an aggregate has no single kind): {:?}",
        module.nodes,
    );
    assert!(
        module.nodes.iter().all(|n| n.layer == GraphLayer::Code),
        "module clusters render in the code layer",
    );
    assert!(
        module.edges.iter().all(|e| e.edge_type.is_none()),
        "module-rollup edges are aggregated dependencies with no single type",
    );
    // The rollup is strictly coarser: fewer cluster vertices than symbols.
    assert!(
        module.total_nodes < symbol.total_nodes,
        "the module rollup is coarser than the symbol tier ({} clusters < {} symbols)",
        module.total_nodes,
        symbol.total_nodes,
    );
}

/// `granularity = File` selects the **file-rollup view** ([`Granularity::File`]):
/// vertices are file **clusters** keyed/labelled by path. Same cluster shape as the
/// module tier (no per-node kind, code layer), and coarser than the symbol tier
/// ([FR-UI-15], [ADR-36], [FR-DB-05]).
#[test]
fn granularity_file_selects_the_file_rollup_cluster_view() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let symbol = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Symbol), false);
    let file = engine.graph_elements(None, None, None, None, Some(GraphGranularity::File), false);

    assert_eq!(file.granularity, GraphGranularity::File, "the file tier is echoed back");
    assert!(!file.nodes.is_empty(), "the file rollup yields cluster vertices");
    assert!(
        file.nodes.iter().all(|n| n.kind.is_none()),
        "file-rollup cluster vertices carry no kind: {:?}",
        file.nodes,
    );
    assert!(
        file.edges.iter().all(|e| e.edge_type.is_none()),
        "file-rollup edges are aggregated dependencies with no single type (parity with the module tier): {:?}",
        file.edges,
    );
    assert!(
        file.nodes.iter().all(|n| n.layer == GraphLayer::Code),
        "file clusters render in the code layer",
    );
    // The altitude ladder is two-level coarse: the module rollup is at least as
    // coarse as the file rollup (ADR-36 — low zoom-out aggregates further). For a
    // fixture without explicit `mod` blocks the two coincide, so the bound is `<=`.
    let module = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Module), false);
    assert!(
        module.total_nodes <= file.total_nodes,
        "the module rollup is at least as coarse as the file rollup ({} modules <= {} files)",
        module.total_nodes,
        file.total_nodes,
    );
    // The fixture's three source files each become a file cluster; the labels are the
    // file paths (the rollup vertex key/label), so they trace to a graph field
    // (NFR-RA-05). At least one of the known fixture paths is present.
    assert!(
        file.nodes.iter().any(|n| n.label.ends_with(".rs")),
        "a file cluster is labelled by its path: {:?}",
        file.nodes,
    );
    assert!(
        file.total_nodes < symbol.total_nodes,
        "the file rollup is coarser than the symbol tier ({} files < {} symbols)",
        file.total_nodes,
        symbol.total_nodes,
    );
}

/// The `edge_types` re-budget filter is a **no-op at the rollup tiers** (S-124): a
/// rollup-aggregate edge spans mixed kinds and carries no single `edge_type`, so the
/// filter — which names concrete kinds — cannot apply to it and must pass it through
/// rather than zero out every cluster edge. This pins the load-bearing
/// `(Some(_), None) => true` arm of `edge_allowed`: an explicit edge-type filter at
/// the file tier yields the SAME aggregate edges as the unfiltered file tier.
#[test]
fn edge_type_filter_is_a_noop_at_the_rollup_tiers() {
    use logos_core::model::EdgeKind;

    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let unfiltered = engine.graph_elements(None, None, None, None, Some(GraphGranularity::File), false);
    assert!(
        !unfiltered.edges.is_empty(),
        "the call chain crosses files, so the file rollup has at least one aggregate edge",
    );

    // An explicit edge-type filter at the file tier must NOT drop the kind-less
    // aggregate edges (unlike the symbol tier, where it filters by concrete kind).
    let filtered = engine.graph_elements(
        None,
        None,
        None,
        Some(&[EdgeKind::Calls]),
        Some(GraphGranularity::File),
        false,
    );
    assert_eq!(
        filtered.edges.len(),
        unfiltered.edges.len(),
        "an edge-type filter is a no-op at the rollup tier — aggregate edges have no \
         filterable kind, so they pass through unchanged ({} filtered vs {} unfiltered)",
        filtered.edges.len(),
        unfiltered.edges.len(),
    );
    assert!(
        filtered.edges.iter().all(|e| e.edge_type.is_none()),
        "the passed-through rollup edges still carry no single type: {:?}",
        filtered.edges,
    );
}

/// FR-DG-06 (generalised to all non-code layers by CR-010/FR-CG-05): the module/file
/// rollup tiers are the **code subgraph** — documentation AND config/artifact
/// files/modules are excluded at hydration — so a cluster tier never surfaces a
/// doc- or artifact-layer node, even though the symbol (visualization) tier does.
/// Indexes a fixture with BOTH a markdown doc file and a YAML config artifact and
/// asserts each reaches the symbol tier (the guard is not vacuous on either axis)
/// but no non-code cluster appears at the file or module tier ([FR-DG-06], [ADR-34]).
///
/// Gated on the markdown + YAML grammars (like the visualization-layer test): without
/// them no doc/config nodes are ingested and the non-vacuity guards would be empty.
#[cfg(all(feature = "lang-markdown", feature = "lang-yaml"))]
#[test]
fn rollup_tiers_exclude_documentation_and_artifact_files_and_modules() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn compute() -> u32 { 41 }\n");
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\n## Overview\n\nThe entry point is `compute` in `src/lib.rs`.\n",
    );
    // A YAML config artifact — its ConfigFile/ConfigSection nodes render in the
    // artifact layer at the symbol tier and must be excluded from the rollups.
    write(tmp.path(), "service.yaml", "service:\n  name: demo\n  port: 8080\n");
    let engine = indexed_engine(&tmp);

    // The symbol (visualization) tier DOES surface the doc AND artifact layers — the
    // guards below are meaningful, not vacuous (this is the ADR-34 behaviour S-113
    // added; the rollups must drop both).
    let symbol = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Symbol), false);
    assert!(
        symbol.nodes.iter().any(|n| n.layer == GraphLayer::Doc),
        "the symbol tier surfaces the doc layer (doc guard is meaningful): {:?}",
        symbol.nodes,
    );
    assert!(
        symbol.nodes.iter().any(|n| n.layer == GraphLayer::Artifact),
        "the symbol tier surfaces the artifact layer (artifact guard is meaningful): {:?}",
        symbol.nodes,
    );

    // The file and module rollup tiers exclude documentation AND config/artifacts
    // entirely (FR-DG-06 / FR-CG-05): only the code layer, no `.md`/`.yaml` cluster.
    for tier in [GraphGranularity::File, GraphGranularity::Module] {
        let rollup = engine.graph_elements(None, None, None, None, Some(tier), false);
        assert!(
            rollup.nodes.iter().all(|n| n.layer == GraphLayer::Code),
            "{tier:?} rollup excludes non-code layers (FR-DG-06/FR-CG-05): {:?}",
            rollup.nodes,
        );
        assert!(
            rollup.nodes.iter().all(|n| !n.label.ends_with(".md") && !n.label.ends_with(".yaml")),
            "{tier:?} rollup excludes documentation/artifact files/modules (FR-DG-06/FR-CG-05): {:?}",
            rollup.nodes,
        );
    }
}

/// The cluster tiers honour the same read-only / honest-cap contract as the symbol
/// tier: a tight cap over the module rollup keeps the most-connected clusters and
/// reports the elided remainder, re-based on the rollup's own in-scope total
/// ([NFR-CC-04]) — the "N more not shown" notice stays correct across a tier switch.
#[test]
fn a_rollup_tier_honours_the_cap_and_reports_the_elided_remainder() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.graph_elements(None, None, None, None, Some(GraphGranularity::File), false);
    assert!(full.total_nodes >= 2, "the file rollup spans several clusters");
    let capped =
        engine.graph_elements(None, Some(1), None, None, Some(GraphGranularity::File), false);

    assert_eq!(capped.granularity, GraphGranularity::File);
    assert_eq!(capped.nodes.len(), 1, "the cap bounds the rendered cluster count");
    assert_eq!(
        capped.total_nodes, full.total_nodes,
        "the cap narrows what is rendered, not the rollup's honest in-scope total",
    );
    assert_eq!(
        capped.elided_nodes,
        full.total_nodes - 1,
        "the elided count re-bases on the rollup tier's own total (NFR-CC-04)",
    );
    assert_eq!(capped.elided_edges, capped.total_edges - capped.edges.len() as u32);

    // Across a tier SWITCH the elided count re-bases on the NEW tier's own total,
    // never a stale denominator carried from the previous tier — the "N more not
    // shown" notice stays correct across a tier switch (NFR-CC-04). A capped fetch at
    // the symbol tier and at the module tier each report `elided == total − rendered`
    // against their OWN total, and those totals differ (clusters vs symbols), proving
    // the denominator is per-tier.
    let capped_symbol = engine.graph_elements(None, Some(1), None, None, Some(GraphGranularity::Symbol), false);
    let capped_module = engine.graph_elements(None, Some(1), None, None, Some(GraphGranularity::Module), false);
    assert_eq!(
        capped_symbol.elided_nodes,
        capped_symbol.total_nodes - 1,
        "the symbol tier's elided count re-bases on its own total after a tier switch",
    );
    assert_eq!(
        capped_module.elided_nodes,
        capped_module.total_nodes - 1,
        "the module tier's elided count re-bases on its own total after a tier switch",
    );
    assert_ne!(
        capped_symbol.total_nodes, capped_module.total_nodes,
        "the symbol and module tiers have different in-scope totals — the elided \
         denominator is per-tier, not shared across a tier switch",
    );
}

// ── FR-UI-16 / ADR-37 / S-128: the bounded documentation-intent overlay ──────

/// With no doc→code intent edges in the graph the overlay has nothing to admit, so
/// turning it on is a **byte-identical no-op** — proving it fabricates nothing
/// ([NFR-RA-05]) and that the off-path is unchanged. The fixture is pure code, so
/// there is no governing-doc node for any `DocReference`/`TracesTo` edge to anchor.
#[test]
fn intent_overlay_is_a_byte_identical_noop_without_governing_docs() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let off = serde_json::to_value(engine.graph_elements(None, None, None, None, None, false)).unwrap();
    let on = serde_json::to_value(engine.graph_elements(None, None, None, None, None, true)).unwrap();
    assert_eq!(
        off, on,
        "with no governing-doc nodes the intent overlay admits nothing — byte-identical \
         (FR-UI-16 off-path, NFR-RA-05)",
    );
}

/// A fixture whose `target` is a four-caller hub — an unambiguously high-degree code
/// node that survives a tight cap as a kept code anchor — plus a governing doc whose
/// Rationale section references `target` over a `DocReference` intent edge ([FR-DG-04]).
/// The doc node is low-degree, so the structural degree budget evicts it first: the
/// exact [CR-014] eviction the overlay must rescue without disturbing the code anchor.
#[cfg(feature = "lang-markdown")]
fn intent_overlay_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn target() {}
pub fn k1() { target(); }
pub fn k2() { target(); }
pub fn k3() { target(); }
pub fn k4() { target(); }
",
    );
    write(
        tmp.path(),
        "docs/guide.md",
        "\
# Guide

## Rationale

The entry point is `target` in `src/lib.rs`.
",
    );
    tmp
}

/// The canonical SCIP symbol for `target` (the kept code anchor), resolved exactly
/// as the canvas's search-to-locate does.
#[cfg(feature = "lang-markdown")]
fn target_symbol(engine: &Engine) -> String {
    let hit = engine.search("target", None, None);
    hit.hits
        .iter()
        .find(|h| h.name == "target")
        .expect("target resolves")
        .symbol
        .clone()
}

/// Overlay ON admits the governing-doc node adjacent (via `DocReference`) to a kept
/// code anchor that the structural cap evicted, and keeps its intent edge — while
/// overlay OFF shows neither ([FR-UI-16], [ADR-37]). Seeded on `target` with cap 1:
/// the seed is the sole kept (code) node, the Rationale doc section is evicted, and
/// the overlay rescues it. The code-anchor set is identical with and without the
/// overlay — the structural ranking is untouched.
#[cfg(feature = "lang-markdown")]
#[test]
fn intent_overlay_admits_governing_doc_nodes_adjacent_to_kept_code() {
    use logos_core::model::EdgeKind;

    let tmp = intent_overlay_fixture();
    let engine = indexed_engine(&tmp);
    let target = target_symbol(&engine);

    let off = engine.graph_elements(Some(&target), Some(1), None, None, None, false);
    let on = engine.graph_elements(Some(&target), Some(1), None, None, None, true);
    assert!(off.warnings.is_empty(), "{:?}", off.warnings);
    assert!(on.warnings.is_empty(), "{:?}", on.warnings);

    // OFF: only the seed survives the cap; no doc node is shown (today's behaviour).
    assert_eq!(off.nodes.len(), 1, "the tight cap keeps only the seed: {:?}", off.nodes);
    assert!(
        !off.nodes.iter().any(|n| n.layer == GraphLayer::Doc),
        "overlay off shows no governing-doc node: {:?}",
        off.nodes,
    );

    // ON: the seed is still kept AND the adjacent governing-doc node is admitted by
    // the separate reserved budget.
    let doc_nodes: Vec<_> = on.nodes.iter().filter(|n| n.layer == GraphLayer::Doc).collect();
    assert!(
        !doc_nodes.is_empty(),
        "overlay on admits the governing-doc node adjacent to the kept code anchor: {:?}",
        on.nodes,
    );
    assert!(
        on.nodes.iter().any(|n| n.id == target),
        "the kept code anchor is still present with the overlay on: {:?}",
        on.nodes,
    );

    // The intent edge connecting an admitted doc node to the kept code anchor is
    // surfaced — a real, already-bound edge (NFR-RA-05), never fabricated.
    let doc_ids: std::collections::HashSet<&str> =
        doc_nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        on.edges.iter().any(|e| {
            matches!(e.edge_type, Some(EdgeKind::DocReference) | Some(EdgeKind::TracesTo))
                && (e.source == target || e.target == target)
                && (doc_ids.contains(e.source.as_str()) || doc_ids.contains(e.target.as_str()))
        }),
        "an intent edge links an admitted doc node to the kept code anchor: {:?}",
        on.edges,
    );
}

/// CR-014 doc-flooding guard (fitness): the overlay is **purely additive** over the
/// completed structural pass, so the kept **code-anchor set is never reduced** by it
/// — identical with the overlay on and off at every cap ([FR-UI-16], [ADR-37],
/// guarding the [CR-014] failure mode). Property-checked across a range of caps.
#[cfg(feature = "lang-markdown")]
#[test]
fn intent_overlay_never_reduces_the_code_anchor_set() {
    let tmp = intent_overlay_fixture();
    let engine = indexed_engine(&tmp);

    for cap in [0usize, 1, 2, 3, 5, 250] {
        let off = engine.graph_elements(None, Some(cap), None, None, None, false);
        let on = engine.graph_elements(None, Some(cap), None, None, None, true);

        let code_ids = |els: &logos_core::models::navigation::GraphElements| {
            els.nodes
                .iter()
                .filter(|n| n.layer == GraphLayer::Code)
                .map(|n| n.id.clone())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            code_ids(&off),
            code_ids(&on),
            "the overlay never adds, removes, or reshuffles a code anchor at cap={cap} \
             (CR-014 guard): the structural ranking is untouched",
        );
    }
}

/// The honest "N more not shown" accounting stays exact with the overlay on
/// ([NFR-CC-04]): the elided invariant `elided == total − rendered` holds, and a
/// governing-doc node the structural pass had evicted moves from *elided* to
/// *rendered* — the in-scope total is unchanged and the elided count drops by exactly
/// the rescued node, never a silent change.
#[cfg(feature = "lang-markdown")]
#[test]
fn intent_overlay_elided_counts_account_for_the_overlay() {
    let tmp = intent_overlay_fixture();
    let engine = indexed_engine(&tmp);
    let target = target_symbol(&engine);

    let off = engine.graph_elements(Some(&target), Some(1), None, None, None, false);
    let on = engine.graph_elements(Some(&target), Some(1), None, None, None, true);

    // The elided invariant holds for both — the accounting is always consistent.
    assert_eq!(
        off.elided_nodes,
        off.total_nodes - off.nodes.len() as u32,
        "overlay-off elided invariant: elided == total − rendered",
    );
    assert_eq!(
        on.elided_nodes,
        on.total_nodes - on.nodes.len() as u32,
        "overlay-on elided invariant: elided == total − rendered (NFR-CC-04)",
    );

    // The rescued doc was already in the seed's in-scope total (a layers=None scope
    // includes the doc layer), so the overlay moves it elided→rendered: the honest
    // in-scope total is unchanged and the elided count drops by exactly the rescued
    // node(s) — the "N more not shown" notice accounts for the overlay.
    assert_eq!(
        on.total_nodes, off.total_nodes,
        "rescuing an in-scope doc leaves the honest total unchanged",
    );
    let rescued = on.nodes.len() as u32 - off.nodes.len() as u32;
    assert!(rescued >= 1, "the overlay rescued at least one governing-doc node");
    assert_eq!(
        on.elided_nodes,
        off.elided_nodes - rescued,
        "the elided count drops by exactly the rescued doc node(s) (NFR-CC-04)",
    );
}

/// The complementary accounting branch ([NFR-CC-04]): when the structural `layers`
/// filter excludes the doc layer, the admitted governing-doc nodes are NOT in the
/// in-scope set, so the overlay **widens** the in-scope total (rather than rescuing
/// from elided). With `layers = [code]` a code-only snapshot shows no doc node; the
/// same filter with the overlay on admits the governing doc adjacent to a kept code
/// anchor, growing `total_nodes` while the elided invariant still holds and the code
/// anchors are unchanged — exercising the `total_nodes +=` out-of-scope branch.
#[cfg(feature = "lang-markdown")]
#[test]
fn intent_overlay_admits_out_of_scope_docs_and_widens_the_total() {
    let tmp = intent_overlay_fixture();
    let engine = indexed_engine(&tmp);

    let code_only = engine.graph_elements(None, None, Some(&[GraphLayer::Code]), None, None, false);
    let with_intent = engine.graph_elements(None, None, Some(&[GraphLayer::Code]), None, None, true);

    // The code-layer filter alone yields no doc node — the structural in-scope set
    // excludes the doc layer entirely.
    assert!(
        !code_only.nodes.iter().any(|n| n.layer == GraphLayer::Doc),
        "the code-only filter excludes the doc layer: {:?}",
        code_only.nodes,
    );
    // The overlay admits the governing doc adjacent to the kept code anchor even though
    // the layer filter excluded the doc layer from the structural scope.
    assert!(
        with_intent.nodes.iter().any(|n| n.layer == GraphLayer::Doc),
        "the overlay admits an out-of-scope governing-doc node: {:?}",
        with_intent.nodes,
    );

    // Out-of-scope admission WIDENS the honest in-scope total (vs rescuing from the
    // existing elided pool), and the elided invariant still holds.
    assert!(
        with_intent.total_nodes > code_only.total_nodes,
        "an out-of-scope doc admission widens the in-scope total ({} > {})",
        with_intent.total_nodes,
        code_only.total_nodes,
    );
    assert_eq!(
        with_intent.elided_nodes,
        with_intent.total_nodes - with_intent.nodes.len() as u32,
        "the elided invariant holds across the out-of-scope widening (NFR-CC-04)",
    );

    // The code-anchor set is untouched by the overlay (CR-014 guard) even under a
    // layer filter.
    let code_ids = |els: &logos_core::models::navigation::GraphElements| {
        els.nodes
            .iter()
            .filter(|n| n.layer == GraphLayer::Code)
            .map(|n| n.id.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        code_ids(&code_only),
        code_ids(&with_intent),
        "the overlay leaves the code anchors unchanged under a layer filter (CR-014 guard)",
    );
}
