//! A C call binds across translation units (S-584, [FR-RS-40], [CR-185]).
//!
//! An `#include` records an `Imports` edge to the one header it names, read as
//! C++'s are (S-579). A bare call binds, in order:
//!
//! 1. through its includes — the caller's own file first, then every file its
//!    includes reach: a definition a header supplies (a `static inline`
//!    function, a macro) decides there; a prototype only names the function;
//! 2. by the **link-unit rule** — the one non-`static` definition of the name
//!    among the repository's C files.
//!
//! A `static` function is visible in its own file alone, and two non-`static`
//! definitions (platform variants such as libuv's `uv_loop_init`) leave the
//! call unbound.
//!
//! Driven through the [`Engine`] façade against the SHIPPED C queries.
//!
//! [FR-RS-40]: ../../docs/specs/requirements/FR-RS-40.md
//! [CR-185]: ../../docs/requests/CR-185-c-resolves-includes-prototypes-headers-and-macros.md
#![cfg(all(feature = "lang-c", feature = "lang-cpp"))]

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
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound edge of `kind` as `(source file:name, target file:name)`, sorted.
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

/// The bound edges of `kind` from the nodes of `file`.
fn from_file(rt: &Runtime, kind: EdgeKind, file: &str) -> Vec<(String, String)> {
    let prefix = format!("{file}:");
    edges(rt, kind).into_iter().filter(|(s, _)| s.starts_with(&prefix)).collect()
}

/// The `Calls` edges from `file` once `files` is indexed.
fn calls_from(files: &[(&str, &str)], file: &str) -> Vec<(String, String)> {
    let tmp = tree(files);
    let engine = index(tmp.path());
    from_file(engine.runtime().unwrap(), EdgeKind::Calls, file)
}

// ── Includes ─────────────────────────────────────────────────────────────────

#[test]
fn an_include_binds_the_header_it_names_and_a_system_header_stays_external() {
    let tmp = tree(&[
        ("src/main.c", "#include \"util.h\"\n#include <stdio.h>\n#include <core/types.h>\nint main(void) { return 0; }\n"),
        ("src/util.h", "int helper(void);\n"),
        ("src/core/types.h", "struct T { int x; };\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Imports, "src/main.c"),
        [
            edge("src/main.c:main", "src/core/types.h:types"),
            edge("src/main.c:main", "src/util.h:util"),
        ]
    );
}

// ── The link-unit rule ───────────────────────────────────────────────────────

/// redis's `processCommand → scriptIsRunning` shape: the prototype sits in a
/// header the caller includes, the body in another `.c` file.
#[test]
fn a_call_binds_the_one_non_static_definition_its_included_prototype_names() {
    let files = [
        ("src/script.h", "int scriptIsRunning(void);\n"),
        ("src/script.c", "#include \"script.h\"\nint scriptIsRunning(void) { return 0; }\n"),
        ("src/server.c", "#include \"script.h\"\nint processCommand(void) { return scriptIsRunning(); }\n"),
    ];
    assert_eq!(
        calls_from(&files, "src/server.c"),
        [edge("src/server.c:processCommand", "src/script.c:scriptIsRunning")]
    );
}

/// No include names the function (an implicit declaration, a header the
/// include roots do not hold): the link-unit rule still decides.
#[test]
fn a_call_binds_by_the_link_unit_rule_without_an_include() {
    let files = [
        ("lib/a.c", "int helper(int x) { return x; }\n"),
        ("app/main.c", "#include <lib/missing.h>\nint main(void) { return helper(1); }\n"),
    ];
    assert_eq!(calls_from(&files, "app/main.c"), [edge("app/main.c:main", "lib/a.c:helper")]);
}

#[test]
fn a_static_function_is_visible_in_its_own_file_alone() {
    let files = [
        ("src/a.c", "static int helper(void) { return 1; }\nint a(void) { return helper(); }\n"),
        ("src/main.c", "int main(void) { return helper(); }\n"),
    ];
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/a.c"), [edge("src/a.c:a", "src/a.c:helper")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/main.c"), []);
}

/// A `static` of another file is no rival: the one exported definition binds.
#[test]
fn a_static_namesake_elsewhere_does_not_make_the_exported_definition_ambiguous() {
    let files = [
        ("src/a.c", "static int helper(void) { return 1; }\n"),
        ("src/b.c", "int helper(void) { return 2; }\n"),
        ("src/main.c", "int main(void) { return helper(); }\n"),
    ];
    assert_eq!(calls_from(&files, "src/main.c"), [edge("src/main.c:main", "src/b.c:helper")]);
}

/// A definition an earlier `static` declaration makes file-local (C11 §6.2.2,
/// redis's `redis-cli.c` `dictListDestructor`) is its file's alone too: no
/// candidate for another file, nor a rival of the exported one.
#[test]
fn a_definition_an_earlier_static_declaration_makes_file_local_is_no_candidate() {
    let a = ("src/a.c", "static int helper(void);\nint a(void) { return helper(); }\nint helper(void) { return 1; }\n");
    let main = ("src/main.c", "int main(void) { return helper(); }\n");
    assert_eq!(calls_from(&[a, main], "src/main.c"), []);
    let c = ("src/c.c", "int helper(void) { return 2; }\n");
    assert_eq!(calls_from(&[a, c, main], "src/main.c"), [edge("src/main.c:main", "src/c.c:helper")]);
    assert_eq!(calls_from(&[a, c, main], "src/a.c"), [edge("src/a.c:a", "src/a.c:helper")]);
}

/// libuv's `uv_default_loop → uv_loop_init`: one definition per platform, both
/// non-`static`, both behind the one prototype — the call stays unbound.
#[test]
fn two_non_static_definitions_leave_the_call_unbound() {
    let files = [
        ("include/uv.h", "int uv_loop_init(void *loop);\n"),
        ("src/unix/loop.c", "#include \"uv.h\"\nint uv_loop_init(void *loop) { return 0; }\n"),
        ("src/win/core.c", "#include \"uv.h\"\nint uv_loop_init(void *loop) { return 1; }\n"),
        ("src/uv-common.c", "#include \"uv.h\"\nvoid *uv_default_loop(void) { uv_loop_init(0); return 0; }\n"),
    ];
    assert_eq!(calls_from(&files, "src/uv-common.c"), []);
}

/// A call through a function pointer — a parameter, a local, a file-scope
/// variable — names no function: it binds none, wherever one of its name is.
#[test]
fn a_call_through_a_function_pointer_binds_no_function() {
    let lib = ("src/lib.c", "int cb(int x) { return x; }\nint op(int x) { return x; }\nint hook(void) { return 1; }\n");
    for (caller, text) in [
        ("src/param.c", "int apply(int (*cb)(int), int x) { return cb(x); }\n"),
        ("src/local.c", "int run(void) { int (*op)(int) = 0; return op(1); }\n"),
        ("src/global.c", "static int (*hook)(void) = 0;\nint main(void) { return hook(); }\n"),
        ("src/global_init.c", "int (*hook)(void);\nint go(void) { return hook(); }\n"),
    ] {
        assert_eq!(calls_from(&[lib, (caller, text)], caller), [], "{caller}");
    }
    // A near miss: a local declared in a block the call is not in, or after
    // the call, shadows nothing.
    let near = (
        "src/near.c",
        "int f(int x) {\n    {\n        int op = 0;\n    }\n    int r = op(x) + cb(x);\n    int cb = 0;\n    return r + cb;\n}\n",
    );
    assert_eq!(
        calls_from(&[lib, near], "src/near.c"),
        [edge("src/near.c:f", "src/lib.c:cb"), edge("src/near.c:f", "src/lib.c:op")]
    );
}

/// The caller's own definition decides before any other file's.
#[test]
fn the_callers_own_definition_decides_before_the_link_unit() {
    let files = [
        ("src/a.c", "int helper(void) { return 1; }\n"),
        ("src/main.c", "int helper(void) { return 2; }\nint main(void) { return helper(); }\n"),
    ];
    assert_eq!(calls_from(&files, "src/main.c"), [edge("src/main.c:main", "src/main.c:helper")]);
}

/// A definition an included header supplies — a `static inline` function, a
/// function-like macro — decides before the link unit, which would otherwise
/// read the `.c` file's namesake.
#[test]
fn a_definition_an_include_supplies_decides_before_the_link_unit() {
    let files = [
        (
            "src/util.h",
            "static inline int clamp(int x) { return x < 0 ? 0 : x; }\n#define MAX(a, b) ((a) > (b) ? (a) : (b))\n",
        ),
        ("src/other.c", "int clamp(int x) { return x; }\n"),
        ("src/main.c", "#include \"util.h\"\nint main(void) { return clamp(MAX(1, 2)); }\n"),
    ];
    assert_eq!(
        calls_from(&files, "src/main.c"),
        [edge("src/main.c:main", "src/util.h:MAX"), edge("src/main.c:main", "src/util.h:clamp")]
    );
}

/// Where C reads the repository's headers, a header's non-`static` definition
/// — a single-header library's implementation — is in the link unit, and its
/// `static inline` one is not.
#[test]
fn a_c_read_headers_non_static_definition_is_in_the_link_unit_and_its_static_one_is_not() {
    let files = [
        ("deps/xxhash/xxhash.h", "unsigned long XXH64(const void *p, int n) { return 0; }\nstatic inline int xxh_local(void) { return 0; }\n"),
        ("src/chk.c", "#include \"xxhash.h\"\nint chk(void) { return XXH64(0, 0) + xxh_local(); }\n"),
    ];
    assert_eq!(calls_from(&files, "src/chk.c"), [edge("src/chk.c:chk", "deps/xxhash/xxhash.h:XXH64")]);
}

/// Where the repository's headers are read as C++ (as many C++ sources as C),
/// a header prototype is a node; reached through an include it pairs with the
/// one non-`static` definition of its name.
#[test]
fn a_prototype_a_cpp_read_header_declares_pairs_with_its_c_definition() {
    let files = [
        ("src/api.h", "int helper(void);\n"),
        ("src/helper.c", "int helper(void) { return 1; }\n"),
        ("src/main.c", "#include \"api.h\"\nint main(void) { return helper(); }\n"),
        ("src/x.cpp", "int x() { return 0; }\n"),
        ("src/y.cpp", "int y() { return 0; }\n"),
    ];
    assert_eq!(calls_from(&files, "src/main.c"), [edge("src/main.c:main", "src/helper.c:helper")]);
}

/// A prototype names a function, never binds one: with no definition of its
/// name in the link unit, or two, the call stays unbound.
#[test]
fn a_prototype_with_no_one_definition_leaves_the_call_unbound() {
    let files = |defs: &[(&'static str, &'static str)]| {
        let mut files = vec![
            ("src/api.h", "int helper(void);\n"),
            ("src/main.c", "#include \"api.h\"\nint main(void) { return helper(); }\n"),
            ("src/x.cpp", "int x() { return 0; }\n"),
            ("src/y.cpp", "int y() { return 0; }\n"),
            ("src/z.cpp", "int z() { return 0; }\n"),
        ];
        files.extend_from_slice(defs);
        files
    };
    assert_eq!(calls_from(&files(&[]), "src/main.c"), []);
    let two = [("src/h1.c", "int helper(void) { return 1; }\n"), ("src/h2.c", "int helper(void) { return 2; }\n")];
    assert_eq!(calls_from(&files(&two), "src/main.c"), []);
}

/// The rule is C's: a C++ call reads its includes alone, and a function no
/// include declares stays unbound there.
#[test]
fn a_cpp_call_never_reaches_the_link_unit() {
    let files = [
        ("src/a.cpp", "int helper() { return 1; }\n"),
        ("src/main.cpp", "int main() { return helper(); }\n"),
    ];
    assert_eq!(calls_from(&files, "src/main.cpp"), []);
}

/// …nor a C call a C++ definition: the link unit is the C files alone.
#[test]
fn a_c_call_never_binds_a_cpp_definition_by_the_link_unit() {
    let files = [
        ("src/a.cpp", "int helper() { return 1; }\n"),
        ("src/main.c", "int main(void) { return helper(); }\n"),
    ];
    assert_eq!(calls_from(&files, "src/main.c"), []);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced one's bound
/// `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, String)> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        write(tmp_a.path(), rel, text);
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);

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
    edges(engine_a.runtime().unwrap(), EdgeKind::Calls)
}

/// A second definition in an untouched file's view, or a `static` dropped from
/// one, moves a call whose file the sync never re-extracts and whose name the
/// change does not spell in that file.
#[test]
fn a_rival_definition_or_a_dropped_static_moves_the_call_on_sync_as_on_reindex() {
    let initial = [
        ("src/a.c", "int helper(void) { return 1; }\n"),
        ("src/b.c", "static int helper(void) { return 2; }\n"),
        ("src/main.c", "int main(void) { return helper(); }\n"),
    ];
    let bound = [edge("src/main.c:main", "src/a.c:helper")];
    assert_eq!(synced_equals_reindexed(&initial, &[]), bound);
    // `static` dropped: two exported definitions, the call unbinds.
    assert_eq!(synced_equals_reindexed(&initial, &[("src/b.c", "int helper(void) { return 2; }\n")]), []);
    // …and restored: it binds again.
    let rival = [initial[0], ("src/b.c", "int helper(void) { return 2; }\n"), initial[2]];
    assert_eq!(synced_equals_reindexed(&rival, &[initial[1]]), bound);
}
