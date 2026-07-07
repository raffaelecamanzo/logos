//! Unit tests for graph hydration ([S-009], [FR-DB-05], [FR-DB-06],
//! [NFR-PE-07], [NFR-RA-06]).
//!
//! The [`build_view`] tests are pure (no I/O): they feed hand-built
//! [`NodeRow`]/[`EdgeRow`] snapshots straight to the builder. The
//! [`HydrationCache`] tests drive a real [`Runtime`] over an on-disk WAL store so
//! the hit / invalidate / evict behaviour is exercised end-to-end through the RO
//! pool, exactly as the Engine uses it.

use std::sync::Arc;

use petgraph::algo::tarjan_scc;
use tempfile::TempDir;

use crate::graph_store::{BatchWriter, EdgeRow, NewNode, NodeRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::runtime::Runtime;

use super::view::build_view;
use super::{Granularity, HydrationCache, HydrationConfig, Scope, SyncStamp};

// ── snapshot builders for the pure build_view tests ──────────────────────────

/// A symbol-level node row keyed by `id`, with a synthetic but valid local
/// symbol and an optional defining file.
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

// ── FR-DB-06: exclude-contains dependency view ───────────────────────────────

#[test]
fn exclude_contains_view_drops_contains_edges() {
    let nodes = [
        node(1, "m", NodeKind::Module, Some("a.rs")),
        node(2, "f", NodeKind::Function, Some("a.rs")),
        node(3, "g", NodeKind::Function, Some("a.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains), // lexical — must be excluded
        edge(1, 3, EdgeKind::Contains), // lexical — must be excluded
        edge(2, 3, EdgeKind::Calls),    // dependency — kept
    ];

    let view = build_view(Granularity::ExcludeContains, &nodes, &edges);
    assert_eq!(view.node_count(), 3, "every symbol is a vertex");
    assert_eq!(
        view.edge_count(),
        1,
        "only the non-Contains (Calls) edge survives (FR-DB-06)"
    );
}

#[test]
fn symbol_view_keeps_contains_edges() {
    let nodes = [
        node(1, "m", NodeKind::Module, Some("a.rs")),
        node(2, "f", NodeKind::Function, Some("a.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Contains), edge(2, 2, EdgeKind::Calls)];

    let view = build_view(Granularity::Symbol, &nodes, &edges);
    assert_eq!(
        view.edge_count(),
        2,
        "the full Symbol view keeps Contains alongside dependency edges"
    );
}

/// CR-005/ADR-21 metric-neutrality: a member-access `Accesses` edge is a
/// navigable structural fact, not a code coupling, so it is excluded from the
/// `ExcludeContains` dependency view the five original metrics run on
/// ([FR-EX-08]) — yet kept in the full `Symbol` view where it is navigable.
/// Adding one therefore leaves the dependency graph (and so the original five
/// dimensions) byte-identical — the metric-neutrality guarantee CR-005 must
/// honour before S-044 adds the new dimensions.
///
/// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
#[test]
fn accesses_edges_are_excluded_from_the_dependency_view_but_kept_in_the_symbol_view() {
    let nodes = [
        node(1, "method", NodeKind::Method, Some("a.rs")),
        node(2, "field", NodeKind::Field, Some("a.rs")),
        node(3, "callee", NodeKind::Function, Some("b.rs")),
    ];
    let without = [edge(1, 3, EdgeKind::Calls)];
    let with_accesses = [edge(1, 3, EdgeKind::Calls), edge(1, 2, EdgeKind::Accesses)];

    // The dependency view is identical with or without the Accesses edge.
    let dep_without = build_view(Granularity::ExcludeContains, &nodes, &without);
    let dep_with = build_view(Granularity::ExcludeContains, &nodes, &with_accesses);
    assert_eq!(
        dep_with.node_count(),
        dep_without.node_count(),
        "the Accesses edge adds no vertex to the dependency view"
    );
    assert_eq!(
        dep_with.edge_count(),
        dep_without.edge_count(),
        "an Accesses edge must not enter the dependency view (FR-EX-08, ADR-21)"
    );
    assert_eq!(
        dep_with.edge_count(),
        1,
        "only the Calls edge is a dependency"
    );

    // The full Symbol view keeps it — Accesses is a navigable field-usage fact.
    let sym = build_view(Granularity::Symbol, &nodes, &with_accesses);
    assert_eq!(
        sym.edge_count(),
        2,
        "the full Symbol view keeps the Accesses edge alongside the Calls edge"
    );
}

/// CR-011/ADR-26 metric-neutrality: the two cross-artifact edge kinds
/// (`ArtifactRef`, `ArtifactBinding`) are fenced out of the code subgraph at the
/// same hydration audit point as the non-code node predicate, so a fully-wired
/// artifact graph leaves the dependency view — and therefore `aggregate_signal`,
/// cycles, DSM, and dead-code — byte-identical ([FR-CG-05], [UAT-CG-04], BR-32).
///
/// Unlike `Accesses` (a code↔code fact kept in the full `Symbol` view), an
/// artifact edge has a non-code endpoint, so it is excluded from **every** view:
/// its config endpoint is dropped as a non-code vertex, and the explicit
/// `is_config_reference` predicate fences it even where both endpoints survive.
///
/// [FR-CG-05]: ../../../docs/specs/requirements/FR-CG-05.md
/// [UAT-CG-04]: ../../../docs/specs/requirements/UAT-CG-04.md
#[test]
fn artifact_edges_are_metric_neutral_in_every_view() {
    // A code subgraph (a calls b) plus a config artifact node and the two
    // artifact edges a binding could ever produce:
    //   - ArtifactBinding: ApiOperation(config) → handler(code)  [cross-layer]
    //   - ArtifactRef:     ConfigFile(config)   → ConfigFile(config)
    let nodes = [
        node(1, "handler", NodeKind::Function, Some("a.rs")),
        node(2, "callee", NodeKind::Function, Some("b.rs")),
        node(3, "GET /x", NodeKind::ApiOperation, Some("openapi.yaml")),
        node(
            4,
            "openapi.yaml",
            NodeKind::ConfigFile,
            Some("openapi.yaml"),
        ),
        node(5, "common.yaml", NodeKind::ConfigFile, Some("common.yaml")),
    ];
    let without = [edge(1, 2, EdgeKind::Calls)];
    let with_artifact = [
        edge(1, 2, EdgeKind::Calls),
        edge(3, 1, EdgeKind::ArtifactBinding), // operation → handler (cross-layer)
        edge(4, 5, EdgeKind::ArtifactRef),     // config → config
    ];

    // Every view is byte-identical with the artifact edges present — the load-
    // bearing UAT-CG-04 invariant, checked across the dependency, full-symbol,
    // and both rollup granularities. Vertex label ordering is the strongest
    // available "same code subgraph" proxy at the hydration level, matching the
    // doc/config neutrality tests in this file.
    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };
    for granularity in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let bare = build_view(granularity, &nodes, &without);
        let wired = build_view(granularity, &nodes, &with_artifact);
        assert_eq!(
            wired.node_count(),
            bare.node_count(),
            "{granularity:?}: artifact edges add no code vertex"
        );
        assert_eq!(
            wired.edge_count(),
            bare.edge_count(),
            "{granularity:?}: artifact edges never enter the view (FR-CG-05, ADR-26)"
        );
        assert_eq!(
            labels(&wired),
            labels(&bare),
            "{granularity:?}: artifact edges perturbed the code subgraph's vertices"
        );
    }

    // Concretely: the dependency view holds exactly the one Calls edge.
    let dep = build_view(Granularity::ExcludeContains, &nodes, &with_artifact);
    assert_eq!(
        dep.edge_count(),
        1,
        "only the code Calls edge is a dependency"
    );
    // Even the full Symbol view drops them (an artifact endpoint is never a code
    // vertex) — distinct from Accesses, which the full view keeps.
    let sym = build_view(Granularity::Symbol, &nodes, &with_artifact);
    assert_eq!(
        sym.edge_count(),
        1,
        "the full Symbol view also excludes artifact edges (non-code endpoint)"
    );
}

/// S-069 metric-neutrality (UAT-CG-04): the OpenAPI operation→route
/// `ArtifactBinding` — whose code endpoint is a promoted `Route` node, a genuine
/// code-subgraph vertex (unlike the config endpoint of every other artifact
/// edge) — must still leave the code subgraph byte-identical. The edge is fenced
/// by [`EdgeKind::is_config_reference`] at the same audit point as the node
/// predicate, so the `RoutesTo` coupling between the route and its handler stays
/// the only edge touching the route, and adding the binding moves no metric.
#[test]
fn an_operation_to_route_binding_is_metric_neutral() {
    // handler --RoutesTo--> nothing; route --RoutesTo--> handler is the real code
    // coupling. The operation→route ArtifactBinding must not perturb it.
    let nodes = [
        node(1, "get_user", NodeKind::Function, Some("src/main.rs")),
        node(2, "GET /users/{id}", NodeKind::Route, Some("src/main.rs")),
        node(3, "get", NodeKind::ApiOperation, Some("api/openapi.yaml")),
    ];
    let without = [edge(2, 1, EdgeKind::RoutesTo)];
    let with_binding = [
        edge(2, 1, EdgeKind::RoutesTo),
        edge(3, 2, EdgeKind::ArtifactBinding), // operation → route (cross-layer)
    ];

    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };
    for granularity in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let bare = build_view(granularity, &nodes, &without);
        let wired = build_view(granularity, &nodes, &with_binding);
        assert_eq!(
            wired.node_count(),
            bare.node_count(),
            "{granularity:?}: the route binding added a code vertex (its ApiOperation must drop)"
        );
        assert_eq!(
            wired.edge_count(),
            bare.edge_count(),
            "{granularity:?}: the route binding entered the view (UAT-CG-04, ADR-26)"
        );
        assert_eq!(
            labels(&wired),
            labels(&bare),
            "{granularity:?}: the route binding perturbed the code subgraph"
        );
    }
}

// ── ADR-34: the presentation-only visualization view ─────────────────────────

/// The visualization granularity ([ADR-34], [FR-UI-08]) is the lone view that
/// *keeps* the non-code layers: every non-code vertex (doc + config/artifact)
/// and every cross-layer edge (`DocReference`/`TracesTo`/`ArtifactRef`/
/// `ArtifactBinding`) is surfaced, layer-tagged by its retained `kind`, so the
/// web canvas receives the whole code/doc/artifact graph — while the four
/// code-subgraph views stay byte-identical with the non-code content present,
/// proving the exclusion predicate / audit point was left untouched (the
/// build-level metric-neutrality guard, [FR-DG-06], [FR-CG-05], [FR-QM-08]).
#[test]
fn visualization_view_admits_non_code_while_code_views_stay_byte_identical() {
    // The bare code subgraph: a calls b.
    let code_only = [
        node(1, "a", NodeKind::Function, Some("a.rs")),
        node(2, "b", NodeKind::Function, Some("b.rs")),
    ];
    let calls_only = [edge(1, 2, EdgeKind::Calls)];

    // The same code, now with all three layers and every cross-layer edge kind:
    //   DocReference (doc → code), TracesTo (story → requirement),
    //   ArtifactRef (config → config), ArtifactBinding (operation → code).
    let with_layers = [
        node(1, "a", NodeKind::Function, Some("a.rs")),
        node(2, "b", NodeKind::Function, Some("b.rs")),
        node(
            3,
            "Guide#Intro",
            NodeKind::DocSection,
            Some("docs/guide.md"),
        ),
        node(4, "FR-UI-08", NodeKind::Requirement, Some("docs/reqs.md")),
        node(5, "S-113", NodeKind::Story, Some("docs/journal.md")),
        node(
            6,
            "openapi.yaml",
            NodeKind::ConfigFile,
            Some("openapi.yaml"),
        ),
        node(7, "GET /x", NodeKind::ApiOperation, Some("openapi.yaml")),
    ];
    let all_edges = [
        edge(1, 2, EdgeKind::Calls),           // code → code
        edge(3, 1, EdgeKind::DocReference),    // doc → code        (cross-layer)
        edge(5, 4, EdgeKind::TracesTo),        // story → requirement
        edge(6, 7, EdgeKind::ArtifactRef),     // config → config
        edge(7, 1, EdgeKind::ArtifactBinding), // operation → code  (cross-layer)
    ];

    // The visualization view keeps every vertex and every edge.
    let vis = build_view(Granularity::Visualization, &with_layers, &all_edges);
    assert_eq!(
        vis.node_count(),
        7,
        "the visualization view keeps all vertices, code and non-code"
    );
    assert_eq!(
        vis.edge_count(),
        5,
        "the visualization view keeps all edges, including the cross-layer ones"
    );

    // Each non-code vertex is present AND kind-tagged (the canvas layers on kind).
    let kinds: std::collections::HashSet<NodeKind> =
        vis.graph().node_weights().filter_map(|v| v.kind).collect();
    for k in [
        NodeKind::DocSection,
        NodeKind::Requirement,
        NodeKind::Story,
        NodeKind::ConfigFile,
        NodeKind::ApiOperation,
    ] {
        assert!(kinds.contains(&k), "{k:?} vertex surfaced and kind-tagged");
    }
    // All four cross-layer edge kinds are surfaced.
    let edge_kinds: std::collections::HashSet<EdgeKind> =
        vis.graph().edge_weights().filter_map(|e| e.kind).collect();
    for k in [
        EdgeKind::DocReference,
        EdgeKind::TracesTo,
        EdgeKind::ArtifactRef,
        EdgeKind::ArtifactBinding,
    ] {
        assert!(edge_kinds.contains(&k), "{k:?} cross-layer edge surfaced");
    }

    // The four code-subgraph views are byte-identical with vs without the
    // non-code content — adding the visualization view moved no metric scope.
    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };
    for g in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let bare = build_view(g, &code_only, &calls_only);
        let wired = build_view(g, &with_layers, &all_edges);
        assert_eq!(
            wired.node_count(),
            bare.node_count(),
            "{g:?}: a non-code vertex leaked into the code subgraph"
        );
        assert_eq!(
            wired.edge_count(),
            bare.edge_count(),
            "{g:?}: a cross-layer edge leaked into the code subgraph"
        );
        assert_eq!(
            labels(&wired),
            labels(&bare),
            "{g:?}: the non-code content perturbed the code subgraph's vertices"
        );
    }
}

// ── tarjan_scc correctness on a known graph (acceptance criterion) ───────────

#[test]
fn tarjan_scc_finds_the_cycle_on_the_exclude_contains_view() {
    // a -> b -> c -> a is one SCC of 3; d -> a is a separate singleton.
    let nodes = [
        node(1, "a", NodeKind::Function, Some("x.rs")),
        node(2, "b", NodeKind::Function, Some("x.rs")),
        node(3, "c", NodeKind::Function, Some("x.rs")),
        node(4, "d", NodeKind::Function, Some("x.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 3, EdgeKind::Calls),
        edge(3, 1, EdgeKind::Calls), // closes the cycle
        edge(4, 1, EdgeKind::Calls),
    ];

    let view = build_view(Granularity::ExcludeContains, &nodes, &edges);
    let sccs = tarjan_scc(view.graph());

    let mut sizes: Vec<usize> = sccs.iter().map(Vec::len).collect();
    sizes.sort_unstable();
    assert_eq!(
        sizes,
        vec![1, 3],
        "tarjan_scc finds the 3-cycle and the singleton d"
    );
    // The size-3 component is exactly {a,b,c}.
    let cycle = sccs.iter().find(|c| c.len() == 3).expect("3-cycle present");
    let mut labels: Vec<&str> = cycle
        .iter()
        .map(|&i| view.graph()[i].label.as_str())
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, vec!["a", "b", "c"]);
}

// ── NFR-RA-06: deterministic build ───────────────────────────────────────────

#[test]
fn build_is_deterministic_across_repeated_builds() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("x.rs")),
        node(2, "b", NodeKind::Function, Some("y.rs")),
        node(3, "c", NodeKind::Function, Some("y.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls),
        edge(2, 3, EdgeKind::References),
    ];

    let first = build_view(Granularity::ExcludeContains, &nodes, &edges);
    let second = build_view(Granularity::ExcludeContains, &nodes, &edges);

    // Same vertex ordering (NodeIndex assignment) ⇒ same labels at each index.
    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };
    assert_eq!(labels(&first), labels(&second));
    assert_eq!(first.edge_count(), second.edge_count());
}

// ── File / module rollups ─────────────────────────────────────────────────────

#[test]
fn file_rollup_groups_symbols_by_file_and_drops_self_loops() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("a.rs")),
        node(2, "b", NodeKind::Function, Some("a.rs")),
        node(3, "c", NodeKind::Function, Some("b.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Calls), // within a.rs → self-loop, dropped
        edge(1, 3, EdgeKind::Calls), // a.rs → b.rs, kept
        edge(2, 3, EdgeKind::Calls), // a.rs → b.rs, aggregated onto the same edge
    ];

    let view = build_view(Granularity::File, &nodes, &edges);
    assert_eq!(view.node_count(), 2, "two files → two vertices");
    assert_eq!(
        view.edge_count(),
        1,
        "within-file self-loop dropped; the two a.rs→b.rs edges aggregate into one"
    );
    let a = view.index_of("a.rs").expect("a.rs vertex");
    let b = view.index_of("b.rs").expect("b.rs vertex");
    let e = view.graph().find_edge(a, b).expect("a.rs→b.rs edge");
    assert_eq!(view.graph()[e].weight, 2, "two underlying edges aggregated");
}

#[test]
fn module_rollup_uses_contains_to_find_the_enclosing_module() {
    // mod m { fn a; fn b; }  mod n { fn c; }   a→c is the only dependency.
    let nodes = [
        node(1, "m", NodeKind::Module, Some("x.rs")),
        node(2, "a", NodeKind::Function, Some("x.rs")),
        node(3, "b", NodeKind::Function, Some("x.rs")),
        node(4, "n", NodeKind::Module, Some("x.rs")),
        node(5, "c", NodeKind::Function, Some("x.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains), // m contains a
        edge(1, 3, EdgeKind::Contains), // m contains b
        edge(4, 5, EdgeKind::Contains), // n contains c
        edge(2, 5, EdgeKind::Calls),    // a → c  ⇒ module m → module n
    ];

    let view = build_view(Granularity::Module, &nodes, &edges);
    assert_eq!(view.node_count(), 2, "two modules → two vertices");
    assert_eq!(view.edge_count(), 1, "a→c lifts to m→n");
    let m = view.index_of("module:local 1").expect("module m vertex");
    let n = view.index_of("module:local 4").expect("module n vertex");
    assert!(
        view.graph().find_edge(m, n).is_some(),
        "the dependency lifts to m → n via Contains membership (FR-DB-06)"
    );
}

// ── FR-DG-06 / ADR-19: documentation is scoped out of the code subgraph ──────

/// A mixed code+documentation snapshot: two functions with one `Calls` edge
/// (the code subgraph), plus a `DocFile`→`DocSection` doc tree, a doc→doc
/// `DocReference`, and a doc→code `DocReference`/`TracesTo` that point at the
/// real code symbols. After hydration only the code vertices and the `Calls`
/// edge may survive — the doc nodes/edges are excluded at build time.
fn mixed_code_and_docs() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "api", NodeKind::Function, Some("src/lib.rs")),
        node(2, "helper", NodeKind::Function, Some("src/lib.rs")),
        node(10, "guide.md", NodeKind::DocFile, Some("docs/guide.md")),
        node(11, "Setup", NodeKind::DocSection, Some("docs/guide.md")),
        node(12, "README.md", NodeKind::DocFile, Some("README.md")),
        node(13, "FR-DG-06", NodeKind::Requirement, Some("docs/reqs.md")),
    ];
    let edges = vec![
        edge(1, 2, EdgeKind::Calls),      // code → code: the only real coupling
        edge(10, 11, EdgeKind::Contains), // DocFile contains DocSection
        edge(11, 12, EdgeKind::DocReference), // doc → doc link
        edge(11, 1, EdgeKind::DocReference), // doc → code reference (resolved)
        edge(13, 2, EdgeKind::TracesTo),  // requirement traces to code
    ];
    (nodes, edges)
}

#[test]
fn symbol_views_exclude_documentation_nodes_and_edges() {
    let (nodes, edges) = mixed_code_and_docs();

    for granularity in [Granularity::ExcludeContains, Granularity::Symbol] {
        let view = build_view(granularity, &nodes, &edges);
        assert_eq!(
            view.node_count(),
            2,
            "{granularity:?}: only the two code symbols are vertices (FR-DG-06)"
        );
        // No surviving vertex carries a documentation kind.
        assert!(
            view.graph()
                .node_weights()
                .all(|v| v.kind.is_some_and(|k| !k.is_documentation())),
            "{granularity:?}: a documentation vertex leaked into the code subgraph"
        );
        // Only the code→code Calls edge survives; every doc edge — and the
        // doc→code edges whose doc endpoint was dropped — are gone.
        assert_eq!(
            view.edge_count(),
            1,
            "{granularity:?}: only the Calls edge survives; doc edges are excluded"
        );
    }
}

#[test]
fn rollup_views_exclude_documentation_files_and_modules() {
    let (nodes, edges) = mixed_code_and_docs();

    // File rollup: src/lib.rs is the only code file; docs/guide.md, README.md,
    // and docs/reqs.md must not become vertices. The single intra-file Calls
    // edge is a self-loop and drops, leaving no edges.
    let file = build_view(Granularity::File, &nodes, &edges);
    assert_eq!(
        file.node_count(),
        1,
        "only src/lib.rs is a code file vertex"
    );
    assert!(
        file.index_of("docs/guide.md").is_none() && file.index_of("README.md").is_none(),
        "a documentation file became a DSM/metric vertex (FR-DG-06)"
    );
    assert_eq!(
        file.edge_count(),
        0,
        "the intra-file call is a dropped self-loop"
    );

    // Module rollup: the two functions roll up to their file (no enclosing
    // module), and no documentation vertex appears.
    let module = build_view(Granularity::Module, &nodes, &edges);
    assert_eq!(
        module.node_count(),
        1,
        "the code symbols share one module vertex"
    );
    assert!(
        module.index_of("file:docs/guide.md").is_none()
            && module.index_of("file:README.md").is_none(),
        "a documentation file became a module-rollup vertex (FR-DG-06)"
    );
}

/// The rollup doc-edge exclusion, asserted non-vacuously: the previous test's
/// only code edge is an intra-file self-loop that drops at every rollup
/// regardless of the doc filter, so it cannot witness doc-edge exclusion. Here
/// `helper` lives in a second file, making `api → helper` a surviving cross-file
/// coupling — so the rollup edge count is non-zero, and asserting it equals
/// exactly 1 proves every doc edge (the doc→doc link and both doc→code
/// references) was dropped rather than rolled into a doc vertex (FR-DG-06). Were
/// docs not excluded, the doc→code references would lift to extra rollup edges
/// and the count would exceed 1.
#[test]
fn rollups_drop_doc_edges_when_a_cross_file_code_edge_survives() {
    let (mut nodes, edges) = mixed_code_and_docs();
    // Move `helper` (id 2) to its own file so api→helper is a cross-file edge.
    nodes[1].file_path = Some("src/other.rs".to_string());

    let file = build_view(Granularity::File, &nodes, &edges);
    assert_eq!(
        file.node_count(),
        2,
        "two code files become vertices; no doc file does (FR-DG-06)"
    );
    assert_eq!(
        file.edge_count(),
        1,
        "only the code→code coupling survives; every doc edge is dropped (FR-DG-06)"
    );

    let module = build_view(Granularity::Module, &nodes, &edges);
    assert_eq!(
        module.node_count(),
        2,
        "the two files (no enclosing module) become two module vertices"
    );
    assert_eq!(
        module.edge_count(),
        1,
        "the doc edges are excluded at module granularity too (FR-DG-06)"
    );
}

/// The byte-identical invariance at the hydration layer: building a view from a
/// code-only snapshot and from the same snapshot with documentation added yields
/// graphs with identical vertex/edge counts and identical vertex labels in index
/// order — the deterministic foundation the aggregate-signal neutrality rests on
/// ([FR-DG-06], [NFR-RA-06]). The doc-bearing build also exercises the doc→code
/// edges, proving "docs that reference code" cannot perturb the code subgraph.
#[test]
fn adding_documentation_does_not_change_the_hydrated_code_subgraph() {
    let code_nodes = [
        node(1, "api", NodeKind::Function, Some("src/lib.rs")),
        node(2, "helper", NodeKind::Function, Some("src/lib.rs")),
    ];
    let code_edges = [edge(1, 2, EdgeKind::Calls)];
    let (with_docs_nodes, with_docs_edges) = mixed_code_and_docs();

    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };

    for granularity in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let code_only = build_view(granularity, &code_nodes, &code_edges);
        let with_docs = build_view(granularity, &with_docs_nodes, &with_docs_edges);
        assert_eq!(
            code_only.node_count(),
            with_docs.node_count(),
            "{granularity:?}: documentation shifted the vertex count"
        );
        assert_eq!(
            code_only.edge_count(),
            with_docs.edge_count(),
            "{granularity:?}: documentation shifted the edge count"
        );
        assert_eq!(
            labels(&code_only),
            labels(&with_docs),
            "{granularity:?}: documentation perturbed the code subgraph (FR-DG-06)"
        );
    }
}

// ── FR-CG-05 / ADR-25: config artifacts are scoped out of the code subgraph ──

/// A mixed code + config-artifact snapshot: two functions with one cross-file
/// `Calls` edge (the code subgraph), plus a `ConfigFile`→`ConfigSection` tree, a
/// nested `ConfigSection`, and a typed anchor (`DockerfileStage`) — all connected
/// only by `Contains` (the layer is Contains-only, CR-010). After hydration only
/// the code vertices and the `Calls` edge may survive — every config node and its
/// `Contains` edges are excluded at build time ([FR-CG-05], [ADR-25]).
fn mixed_code_and_config() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "api", NodeKind::Function, Some("src/lib.rs")),
        node(2, "helper", NodeKind::Function, Some("src/other.rs")),
        node(
            20,
            "values.yaml",
            NodeKind::ConfigFile,
            Some("deploy/values.yaml"),
        ),
        node(
            21,
            "service",
            NodeKind::ConfigSection,
            Some("deploy/values.yaml"),
        ),
        node(
            22,
            "ports",
            NodeKind::ConfigSection,
            Some("deploy/values.yaml"),
        ),
        node(23, "build", NodeKind::DockerfileStage, Some("Dockerfile")),
        node(24, "Dockerfile", NodeKind::ConfigFile, Some("Dockerfile")),
    ];
    let edges = vec![
        edge(1, 2, EdgeKind::Calls),      // code → code: the only real coupling
        edge(20, 21, EdgeKind::Contains), // ConfigFile contains a section
        edge(21, 22, EdgeKind::Contains), // section contains a nested section
        edge(24, 23, EdgeKind::Contains), // ConfigFile contains a typed anchor
    ];
    (nodes, edges)
}

#[test]
fn symbol_views_exclude_config_nodes_and_edges() {
    let (nodes, edges) = mixed_code_and_config();

    for granularity in [Granularity::ExcludeContains, Granularity::Symbol] {
        let view = build_view(granularity, &nodes, &edges);
        assert_eq!(
            view.node_count(),
            2,
            "{granularity:?}: only the two code symbols are vertices (FR-CG-05)"
        );
        // No surviving vertex carries a config kind.
        assert!(
            view.graph()
                .node_weights()
                .all(|v| v.kind.is_some_and(|k| !k.is_config())),
            "{granularity:?}: a config vertex leaked into the code subgraph"
        );
        // Only the code→code Calls edge survives; every config `Contains` edge is
        // gone (its endpoints were dropped) — the layer moves no metric.
        assert_eq!(
            view.edge_count(),
            1,
            "{granularity:?}: only the Calls edge survives; config edges are excluded"
        );
    }
}

/// The load-bearing metric-neutrality invariant at the hydration layer
/// ([FR-CG-05], [ADR-25], [UAT-CG-01]): building a view from a code-only snapshot
/// and from the same snapshot **with config artifacts added** yields graphs with
/// identical vertex/edge counts and identical vertex labels in index order — at
/// every granularity. Because `aggregate_signal`, DSM, cycles, and dead-code are
/// all pure functions of these views, a byte-identical view means a byte-identical
/// signal: adding or removing config artifacts cannot move the number.
#[test]
fn adding_config_artifacts_does_not_change_the_hydrated_code_subgraph() {
    let code_nodes = [
        node(1, "api", NodeKind::Function, Some("src/lib.rs")),
        node(2, "helper", NodeKind::Function, Some("src/other.rs")),
    ];
    let code_edges = [edge(1, 2, EdgeKind::Calls)];
    let (with_config_nodes, with_config_edges) = mixed_code_and_config();

    let labels = |v: &super::GraphView| -> Vec<String> {
        v.graph().node_weights().map(|w| w.label.clone()).collect()
    };

    for granularity in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let code_only = build_view(granularity, &code_nodes, &code_edges);
        let with_config = build_view(granularity, &with_config_nodes, &with_config_edges);
        assert_eq!(
            code_only.node_count(),
            with_config.node_count(),
            "{granularity:?}: config artifacts shifted the vertex count"
        );
        assert_eq!(
            code_only.edge_count(),
            with_config.edge_count(),
            "{granularity:?}: config artifacts shifted the edge count"
        );
        assert_eq!(
            labels(&code_only),
            labels(&with_config),
            "{granularity:?}: config artifacts perturbed the code subgraph (FR-CG-05)"
        );
    }
}

// ── HydrationCache: hit / invalidate / evict, against a real Runtime ─────────

/// Open a runtime over a fresh on-disk WAL store and seed `count` function nodes.
fn seeded_runtime(count: usize) -> (Runtime, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let runtime = Runtime::open(dir.path().join("logos.db")).expect("runtime opens");
    runtime
        .submit_write(move |w: &BatchWriter<'_>| {
            for i in 0..count {
                let sym = LogosSymbol::parse(&format!("local seed{i}"))?;
                let symbol_id = w.upsert_symbol(&sym)?;
                w.insert_node(&NewNode::plain(
                    symbol_id,
                    NodeKind::Function,
                    &format!("seed_{i}"),
                ))?;
            }
            Ok(())
        })
        .expect("seed write commits");
    (runtime, dir)
}

#[test]
fn second_run_at_the_same_stamp_is_a_cache_hit() {
    let (runtime, _dir) = seeded_runtime(3);
    let cache = HydrationCache::default();
    let stamp = SyncStamp::INITIAL;

    let first = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            stamp,
        )
        .expect("first hydrate builds");
    let second = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            stamp,
        )
        .expect("second hydrate hits cache");

    // Same Arc instance ⇒ the petgraph was not rebuilt (NFR-PE-07).
    assert!(
        Arc::ptr_eq(&first, &second),
        "the repeated run returns the cached view, not a rebuilt one"
    );
    let stats = cache.stats();
    assert_eq!((stats.hits, stats.misses), (1, 1));
}

#[test]
fn advancing_the_stamp_invalidates_the_cache() {
    let (runtime, _dir) = seeded_runtime(2);
    let cache = HydrationCache::default();

    let v0 = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp(0),
        )
        .expect("hydrate at stamp 0");
    // Same scope+granularity but an advanced stamp ⇒ a fresh build, not a hit.
    let v1 = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp(1),
        )
        .expect("hydrate at stamp 1");

    assert!(
        !Arc::ptr_eq(&v0, &v1),
        "an advanced last_sync_at rebuilds rather than serving the stale view"
    );
    let stats = cache.stats();
    assert_eq!(stats.misses, 2, "both builds are misses");
    assert_eq!(
        stats.entries, 1,
        "the stale stamp-0 entry was evicted on advance — only stamp 1 remains"
    );
}

#[test]
fn distinct_granularities_are_cached_independently() {
    let (runtime, _dir) = seeded_runtime(2);
    let cache = HydrationCache::default();
    let stamp = SyncStamp::INITIAL;

    for g in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        cache
            .get_or_build(&runtime, Scope::Project, g, stamp)
            .expect("each granularity hydrates");
    }
    // Re-request one: it must hit, proving the four are keyed separately.
    cache
        .get_or_build(&runtime, Scope::Project, Granularity::File, stamp)
        .expect("re-request hits");

    let stats = cache.stats();
    assert_eq!(stats.entries, 4, "four distinct views cached");
    assert_eq!((stats.hits, stats.misses), (1, 4));
}

/// The presentation-only visualization view is keyed distinctly from the four
/// code-subgraph views ([ADR-34]): the cache key carries the granularity, so the
/// new view is its own entry and never collides with — or evicts — a metric view.
#[test]
fn the_visualization_view_is_cached_distinctly_from_the_code_views() {
    let (runtime, _dir) = seeded_runtime(2);
    let cache = HydrationCache::default();
    let stamp = SyncStamp::INITIAL;

    // Build all five granularities once each — five misses, five entries.
    for g in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
        Granularity::Visualization,
    ] {
        cache
            .get_or_build(&runtime, Scope::Project, g, stamp)
            .expect("each granularity hydrates");
    }

    // Re-request the visualization view and one code view: both must HIT,
    // proving the visualization key is separate from (and coexists with) the
    // code-subgraph keys.
    let hit = cache
        .get_or_build(&runtime, Scope::Project, Granularity::Visualization, stamp)
        .expect("visualization re-request hits");
    assert_eq!(
        hit.granularity(),
        Granularity::Visualization,
        "the cached entry is the visualization view"
    );
    cache
        .get_or_build(&runtime, Scope::Project, Granularity::Symbol, stamp)
        .expect("symbol re-request hits");

    let stats = cache.stats();
    assert_eq!(
        stats.entries, 5,
        "five distinct views cached — visualization is its own entry, not a code view"
    );
    assert_eq!((stats.hits, stats.misses), (2, 5));
}

#[test]
fn a_tight_entry_bound_evicts_least_recently_used() {
    let (runtime, _dir) = seeded_runtime(2);
    // Bound to a single entry: each new granularity must evict the previous.
    let cache = HydrationCache::new(HydrationConfig {
        max_entries: Some(1),
        max_bytes: None,
    });
    let stamp = SyncStamp::INITIAL;

    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            stamp,
        )
        .unwrap();
    cache
        .get_or_build(&runtime, Scope::Project, Granularity::Symbol, stamp)
        .unwrap();

    assert_eq!(
        cache.stats().entries,
        1,
        "the tight entry bound evicted the first view when the second arrived"
    );

    // The evicted ExcludeContains view must rebuild (a miss), proving eviction.
    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            stamp,
        )
        .unwrap();
    let stats = cache.stats();
    assert_eq!(
        stats.hits, 0,
        "every request missed — nothing stayed resident"
    );
    assert_eq!(stats.misses, 3);
    assert_eq!(stats.entries, 1);
}

#[test]
fn a_tight_byte_budget_also_bounds_the_cache() {
    let (runtime, _dir) = seeded_runtime(4);
    // 1-byte budget: any second entry exceeds it, so only one stays resident,
    // but the most recently built is always kept (graceful degradation).
    let cache = HydrationCache::new(HydrationConfig {
        max_entries: None,
        max_bytes: Some(1),
    });
    let stamp = SyncStamp::INITIAL;

    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            stamp,
        )
        .unwrap();
    cache
        .get_or_build(&runtime, Scope::Project, Granularity::Symbol, stamp)
        .unwrap();

    assert_eq!(
        cache.stats().entries,
        1,
        "the byte budget keeps only the most recent view resident"
    );
}

#[test]
fn hydration_is_empty_but_valid_on_an_empty_graph() {
    let (runtime, _dir) = seeded_runtime(0);
    let cache = HydrationCache::default();
    let view = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp::INITIAL,
        )
        .expect("hydrating an empty graph succeeds");
    assert_eq!(view.node_count(), 0);
    assert_eq!(view.edge_count(), 0);
    assert!(tarjan_scc(view.graph()).is_empty());
}

// ── Vertex/edge weight invariants (public API contract) ──────────────────────

#[test]
fn symbol_level_vertices_carry_kind_and_node_id_rollup_vertices_do_not() {
    let nodes = [
        node(1, "a", NodeKind::Function, Some("a.rs")),
        node(2, "b", NodeKind::Function, Some("b.rs")),
    ];
    let edges = [edge(1, 2, EdgeKind::Calls)];

    // Symbol-level: every vertex is one symbol, so kind + node_id are populated.
    let symbol = build_view(Granularity::ExcludeContains, &nodes, &edges);
    assert!(
        symbol
            .graph()
            .node_weights()
            .all(|v| v.kind.is_some() && v.node_id.is_some()),
        "symbol-level vertices carry kind + node_id"
    );
    // The single symbol-level edge keeps its concrete EdgeKind.
    assert!(
        symbol
            .graph()
            .edge_weights()
            .all(|e| e.kind == Some(EdgeKind::Calls) && e.weight == 1),
        "symbol-level edges keep their concrete kind and unit weight"
    );

    // File rollup: vertices are aggregates, so kind + node_id are None and the
    // lifted edge carries no single kind.
    let file = build_view(Granularity::File, &nodes, &edges);
    assert!(
        file.graph()
            .node_weights()
            .all(|v| v.kind.is_none() && v.node_id.is_none()),
        "rollup vertices carry neither kind nor node_id"
    );
    assert!(
        file.graph().edge_weights().all(|e| e.kind.is_none()),
        "aggregated rollup edges carry no single EdgeKind"
    );
}

// ── Rollup sentinels and fallbacks (GAP coverage for file-less / module-less) ─

#[test]
fn file_rollup_maps_a_file_less_node_to_the_unbound_vertex() {
    let nodes = [node(1, "f", NodeKind::Function, None)];
    let view = build_view(Granularity::File, &nodes, &[]);
    assert_eq!(view.node_count(), 1);
    assert!(
        view.index_of("<unbound>").is_some(),
        "a node bound to no file rolls up to the <unbound> sentinel vertex"
    );
}

#[test]
fn module_rollup_falls_back_to_file_then_unbound_without_a_module_ancestor() {
    // A function with no enclosing Module: keyed by its file.
    let with_file = [node(1, "f", NodeKind::Function, Some("x.rs"))];
    let view = build_view(Granularity::Module, &with_file, &[]);
    assert!(
        view.index_of("file:x.rs").is_some(),
        "no module ancestor + a file ⇒ file:<path> fallback key"
    );

    // A function with neither a module ancestor nor a file: the <unbound> sentinel.
    let without_file = [node(1, "f", NodeKind::Function, None)];
    let view = build_view(Granularity::Module, &without_file, &[]);
    assert!(
        view.index_of("<unbound>").is_some(),
        "no module ancestor + no file ⇒ <unbound> fallback key"
    );
}

#[test]
fn module_rollup_drops_intra_module_self_loops() {
    let nodes = [
        node(1, "m", NodeKind::Module, Some("x.rs")),
        node(2, "a", NodeKind::Function, Some("x.rs")),
        node(3, "b", NodeKind::Function, Some("x.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(1, 3, EdgeKind::Contains),
        edge(2, 3, EdgeKind::Calls), // both endpoints roll up to module m
    ];
    let view = build_view(Granularity::Module, &nodes, &edges);
    assert_eq!(view.node_count(), 1, "one module vertex");
    assert_eq!(
        view.edge_count(),
        0,
        "an intra-module dependency is a self-loop and is dropped"
    );
}

#[test]
fn module_rollup_terminates_on_a_contains_cycle_in_a_corrupt_store() {
    // A corrupt store with a Contains cycle (a↔b, neither a Module): the ancestry
    // walk must terminate (guarded by the seen-set) and fall back to the file.
    let nodes = [
        node(1, "a", NodeKind::Function, Some("x.rs")),
        node(2, "b", NodeKind::Function, Some("x.rs")),
    ];
    let edges = [
        edge(1, 2, EdgeKind::Contains),
        edge(2, 1, EdgeKind::Contains), // cycle
    ];
    // Must not hang; both fall back to file:x.rs ⇒ a single vertex.
    let view = build_view(Granularity::Module, &nodes, &edges);
    assert_eq!(view.node_count(), 1);
    assert!(view.index_of("file:x.rs").is_some());
}

// ── Cache: byte accounting + stamp-regression guard ──────────────────────────

#[test]
fn stats_track_estimated_bytes_for_resident_views() {
    let (runtime, _dir) = seeded_runtime(3);
    let cache = HydrationCache::default();
    assert_eq!(
        cache.stats().estimated_bytes,
        0,
        "empty cache holds no bytes"
    );

    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp::INITIAL,
        )
        .unwrap();
    assert!(
        cache.stats().estimated_bytes > 0,
        "a resident view contributes to the byte estimate (the byte-budget bound)"
    );
}

#[test]
fn a_regressed_stamp_does_not_evict_the_newer_generation() {
    let (runtime, _dir) = seeded_runtime(2);
    let cache = HydrationCache::default();

    // Establish generation 1 in the cache.
    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp(1),
        )
        .unwrap();
    assert_eq!(cache.stats().entries, 1);

    // A request for an OLDER stamp (the regressed-build case) must still serve a
    // valid view but must not roll the cache back or evict the gen-1 entry.
    let stale = cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp(0),
        )
        .expect("an older-stamp request still returns a consistent view");
    assert_eq!(
        stale.node_count(),
        2,
        "the older-stamp caller gets a real view"
    );
    assert_eq!(
        cache.stats().entries,
        1,
        "the newer generation-1 entry is preserved, not evicted by the regressed stamp"
    );

    // The gen-1 view is still cached: re-requesting stamp 1 is a hit.
    let before_hits = cache.stats().hits;
    cache
        .get_or_build(
            &runtime,
            Scope::Project,
            Granularity::ExcludeContains,
            SyncStamp(1),
        )
        .unwrap();
    assert_eq!(
        cache.stats().hits,
        before_hits + 1,
        "generation 1 survived the regressed-stamp request and still hits"
    );
}
