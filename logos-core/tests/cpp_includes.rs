//! C++ includes resolve, and free, qualified calls and base classes bind
//! (S-579, [FR-RS-38], [CR-184]).
//!
//! An `#include` records an `Imports` edge to the one header it names — a
//! quoted one beside the including file first, then under the include roots
//! (`include/`, `src/`, a directory holding `include/`); a system header stays
//! external. A free or qualified call (`f()`, `ns::f()`, `Cls::sf()`) and a base
//! class (`class A : public B`) bind through the caller's namespaces and class,
//! its `using`s, and the declarations its includes make visible — exactly one,
//! or nothing.
//!
//! Driven through the [`Engine`] façade against the SHIPPED C++ queries.
//! Fixtures are written inline, like every sibling C++ suite.
//!
//! [FR-RS-38]: ../../docs/specs/requirements/FR-RS-38.md
//! [CR-184]: ../../docs/requests/CR-184-cpp-resolves-includes-out-of-line-members-and-relations.md
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

// ── Includes ─────────────────────────────────────────────────────────────────

#[test]
fn a_quoted_include_binds_the_header_beside_it_before_the_include_roots() {
    let tmp = tree(&[
        ("src/app/main.cpp", "#include \"util.hpp\"\n#include \"core/types.hpp\"\nint main() { return 0; }\n"),
        ("src/app/util.hpp", "int util();\n"),
        ("src/util.hpp", "int util();\n"),
        ("src/core/types.hpp", "struct T {};\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Imports, "src/app/main.cpp"),
        [
            // Beside the including file, never the root's `src/util.hpp`.
            edge("src/app/main.cpp:main", "src/app/util.hpp:util"),
            // Nothing beside it: under the `src/` root.
            edge("src/app/main.cpp:main", "src/core/types.hpp:types"),
        ]
    );
}

#[test]
fn an_angle_include_binds_under_the_include_roots_and_a_system_header_stays_external() {
    let tmp = tree(&[
        ("src/ccache/ccache.cpp", "#include <ccache/hash.hpp>\n#include <vector>\n#include <sibling.hpp>\n"),
        ("src/ccache/hash.hpp", "class Hash {};\n"),
        // A header beside the including file is not read for `<…>`: no root
        // holds `sibling.hpp`.
        ("src/ccache/sibling.hpp", "class Sibling {};\n"),
        ("tests/unit.cpp", "#include <nlohmann/json.hpp>\n"),
        ("include/nlohmann/json.hpp", "namespace nlohmann { class json {}; }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        from_file(rt, EdgeKind::Imports, "src/ccache/ccache.cpp"),
        [edge("src/ccache/ccache.cpp:ccache", "src/ccache/hash.hpp:hash")]
    );
    assert_eq!(
        from_file(rt, EdgeKind::Imports, "tests/unit.cpp"),
        [edge("tests/unit.cpp:unit", "include/nlohmann/json.hpp:json")]
    );
}

#[test]
fn an_include_names_its_header_whole_and_one_two_roots_hold_stays_unbound() {
    let tmp = tree(&[
        // `hash.hpp` beside `hash.cpp`: the extension names the file.
        ("src/a.cpp", "#include \"hash.hpp\"\n#include \"hash.h\"\n#include <both.h>\n"),
        ("src/hash.hpp", "int h();\n"),
        ("src/hash.cpp", "int h() { return 1; }\n"),
        ("src/both.h", "int b();\n"),
        ("include/both.h", "int b();\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Imports, "src/a.cpp"),
        // `"hash.h"` names no file (a near miss of `hash.hpp`), and `<both.h>`
        // two roots hold.
        [edge("src/a.cpp:a", "src/hash.hpp:hash")]
    );
}

// ── Calls ────────────────────────────────────────────────────────────────────

const FILE_HPP: &str = "\
namespace util {
bool exists(const char* path);
}
";

#[test]
fn a_qualified_call_binds_through_an_include_and_never_without_one() {
    let tmp = tree(&[
        ("src/util/file.hpp", FILE_HPP),
        ("src/main.cpp", "#include \"util/file.hpp\"\nint main() { return util::exists(\"x\"); }\n"),
        // The same call in a file that includes nothing names nothing it sees.
        ("src/other.cpp", "int other() { return util::exists(\"x\"); }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/main.cpp"), [edge("src/main.cpp:main", "src/util/file.hpp:exists")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/other.cpp"), []);
}

#[test]
fn a_free_call_binds_through_its_enclosing_namespace_and_transitive_includes() {
    let tmp = tree(&[
        ("src/core/a.hpp", "namespace core {\nvoid helper();\n}\n"),
        ("src/core/b.hpp", "#include \"a.hpp\"\n"),
        ("src/core/c.cpp", "#include \"b.hpp\"\nnamespace core {\nvoid run() { helper(); }\n}\n"),
        // Outside `core` the bare name is not `core::helper`.
        ("src/d.cpp", "#include \"core/b.hpp\"\nvoid go() { helper(); }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/core/c.cpp"), [edge("src/core/c.cpp:run", "src/core/a.hpp:helper")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/d.cpp"), []);
}

const WIDGET_HPP: &str = "\
class Widget {
public:
  static Widget make();
  void reset();
  void use();
};
";

const WIDGET_CPP: &str = "\
#include \"widget.hpp\"
#include \"free.hpp\"

Widget Widget::make() { return Widget(); }

void Widget::reset() {}

void Widget::use() { reset(); }

void tick() { reset(); }
";

#[test]
fn a_static_member_call_binds_the_class_member_and_an_out_of_line_one_its_definition() {
    let tmp = tree(&[
        ("src/widget.hpp", WIDGET_HPP),
        ("src/free.hpp", "void reset();\n"),
        ("src/widget.cpp", WIDGET_CPP),
        ("src/main.cpp", "#include \"widget.hpp\"\nint main() { Widget w = Widget::make(); return 0; }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Another file reaches the declaration its include makes visible.
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/main.cpp"), [edge("src/main.cpp:main", "src/widget.hpp:make")]);
    let widget = from_file(rt, EdgeKind::Calls, "src/widget.cpp");
    // Inside an out-of-line member, a bare name is the class's member first —
    // its definition in this file, never the free `reset` `free.hpp` declares.
    assert!(widget.contains(&edge("src/widget.cpp:use", "src/widget.cpp:reset")), "{widget:#?}");
    // Outside the class, `reset` is the free function: an out-of-line member is
    // reached only through its class's name.
    assert!(widget.contains(&edge("src/widget.cpp:tick", "src/free.hpp:reset")), "{widget:#?}");
    assert_eq!(widget.len(), 2, "{widget:#?}");
}

/// fmt's shape, measured on ccache's vendored copy (S-579 sample): a class
/// template specialization is no class node — its name is no plain identifier —
/// so its static member sits under the namespace as a method with no class
/// path. A bare call from another file never reaches a class member.
#[test]
fn a_member_of_a_class_extraction_never_captured_is_no_free_function_to_an_includer() {
    let tmp = tree(&[
        (
            "src/format.h",
            "template <typename T> struct formatter;\ntemplate <> struct formatter<int> {\n  static int format_as(int v) { return v; }\n};\n",
        ),
        ("src/std.h", "#include \"format.h\"\nint use() { return format_as(1); }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(from_file(engine.runtime().unwrap(), EdgeKind::Calls, "src/std.h"), []);
}

/// A bare call prefers a free function to a same-named method, as the file's
/// own scope walk did before includes were read (CR-068 Part B): nlohmann's
/// `json.hpp`, whose class a macro hides, holds its `empty()` members beside
/// free functions of the name.
#[test]
fn a_bare_call_prefers_the_free_function_to_a_same_named_floating_member() {
    let tmp = tree(&[(
        "src/json.hpp",
        "template <typename T> struct basic;\ntemplate <> struct basic<int> {\n  bool empty() const { return true; }\n};\nbool empty() { return false; }\nbool front() { return empty(); }\n",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/json.hpp"), [edge("src/json.hpp:front", "src/json.hpp:empty")]);
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("read runs");
    let target = edges.iter().find(|e| e.kind == EdgeKind::Calls).map(|e| e.target).expect("one call edge");
    let line = nodes.iter().find(|n| n.id == target).and_then(|n| n.start_line);
    assert_eq!(line, Some(5), "the free `empty`, never the member at line 3");
}

#[test]
fn a_definition_beside_its_own_header_prototype_binds_as_before() {
    let tmp = tree(&[
        ("src/hash.hpp", "int digest(int x);\n"),
        ("src/hash.cpp", "#include \"hash.hpp\"\nint digest(int x) { return x; }\nint twice(int x) { return digest(x) + digest(x); }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Calls, "src/hash.cpp"),
        [edge("src/hash.cpp:twice", "src/hash.cpp:digest")]
    );
}

/// What the file's own scope walk bound before includes were read, measured
/// on ccache's `config.cpp` and nlohmann's `binary_reader.hpp` (S-579): a
/// destructor's body is no captured declaration, so its calls are the file's —
/// and reach the class's out-of-line members as names of the file — and a
/// declaration a function nests is in scope in its body.
#[test]
fn a_destructor_body_and_a_declaration_a_function_nests_bind_as_before() {
    let tmp = tree(&[
        ("src/config.hpp", "class Config {\npublic:\n  void reset();\n  ~Config();\n};\n"),
        (
            "src/config.cpp",
            "#include \"config.hpp\"\nvoid Config::reset() {}\nConfig::~Config() { reset(); }\nint outer() {\n  int inner(int);\n  return inner(1);\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Calls, "src/config.cpp"),
        [edge("src/config.cpp:config", "src/config.cpp:reset"), edge("src/config.cpp:outer", "src/config.cpp:inner")]
    );
}

#[test]
fn using_declarations_and_directives_bring_a_name_in_and_two_namespaces_offering_it_bind_neither() {
    let tmp = tree(&[
        ("src/util/file.hpp", FILE_HPP),
        ("src/net/file.hpp", "namespace net {\nbool exists(const char* path);\n}\n"),
        ("src/decl.cpp", "#include \"util/file.hpp\"\nusing util::exists;\nbool a() { return exists(\"x\"); }\n"),
        ("src/dir.cpp", "#include \"util/file.hpp\"\nusing namespace util;\nbool b() { return exists(\"x\"); }\n"),
        (
            "src/both.cpp",
            "#include \"util/file.hpp\"\n#include \"net/file.hpp\"\nusing namespace util;\nusing namespace net;\nbool c() { return exists(\"x\"); }\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/decl.cpp"), [edge("src/decl.cpp:a", "src/util/file.hpp:exists")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/dir.cpp"), [edge("src/dir.cpp:b", "src/util/file.hpp:exists")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/both.cpp"), []);
    // The `using` declaration itself binds what it names; a directive names a
    // namespace, here declared once, so it binds that block.
    let imports = from_file(rt, EdgeKind::Imports, "src/decl.cpp");
    assert!(imports.contains(&edge("src/decl.cpp:decl", "src/util/file.hpp:exists")), "{imports:#?}");
    let imports = from_file(rt, EdgeKind::Imports, "src/dir.cpp");
    assert!(imports.contains(&edge("src/dir.cpp:dir", "src/util/file.hpp:util")), "{imports:#?}");
}

/// A rooted `::f()` names the global `f` alone (S-579 review): ccache's
/// `util::setenv` calls `::setenv`, which bound the caller itself.
#[test]
fn a_rooted_call_names_the_global_function_never_an_enclosing_namespaces() {
    let tmp = tree(&[
        ("src/a.hpp", "void f();\nnamespace ns {\nvoid f();\n}\n"),
        ("src/main.cpp", "#include \"a.hpp\"\nnamespace ns {\nvoid g() { ::f(); }\nvoid h() { f(); }\n}\n"),
        ("src/env.cpp", "namespace util {\nvoid setenv(const char* n) { ::setenv(n); }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let (nodes, edges) = rt.submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?))).expect("read runs");
    let line_of = |caller: &str| {
        let source = nodes.iter().find(|n| n.name == caller && n.file_path.as_deref() == Some("src/main.cpp")).expect("caller");
        let e = edges.iter().find(|e| e.kind == EdgeKind::Calls && e.source == source.id).expect("one call edge");
        nodes.iter().find(|n| n.id == e.target).and_then(|n| n.start_line)
    };
    assert_eq!(line_of("g"), Some(1), "`::f()` is the global `f`");
    assert_eq!(line_of("h"), Some(3), "a bare `f()` inside `ns` is `ns::f`");
    // The system `::setenv` is no repository function, and never the caller.
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/env.cpp"), []);
}

const A_HPP: &str = "namespace a {\nvoid f();\n}\n";
const B_HPP: &str = "namespace b {\nvoid f();\n}\n";

/// A `using` is in force where it is written and nowhere else, and every one
/// naming a name is read (S-579 review): two declarations of `f` bind neither,
/// one inside `g2` reaches only `g2`'s calls, a class's `using Base::f;` and a
/// namespace's `using a::f;` reach no free function outside them.
#[test]
fn a_using_is_read_where_it_is_written_and_two_naming_one_name_bind_neither() {
    let tmp = tree(&[
        ("src/a.hpp", A_HPP),
        ("src/b.hpp", B_HPP),
        ("src/base.hpp", "struct Base {\n  static void f();\n};\n"),
        ("src/two.cpp", "#include \"a.hpp\"\n#include \"b.hpp\"\nusing a::f;\nusing b::f;\nvoid g() { f(); }\n"),
        (
            "src/local.cpp",
            "#include \"a.hpp\"\n#include \"b.hpp\"\nvoid g1() { using a::f; f(); }\nvoid g2() { using b::f; f(); }\nvoid g3() { f(); }\n",
        ),
        ("src/member.cpp", "#include \"base.hpp\"\nstruct D : Base { using Base::f; };\nvoid g() { f(); }\n"),
        ("src/ns.cpp", "#include \"a.hpp\"\nnamespace x { using a::f; }\nvoid g() { f(); }\nnamespace x { void h() { f(); } }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/two.cpp"), []);
    assert_eq!(
        from_file(rt, EdgeKind::Calls, "src/local.cpp"),
        [edge("src/local.cpp:g1", "src/a.hpp:f"), edge("src/local.cpp:g2", "src/b.hpp:f")]
    );
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/member.cpp"), []);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/ns.cpp"), [edge("src/ns.cpp:h", "src/a.hpp:f")]);
}

/// A `using` declaration heads a qualified name (`using util::Args;
/// Args::rest()`), and a file's directive is read at the global level — after
/// the namespace the caller sits in (S-579 review).
#[test]
fn a_using_declaration_heads_a_path_and_a_directive_yields_to_the_enclosing_namespace() {
    let tmp = tree(&[
        ("src/util/args.hpp", "namespace util {\nclass Args {\npublic:\n  static int rest();\n};\n}\n"),
        ("src/util/file.hpp", "namespace util {\nvoid f();\n}\n"),
        ("src/core/a.hpp", "namespace core {\nvoid f();\n}\n"),
        ("src/main.cpp", "#include \"util/args.hpp\"\nusing util::Args;\nint main() { return Args::rest(); }\n"),
        ("src/core/c.cpp", "#include \"util/file.hpp\"\n#include \"core/a.hpp\"\nusing namespace util;\nnamespace core {\nvoid run() { f(); }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/main.cpp"), [edge("src/main.cpp:main", "src/util/args.hpp:rest")]);
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/core/c.cpp"), [edge("src/core/c.cpp:run", "src/core/a.hpp:f")]);
}

/// What a repository declares in `std` is an overload of a standard entity
/// (S-579 review, nlohmann's `std::swap` of `basic_json`): `std::swap(a, b)`
/// and a `using std::swap;` name the standard's overload set, never that one.
#[test]
fn a_declaration_a_repository_adds_to_std_is_never_the_standards_name() {
    let tmp = tree(&[
        ("src/json.hpp", "class json {};\nnamespace std {\ninline void swap(json& a, json& b) {}\n}\n"),
        (
            "src/use.cpp",
            "#include \"json.hpp\"\nvoid f(int& a, int& b) { std::swap(a, b); }\nusing std::swap;\nvoid g(int& a, int& b) { swap(a, b); }\nusing namespace std;\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(from_file(rt, EdgeKind::Calls, "src/use.cpp"), []);
    assert_eq!(
        from_file(rt, EdgeKind::Imports, "src/use.cpp"),
        [edge("src/use.cpp:use", "src/json.hpp:json")],
        "the include binds; `using std::swap;` and `using namespace std;` bind nothing"
    );
}

/// Two headers that include each other — `#pragma once` makes it legal —
/// end the include walk, and what either declares is still reached through
/// the other (S-579 review).
#[test]
fn an_include_cycle_ends_and_still_reaches_through_it() {
    let tmp = tree(&[
        ("src/a.hpp", "#pragma once\n#include \"b.hpp\"\nnamespace core {\nvoid fa();\n}\n"),
        ("src/b.hpp", "#pragma once\n#include \"a.hpp\"\nnamespace core {\nvoid fb();\n}\n"),
        ("src/c.cpp", "#include \"a.hpp\"\nnamespace core {\nvoid run() { fb(); }\n}\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(from_file(engine.runtime().unwrap(), EdgeKind::Calls, "src/c.cpp"), [edge("src/c.cpp:run", "src/b.hpp:fb")]);
}

/// An out-of-line member of a nested class (`int Outer::Inner::g()`) reads
/// every class of its path, the outer one included (S-579 review).
#[test]
fn a_nested_out_of_line_member_reads_every_class_of_its_path() {
    let tmp = tree(&[
        (
            "src/outer.hpp",
            "class Outer {\npublic:\n  static int base();\n  class Inner {\n  public:\n    int g();\n  };\n};\n",
        ),
        ("src/outer.cpp", "#include \"outer.hpp\"\nint Outer::Inner::g() { return base(); }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Calls, "src/outer.cpp"),
        [edge("src/outer.cpp:g", "src/outer.hpp:base")]
    );
}

/// A base clause error recovery leaves outside its class names no base of the
/// file (S-579 review): the row is the file's, and a file extends nothing.
#[test]
fn a_base_clause_error_recovery_leaves_outside_a_class_records_nothing() {
    let tmp = tree(&[("src/b.hpp", "class Base {};\nclass B : public Base\n")]);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let _ = engine.index();
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Extends), []);
}

// ── Base classes ─────────────────────────────────────────────────────────────

const SERIALIZER_HPP: &str = "\
namespace core {
class Serializer {
public:
  virtual void serialize() = 0;
  void flush() {}
};
template <typename T> class Box {};
struct Plain {};
class Root {};
}
class Root {};
";

const MANIFEST_HPP: &str = "\
#pragma once
#include <ccache/core/serializer.hpp>
#include <stdexcept>

namespace core {
class Manifest : public Serializer {
public:
  void serialize() override { this->flush(); }
};
class Holder : public core::Box<int>, private Plain {};
class Failure : public std::runtime_error {};
class Rooted : public ::Root {};
}
";

#[test]
fn a_base_class_records_extends_through_the_include_and_an_external_base_none() {
    let tmp = tree(&[
        ("src/ccache/core/serializer.hpp", SERIALIZER_HPP),
        ("src/ccache/core/manifest.hpp", MANIFEST_HPP),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let header = "src/ccache/core/manifest.hpp";
    let base = "src/ccache/core/serializer.hpp";
    assert_eq!(
        from_file(rt, EdgeKind::Extends, header),
        [
            edge(&format!("{header}:Holder"), &format!("{base}:Box")),
            edge(&format!("{header}:Holder"), &format!("{base}:Plain")),
            edge(&format!("{header}:Manifest"), &format!("{base}:Serializer")),
            // `::Root` is the global `Root`, never `core::Root` (line 9).
            edge(&format!("{header}:Rooted"), &format!("{base}:Root")),
        ]
    );
    let (nodes, edges) = rt.submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?))).expect("read runs");
    let rooted = nodes.iter().find(|n| n.name == "Rooted").expect("Rooted");
    let target = edges.iter().find(|e| e.kind == EdgeKind::Extends && e.source == rooted.id).map(|e| e.target);
    assert_eq!(nodes.iter().find(|n| Some(n.id) == target).and_then(|n| n.start_line), Some(11));
    // The hierarchy now holds the base, so a `this->` call reaches its member.
    assert_eq!(
        from_file(rt, EdgeKind::Calls, header),
        [edge(&format!("{header}:serialize"), &format!("{base}:flush"))]
    );
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

#[test]
fn a_declaration_two_includes_away_binds_on_sync_as_on_reindex() {
    let initial = [
        ("src/core/a.hpp", "namespace core {\n}\n"),
        ("src/core/b.hpp", "#include \"a.hpp\"\n"),
        ("src/core/c.cpp", "#include \"b.hpp\"\nnamespace core {\nvoid run() { helper(); }\n}\n"),
    ];
    // Only the header two includes away changes; the call's file is untouched.
    let added = synced_equals_reindexed(&initial, &[("src/core/a.hpp", "namespace core {\nvoid helper();\n}\n")]);
    assert_eq!(added, [edge("src/core/c.cpp:run", "src/core/a.hpp:helper")]);
    // And removing it again unbinds the call.
    let bound = [("src/core/a.hpp", "namespace core {\nvoid helper();\n}\n"), initial[1], initial[2]];
    assert_eq!(synced_equals_reindexed(&bound, &[("src/core/a.hpp", "namespace core {\n}\n")]), []);
}

/// The case only the include scope's re-selection decides: a bare call, and a
/// header that gains an include of a file which already declares the name —
/// the sync dirties no token the call spells, and the call's file is untouched.
#[test]
fn a_bare_call_binds_when_a_header_between_gains_an_include_on_sync_as_on_reindex() {
    let initial = [
        ("src/core/a.hpp", "namespace core {\nvoid helper();\n}\n"),
        ("src/core/b.hpp", "// nothing yet\n"),
        ("src/core/c.cpp", "#include \"b.hpp\"\nnamespace core {\nvoid run() { helper(); }\n}\n"),
    ];
    let added = synced_equals_reindexed(&initial, &[("src/core/b.hpp", "#include \"a.hpp\"\n")]);
    assert_eq!(added, [edge("src/core/c.cpp:run", "src/core/a.hpp:helper")]);
}

const SER_HPP: &str = "namespace core {\nclass Serializer {\npublic:\n  void flush() {}\n};\n}\n";
const MAN_HPP: &str = "#include \"mid.hpp\"\nnamespace core {\nclass Manifest : public Serializer {\npublic:\n  void run() { this->flush(); }\n  void other() { flush(); }\n};\n}\n";

/// A base a header between makes visible moves the class's `Extends` and the
/// `this->` and in-class calls that walk it (S-579 review): the sync dirties
/// no token either spells, and neither row's file changed.
#[test]
fn a_base_a_header_between_brings_in_or_takes_away_moves_this_calls_on_sync_as_on_reindex() {
    let without = [("src/core/serializer.hpp", SER_HPP), ("src/core/mid.hpp", "// nothing yet\n"), ("src/core/manifest.hpp", MAN_HPP)];
    let with = [without[0], ("src/core/mid.hpp", "#include \"serializer.hpp\"\n"), without[2]];
    let header = "src/core/manifest.hpp";
    let base = "src/core/serializer.hpp";
    let bound = [edge(&format!("{header}:other"), &format!("{base}:flush")), edge(&format!("{header}:run"), &format!("{base}:flush"))];
    assert_eq!(synced_equals_reindexed(&without, &[with[1]]), bound);
    assert_eq!(synced_equals_reindexed(&with, &[without[1]]), []);
}

#[test]
fn a_base_a_header_between_brings_in_records_extends_on_sync_as_on_reindex() {
    let initial = [
        ("src/core/base.hpp", "namespace core {\nclass Base {};\n}\n"),
        ("src/core/mid.hpp", "// nothing yet\n"),
        ("src/core/derived.hpp", "#include \"mid.hpp\"\nnamespace core {\nclass Derived : public Base {};\n}\n"),
    ];
    let tmp = tree(&initial);
    let engine = index(tmp.path());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    write(tmp.path(), "src/core/mid.hpp", "#include \"base.hpp\"\n");
    engine.sync(&[root.join("src/core/mid.hpp")]);
    assert_eq!(
        from_file(engine.runtime().unwrap(), EdgeKind::Extends, "src/core/derived.hpp"),
        [edge("src/core/derived.hpp:Derived", "src/core/base.hpp:Base")]
    );
}

#[test]
fn an_include_another_file_gains_moves_its_includers_on_sync_as_on_reindex() {
    let initial = [
        ("src/util/file.hpp", FILE_HPP),
        ("src/mid.hpp", "// nothing yet\n"),
        ("src/main.cpp", "#include \"mid.hpp\"\nint main() { return util::exists(\"x\"); }\n"),
    ];
    let added = synced_equals_reindexed(&initial, &[("src/mid.hpp", "#include \"util/file.hpp\"\n")]);
    assert_eq!(added, [edge("src/main.cpp:main", "src/util/file.hpp:exists")]);
}
