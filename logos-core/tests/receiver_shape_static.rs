//! C#, Kotlin, Scala and C++ record their receivers' shapes (S-516, CR-169,
//! FR-EX-13, FR-RS-12, NFR-RA-05) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with each language's
//! SHIPPED `references.scm` and `plugin.toml` (no query override).
//!
//! Each language is checked on the same four cases over methods `A.m` / `B.m`:
//! its self form inside `A.n`, and the bare `m()` inside `A.j` that the plugin
//! declares a call on the current instance (`implicit_receiver = "self"`),
//! record `self` and bind `A.m`; `other.m()` inside `A.m` records `other` and stays unbound
//! (`no-receiver-evidence`, a fixed function of the shape pinned by the binder
//! unit tests); the super form stays unbound, because no plugin of these four
//! records a proven `Extends`; and no `Calls` edge is a self-loop. A bare call
//! outside any type is a free call again, bound by the lexical scope, and a
//! bare call inside a type without `m` never reaches a same-named top-level
//! function (Kotlin, Scala and C++ declare one; C# has no top-level functions).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`receiver_shape.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(all(
    feature = "lang-c-sharp",
    feature = "lang-kotlin",
    feature = "lang-scala",
    feature = "lang-cpp"
))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
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

/// One `Calls` ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` ledger rows sourced in `file`, sorted.
fn call_rows(rt: &Runtime, file: &str) -> Vec<Row> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let prefix = format!("{file}:");
    let rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

fn sorted_edges(mut edges: Vec<(String, String)>) -> Vec<(String, String)> {
    edges.sort();
    edges
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

fn row(source: &str, target: &str, form: RefForm, receiver: Option<ReceiverShape>, resolved: bool) -> Row {
    (source.to_string(), target.to_string(), form, receiver, resolved)
}

/// No bound `Calls` edge starts and ends at the same node.
fn assert_no_self_loop(edges: &[(String, String)]) {
    let loops: Vec<_> = edges.iter().filter(|(from, to)| from == to).collect();
    assert!(loops.is_empty(), "self-loop Calls edges: {loops:?}");
}

const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);
const SUPER: Option<ReceiverShape> = Some(ReceiverShape::Super);
const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

// ── C# ────────────────────────────────────────────────────────────────────────

const CS_FILE: &str = "src/A.cs";

/// `Base.M` is declared in the file, so a `base.M()` that bound anything at
/// all could reach it — it must not, without a proven `Extends`.
const CS: &str = "\
class Base {
    public int M() { return 1; }
}

class A : Base {
    public int M() { return other.M(); }
    public int N() { return this.M(); }
    public int J() { return M(); }
    public int K() { return base.M(); }
    public int L() { return Helpers.Run(); }
    public int P() { return this.f.M(); }
}

class B {
    public int M() { return 2; }
}

class C {
    public int N() { return M(); }
}
";

#[test]
fn csharp_this_and_bare_calls_bind_the_callers_own_method_and_no_other_receiver_does() {
    let tmp = tree(&[(CS_FILE, CS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{CS_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, CS_FILE),
        sorted(vec![
            // `other.M()` inside `A.M`: no self-loop.
            row(&a(6, "M"), "M", RefForm::Method, OTHER, false),
            // `this.M()` and the bare `M()`: `A.M`.
            row(&a(7, "N"), "M", RefForm::Method, SELF, true),
            row(&a(8, "J"), "M", RefForm::Method, SELF, true),
            // `base.M()`: no proven base, so not even the in-file `Base.M`.
            row(&a(9, "K"), "M", RefForm::Method, SUPER, false),
            // A static call on a type name is a receiver like any other.
            row(&a(10, "L"), "Run", RefForm::Method, OTHER, false),
            // A chain rooted at `this` is a receiver expression, not `this`.
            row(&a(11, "P"), "M", RefForm::Method, OTHER, false),
            // A bare `M()` in `C`, which has no `M`: never `A.M`, `B.M`, `Base.M`.
            row(&a(19, "N"), "M", RefForm::Method, SELF, false),
        ])
    );
    let edges = call_edges(rt);
    assert_eq!(
        edges,
        sorted_edges(vec![edge(&a(7, "N"), &a(6, "M")), edge(&a(8, "J"), &a(6, "M"))])
    );
    assert_no_self_loop(&edges);
}

// ── Kotlin ───────────────────────────────────────────────────────────────────

const KT_FILE: &str = "src/A.kt";

/// A top-level `m` the class methods are named like, a `Base.m` the super
/// call must not reach, and two top-level functions whose bare call is free.
const KT: &str = "\
fun m(): Int = 0

open class Base {
    open fun m(): Int = 1
}

class A : Base() {
    override fun m(): Int = other.m()
    fun n(): Int = this.m()
    fun j(): Int = m()
    fun k(): Int = super.m()
    fun l(): Int = this@A.m()
}

class B {
    fun m(): Int = 2
}

class C {
    fun n(): Int = m()
}

fun helper(): Int = 0

fun free(): Int = helper()
";

#[test]
fn kotlin_this_and_bare_calls_bind_the_callers_own_method_and_no_other_receiver_does() {
    let tmp = tree(&[(KT_FILE, KT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{KT_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, KT_FILE),
        sorted(vec![
            // `other.m()` inside `A.m`: the receiver `other` is no call (KOTLIN-G4).
            row(&a(8, "m"), "m", RefForm::Method, OTHER, false),
            // `this.m()` and the bare `m()`: `A.m`, never the top-level `m`.
            row(&a(9, "n"), "m", RefForm::Method, SELF, true),
            row(&a(10, "j"), "m", RefForm::Method, SELF, true),
            // `super.m()`: no proven base, so not even the in-file `Base.m`.
            row(&a(11, "k"), "m", RefForm::Method, SUPER, false),
            // A labelled `this@A` names an instance by label, not by position.
            row(&a(12, "l"), "m", RefForm::Method, OTHER, false),
            // A bare `m()` in `C`, which has no `m`: never the top-level `m`.
            row(&a(20, "n"), "m", RefForm::Method, SELF, false),
            // A bare call outside a class is a free call, bound by the scope.
            row(&a(25, "free"), "helper", RefForm::Path, None, true),
        ])
    );
    let edges = call_edges(rt);
    assert_eq!(
        edges,
        sorted_edges(vec![
            edge(&a(9, "n"), &a(8, "m")),
            edge(&a(10, "j"), &a(8, "m")),
            edge(&a(25, "free"), &a(23, "helper")),
        ])
    );
    assert_no_self_loop(&edges);
}

/// KOTLIN-G4: `navigation_expression` has no fields and a receiver can be a
/// bare `identifier`, so only the navigation's last child is a call target.
#[test]
fn kotlin_records_only_the_last_segment_of_a_navigation_as_a_call() {
    let file = "src/Chain.kt";
    let src = "\
class Chain {
    fun a(): Int = 0
    fun b(): Int = 0
    fun run(): Int = a.b.c()
    fun rooms(): Int = rooms.map()
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        call_rows(rt, file),
        sorted(vec![
            row(&at(4, "run"), "c", RefForm::Method, OTHER, false),
            row(&at(5, "rooms"), "map", RefForm::Method, OTHER, false),
        ]),
        "`a`, `b` and the receiver `rooms` are never call targets"
    );
    assert_eq!(call_edges(rt), Vec::<(String, String)>::new());
}

/// An `object : T { … }` body's instance has no node: a `this.` call inside it
/// must not reach the enclosing class's same-named method.
#[test]
fn kotlin_a_this_call_inside_an_object_expression_never_binds_the_enclosing_class() {
    let file = "src/Anon.kt";
    let src = "\
class Outer {
    fun m(): Int = 0
    fun make(): Runnable = object : Runnable {
        override fun run() {
            this.m()
        }
    }
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows(rt, file),
        vec![row(&format!("{file}:run@4"), "m", RefForm::Method, OTHER, false)]
    );
    assert_eq!(call_edges(rt), Vec::<(String, String)>::new());
}

/// A member extension function's `this` is its extension receiver, not the
/// enclosing class's instance: `this.m()` inside `fun G.ext()` declared in `W`
/// must not bind `W.m`. An ordinary member with a declared return type keeps
/// the `self` shape.
#[test]
fn kotlin_a_this_call_inside_an_extension_function_never_binds_the_enclosing_class() {
    let file = "src/Ext.kt";
    let src = "\
class G {
    fun m() {}
}

class W {
    fun m() {}
    fun G.ext() { this.m() }
    fun G?.ext2() { this.m() }
    fun own(): G { this.m(); return G() }
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    let rows = call_rows(rt, file);
    assert!(
        rows.contains(&row(&at(7, "ext"), "m", RefForm::Method, OTHER, false)),
        "{rows:?}"
    );
    assert!(
        rows.contains(&row(&at(8, "ext2"), "m", RefForm::Method, OTHER, false)),
        "{rows:?}"
    );
    assert!(
        rows.contains(&row(&at(9, "own"), "m", RefForm::Method, SELF, true)),
        "{rows:?}"
    );
    assert_eq!(call_edges(rt), vec![edge(&at(9, "own"), &at(6, "m"))]);
}

// ── Scala ────────────────────────────────────────────────────────────────────

const SC_FILE: &str = "src/A.scala";

const SC: &str = "\
class Base {
  def m(): Int = 1
}

class A extends Base {
  override def m(): Int = other.m()
  def n(): Int = this.m()
  def j(): Int = m()
  def k(): Int = super.m()
  def q(): Int = A.this.m()
}

class B {
  def m(): Int = 2
}

object C {
  def n(): Int = m()
}

def helper(): Int = 0

def free(): Int = helper()

def m(): Int = 0
";

#[test]
fn scala_this_and_bare_calls_bind_the_callers_own_method_and_no_other_receiver_does() {
    let tmp = tree(&[(SC_FILE, SC)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{SC_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, SC_FILE),
        sorted(vec![
            // `other.m()` inside `A.m`: no self-loop.
            row(&a(6, "m"), "m", RefForm::Method, OTHER, false),
            // `this.m()` and the bare `m()`: `A.m`.
            row(&a(7, "n"), "m", RefForm::Method, SELF, true),
            row(&a(8, "j"), "m", RefForm::Method, SELF, true),
            // `super.m()`: no proven base, so not even the in-file `Base.m`.
            row(&a(9, "k"), "m", RefForm::Method, SUPER, false),
            // A qualified `A.this` is a receiver expression, not `this`.
            row(&a(10, "q"), "m", RefForm::Method, OTHER, false),
            // A bare `m()` in `object C`, which has no `m`: never the top-level `m`.
            row(&a(18, "n"), "m", RefForm::Method, SELF, false),
            // A bare call in a top-level `def` is a free call.
            row(&a(23, "free"), "helper", RefForm::Path, None, true),
        ])
    );
    let edges = call_edges(rt);
    assert_eq!(
        edges,
        sorted_edges(vec![
            edge(&a(7, "n"), &a(6, "m")),
            edge(&a(8, "j"), &a(6, "m")),
            edge(&a(23, "free"), &a(21, "helper")),
        ])
    );
    assert_no_self_loop(&edges);
}

/// A `new T { … }` body's instance has no node: a `this.` call inside it must
/// not reach the enclosing class's same-named method.
#[test]
fn scala_a_this_call_inside_an_anonymous_class_never_binds_the_enclosing_class() {
    let file = "src/Anon.scala";
    let src = "\
class Outer {
  def m(): Int = 0
  def make(): Runnable = new Runnable {
    def run(): Unit = this.m()
  }
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows(rt, file),
        vec![row(&format!("{file}:run@4"), "m", RefForm::Method, OTHER, false)]
    );
    assert_eq!(call_edges(rt), Vec::<(String, String)>::new());
}

/// An auxiliary constructor's `this(…)` delegates to the primary constructor,
/// which has no node: it is no call, so it never binds the auxiliary
/// constructor (a `def this`) to itself.
#[test]
fn scala_an_auxiliary_constructor_delegation_is_no_call() {
    let file = "src/Ctor.scala";
    let src = "\
class J(a: Int, b: Int) {
  def this(a: Int) = this(a, 0)
  def m(): Int = 0
  def n(): Int = m()
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows(rt, file),
        vec![row(&format!("{file}:n@4"), "m", RefForm::Method, SELF, true)]
    );
    assert_eq!(call_edges(rt), vec![edge(&format!("{file}:n@4"), &format!("{file}:m@3"))]);
}

/// A bare call to a callable declared in an enclosing callable is the local
/// one, not a member: lexical scope wins over the implicit receiver. The
/// nested `@tailrec def tsort` that shadows its enclosing `tsort` is the shape
/// the sprint-time check measured on gitbucket (`JDBCUtil.tsort`), where the
/// `self` rule had bound both calls to the outer `tsort`.
#[test]
fn scala_a_bare_call_to_a_nested_def_binds_the_nested_def_not_the_member() {
    let file = "src/Nested.scala";
    let src = "\
object U {
  def tsort(edges: Int): Int = {
    def tsort(a: Int, b: Int): Int = if (a == 0) b else tsort(a - 1, b)
    tsort(edges, 0)
  }
  def m(): Int = 0
  def n(): Int = m()
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        call_rows(rt, file),
        sorted(vec![
            row(&at(2, "tsort"), "tsort", RefForm::Path, None, true),
            row(&at(3, "tsort"), "tsort", RefForm::Path, None, true),
            row(&at(7, "n"), "m", RefForm::Method, SELF, true),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&at(2, "tsort"), &at(3, "tsort")),
            edge(&at(3, "tsort"), &at(3, "tsort")),
            edge(&at(7, "n"), &at(6, "m")),
        ])
    );
}

/// The same rule in Kotlin: a local `fun m()` shadows the class's `m` for a
/// bare call in the function that declares it, while a local function of
/// another name leaves the bare call a call on the current instance.
#[test]
fn kotlin_a_bare_call_to_a_local_function_binds_the_local_function_not_the_member() {
    let file = "src/Local.kt";
    let src = "\
class K {
    fun m() {}
    fun run() {
        fun m() {}
        m()
    }
    fun j() {
        fun helper() {}
        m()
    }
}
";
    let tmp = tree(&[(file, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        call_rows(rt, file),
        sorted(vec![
            row(&at(3, "run"), "m", RefForm::Path, None, true),
            row(&at(7, "j"), "m", RefForm::Method, SELF, true),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![edge(&at(3, "run"), &at(4, "m")), edge(&at(7, "j"), &at(2, "m"))])
    );
}

// ── C++ ──────────────────────────────────────────────────────────────────────

const CPP_FILE: &str = "src/a.cpp";

/// C++ has no `super` keyword: its super form is the base-qualified call
/// `Base::m()`, which is the same syntax as a namespace call, so it records
/// `other` — and stays unbound like every super form here.
const CPP: &str = "\
class Base {
public:
    int m() { return 1; }
};

class A : public Base {
public:
    int m() { return other.m(); }
    int n() { return this->m(); }
    int j() { return m(); }
    int k() { return Base::m(); }
    int p() { return (*this).m(); }
    int q() { return ptr->m(); }
    int r() { return this->f.m(); }
};

class B {
public:
    int m() { return 2; }
};

int helper() { return 0; }

int free_fn() { return helper(); }

int m() { return 0; }

class C {
public:
    int n() { return m(); }
};
";

#[test]
fn cpp_this_and_bare_calls_bind_the_callers_own_method_and_no_other_receiver_does() {
    let tmp = tree(&[(CPP_FILE, CPP)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{CPP_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, CPP_FILE),
        sorted(vec![
            // `other.m()` inside `A::m`: no self-loop.
            row(&a(8, "m"), "m", RefForm::Method, OTHER, false),
            // `this->m()`, the bare `m()` and `(*this).m()`: `A::m`.
            row(&a(9, "n"), "m", RefForm::Method, SELF, true),
            row(&a(10, "j"), "m", RefForm::Method, SELF, true),
            row(&a(12, "p"), "m", RefForm::Method, SELF, true),
            // `Base::m()`: not even the in-file `Base::m`.
            row(&a(11, "k"), "m", RefForm::Method, OTHER, false),
            row(&a(13, "q"), "m", RefForm::Method, OTHER, false),
            // A field reached through `this->` is another object.
            row(&a(14, "r"), "m", RefForm::Method, OTHER, false),
            // A bare call in a free function is a free call.
            row(&a(24, "free_fn"), "helper", RefForm::Path, None, true),
            // A bare `m()` in `C`, which has no `m`: never the top-level `m`.
            row(&a(30, "n"), "m", RefForm::Method, SELF, false),
        ])
    );
    let edges = call_edges(rt);
    assert_eq!(
        edges,
        sorted_edges(vec![
            edge(&a(9, "n"), &a(8, "m")),
            edge(&a(10, "j"), &a(8, "m")),
            edge(&a(12, "p"), &a(8, "m")),
            edge(&a(24, "free_fn"), &a(22, "helper")),
        ])
    );
    assert_no_self_loop(&edges);
}
