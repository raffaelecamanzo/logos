//! A C++ prototype pairs with its definition in another file, and its callers
//! reach the definition (S-820, [CR-254], [FR-EX-29], [FR-RS-38], [FR-NV-15]).
//!
//! A bodiless prototype and a bodied definition in another file are a pair
//! when the definition's file reaches the prototype's through its includes,
//! both have one scope path and name, both record a parameter shape and the
//! shapes are equal, neither has a second match, and the definition is not
//! file-local. Both nodes stay; the prototype records a `References` link to
//! the definition, a call landing on the prototype binds the definition, and a
//! bare-name lookup lands on the definition whatever the files' order.
//!
//! Driven through the [`Engine`] façade against the SHIPPED C++ queries.
//! Fixtures are written inline, like every sibling C++ suite.
//!
//! [CR-254]: ../../docs/requests/CR-254-cpp-prototype-pairs-with-definition-across-files.md
//! [FR-EX-29]: ../../docs/specs/requirements/FR-EX-29.md
//! [FR-RS-38]: ../../docs/specs/requirements/FR-RS-38.md
//! [FR-NV-15]: ../../docs/specs/requirements/FR-NV-15.md
#![cfg(feature = "lang-cpp")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::EdgeKind;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.pipeline().index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("read runs");
    let label = |id| {
        let n = nodes.iter().find(|n| n.id == id).expect("edge endpoint is a node");
        format!("{}:{}", n.file_path.as_deref().unwrap_or("?"), n.name)
    };
    let mut out: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label(e.source), label(e.target)))
        .collect();
    out.sort();
    out
}

fn edge(source: &str, target: &str) -> (String, String) {
    (source.to_string(), target.to_string())
}

/// The canonical symbol of the one node named `name` in `file`.
fn symbol_in(rt: &Runtime, file: &str, name: &str) -> String {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let found: Vec<String> = nodes
        .iter()
        .filter(|n| n.name == name && n.file_path.as_deref() == Some(file))
        .map(|n| n.symbol.as_str().to_string())
        .collect();
    match found.as_slice() {
        [only] => only.clone(),
        many => panic!("expected one {name} in {file}, got {many:?}"),
    }
}

// ── The repo.h / repo.cpp / use.cpp case ─────────────────────────────────────

const REPO_H: &str = "class Repo {\npublic:\n  void save();\n};\n";
const REPO_CPP: &str = "#include \"repo.h\"\nvoid Repo::save() {}\n";
const USE_CPP: &str = "#include \"repo.h\"\nvoid use(Repo& r) { r.save(); }\n";

/// The Sprint 109 testing guide's T4: the call on a typed receiver lands on
/// the header's prototype, which pairs with `repo.cpp`'s out-of-line member.
#[test]
fn a_call_on_a_paired_prototype_binds_its_definition_in_another_file() {
    let tmp = tree(&[("src/repo.h", REPO_H), ("src/repo.cpp", REPO_CPP), ("src/use.cpp", USE_CPP)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        [edge("src/use.cpp:use", "src/repo.cpp:save")],
        "the call binds the definition, never the bodiless prototype"
    );
    assert_eq!(
        edges(rt, EdgeKind::References),
        [edge("src/repo.h:save", "src/repo.cpp:save")],
        "the prototype links to its definition"
    );
    let definition = symbol_in(rt, "src/repo.cpp", "save");
    let callers = engine.navigation().callers(&definition, None);
    let names: Vec<&str> = callers.callers.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["use"], "callers of the definition name the caller");
}

/// A namespace function pairs as a member does, written inside the namespace
/// or qualified by it.
#[test]
fn a_namespace_function_pairs_with_its_definition_whichever_way_it_is_written() {
    let header = "namespace util {\nint clamp(int v);\n}\n";
    let main = "#include \"util.h\"\nint main() { return util::clamp(1); }\n";
    for definition in [
        "#include \"util.h\"\nnamespace util {\nint clamp(int v) { return v; }\n}\n",
        "#include \"util.h\"\nint util::clamp(int value) { return value; }\n",
    ] {
        let tmp = tree(&[("src/util.h", header), ("src/util.cpp", definition), ("src/main.cpp", main)]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/main.cpp:main", "src/util.cpp:clamp")], "{definition}");
        assert_eq!(edges(rt, EdgeKind::References), [edge("src/util.h:clamp", "src/util.cpp:clamp")], "{definition}");
    }
}

/// A qualified call that reaches an inherited member through a derived class
/// (`Derived::make()`, the base's `make` found up its supertypes) binds the
/// member's definition too (S-820 review).
#[test]
fn an_inherited_member_reached_through_a_derived_class_binds_its_definition() {
    let tmp = tree(&[
        ("src/base.h", "struct Base {\n  static int make();\n};\n"),
        ("src/derived.h", "#include \"base.h\"\nstruct Derived : Base {};\n"),
        ("src/base.cpp", "#include \"base.h\"\nint Base::make() { return 0; }\n"),
        ("src/main.cpp", "#include \"derived.h\"\nint main() { return Derived::make(); }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Calls), [edge("src/main.cpp:main", "src/base.cpp:make")]);
}

/// [FR-NV-15]: a bare name both halves carry lands on the definition, even
/// when the header's path sorts first and so holds the lower node id.
#[test]
fn a_bare_name_lookup_lands_on_the_definition_when_the_header_sorts_first() {
    let tmp = tree(&[
        ("src/api/repo.h", REPO_H),
        ("src/repo.cpp", "#include \"api/repo.h\"\nvoid Repo::save() {}\n"),
    ]);
    let engine = index(tmp.path());
    let found = engine.navigation().node("save", false);
    let node = found.node.expect("save resolves");
    assert_eq!(node.symbol.file.as_deref(), Some("src/repo.cpp"), "the definition, not the prototype");
    let passed_over: Vec<Option<&str>> = found.alternatives.iter().map(|a| a.file.as_deref()).collect();
    assert_eq!(passed_over, [Some("src/api/repo.h")], "the prototype heads what was passed over");
}

// ── What never pairs ─────────────────────────────────────────────────────────

/// Two matching definitions, a file-local one and one of other parameters
/// record no pair, and the call binds as it did before: the prototype.
#[test]
fn an_ambiguous_file_local_or_differently_shaped_definition_records_no_pair() {
    let header = "namespace util {\nint clamp(int v);\n}\n";
    let main = "#include \"util.h\"\nint main() { return util::clamp(1); }\n";
    let cases: [&[(&str, &str)]; 3] = [
        &[
            ("src/a.cpp", "#include \"util.h\"\nnamespace util {\nint clamp(int v) { return v; }\n}\n"),
            ("src/b.cpp", "#include \"util.h\"\nnamespace util {\nint clamp(int v) { return -v; }\n}\n"),
        ],
        &[("src/a.cpp", "#include \"util.h\"\nnamespace util {\nstatic int clamp(int v) { return v; }\n}\n")],
        &[("src/a.cpp", "#include \"util.h\"\nnamespace util {\nint clamp(long v) { return 0; }\n}\n")],
    ];
    for definitions in cases {
        let mut files = vec![("src/util.h", header), ("src/main.cpp", main)];
        files.extend_from_slice(definitions);
        let tmp = tree(&files);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(edges(rt, EdgeKind::References), [], "{definitions:?}");
        assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/main.cpp:main", "src/util.h:clamp")], "{definitions:?}");
    }
}

/// A definition two prototypes of one name and shape match — two headers its
/// file includes — pairs with neither: the prototype side is as exact-one as
/// the definition side (S-820 review).
#[test]
fn a_definition_two_reachable_prototypes_match_records_no_pair() {
    let tmp = tree(&[
        ("src/a.h", "int clamp(int v);\n"),
        ("src/b.h", "int clamp(int v);\n"),
        ("src/def.cpp", "#include \"a.h\"\n#include \"b.h\"\nint clamp(int v) { return v; }\n"),
        ("src/main.cpp", "#include \"a.h\"\nint main() { return clamp(1); }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::References), []);
    assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/main.cpp:main", "src/a.h:clamp")]);
}

/// A method of a class a function's body declares has no scope another file
/// names: it never pairs with a header's same-named member (S-820 review).
#[test]
fn a_member_of_a_function_local_class_records_no_pair() {
    let tmp = tree(&[
        ("src/l.h", "struct L {\n  void m();\n};\n"),
        ("src/b.cpp", "#include \"l.h\"\nvoid f() {\n  struct L { void m() {} };\n}\n"),
        ("src/use.cpp", "#include \"l.h\"\nvoid use(L& l) { l.m(); }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::References), []);
    assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/use.cpp:use", "src/l.h:m")]);
}

/// A definition in an anonymous namespace has internal linkage: it is never
/// the global function a header declares, so no pair.
#[test]
fn a_definition_in_an_anonymous_namespace_records_no_pair() {
    let tmp = tree(&[
        ("src/clamp.h", "int clamp(int v);\n"),
        ("src/a.cpp", "#include \"clamp.h\"\nnamespace {\nint clamp(int v) { return v; }\n}\n"),
        ("src/main.cpp", "#include \"clamp.h\"\nint main() { return clamp(1); }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::References), []);
    assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/main.cpp:main", "src/clamp.h:clamp")]);
}

/// A definition whose file never includes the header is out of the include
/// map's reach: no pair.
#[test]
fn a_definition_whose_file_does_not_include_the_header_records_no_pair() {
    let tmp = tree(&[("src/repo.h", REPO_H), ("src/repo.cpp", "void Repo::save() {}\n"), ("src/use.cpp", USE_CPP)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::References), []);
    assert_eq!(edges(rt, EdgeKind::Calls), [edge("src/use.cpp:use", "src/repo.h:save")]);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced one's
/// `References` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, String)> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        write(tmp_a.path(), rel, text);
        changed.push(root_a.join(rel));
    }
    engine_a.pipeline().sync(&changed);

    let mut final_state: std::collections::BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    edges(engine_a.runtime().unwrap(), EdgeKind::References)
}

const PAIR: [&str; 2] = ["src/repo.h:save", "src/repo.cpp:save"];

#[test]
fn a_definition_added_or_removed_pairs_and_unpairs_on_sync_as_on_reindex() {
    let without = [("src/repo.h", REPO_H), ("src/repo.cpp", "#include \"repo.h\"\n"), ("src/use.cpp", USE_CPP)];
    let with = [without[0], ("src/repo.cpp", REPO_CPP), without[2]];
    assert_eq!(synced_equals_reindexed(&without, &[with[1]]), [edge(PAIR[0], PAIR[1])]);
    assert_eq!(synced_equals_reindexed(&with, &[without[1]]), []);
}

/// The definition keeps its symbol when its parameters change, so the edge
/// into it is captured before its file is re-extracted: the capture must not
/// restore a pair the shapes no longer make.
#[test]
fn a_definition_whose_parameters_change_unpairs_on_sync_as_on_reindex() {
    let initial = [("src/repo.h", REPO_H), ("src/repo.cpp", REPO_CPP), ("src/use.cpp", USE_CPP)];
    let edit = [("src/repo.cpp", "#include \"repo.h\"\nvoid Repo::save(int n) {}\n")];
    assert_eq!(synced_equals_reindexed(&initial, &edit), []);
}

/// A second definition in a third file unpairs the two, although neither of
/// their files changed: the committed link is retracted, not captured.
#[test]
fn a_second_definition_elsewhere_unpairs_on_sync_as_on_reindex() {
    let initial = [
        ("src/repo.h", REPO_H),
        ("src/repo.cpp", REPO_CPP),
        ("src/use.cpp", USE_CPP),
        ("src/other.cpp", "#include \"repo.h\"\n"),
    ];
    assert_eq!(synced_equals_reindexed(&initial, &[("src/other.cpp", REPO_CPP)]), []);
}

/// A prototype whose default argument calls is the source of a row that every
/// sync re-binds, so its edges are swept: the pair link is not a row's, and
/// the pass writes it again.
#[test]
fn a_prototype_whose_default_argument_calls_keeps_its_pair_on_sync_as_on_reindex() {
    let header = "int limit();\nclass Repo {\npublic:\n  void save(int n = limit());\n};\n";
    let initial = [
        ("src/repo.h", header),
        ("src/repo.cpp", "#include \"repo.h\"\nvoid Repo::save(int n) {}\n"),
        ("src/use.cpp", USE_CPP),
    ];
    let edit = [("src/use.cpp", "#include \"repo.h\"\nvoid use(Repo& r) { r.save(1); }\n")];
    assert_eq!(synced_equals_reindexed(&initial, &edit), [edge(PAIR[0], PAIR[1])]);
}

#[test]
fn a_header_prototype_added_or_removed_pairs_and_unpairs_on_sync_as_on_reindex() {
    let bare = "class Repo {\npublic:\n};\n";
    let without = [("src/repo.h", bare), ("src/repo.cpp", REPO_CPP), ("src/use.cpp", USE_CPP)];
    let with = [("src/repo.h", REPO_H), without[1], without[2]];
    assert_eq!(synced_equals_reindexed(&without, &[with[0]]), [edge(PAIR[0], PAIR[1])]);
    assert_eq!(synced_equals_reindexed(&with, &[without[0]]), []);
}

#[test]
fn the_include_between_them_added_or_removed_pairs_and_unpairs_on_sync_as_on_reindex() {
    let without = [("src/repo.h", REPO_H), ("src/repo.cpp", "void Repo::save() {}\n"), ("src/use.cpp", USE_CPP)];
    let with = [without[0], ("src/repo.cpp", REPO_CPP), without[2]];
    assert_eq!(synced_equals_reindexed(&without, &[with[1]]), [edge(PAIR[0], PAIR[1])]);
    assert_eq!(synced_equals_reindexed(&with, &[without[1]]), []);
}

/// The include can sit in a header between: the definition's file reaches the
/// prototype's transitively, and a sync of that header alone moves the pair.
#[test]
fn an_include_a_header_between_gains_pairs_on_sync_as_on_reindex() {
    let initial = [
        ("src/repo.h", REPO_H),
        ("src/mid.h", "// nothing yet\n"),
        ("src/repo.cpp", "#include \"mid.h\"\nvoid Repo::save() {}\n"),
        ("src/use.cpp", USE_CPP),
    ];
    assert_eq!(synced_equals_reindexed(&initial, &[("src/mid.h", "#include \"repo.h\"\n")]), [edge(PAIR[0], PAIR[1])]);
}
