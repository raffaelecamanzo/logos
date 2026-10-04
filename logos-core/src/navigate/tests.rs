//! Unit tests for the navigation service's pure helpers (S-013).
//!
//! Everything that needs a live runtime/store is exercised end-to-end in
//! `tests/navigation.rs`; here we pin the I/O-free seams: code slicing with
//! its path-containment guard, and line-number wire conversion.

use std::fs;

use tempfile::TempDir;

use super::{
    anchor_sharers, is_registration_edge, line_u32, precedent_anchors,
    precedent_degraded, prefer_code, read_code, PrecedentAnchor,
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
    assert!(is_test_path("src/UserServiceTest.java"));
    assert!(is_test_path("src/UserServiceTests.java"));
    assert!(is_test_path("src/test/java/UserServiceTest.java"));
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

/// [S-524] A `test` segment under a production source root is a package name,
/// not test code; Gradle `*Test` source sets and `src/it` are test code.
#[test]
fn is_test_path_respects_production_source_roots() {
    use super::is_test_path;
    // koin: `org.koin.test` is a production package under `commonMain` / `src/main`.
    assert!(!is_test_path(
        "projects/core/koin-core/src/commonMain/kotlin/org/koin/test/Check.kt"
    ));
    assert!(!is_test_path("src/jvmMain/kotlin/org/koin/test/Check.kt"));
    assert!(!is_test_path("app/src/main/java/org/acme/test/Probe.java"));
    // ...while a `test` segment ABOVE the root is still a test tree (fixtures).
    assert!(is_test_path("test/fixtures/proj/src/main/java/Foo.java"));
    // FR-AN-05: the exemption covers every test-directory segment, not `test` alone.
    assert!(!is_test_path("src/main/java/org/acme/tests/Probe.java"));
    assert!(!is_test_path("src/main/js/__tests__/a.js"));
    assert!(!is_test_path("src/commonMain/kotlin/spec/A.kt"));
    assert!(!is_test_path("src/main/resources/spec/api.yaml"));
    // ...and each of the four still marks above the root.
    assert!(is_test_path("tests/fixtures/p/src/main/java/Foo.java"));
    assert!(is_test_path("__tests__/p/src/main/java/Foo.java"));
    assert!(is_test_path("spec/p/src/main/java/Foo.java"));
    // Near misses of the root names are not roots.
    assert!(is_test_path("src/mainline/test/Foo.java"));
    assert!(is_test_path("Main/test/Foo.kt")); // bare `Main` is not a `*Main` source set
    assert!(is_test_path("src/FooMain/kotlin/test/X.kt")); // PascalCase is not one either
    // `src/main` needs its `src/` parent: a bare `main` directory is no root.
    assert!(is_test_path("main/test/Foo.java"));
    assert!(is_test_path("cmd/main/test/x.go"));
}

/// [HF-2] A production source root overrides the FILENAME conventions as well
/// as the directory ones: a `*Test.kt` under `commonMain` / `src/main` is a
/// library class (koin's `KoinTest`, `AutoCloseKoinTest`), not test code.
#[test]
fn is_test_path_production_root_overrides_filename_conventions() {
    use super::is_test_path;
    // koin: the three files that stayed `is_test` after S-524.
    assert!(!is_test_path(
        "projects/core/koin-test/src/commonMain/kotlin/org/koin/test/KoinTest.kt"
    ));
    assert!(!is_test_path(
        "projects/core/koin-test-junit4/src/main/kotlin/org/koin/test/AutoCloseKoinTest.kt"
    ));
    assert!(!is_test_path(
        "projects/core/koin-test-junit5/src/main/kotlin/org/koin/test/junit5/AutoCloseKoinTest.kt"
    ));
    // Every filename convention is overridden, not `Test` alone.
    assert!(!is_test_path("src/main/java/FooTests.java"));
    assert!(!is_test_path("src/jvmMain/kotlin/foo_test.kt"));
    assert!(!is_test_path("src/main/rs/foo_tests.rs"));
    assert!(!is_test_path("src/main/rs/foo_spec.rb"));
    assert!(!is_test_path("src/main/py/test_foo.py"));
    assert!(!is_test_path("src/main/ts/foo.test.ts"));
    assert!(!is_test_path("src/main/ts/foo.spec.ts"));
    assert!(!is_test_path("src/commonMain/kotlin/tests.kt"));
    // Gradle test source sets and `src/it` keep marking — the same filename
    // under them is test code by directory AND by name.
    assert!(is_test_path("koin-core/src/test/kotlin/FooTest.kt"));
    assert!(is_test_path("koin-core/src/commonTest/kotlin/FooTest.kt"));
    assert!(is_test_path("koin-core/src/jvmTest/kotlin/FooTest.kt"));
    assert!(is_test_path("svc/src/it/java/FooIT.java"));
    // Outside any production root the filename conventions are unchanged.
    assert!(is_test_path("pkg/foo_test.go"));
    assert!(is_test_path("web/foo.test.ts"));
    assert!(is_test_path("lib/FooTest.kt"));
    // The root must be a real one: near misses keep the filename rule.
    assert!(is_test_path("src/mainline/FooTest.java"));
    assert!(is_test_path("Main/FooTest.kt")); // bare `Main` is not a source set
    assert!(is_test_path("src/FooMain/FooTest.kt")); // PascalCase is not one either
    assert!(is_test_path("main/FooTest.java")); // `main` needs its `src/` parent
    // A test directory ABOVE the root still marks, whatever the filename.
    assert!(is_test_path("test/fixtures/p/src/main/java/Bar.java"));
}

#[test]
fn is_test_path_marks_gradle_test_source_sets() {
    use super::is_test_path;
    assert!(is_test_path("koin-core/src/commonTest/kotlin/org/koin/Util.kt"));
    assert!(is_test_path("koin-core/src/jvmTest/kotlin/org/koin/Util.kt"));
    assert!(is_test_path("lib/src/androidInstrumentedTest/kotlin/Util.kt"));
    assert!(is_test_path("svc/src/it/java/org/acme/Support.java"));
    // Source sets are direct children of `src/`: a bare `it` locale directory
    // or a stray `*Test` directory elsewhere is not one.
    assert!(!is_test_path("locales/it/messages.json"));
    assert!(!is_test_path("src/it_support/Support.java"));
    assert!(!is_test_path("pkg/fooTest/Support.java"));
    assert!(!is_test_path("src/Test/Support.java")); // bare `Test` is no source-set name
    assert!(!is_test_path("src/FooTest/Support.java")); // PascalCase is not a source-set name
    assert!(!is_test_path("src/commonMain/kotlin/Util.kt"));
}

#[test]
fn is_test_path_tag_needs_a_three_part_filename() {
    use super::is_test_path;
    // werkzeug's `test.py` is a production module that merely says `test`.
    assert!(!is_test_path("src/werkzeug/test.py"));
    assert!(!is_test_path("src/spec.ts"));
    assert!(!is_test_path("src/test.rs"));
    // The three-part `*.test.*` / `*.spec.*` tag still marks.
    assert!(is_test_path("src/foo.test.ts"));
    assert!(is_test_path("src/foo.spec.js"));
    assert!(is_test_path("src/foo.bar.test.ts"));
    // Existing sibling conventions are untouched.
    assert!(is_test_path("tests/conftest.py"));
    assert!(is_test_path("src/test_foo.py"));
    assert!(is_test_path("pkg/foo_test.go"));
    assert!(is_test_path("src/foo_tests.rs"));
    assert!(is_test_path("src/__tests__/foo.ts"));
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

/// `precedent`'s empty reasons gain the reach clause **only** for a language
/// with no cross-file `Calls` figure ([FR-NV-12] AC 4, [CR-143] §3.7): a Rust
/// target, whose resolver binds across files, keeps the structural wording
/// unchanged, and a same-file-only language is named with its own state and
/// counts. The expected tag is read off the serialiser rather than spelled
/// here, so this file adds no absence wording of its own.
///
/// [FR-NV-12]: ../../../docs/specs/requirements/FR-NV-12.md
/// [CR-143]: ../../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
#[test]
fn the_precedent_reach_clause_names_only_a_language_without_a_cross_file_figure() {
    use crate::models::navigation::{LanguageResolution, RelationResolution, ResolutionDenominator};
    let row = |language: &str, cross_file: u64| LanguageResolution {
        language: language.to_string(),
        files: 3,
        calls: RelationResolution::measured(20, 7, 7, cross_file),
        imports: RelationResolution::measured(0, 0, 0, 0),
        call_residue: None,
    };
    let over = |rows: Vec<LanguageResolution>| {
        let anchors: Vec<Option<String>> = rows.iter().map(|r| Some(r.language.clone())).collect();
        ResolutionDenominator::measured(rows, &anchors)
    };

    assert_eq!(over(vec![row("rust", 4)]).unresolved_calls_clause(), None);
    // A language whose files recorded no call is absent of calls, not
    // unresolved: `no-references-recorded` earns no clause (R1).
    let call_free = LanguageResolution {
        calls: RelationResolution::measured(0, 0, 0, 0),
        ..row("python", 0)
    };
    assert!(call_free.calls.cross_file_absence.is_some());
    assert_eq!(over(vec![call_free]).unresolved_calls_clause(), None);
    assert_eq!(ResolutionDenominator::not_available().unresolved_calls_clause(), None);

    let typescript = row("typescript", 0);
    let tag = serde_json::to_value(typescript.calls.cross_file_absence.unwrap()).unwrap()["cause"]
        .as_str()
        .unwrap()
        .to_string();
    let clause = over(vec![row("rust", 4), typescript])
        .unresolved_calls_clause()
        .expect("the TypeScript row has no cross-file figure");
    assert_eq!(
        clause,
        format!(
            "typescript binds no Calls edge across a file boundary ({tag}; 7 of 20 Calls \
             reference(s) bound)"
        ),
        "only the language without a figure is named, with its state and counts"
    );
}

// ── FR-NV-15: a bare name prefers code to a module to a doc node ─────────────

/// A node of `kind` named `Utils`, with the given row id.
fn named(id: i64, kind: NodeKind) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("local symbol parses"),
        kind,
        name: "Utils".to_string(),
        file_path: None,
        start_line: None,
        end_line: None,
    }
}

/// `matches` in the order the store returns them (by id), ranked: the winner's
/// kind, then the passed-over kinds in order.
fn ranked(matches: Vec<NodeRow>) -> (NodeKind, Vec<NodeKind>) {
    let (chosen, rest) = prefer_code(matches).expect("a match");
    (chosen.kind, rest.iter().map(|r| r.kind).collect())
}

#[test]
fn a_bare_name_ranks_code_type_then_callable_then_module_then_doc() {
    // Stored in the REVERSE of the preference order, so lowest-id-wins — the
    // rule this replaced — would pick the doc section.
    let (winner, passed_over) = ranked(vec![
        named(1, NodeKind::DocSection),
        named(2, NodeKind::Module),
        named(3, NodeKind::Function),
        named(4, NodeKind::Class),
    ]);
    assert_eq!(winner, NodeKind::Class);
    assert_eq!(
        passed_over,
        [NodeKind::Function, NodeKind::Module, NodeKind::DocSection],
        "the alternatives follow in preference order"
    );
}

#[test]
fn every_code_type_kind_outranks_a_callable_a_module_and_a_doc() {
    for kind in [
        NodeKind::Class,
        NodeKind::Interface,
        NodeKind::Trait,
        NodeKind::Struct,
        NodeKind::Enum,
        NodeKind::TypeAlias,
    ] {
        let (winner, _) = ranked(vec![
            named(1, NodeKind::DocFile),
            named(2, NodeKind::Module),
            named(3, NodeKind::Method),
            named(4, kind),
        ]);
        assert_eq!(winner, kind, "{kind:?} is a code type");
    }
}

#[test]
fn a_callable_outranks_a_module_and_a_doc_when_no_type_matches() {
    for kind in [NodeKind::Function, NodeKind::Method] {
        let (winner, passed_over) =
            ranked(vec![named(1, NodeKind::Module), named(2, NodeKind::Adr), named(3, kind)]);
        assert_eq!(winner, kind);
        assert_eq!(passed_over, [NodeKind::Module, NodeKind::Adr]);
    }
}

#[test]
fn a_module_outranks_every_doc_kind() {
    for doc in [
        NodeKind::DocFile,
        NodeKind::DocSection,
        NodeKind::Requirement,
        NodeKind::Adr,
        NodeKind::Story,
    ] {
        let (winner, _) = ranked(vec![named(1, doc), named(2, NodeKind::Module)]);
        assert_eq!(winner, NodeKind::Module, "a module beats {doc:?}");
    }
}

#[test]
fn a_declaration_outside_the_four_classes_outranks_the_module_that_holds_it() {
    let (winner, passed_over) = ranked(vec![
        named(1, NodeKind::DocSection),
        named(2, NodeKind::Module),
        named(3, NodeKind::Constant),
    ]);
    assert_eq!(winner, NodeKind::Constant);
    assert_eq!(passed_over, [NodeKind::Module, NodeKind::DocSection]);
}

#[test]
fn within_one_class_the_lowest_id_still_wins() {
    let (chosen, rest) = prefer_code(vec![
        named(5, NodeKind::Class),
        named(7, NodeKind::Struct),
        named(9, NodeKind::Module),
    ])
    .expect("a match");
    assert_eq!(chosen.id, NodeId(5), "ties keep the id order the store returns");
    assert_eq!(rest.iter().map(|r| r.id).collect::<Vec<_>>(), [NodeId(7), NodeId(9)]);
}

#[test]
fn a_single_match_passes_over_nothing_and_no_match_is_none() {
    let (chosen, rest) = prefer_code(vec![named(1, NodeKind::Module)]).expect("a match");
    assert_eq!(chosen.kind, NodeKind::Module);
    assert!(rest.is_empty());
    assert!(prefer_code(Vec::new()).is_none());
}

/// Every config/artifact kind ([`NodeKind::is_config`]) — a YAML key, a shell
/// function, a Dockerfile stage, a proto message — is not a code declaration.
const CONFIG_KINDS: [NodeKind; 12] = [
    NodeKind::ConfigFile,
    NodeKind::ConfigSection,
    NodeKind::ShellFunction,
    NodeKind::DockerfileStage,
    NodeKind::MakeTarget,
    NodeKind::ProtoMessage,
    NodeKind::ProtoService,
    NodeKind::GqlType,
    NodeKind::SqlObject,
    NodeKind::TfBlock,
    NodeKind::ApiPath,
    NodeKind::ApiOperation,
];

#[test]
fn a_config_artifact_never_outranks_the_code_module_but_does_outrank_a_doc() {
    assert!(CONFIG_KINDS.iter().all(|k| k.is_config()), "the list is the config layer");
    for kind in CONFIG_KINDS {
        // A `server` key in `application.yml` must not beat the module `server.py`.
        let (winner, passed_over) = ranked(vec![
            named(1, kind),
            named(2, NodeKind::DocSection),
            named(3, NodeKind::Module),
        ]);
        assert_eq!(winner, NodeKind::Module, "a module beats the config kind {kind:?}");
        assert_eq!(passed_over, [kind, NodeKind::DocSection], "{kind:?} sits before a doc");
    }
}

#[test]
fn a_field_constant_or_route_ranks_after_a_callable_and_before_a_module() {
    // The class `Other` — declarations the requirement does not name — pinned
    // against BOTH neighbours it sits between, and against the code type above.
    for kind in [NodeKind::Field, NodeKind::Constant, NodeKind::Variable, NodeKind::Macro, NodeKind::Route] {
        let (winner, passed_over) = ranked(vec![
            named(1, NodeKind::Module),
            named(2, kind),
            named(3, NodeKind::Function),
            named(4, NodeKind::Class),
        ]);
        assert_eq!(winner, NodeKind::Class);
        assert_eq!(
            passed_over,
            [NodeKind::Function, kind, NodeKind::Module],
            "{kind:?} follows a callable and precedes a module"
        );
        let (winner, _) = ranked(vec![named(1, kind), named(2, NodeKind::Method)]);
        assert_eq!(winner, NodeKind::Method, "a method beats {kind:?}");
    }
}
