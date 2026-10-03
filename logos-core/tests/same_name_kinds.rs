//! Same-named declarations of different kinds get distinct symbols (S-512,
//! [CR-168]), driven end-to-end through the public [`Engine`] façade.
//!
//! `descriptor_for` renders several node kinds with one SCIP suffix — a Go
//! function and method are both `name().`, a TS interface and class both
//! `name#` — so numbering same-name siblings per *kind* gave two declarations
//! one symbol. The store's `symbol_id` upsert folded the two into one node, the
//! second `Contains` edge to it failed `UNIQUE(source, target, kind)`, and that
//! aborted the whole index with exit 0 and `files_indexed: 0`. Each fixture here
//! is one shape the 2026-10-03 language inspection found doing that on a real
//! repository, and each must now index every admitted file, warn nothing, and
//! store both declarations as distinct nodes ([FR-EX-02], [ADR-07]).
//!
//! Every fixture writes the colliding file **and** a clean bystander file, so
//! `files_indexed == 2` proves the run was not aborted rather than merely that
//! one file happened to persist.
//!
//! [CR-168]: ../../docs/requests/CR-168-an-index-never-silently-empties.md
//! [FR-EX-02]: ../../docs/specs/requirements/FR-EX-02.md
//! [ADR-07]: ../../docs/specs/architecture/decisions/ADR-07.md

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use logos_core::graph_store::NodeRow;
use logos_core::model::NodeKind;
use logos_core::models::pipeline::IndexResult;
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Index `files` in a fresh project and return the result plus every stored
/// node, read back from the store.
fn index(files: &[(&str, &str)]) -> (IndexResult, Vec<NodeRow>) {
    let tmp = TempDir::new().unwrap();
    for (rel, src) in files {
        write(tmp.path(), rel, src);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    let nodes = engine
        .runtime()
        .unwrap()
        .submit_read(|store| store.all_nodes())
        .unwrap_or_else(|err| {
            panic!(
                "every stored symbol reads back as a valid SCIP symbol \
                 (an empty or MISSING name renders a bare suffix): {err:#}"
            )
        });
    (result, nodes)
}

/// Index `rel` (holding the collision) beside a clean bystander file, and assert
/// the run indexed both files and warned nothing. Returns the stored nodes.
fn index_with_bystander(rel: &str, src: &str, bystander: (&str, &str)) -> Vec<NodeRow> {
    let (result, nodes) = index(&[(rel, src), bystander]);
    assert!(
        !result
            .warnings
            .iter()
            .any(|w| w.starts_with("index failed")),
        "the collision must not abort the index: {:?}",
        result.warnings
    );
    assert!(result.files_failed.is_empty(), "{:?}", result.files_failed);
    assert_eq!(
        result.files_indexed, 2,
        "files_indexed equals the admitted count (collision file + bystander): {:?}",
        result.warnings
    );
    nodes
}

/// The stored nodes named `name`, as `(kind, symbol)` pairs.
fn named(nodes: &[NodeRow], name: &str) -> Vec<(NodeKind, String)> {
    nodes
        .iter()
        .filter(|n| n.name == name)
        .map(|n| (n.kind, n.symbol.as_str().to_string()))
        .collect()
}

/// Assert exactly the `kinds` are stored under `name`, each with its own symbol.
fn assert_distinct(nodes: &[NodeRow], name: &str, kinds: &[NodeKind]) {
    let found = named(nodes, name);
    let mut got: Vec<NodeKind> = found.iter().map(|(k, _)| *k).collect();
    let mut want = kinds.to_vec();
    got.sort_by_key(|k| k.as_i32());
    want.sort_by_key(|k| k.as_i32());
    assert_eq!(
        got, want,
        "both declarations of `{name}` are stored: {found:?}"
    );
    let symbols: BTreeSet<&str> = found.iter().map(|(_, s)| s.as_str()).collect();
    assert_eq!(
        symbols.len(),
        found.len(),
        "each declaration of `{name}` has its own symbol: {found:?}"
    );
}

/// Assert `name` is stored as at least one `a` and one `b` node, every node of
/// that name with its own symbol — for grammars that also capture a nested
/// reference to the same name (the C++ query captures the `struct S` inside
/// `typedef struct S S;` as a child of the typedef).
fn assert_both_distinct(nodes: &[NodeRow], name: &str, a: NodeKind, b: NodeKind) {
    let found = named(nodes, name);
    let kinds: Vec<NodeKind> = found.iter().map(|(k, _)| *k).collect();
    assert!(
        kinds.contains(&a) && kinds.contains(&b),
        "both the {a:?} and the {b:?} `{name}` are stored: {found:?}"
    );
    assert_distinct(nodes, name, &kinds);
}

#[cfg(feature = "lang-go")]
#[test]
fn go_function_and_method_of_one_name_are_distinct_nodes() {
    // GO-G1 (zap, ollama): a free func and a method share the `().` slot.
    let nodes = index_with_bystander(
        "run.go",
        "package p\n\ntype T struct{}\n\nfunc (T) F() {}\n\nfunc F() {}\n",
        ("other.go", "package p\n\nfunc Other() {}\n"),
    );
    assert_distinct(&nodes, "F", &[NodeKind::Function, NodeKind::Method]);
}

#[cfg(feature = "lang-typescript")]
#[test]
fn ts_interface_and_class_of_one_name_are_distinct_nodes() {
    // TS declaration merging: `interface X` + `class X` share the `#` slot.
    let nodes = index_with_bystander(
        "src/x.ts",
        "export interface X { a: number }\nexport class X { b = 1; }\n",
        ("src/other.ts", "export function other() {}\n"),
    );
    assert_distinct(&nodes, "X", &[NodeKind::Interface, NodeKind::Class]);
}

#[cfg(feature = "lang-typescript")]
#[test]
fn js_function_and_object_literal_method_of_one_name_are_distinct_nodes() {
    // JS-G1 (preact): `function g` + `{ g() {} }` in one scope share `().`.
    let nodes = index_with_bystander(
        "src/lib.js",
        "function g() {}\nexport const o = { g() { return 1; } };\n",
        ("src/other.js", "export function other() {}\n"),
    );
    assert_distinct(&nodes, "g", &[NodeKind::Function, NodeKind::Method]);
}

#[cfg(feature = "lang-scala")]
#[test]
fn scala_trait_and_companion_object_are_distinct_nodes() {
    // gitbucket, ox: a trait and its companion `object` share the `#` slot.
    let nodes = index_with_bystander(
        "src/Shapes.scala",
        "trait X { def a: Int }\nobject X { def b: Int = 1 }\n",
        ("src/Other.scala", "object Other { def c: Int = 2 }\n"),
    );
    let kinds: Vec<NodeKind> = named(&nodes, "X").into_iter().map(|(k, _)| k).collect();
    assert_eq!(kinds.len(), 2, "trait and object both stored: {kinds:?}");
    assert_distinct(&nodes, "X", &kinds);
}

#[cfg(feature = "lang-cpp")]
#[test]
fn c_header_typedef_struct_and_its_definition_are_distinct_nodes() {
    // C-G1 (redis `server.h`, libuv): `typedef struct S S;` plus the struct's
    // definition share the `#` slot. A `.h` header belongs to the C++ plugin,
    // which captures the struct; the C plugin deliberately captures none.
    let nodes = index_with_bystander(
        "src/s.h",
        "typedef struct S S;\nstruct S { int a; };\n",
        ("src/other.c", "int other(void) { return 0; }\n"),
    );
    assert_both_distinct(&nodes, "S", NodeKind::Struct, NodeKind::TypeAlias);
}

#[cfg(feature = "lang-c")]
#[test]
fn c_enum_and_same_named_typedef_are_distinct_nodes() {
    // C-G1 in a `.c` file: `typedef enum E E;` plus the enum's definition share
    // the `#` slot under the C plugin's own query.
    let nodes = index_with_bystander(
        "src/e.c",
        "typedef enum E E;\nenum E { E_A };\n",
        ("src/other.c", "int other(void) { return 0; }\n"),
    );
    assert_both_distinct(&nodes, "E", NodeKind::Enum, NodeKind::TypeAlias);
}

#[cfg(feature = "lang-cpp")]
#[test]
fn cpp_struct_and_same_named_using_alias_are_distinct_nodes() {
    // CPP-G1 (nlohmann/json): a struct and a same-named `using` alias, `#` slot.
    let nodes = index_with_bystander(
        "src/w.cpp",
        "struct Widget { int x; };\nusing Widget = Widget;\n",
        ("src/other.cpp", "int other() { return 0; }\n"),
    );
    assert_distinct(&nodes, "Widget", &[NodeKind::Struct, NodeKind::TypeAlias]);
}

#[cfg(feature = "lang-cpp")]
#[test]
fn cpp_template_and_non_template_overloads_are_distinct_nodes() {
    // CPP-G1 (ccache, nlohmann/json): a member function template beside a plain
    // overload — the template is captured as a `Function`, the plain one as a
    // `Method`, and both render `get().`.
    let nodes = index_with_bystander(
        "src/box.cpp",
        "struct Box {\n  template <typename T> T get(T v) { return v; }\n  int get(int v) { return v; }\n};\n",
        ("src/other.cpp", "int other() { return 0; }\n"),
    );
    let found = named(&nodes, "get");
    assert_eq!(found.len(), 2, "both overloads stored: {found:?}");
    let kinds: Vec<NodeKind> = found.iter().map(|(k, _)| *k).collect();
    assert_distinct(&nodes, "get", &kinds);
}

#[cfg(feature = "lang-cpp")]
#[test]
fn cpp_anonymous_enum_emits_no_node_and_its_file_persists() {
    // CPP-G2: `enum : uint8_t {}` has an empty name; it must emit no symbol
    // (an empty descriptor is not a valid SCIP symbol) and must not cost the file.
    let nodes = index_with_bystander(
        "src/flags.cpp",
        "enum : uint8_t { kA, kB };\nint after() { return 0; }\n",
        ("src/other.cpp", "int other() { return 0; }\n"),
    );
    assert!(
        nodes.iter().all(|n| !n.name.is_empty()),
        "no node carries an empty name: {:?}",
        nodes.iter().map(|n| (&n.name, n.kind)).collect::<Vec<_>>()
    );
    assert_eq!(
        named(&nodes, "after").len(),
        1,
        "the file's other declarations persist"
    );
}

#[cfg(feature = "lang-c")]
#[test]
fn c_declaration_with_a_missing_name_emits_no_node_and_its_file_persists() {
    // CPP-G2's error-recovery half: `typedef struct { … } ;` parses with a
    // MISSING declarator, a zero-width name node. It must emit no symbol and must
    // not cost the file its other declarations.
    let nodes = index_with_bystander(
        "src/anon.c",
        "typedef struct { int a; } ;\nint after(void) { return 0; }\n",
        ("src/other.c", "int other(void) { return 0; }\n"),
    );
    assert!(
        nodes.iter().all(|n| !n.name.is_empty()),
        "no node carries an empty name: {:?}",
        nodes.iter().map(|n| (&n.name, n.kind)).collect::<Vec<_>>()
    );
    assert_eq!(
        named(&nodes, "after").len(),
        1,
        "the file's other declarations persist"
    );
}
