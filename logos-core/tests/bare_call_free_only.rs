//! A bare call never binds an instance member in a language where it cannot
//! reach one (S-590, CR-189, FR-RS-07, FR-RS-12, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against each language's SHIPPED
//! `references.scm` and `plugin.toml`, never a query override.
//!
//! Go, Rust, Python, PHP, JavaScript and TypeScript declare
//! `implicit_receiver = "none"` explicitly: a method is reached only through a
//! receiver (`self.`, `$this->`, `this.`, `s.`), so a single-segment bare call
//! `f()` binds free callables only. Every fixture in those languages is the same
//! two-case shape:
//!
//! - a bare `f()` inside a method `f`, with no free `f` in scope, binds nothing
//!   and makes no self-loop — the Sprint 87 review's R1 shape (a PHP `fwrite`
//!   inside a method `fwrite`, a Go `performWebSearch` beside a method of that
//!   name);
//! - with a free `f` beside the method, the bare call binds the free one.
//!
//! A nested function still binds. The languages whose bare call does reach the
//! instance — C#, Kotlin, Scala, C++ and Ruby declare `"self"`, Java declares
//! nothing — are pinned to the edges they bound before the rule existed.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

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
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    let label = labels(rt);
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// Index `files` and return the bound `Calls` edges.
fn edges_of(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let tmp = tree(files);
    let engine = index(tmp.path());
    call_edges(engine.runtime().unwrap())
}

/// `(from, to)` pairs as owned, sorted edges.
fn expect(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = pairs
        .iter()
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect();
    out.sort();
    out
}

fn self_loops(edges: &[(String, String)]) -> Vec<&(String, String)> {
    edges.iter().filter(|(from, to)| from == to).collect()
}

// ── The `"none"` languages: a method `f` calling a bare `f()` ────────────────

#[cfg(feature = "lang-python")]
#[test]
fn python_a_bare_call_inside_a_same_named_method_binds_nothing() {
    let edges = edges_of(&[(
        "app.py",
        "\
from copy import deepcopy


class A:
    def f(self):
        return f()

    def deepcopy(self, x):
        return deepcopy(x)

    def outer(self):
        def g():
            return 1
        return g()
",
    )]);
    assert!(self_loops(&edges).is_empty(), "self-loop Calls edges: {edges:?}");
    // The nested function still binds; neither bare call reaches a method.
    assert_eq!(edges, expect(&[("app.py:outer@11", "app.py:g@12")]));
}

#[cfg(feature = "lang-python")]
#[test]
fn python_a_bare_call_binds_the_free_function_beside_a_same_named_method() {
    let edges = edges_of(&[(
        "app.py",
        "\
def f():
    return 0


class A:
    def f(self):
        return f()
",
    )]);
    assert_eq!(edges, expect(&[("app.py:f@6", "app.py:f@1")]));
}

#[cfg(feature = "lang-php")]
#[test]
fn php_a_bare_call_inside_a_same_named_method_binds_nothing() {
    let edges = edges_of(&[(
        "src/Sock.php",
        "\
<?php
class Sock {
    function fwrite($h) {
        return fwrite($h);
    }
}
",
    )]);
    assert!(edges.is_empty(), "the builtin is external, never the method: {edges:?}");
}

#[cfg(feature = "lang-php")]
#[test]
fn php_a_bare_call_binds_the_free_function_beside_a_same_named_method() {
    let edges = edges_of(&[(
        "src/Sock.php",
        "\
<?php
function f() {
    return 0;
}
class A {
    function f() {
        return f();
    }
}
",
    )]);
    assert_eq!(edges, expect(&[("src/Sock.php:f@6", "src/Sock.php:f@2")]));
}

/// A TS-family class whose methods `f` and `componentDidMount` call themselves
/// bare, and whose method `m` calls a function it nests.
const TS_SELF_NAMED: &str = "\
class A {
  f() {
    return f();
  }

  componentDidMount() {
    componentDidMount();
  }

  m() {
    function g() {
      return 1;
    }
    return g();
  }
}
";

/// A free `f` beside a class whose method `f` calls `f()` bare.
const TS_FREE: &str = "\
function f() {
  return 0;
}

class A {
  f() {
    return f();
  }
}
";

#[cfg(feature = "lang-typescript")]
fn assert_ts_family(ext: &str) {
    let file = format!("src/a.{ext}");
    let edges = edges_of(&[(&file, TS_SELF_NAMED)]);
    assert!(self_loops(&edges).is_empty(), "{ext}: self-loop Calls edges: {edges:?}");
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(edges, expect(&[(&at(10, "m"), &at(11, "g"))]), "{ext}");

    let edges = edges_of(&[(&file, TS_FREE)]);
    assert_eq!(edges, expect(&[(&at(6, "f"), &at(1, "f"))]), "{ext}");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_a_bare_call_reaches_only_a_free_function() {
    assert_ts_family("ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn javascript_a_bare_call_reaches_only_a_free_function() {
    assert_ts_family("js");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn tsx_a_bare_call_reaches_only_a_free_function() {
    assert_ts_family("tsx");
}

#[cfg(feature = "lang-go")]
#[test]
fn go_a_bare_call_inside_a_same_named_method_binds_nothing() {
    // The ollama shapes: a package function's name on a method, and a
    // conversion `DType(x)` whose type shares its name with a method.
    let edges = edges_of(&[(
        "web/search.go",
        "\
package web

type DType int

type Array struct{}

type BrowserWebSearch struct{}

func (b *BrowserWebSearch) performWebSearch() int {
	return performWebSearch()
}

func (a *Array) DType() DType {
	return DType(0)
}
",
    )]);
    assert!(edges.is_empty(), "no bare call reaches a method: {edges:?}");
}

#[cfg(feature = "lang-go")]
#[test]
fn go_a_bare_call_binds_the_package_function_beside_a_same_named_method() {
    let edges = edges_of(&[
        (
            "go.mod",
            "module example.com/app\n\ngo 1.22\n",
        ),
        (
            "web/search.go",
            "\
package web

type BrowserWebSearch struct{}

func (b *BrowserWebSearch) performWebSearch() int {
	return performWebSearch()
}

func performWebSearch() int {
	return 0
}
",
        ),
    ]);
    assert_eq!(
        edges,
        expect(&[("web/search.go:performWebSearch@5", "web/search.go:performWebSearch@9")])
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_a_bare_call_inside_a_same_named_method_binds_nothing() {
    let edges = edges_of(&[(
        "src/lib.rs",
        "\
pub struct T;

impl T {
    pub fn f(&self) -> i32 {
        f()
    }

    pub fn n(&self) -> i32 {
        fn g() -> i32 {
            1
        }
        g()
    }
}
",
    )]);
    assert!(self_loops(&edges).is_empty(), "self-loop Calls edges: {edges:?}");
    // The function `n` nests still binds.
    assert_eq!(edges, expect(&[("src/lib.rs:n@8", "src/lib.rs:g@9")]));
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_a_bare_call_binds_the_free_function_beside_a_same_named_method() {
    let edges = edges_of(&[(
        "src/lib.rs",
        "\
pub fn f() -> i32 {
    0
}

pub struct T;

impl T {
    pub fn f(&self) -> i32 {
        f()
    }
}
",
    )]);
    assert_eq!(edges, expect(&[("src/lib.rs:f@8", "src/lib.rs:f@1")]));
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_a_bare_call_through_a_glob_import_never_binds_an_associated_function() {
    // `use super::*` brings the parent module's items into scope, never an
    // `impl`'s associated functions: `new()` names no free function.
    let edges = edges_of(&[(
        "src/lib.rs",
        "\
pub struct Store;

impl Store {
    pub fn new() -> Store {
        Store
    }
}

pub fn open() -> Store {
    Store::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Store {
        new()
    }

    fn reopen() -> Store {
        open()
    }
}
",
    )]);
    assert_eq!(
        edges,
        expect(&[
            ("src/lib.rs:open@9", "src/lib.rs:new@4"),
            ("src/lib.rs:reopen@21", "src/lib.rs:open@9"),
        ])
    );
}

// ── The `"none"` languages: a method `f` calling an imported `f` ─────────────
//
// The scope walk passes over the method and reaches the file's import of the
// free `f` in another file. Go has no bare imported function (a dot import
// aside), and a PHP `use function` import binds nothing yet from any caller,
// so neither has a case here.

#[cfg(feature = "lang-python")]
#[test]
fn python_a_bare_call_inside_a_same_named_method_binds_the_imported_function() {
    let edges = edges_of(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", "def f():\n    return 0\n"),
        (
            "pkg/app.py",
            "\
from pkg.util import f


class A:
    def f(self):
        return f()
",
        ),
    ]);
    assert_eq!(edges, expect(&[("pkg/app.py:f@5", "pkg/util.py:f@1")]));
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_a_bare_call_inside_a_same_named_method_binds_the_imported_function() {
    let edges = edges_of(&[
        ("src/lib.rs", "pub mod util;\npub mod t;\n"),
        ("src/util.rs", "pub fn f() -> i32 {\n    0\n}\n"),
        (
            "src/t.rs",
            "\
use crate::util::f;

pub struct T;

impl T {
    pub fn f(&self) -> i32 {
        f()
    }
}
",
        ),
    ]);
    assert_eq!(edges, expect(&[("src/t.rs:f@6", "src/util.rs:f@1")]));
}

/// A TS-family file importing `f`, whose class declares a method `f` that
/// calls it, beside a free caller of it. The method never shadows the import.
const TS_IMPORTED: &str = "\
import { f } from './util';

function g() {
  return f();
}

class A {
  f() {
    return f();
  }
}
";

#[cfg(feature = "lang-typescript")]
fn assert_ts_family_imported(ext: &str) {
    let file = format!("src/a.{ext}");
    let util = format!("src/util.{ext}");
    let edges = edges_of(&[(&util, "export function f() {\n  return 0;\n}\n"), (&file, TS_IMPORTED)]);
    let free = format!("{util}:f@1");
    assert_eq!(
        edges,
        expect(&[(&format!("{file}:f@8"), &free), (&format!("{file}:g@3"), &free)]),
        "{ext}"
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_a_bare_call_inside_a_same_named_method_binds_the_imported_function() {
    assert_ts_family_imported("ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn javascript_a_bare_call_inside_a_same_named_method_binds_the_imported_function() {
    assert_ts_family_imported("js");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn tsx_a_bare_call_inside_a_same_named_method_binds_the_imported_function() {
    assert_ts_family_imported("tsx");
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical, and the synced one's
/// edges are returned.
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
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: std::collections::BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    call_edges(rt_a)
}

#[cfg(feature = "lang-rust")]
#[test]
fn moving_the_free_function_into_an_impl_unbinds_the_bare_call_on_sync() {
    const CALLER: &str = "\
pub fn run() -> i32 {
    helper()
}
";
    const FREE: &str = "\
pub fn helper() -> i32 {
    0
}
";
    const MEMBER: &str = "\
pub struct H;

impl H {
    pub fn helper() -> i32 {
        0
    }
}
";
    // The caller's file never changes: its `use` of `helper` binds the free
    // function, then — once `helper` is an associated function — nothing.
    let initial = [
        ("src/lib.rs", "pub mod util;\npub mod run;\n"),
        ("src/util.rs", FREE),
        ("src/run.rs", &format!("use crate::util::helper;\n{CALLER}") as &str),
    ];
    let edges = synced_equals_reindexed(&initial, &[("src/util.rs", MEMBER)]);
    assert!(
        !edges.iter().any(|(_, to)| to.contains(":helper@")),
        "an associated function is never a bare call's target: {edges:?}"
    );

    // And back: the free function returns, the bare call binds it again.
    let moved = [
        ("src/lib.rs", "pub mod util;\npub mod run;\n"),
        ("src/util.rs", MEMBER),
        ("src/run.rs", &format!("use crate::util::helper;\n{CALLER}") as &str),
    ];
    let edges = synced_equals_reindexed(&moved, &[("src/util.rs", FREE)]);
    assert!(
        edges.iter().any(|(from, to)| from.ends_with(":run@2") && to == "src/util.rs:helper@1"),
        "the free function binds again: {edges:?}"
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn adding_a_free_function_binds_the_bare_call_on_sync() {
    const METHOD_ONLY: &str = "\
class A:
    def f(self):
        return f()
";
    const WITH_FREE: &str = "\
def f():
    return 0


class A:
    def f(self):
        return f()
";
    let edges = synced_equals_reindexed(&[("app.py", WITH_FREE)], &[("app.py", METHOD_ONLY)]);
    assert!(edges.is_empty(), "{edges:?}");
    let edges = synced_equals_reindexed(&[("app.py", METHOD_ONLY)], &[("app.py", WITH_FREE)]);
    assert_eq!(edges, expect(&[("app.py:f@6", "app.py:f@1")]));
}

// ── The languages that reach the instance: unchanged ─────────────────────────
//
// Each pin is the edge set the binder produced before S-590 for the same
// bare-call shapes; a rule that leaked past the explicit `"none"` declaration
// would drop a member edge here.

#[cfg(feature = "lang-java")]
#[test]
fn java_bare_in_class_inherited_and_outer_class_calls_are_unchanged() {
    let edges = edges_of(&[(
        "src/main/java/app/A.java",
        "\
package app;

class Base {
    void inherited() {}
}

class A extends Base {
    void f() {
        g();
        inherited();
    }

    void g() {}

    class Inner {
        void h() {
            g();
        }
    }
}
",
    )]);
    assert_eq!(edges, expect(&[
            ("src/main/java/app/A.java:f@8", "src/main/java/app/A.java:g@13"),
            ("src/main/java/app/A.java:f@8", "src/main/java/app/A.java:inherited@4"),
            ("src/main/java/app/A.java:h@16", "src/main/java/app/A.java:g@13"),
        ]), "{edges:?}");
}

#[cfg(feature = "lang-c-sharp")]
#[test]
fn csharp_bare_in_class_calls_are_unchanged() {
    let edges = edges_of(&[(
        "src/A.cs",
        "\
namespace App {
    class A {
        void F() {
            G();
        }

        void G() {}
    }
}
",
    )]);
    assert_eq!(edges, expect(&[("src/A.cs:F@3", "src/A.cs:G@7")]), "{edges:?}");
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_bare_in_class_and_top_level_calls_are_unchanged() {
    let edges = edges_of(&[(
        "src/main/kotlin/app/A.kt",
        "\
package app

class A {
    fun f() {
        g()
    }

    fun g() {}
}

fun top() {
    helper()
}

fun helper() {}
",
    )]);
    assert_eq!(edges, expect(&[
            ("src/main/kotlin/app/A.kt:f@4", "src/main/kotlin/app/A.kt:g@8"),
            ("src/main/kotlin/app/A.kt:top@11", "src/main/kotlin/app/A.kt:helper@15"),
        ]), "{edges:?}");
}

#[cfg(feature = "lang-scala")]
#[test]
fn scala_bare_in_class_calls_are_unchanged() {
    let edges = edges_of(&[(
        "src/main/scala/app/A.scala",
        "\
package app

class A {
  def f(): Unit = {
    g()
  }

  def g(): Unit = {}
}
",
    )]);
    assert_eq!(edges, expect(&[("src/main/scala/app/A.scala:f@4", "src/main/scala/app/A.scala:g@8")]), "{edges:?}");
}

#[cfg(feature = "lang-cpp")]
#[test]
fn cpp_bare_in_class_calls_are_unchanged() {
    let edges = edges_of(&[(
        "src/a.cpp",
        "\
class A {
public:
    void f() {
        g();
    }

    void g() {}
};
",
    )]);
    assert_eq!(edges, expect(&[("src/a.cpp:f@3", "src/a.cpp:g@7")]), "{edges:?}");
}

#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_module_function_and_bare_in_class_calls_are_unchanged() {
    let edges = edges_of(&[(
        "lib/a.rb",
        "\
module M
  def self.helper
    1
  end

  def self.run
    helper()
  end
end

class A
  def f
    g()
  end

  def g
  end
end
",
    )]);
    assert_eq!(edges, expect(&[
            ("lib/a.rb:f@12", "lib/a.rb:g@16"),
            ("lib/a.rb:run@6", "lib/a.rb:helper@2"),
        ]), "{edges:?}");
}

