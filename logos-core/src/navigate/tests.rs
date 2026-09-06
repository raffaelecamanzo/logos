//! Unit tests for the navigation service's pure helpers (S-013).
//!
//! Everything that needs a live runtime/store is exercised end-to-end in
//! `tests/navigation.rs`; here we pin the I/O-free seams: code slicing with
//! its path-containment guard, and line-number wire conversion.

use std::fs;

use tempfile::TempDir;

use super::{
    anchor_sharers, is_registration_edge, line_u32, precedent_anchors,
    precedent_degraded, read_code, PrecedentAnchor,
};
use crate::graph_store::NodeRow;
use crate::model::{LogosSymbol, NodeId, NodeKind};

/// A node row bound to `file` with the given 1-based line span.
fn row(file: Option<&str>, start: Option<i64>, end: Option<i64>) -> NodeRow {
    NodeRow {
        id: NodeId(1),
        symbol: LogosSymbol::parse("local 1").expect("local symbol parses"),
        kind: NodeKind::Function,
        name: "f".to_string(),
        file_path: file.map(str::to_string),
        start_line: start,
        end_line: end,
    }
}

#[test]
fn read_code_slices_the_declared_line_span() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("lib.rs"), "l1\nl2\nl3\nl4\nl5\n").unwrap();

    let code = read_code(tmp.path(), &row(Some("lib.rs"), Some(2), Some(4)));
    assert_eq!(code.as_deref(), Some("l2\nl3\nl4"));

    // A single-line declaration (no end_line) yields exactly that line.
    let one = read_code(tmp.path(), &row(Some("lib.rs"), Some(3), None));
    assert_eq!(one.as_deref(), Some("l3"));
}

#[test]
fn read_code_is_none_without_a_file_or_line_binding() {
    let tmp = TempDir::new().unwrap();
    assert!(read_code(tmp.path(), &row(None, Some(1), Some(1))).is_none());
    assert!(read_code(tmp.path(), &row(Some("lib.rs"), None, None)).is_none());
}

#[test]
fn read_code_tolerates_drift_and_missing_files() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("lib.rs"), "only one line\n").unwrap();

    // The file shrank since indexing (best-effort drift, NFR-DM-02) → None,
    // never an error or a panic.
    assert!(read_code(tmp.path(), &row(Some("lib.rs"), Some(10), Some(12))).is_none());
    // The file vanished since indexing.
    assert!(read_code(tmp.path(), &row(Some("gone.rs"), Some(1), Some(1))).is_none());
}

#[test]
fn read_code_refuses_paths_that_escape_the_root() {
    let tmp = TempDir::new().unwrap();
    // Defensive containment: stored paths are project-relative by
    // construction, but a hostile/corrupt store must not read outside root.
    assert!(read_code(tmp.path(), &row(Some("../secrets.txt"), Some(1), Some(1))).is_none());
    assert!(read_code(tmp.path(), &row(Some("/etc/hosts"), Some(1), Some(1))).is_none());
}

/// A repo-controlled symlink has an innocent relative path but points outside
/// the root — the canonical-path check must refuse it (review round 1,
/// security finding).
#[cfg(unix)]
#[test]
fn read_code_refuses_symlinks_that_escape_the_root() {
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), "leak me\n").unwrap();

    let tmp = TempDir::new().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        tmp.path().join("evil.rs"),
    )
    .unwrap();

    assert!(
        read_code(tmp.path(), &row(Some("evil.rs"), Some(1), Some(1))).is_none(),
        "a symlink escaping the root must not be readable"
    );

    // A symlink that stays INSIDE the root is fine (vendored layouts do this).
    fs::write(tmp.path().join("real.rs"), "fn ok() {}\n").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("real.rs"), tmp.path().join("alias.rs")).unwrap();
    assert_eq!(
        read_code(tmp.path(), &row(Some("alias.rs"), Some(1), Some(1))).as_deref(),
        Some("fn ok() {}")
    );
}

#[test]
fn line_numbers_convert_to_wire_u32_dropping_nonsense() {
    assert_eq!(line_u32(Some(42)), Some(42));
    assert_eq!(line_u32(Some(0)), None, "0 is not a valid 1-based line");
    assert_eq!(line_u32(Some(-3)), None);
    assert_eq!(line_u32(None), None);
}

// ── FR-CL-04 `--tests-only`: the test-path naming heuristic ─────────────────

#[test]
fn is_test_path_recognises_per_language_test_conventions() {
    use super::is_test_path;
    // Directory segments, any language.
    assert!(is_test_path("tests/navigation.rs"));
    assert!(is_test_path("pkg/test/helper.go"));
    assert!(is_test_path("src/__tests__/app.tsx"));
    // Filename idioms: Rust/Go `_test`, Python `test_`, JS `.test`/`.spec`,
    // Java `*Test(s)`.
    assert!(is_test_path("src/core_test.rs"));
    assert!(is_test_path("pkg/server_test.go"));
    assert!(is_test_path("app/test_models.py"));
    assert!(is_test_path("src/Button.test.tsx"));
    assert!(is_test_path("src/api.spec.ts"));
    assert!(is_test_path("src/main/UserServiceTest.java"));
    assert!(is_test_path("src/main/UserServiceTests.java"));
    // Ruby: RSpec `spec/` directory + `*_spec.rb`, minitest `*_test.rb`.
    assert!(is_test_path("spec/models/user_spec.rb"));
    assert!(is_test_path("models/user_spec.rb"));
    assert!(is_test_path("test/user_test.rb"));
    // CR-075: the plural Rust inline-unit-test conventions — a bare `tests.rs`
    // file and the snake_case `*_tests.rs` suffix (the CamelCase `*Tests`
    // plural above already matched; these were the asymmetry).
    assert!(is_test_path("src/tests.rs"));
    assert!(is_test_path("logos-core/src/navigate/tests.rs"));
    assert!(is_test_path("src/annotate_tests.rs"));
    assert!(is_test_path("pkg/resolve_tests.rs"));
}

#[test]
fn is_test_path_does_not_mark_production_files() {
    use super::is_test_path;
    assert!(!is_test_path("src/lib.rs"));
    assert!(!is_test_path("src/testing_utils.rs")); // prefix ≠ marker
    assert!(!is_test_path("src/test_utils.rs")); // test_ prefix marks .py only
    assert!(!is_test_path("src/contest.rs")); // substring ≠ suffix token
    assert!(!is_test_path("app/models.py"));
    assert!(!is_test_path("src/latest.ts")); // "test" inside a word
    assert!(!is_test_path("src/protest/march.rs")); // segment must equal
    assert!(!is_test_path("src/footests.rs")); // no underscore, no token boundary
    assert!(!is_test_path("src/latests.rs")); // "tests" inside a word, not the stem
}

// ── FR-NV-01 search: raw query → safe FTS5 phrase ───────────────────────────

#[test]
fn phrase_query_wraps_raw_text_so_punctuation_is_inert() {
    use super::phrase_query;
    // The reported bug: `web-surface` must become one quoted phrase, not a
    // bare expression whose `-` FTS5 reads as syntax.
    assert_eq!(phrase_query("web-surface").as_deref(), Some("\"web-surface\""));
    // A plain token is quoted too, but matches exactly as before.
    assert_eq!(phrase_query("surface").as_deref(), Some("\"surface\""));
    // Embedded double-quotes are doubled per FTS5 string quoting.
    assert_eq!(phrase_query("a\"b").as_deref(), Some("\"a\"\"b\""));
    // Empty/whitespace is a well-defined no-op, not an FTS syntax error.
    assert_eq!(phrase_query(""), None);
    assert_eq!(phrase_query("   "), None);
}

/// [`PrecedentFacet::ALL`] is hand-written, and a facet missing from it is
/// invisible rather than wrong — no reason is emitted for it and no test fails.
/// The `match` below is the guard: a fourth variant makes it non-exhaustive, so
/// the build stops here, and the arm it forces you to write is next to the
/// length assertion that sends you to `ALL` and to `PrecedentRank`'s per-facet
/// fields, which are the two places arity is still written down by hand.
#[test]
fn precedent_facet_all_lists_every_variant() {
    for facet in crate::models::navigation::PrecedentFacet::ALL {
        match facet {
            PrecedentFacet::SharedSupertype
            | PrecedentFacet::SharedRegistration
            | PrecedentFacet::SharedCallee => {}
        }
    }
    assert_eq!(
        crate::models::navigation::PrecedentFacet::ALL.len(),
        3,
        "a facet was added to the enum: add it to ALL, and give PrecedentRank \
         the matching per-facet count field"
    );
}

/// The registration facet must not double-count the supertype facet, and must
/// not read the derived governance edge as a structural fact ([FR-NV-12]).
#[test]
fn registration_edges_exclude_the_supertype_and_derived_kinds() {
    use crate::model::EdgeKind;

    for kind in [
        EdgeKind::Calls,
        EdgeKind::References,
        EdgeKind::Instantiates,
        EdgeKind::TypeUses,
        EdgeKind::RoutesTo,
    ] {
        assert!(is_registration_edge(kind), "{kind:?} registers its target");
    }
    for kind in [
        // Counted by the supertype facet from the other side — counting it here
        // too would inflate one graph fact into two.
        EdgeKind::Implements,
        EdgeKind::Extends,
        // A derived governance marker mirroring an edge already counted.
        EdgeKind::ForbiddenDependency,
        // Lexical nesting and field access are not registrations: sharing a
        // parent module is not an analogy.
        EdgeKind::Contains,
        EdgeKind::Accesses,
        // The regression this constant exists to hold. `Imports` runs from a
        // MODULE node to everything its file `use`s, so admitting it made every
        // pair of co-imported symbols "analogous" — 67 of them for one enum in
        // this repository's own index, all tied at one facet. A `use` list is
        // not a registry.
        EdgeKind::Imports,
        // A broker coupling, not a declaration site; out of scope for the three
        // facets [FR-NV-12] names.
        EdgeKind::Publishes,
        EdgeKind::Subscribes,
    ] {
        assert!(
            !is_registration_edge(kind),
            "{kind:?} must not count as a registration"
        );
    }
}

// ── Structural precedent: the branches no source fixture reaches ────────────
//
// `precedent_anchors` and `anchor_sharers` read a hydrated `DiGraph` and nothing
// else, so a hand-built graph reaches edge kinds this workspace's Rust grammar
// never emits (`Extends`, `Instantiates`) and pins invariants that need two
// edge kinds between the same pair of nodes — neither of which any indexed
// fixture can express.

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::BTreeSet;

use crate::hydrate::{EdgeData, Vertex};
use crate::model::EdgeKind;
use crate::models::navigation::PrecedentFacet;

/// A symbol-level vertex, as `build_symbol_level` would emit it.
fn vertex(graph: &mut DiGraph<Vertex, EdgeData>, key: &str) -> NodeIndex {
    graph.add_node(Vertex {
        key: key.to_string(),
        label: key.to_string(),
        kind: Some(NodeKind::Function),
        node_id: Some(NodeId(graph.node_count() as i64 + 1)),
    })
}

fn edge(graph: &mut DiGraph<Vertex, EdgeData>, from: NodeIndex, to: NodeIndex, kind: EdgeKind) {
    graph.add_edge(
        from,
        to,
        EdgeData {
            kind: Some(kind),
            weight: 1,
        },
    );
}

/// `Extends` is the second half of the supertype facet and no Rust fixture in
/// this workspace produces one — the grammar emits `Implements` for a trait impl
/// and nothing for inheritance, so deleting `| EdgeKind::Extends` from
/// [`precedent_anchors`] compiles and passes every integration test.
///
/// [FR-NV-12] names "shared trait **or interface implementation**", which in a
/// class language is `Extends`, so the arm has to be pinned somewhere.
#[test]
fn extends_is_a_supertype_anchor_just_as_implements_is() {
    let mut graph = DiGraph::<Vertex, EdgeData>::new();
    let base = vertex(&mut graph, "Base");
    let subclass = vertex(&mut graph, "Subclass");
    let sibling = vertex(&mut graph, "Sibling");
    edge(&mut graph, subclass, base, EdgeKind::Extends);
    edge(&mut graph, sibling, base, EdgeKind::Extends);

    let seeds: BTreeSet<NodeIndex> = [subclass].into_iter().collect();
    let anchors = precedent_anchors(&graph, &seeds);

    assert_eq!(anchors.len(), 1, "the superclass is the one anchor");
    assert_eq!(anchors[0].facet, PrecedentFacet::SharedSupertype);
    assert_eq!(graph[anchors[0].node].key, "Base");

    let sharers = anchor_sharers(&graph, anchors[0]);
    assert!(
        sharers.contains(&sibling),
        "the other subclass shares the superclass"
    );
}

/// An anchor matches only through the **same edge kind**. Two nodes that a
/// registry relates differently — one called, one instantiated — are not
/// analogous through it, and the code comment on [`anchor_sharers`] claims
/// exactly that.
///
/// No indexed fixture can express it: it needs one node with two different
/// outbound edge kinds to two different targets.
#[test]
fn a_registration_anchor_does_not_match_across_edge_kinds() {
    let mut graph = DiGraph::<Vertex, EdgeData>::new();
    let registry = vertex(&mut graph, "registry_fn");
    let target = vertex(&mut graph, "target");
    let called_too = vertex(&mut graph, "called_too");
    let merely_built = vertex(&mut graph, "merely_built");
    edge(&mut graph, registry, target, EdgeKind::Calls);
    edge(&mut graph, registry, called_too, EdgeKind::Calls);
    edge(&mut graph, registry, merely_built, EdgeKind::Instantiates);

    let seeds: BTreeSet<NodeIndex> = [target].into_iter().collect();
    let anchors = precedent_anchors(&graph, &seeds);

    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].facet, PrecedentFacet::SharedRegistration);
    assert_eq!(anchors[0].kind, EdgeKind::Calls);

    let sharers = anchor_sharers(&graph, anchors[0]);
    assert!(
        sharers.contains(&called_too),
        "the node the registry CALLS shares the registration"
    );
    assert!(
        !sharers.contains(&merely_built),
        "the node the registry INSTANTIATES does not — a shared registrar is \
         not enough, the relation must be the same one"
    );
}

/// `Instantiates` reaches the registration facet, and `Imports` does not — the
/// regression the S-359 review removed. Neither is expressible through the Rust
/// fixtures, where every `Imports` edge is module-sourced.
#[test]
fn instantiates_registers_and_imports_does_not() {
    for (kind, expected) in [(EdgeKind::Instantiates, true), (EdgeKind::Imports, false)] {
        let mut graph = DiGraph::<Vertex, EdgeData>::new();
        let source = vertex(&mut graph, "source");
        let target = vertex(&mut graph, "target");
        edge(&mut graph, source, target, kind);

        let seeds: BTreeSet<NodeIndex> = [target].into_iter().collect();
        let anchors = precedent_anchors(&graph, &seeds);

        assert_eq!(
            !anchors.is_empty(),
            expected,
            "{kind:?} registration-anchor admission drifted"
        );
    }
}

/// A self-loop cannot make a node its own precedent, and parallel edges cannot
/// inflate a count — the two petgraph shapes a hand-built graph can force.
#[test]
fn self_loops_and_parallel_edges_do_not_distort_the_anchor_set() {
    let mut graph = DiGraph::<Vertex, EdgeData>::new();
    let target = vertex(&mut graph, "target");
    let helper = vertex(&mut graph, "helper");
    let sibling = vertex(&mut graph, "sibling");
    edge(&mut graph, target, target, EdgeKind::Calls); // recursive
    edge(&mut graph, target, helper, EdgeKind::Calls);
    edge(&mut graph, target, helper, EdgeKind::Calls); // parallel duplicate
    edge(&mut graph, sibling, helper, EdgeKind::Calls);

    let anchors = precedent_anchors(&graph, &[target].into_iter().collect());

    // Exactly one anchor survives: the helper. The parallel edge collapses, and
    // the recursive self-call contributes nothing — left in, it became a
    // registration anchor on the target itself, which made every node the
    // target calls "analogous to it because we are both called by it".
    assert_eq!(
        anchors.len(),
        1,
        "a self-loop is not a relation to a third party, and a parallel edge is \
         not a second anchor: {anchors:?}"
    );
    let helper_anchor: &PrecedentAnchor = &anchors[0];
    assert_eq!(graph[helper_anchor.node].key, "helper");
    assert_eq!(helper_anchor.facet, PrecedentFacet::SharedCallee);

    let sharers = anchor_sharers(&graph, *helper_anchor);
    assert_eq!(
        sharers.len(),
        2,
        "target and sibling, each once despite the duplicate edge: {sharers:?}"
    );
    assert!(sharers.contains(&sibling));
}

/// The [ADR-14] degraded payload states its notion and its limits exactly as a
/// success does — the one answer with no data behind it is the one that most
/// needs them, and it is the path an integration test cannot reach.
#[test]
fn the_degraded_precedent_payload_still_states_itself() {
    let degraded = precedent_degraded("some::target", "precedent failed: boom".to_string());

    assert_eq!(degraded.query, "some::target");
    assert!(degraded.precedents.is_empty());
    assert_eq!(
        degraded.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("query_failed")
    );
    assert_eq!(degraded.warnings, vec!["precedent failed: boom".to_string()]);
    // Not `..Default::default()`: an empty notion here would violate
    // [FR-NV-12] AC 1 on the one path where nothing else is true either.
    assert!(degraded.notion.contains("never a score"));
    assert!(degraded.ranked_by.contains("canonical symbol ascending"));
    assert!(!degraded.coverage.statement.is_empty());
}

/// The wire name a consumer branches on is hand-written twice — once in
/// `serde`'s `rename_all`, once in `as_str`. Nothing else pins them together.
#[test]
fn precedent_facet_wire_names_match_their_serde_representation() {
    for facet in PrecedentFacet::ALL {
        assert_eq!(
            serde_json::to_value(facet).unwrap(),
            serde_json::Value::from(facet.as_str()),
            "{facet:?} serialises differently from as_str()"
        );
    }
}

/// Every [`EmptyPrecedentCode`]'s **serialized value**, pinned against literals.
///
/// The vocabulary was `String` literals until the Sprint 64 human review typed
/// it; the whole point of that change is that the wire stayed byte-identical,
/// so the literals here are the ones the strings carried, not the ones the
/// derive happens to produce. Dropping `rename_all` would emit
/// `"TargetUnresolved"`, leave every other test green, and break every consumer
/// branching on the documented code — the same trap
/// `every_degraded_cause_has_a_stable_kebab_case_wire_value` guards for
/// [`DegradedCause`](crate::federation::open_state::DegradedCause).
///
/// The `match` is the completeness half: a ninth code cannot compile until it
/// is listed below with its wire spelling.
#[test]
fn every_empty_precedent_code_has_a_stable_snake_case_wire_value() {
    use crate::models::navigation::EmptyPrecedentCode as C;
    for (code, wire) in [
        (C::TargetUnresolved, "target_unresolved"),
        (C::GraphEmpty, "graph_empty"),
        (C::TargetAbsentFromView, "target_absent_from_view"),
        (C::NoStructuralAnchors, "no_structural_anchors"),
        (C::AnchorsAreUnshared, "anchors_are_unshared"),
        (C::AnchorsAreUbiquitous, "anchors_are_ubiquitous"),
        (C::QueryFailed, "query_failed"),
        (C::ResultsUnavailable, "results_unavailable"),
    ] {
        match code {
            C::TargetUnresolved
            | C::GraphEmpty
            | C::TargetAbsentFromView
            | C::NoStructuralAnchors
            | C::AnchorsAreUnshared
            | C::AnchorsAreUbiquitous
            | C::QueryFailed
            | C::ResultsUnavailable => {}
        }
        assert_eq!(serde_json::to_value(code).unwrap(), wire);
        assert_eq!(code.as_str(), wire, "as_str disagrees with the wire value");
    }
}
