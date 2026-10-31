//! A local binding shadows the receiver and the callee (S-594, CR-191,
//! FR-RS-44, FR-EX-13 as amended, NFR-RA-05) — exercised end to end through
//! the public [`Engine`] façade against temp-directory fixtures of the shipped
//! plugins.
//!
//! A local binding is in force at a call when its scope encloses the call and
//! it is declared before the call's statement, or is hoisted (the S-593 facts):
//!
//! - **Go receiver** — a receiver name a `range`, a `:=`, a closure parameter
//!   or a type switch rebinds records `other`, so it binds nowhere; the plain
//!   own-receiver call still binds.
//! - **Local import** — Python's `from m import f` and C++'s block `using
//!   ns::f` make the bare call a call through that import, never the caller
//!   itself or a same-named function the import hides.
//! - **Local value** — a JavaScript `var f` or a parameter `f` makes `f()` a
//!   call through a value: unbound, counted `local-binding` in `status`.
//! - A binding after the call, in a sibling block, or in the call's own
//!   declaration statement does not shadow; a hoisted `var` shadows its whole
//!   function; a nested function the graph captures is still the call's
//!   target; and Java — whose calls and values are separate namespaces — is
//!   untouched.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

/// The fixture tree, its repository coordinate pinned (S-703) so two trees'
/// graphs compare equal.
fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
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

/// Every bound `Calls` / `Instantiates` edge as `source label → target label`,
/// sorted.
fn call_edges(engine: &Engine) -> Vec<String> {
    let rt = engine.runtime().unwrap();
    let label = labels(rt);
    let mut out: Vec<String> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls | EdgeKind::Instantiates))
        .map(|e| format!("{} -> {}", label[&e.source], label[&e.target]))
        .collect();
    out.sort();
    out
}

/// One `Calls` ledger row: `target form alias receiver resolved@line`.
fn call_rows(engine: &Engine) -> Vec<String> {
    let rt = engine.runtime().unwrap();
    let mut rows: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .map(|r| {
            let shape = r.receiver.map_or("-", ReceiverShape::as_str);
            let form = if r.form == RefForm::Method { "method" } else { "path" };
            let alias = r.alias.as_deref().unwrap_or("-");
            format!("{}@{} {form} {alias} {shape} {}", r.target, r.line.unwrap_or(0), if r.resolved { "bound" } else { "unbound" })
        })
        .collect();
    rows.sort();
    rows
}

/// The `call_residue` reasons of `language`'s `status` row, the zeros left out.
fn residue(engine: &Engine, language: &str) -> BTreeMap<R, u64> {
    let residue: CallResidue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == language)
        .unwrap_or_else(|| panic!("a {language} row"))
        .call_residue
        .unwrap_or_else(|| panic!("the {language} row states its call residue"));
    assert_eq!(
        residue.unbound,
        residue.reasons.values().sum::<u64>() + residue.unclassified,
        "the reasons partition the unbound rows"
    );
    residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect()
}

// ── Go: a rebound receiver ───────────────────────────────────────────────────

/// `s` rebound four ways inside methods of `Svc`, beside the plain own call in
/// `Plain`, a `:=` whose right-hand side still reads the receiver, a call
/// written before a `:=` of the same block (`Short`), and one before the `range`
/// that rebinds the name (`Before`). Each `:=` sits in a block of its own, as Go
/// requires of a name the receiver already binds.
#[cfg(feature = "lang-go")]
const GO: &str = "\
package svc

type Svc struct{ conns []*Conn }
type Conn struct{}

func (s *Svc) Close() {}
func (s *Svc) Clone() *Svc { return s }

func (c *Conn) Close() {}

func (s *Svc) Range(conns []*Conn) {
\tfor _, s := range conns {
\t\ts.Close()
\t}
}

func (s *Svc) Short(c *Conn) {
\tif c != nil {
\t\ts.Close()
\t\ts := c
\t\ts.Close()
\t}
}

func (s *Svc) Closure(c *Conn) {
\tf := func(s *Conn) { s.Close() }
\tf(c)
}

func (s *Svc) Switch(v interface{}) {
\tswitch s := v.(type) {
\tcase *Conn:
\t\ts.Close()
\t}
}

func (s *Svc) Plain() {
\ts.Close()
}

func (s *Svc) Again() {
\tif s != nil {
\t\ts := s.Clone()
\t\t_ = s
\t}
}

func (s *Svc) Before(conns []*Conn) {
\ts.Close()
\tfor _, s := range conns {
\t\t_ = s
\t}
}
";

#[cfg(feature = "lang-go")]
#[test]
fn a_go_receiver_name_rebound_in_the_method_is_another_value() {
    let tmp = tree(&[("svc.go", GO)]);
    let engine = index(tmp.path());
    let rows = call_rows(&engine);
    for line in [13, 21, 26, 33] {
        assert!(
            rows.contains(&format!("Close@{line} method - other unbound")),
            "line {line}: the rebound `s` is `other`: {rows:#?}"
        );
    }
    // The own receiver still binds — and the right-hand side of `s := s.Clone()`
    // still reads it: the new `s` is not in force in its own declaration, nor
    // before the `:=` or the `range` that declares it.
    assert_eq!(
        call_edges(&engine),
        vec![
            "svc.go:Again@41 -> svc.go:Clone@7".to_string(),
            "svc.go:Before@48 -> svc.go:Close@6".to_string(),
            "svc.go:Plain@37 -> svc.go:Close@6".to_string(),
            "svc.go:Short@17 -> svc.go:Close@6".to_string(),
        ],
        "{rows:#?}"
    );
}

// ── Python: a local import ───────────────────────────────────────────────────

/// `f` imports the `f` it calls; `g` calls the module function `helper` a local
/// import hides; and `k`, a sibling function, calls the module's `helper`: `g`'s
/// local import is `g`'s alone.
#[cfg(feature = "lang-python")]
const PY_APP: &str = "\
def f():
    from lib import f
    f()


def helper():
    pass


def g():
    from lib import helper
    helper()


def k():
    helper()
";

#[cfg(feature = "lang-python")]
const PY_LIB: &str = "\
def f():
    pass


def helper():
    pass
";

#[cfg(feature = "lang-python")]
#[test]
fn a_python_local_import_is_the_call_target() {
    let tmp = tree(&[("app.py", PY_APP), ("lib.py", PY_LIB)]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec![
            "app.py:f@1 -> lib.py:f@1".to_string(),
            "app.py:g@10 -> lib.py:helper@5".to_string(),
            "app.py:k@15 -> app.py:helper@6".to_string(),
        ],
        "{:#?}",
        call_rows(&engine)
    );
}

/// A parameter and a local import of one name both in force (werkzeug's
/// `redirect(…, Response=None)` re-importing `Response`): the innermost, latest
/// binding — the import — decides, so the call binds the import's target.
#[cfg(feature = "lang-python")]
#[test]
fn the_latest_of_two_bindings_in_force_decides() {
    let tmp = tree(&[
        ("lib.py", "def resp():\n    pass\n"),
        ("app.py", "def redirect(resp=None):\n    from lib import resp\n    return resp()\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(call_edges(&engine), vec!["app.py:redirect@1 -> lib.py:resp@1".to_string()], "{:#?}", call_rows(&engine));
}

/// A language joins the `call_residue` readout once it holds a local-binding
/// row, and only to state those rows: its other unbound calls are not re-walked
/// (the residue index is not the scopes it binds with), so nothing else it
/// reports moves — an unrelated file calling a parameter leaves `fetch`'s call
/// through `from requests import get` exactly as provably external, or not, as
/// it was.
#[cfg(feature = "lang-python")]
#[test]
fn a_local_binding_row_moves_no_other_figure_of_its_language() {
    const APP: &str = "def get():\n    return 1\n\n\ndef fetch():\n    from requests import get\n    return get()\n";
    let python = |files: &[(&str, &str)]| {
        let tmp = tree(files);
        let engine = index(tmp.path());
        let row = engine.status().resolution_by_language.into_iter().find(|r| r.language == "python").expect("a python row");
        (row.calls.provably_external, row.call_residue)
    };
    let (alone, none) = python(&[("app.py", APP)]);
    assert!(none.is_none(), "no local-binding row, no residue: {none:?}");
    // The parameter is named `get`, a name the graph declares, so the name
    // test alone never calls it external.
    let (beside, residue) = python(&[("app.py", APP), ("other.py", "def apply(get):\n    return get()\n")]);
    assert_eq!(beside, alone, "the unrelated file moves no provably-external figure");
    let residue = residue.expect("the local-binding row brings the residue");
    assert_eq!(residue.reasons.iter().filter(|(_, n)| **n > 0).collect::<Vec<_>>(), [(&R::LocalBinding, &1)]);
    assert_eq!((residue.unbound, residue.unclassified), (2, 1), "`get()` is counted, unclassified");
}

/// The same in a repository whose Rust files make the binder build its residue
/// index (this repository's shape): the JavaScript file is still not
/// re-walked, so its receiver call stays `unclassified` beside the one
/// `local-binding` row, never `no-receiver-evidence`.
#[cfg(all(feature = "lang-typescript", feature = "lang-rust"))]
#[test]
fn a_local_binding_row_admits_no_walk_beside_a_walked_language() {
    let tmp = tree(&[
        ("Cargo.toml", "[package]\nname = \"mixed\"\nversion = \"0.1.0\"\n"),
        ("src/lib.rs", "pub fn r() {}\n"),
        ("web/app.js", "function apply(f) {\n  return f();\n}\n\nfunction pick() {\n  return Math.max(1, 2);\n}\n"),
    ]);
    let engine = index(tmp.path());
    let row = engine.status().resolution_by_language.into_iter().find(|r| r.language == "javascript").expect("a javascript row");
    let residue = row.call_residue.expect("the local-binding row brings the residue");
    assert_eq!(residue.reasons.iter().filter(|(_, n)| **n > 0).collect::<Vec<_>>(), [(&R::LocalBinding, &1)], "{residue:?}");
    assert_eq!((residue.unbound, residue.unclassified), (2, 1), "`Math.max` is counted, unclassified: {residue:?}");
}

// ── C++: a block-scope `using` ───────────────────────────────────────────────

#[cfg(feature = "lang-cpp")]
const CPP: &str = "\
namespace ns {
void from_json(int x) {}
}

namespace detail {
void from_json(int x) {
    using ns::from_json;
    from_json(x);
}

void other(int x) {
    {
        using ns::from_json;
    }
    from_json(x);
}
}
";

/// The block `using` makes `from_json(x)` the `ns` one, never the caller. A
/// `using` in a sibling block does not shadow: `other`'s call is no call
/// through it. It stays unbound all the same, because the include scope reads a
/// `using` written in a function for the whole function (S-579), so it offers
/// `ns::from_json` beside `detail::from_json` — two candidates, a missed edge.
#[cfg(feature = "lang-cpp")]
#[test]
fn a_cpp_block_using_is_the_call_target() {
    let tmp = tree(&[("src/a.cpp", CPP)]);
    let engine = index(tmp.path());
    let rows = call_rows(&engine);
    assert_eq!(
        rows,
        vec!["from_json@15 path - - unbound".to_string(), "from_json@8 path local-import - bound".to_string()]
    );
    assert_eq!(call_edges(&engine), vec!["src/a.cpp:from_json@6 -> src/a.cpp:from_json@2".to_string()], "{rows:#?}");
}

/// A block `using` naming a declaration only an included header holds: the call
/// is read through that `using` as the include scope reads it.
#[cfg(feature = "lang-cpp")]
#[test]
fn a_cpp_block_using_reaches_an_included_header() {
    let tmp = tree(&[
        ("include/lib.hpp", "namespace ns {\ninline void parse(int x) {}\n}\n"),
        (
            "src/b.cpp",
            "#include \"lib.hpp\"\nnamespace app {\nvoid parse(int x) {\n    using ns::parse;\n    parse(x);\n}\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec!["src/b.cpp:parse@3 -> include/lib.hpp:parse@2".to_string()],
        "{:#?}",
        call_rows(&engine)
    );
}

/// Two functions, each with its own block `using` of one name: each call reads
/// the `using` in force where it is written, never the file's first one.
#[cfg(feature = "lang-cpp")]
#[test]
fn a_cpp_block_using_is_read_where_it_is_written() {
    let tmp = tree(&[(
        "src/c.cpp",
        "namespace a {\nvoid f(int x) {}\n}\nnamespace b {\nvoid f(int x) {}\n}\n\
         void one(int x) {\n    using a::f;\n    f(x);\n}\n\
         void two(int x) {\n    using b::f;\n    f(x);\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec!["src/c.cpp:one@7 -> src/c.cpp:f@2".to_string(), "src/c.cpp:two@11 -> src/c.cpp:f@5".to_string()],
        "{:#?}",
        call_rows(&engine)
    );
}

// ── Rust: two block `use`s of one name ───────────────────────────────────────

/// Each function `use`s a `parse` of its own module: the file's import table
/// cannot say which is in force where, so neither binds — never the first
/// one's for both.
#[cfg(feature = "lang-rust")]
#[test]
fn two_block_uses_of_one_name_bind_neither() {
    let tmp = tree(&[(
        "src/lib.rs",
        "pub mod a { pub fn parse() {} }\npub mod b { pub fn parse() {} }\n\
         pub fn one() {\n    use crate::a::parse;\n    parse();\n}\n\
         pub fn two() {\n    use crate::b::parse;\n    parse();\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(call_edges(&engine), Vec::<String>::new(), "{:#?}", call_rows(&engine));
    assert_eq!(residue(&engine, "rust").get(&R::OverloadAmbiguous), Some(&2));

    // One of the two names nothing here (an external crate's, a module that is
    // gone): the other is still not known to be the one in force.
    let tmp = tree(&[(
        "src/lib.rs",
        "pub mod a { pub fn parse() {} }\n\
         pub fn one() {\n    use crate::a::parse;\n    parse();\n}\n\
         pub fn two() {\n    use other::parse;\n    parse();\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(call_edges(&engine), Vec::<String>::new(), "{:#?}", call_rows(&engine));

    // A single block `use` whose path leaves the repository is `external-type`.
    let tmp = tree(&[("src/lib.rs", "pub fn one() {\n    use other::parse;\n    parse();\n}\n")]);
    let engine = index(tmp.path());
    assert_eq!(residue(&engine, "rust").get(&R::ExternalType), Some(&1), "{:#?}", call_rows(&engine));
}

// ── Rust: a closure called inside a macro ────────────────────────────────────

/// A closure hides the module's `fn f` inside a macro's arguments as it does
/// outside one: neither call binds `f`.
#[cfg(feature = "lang-rust")]
#[test]
fn a_closure_called_inside_a_macro_is_a_local_value() {
    let tmp = tree(&[(
        "src/lib.rs",
        "pub fn f(x: i32) -> i32 { x }\npub fn g() {\n    let f = |x: i32| x + 1;\n    assert_eq!(f(1), 2);\n    let _ = f(2);\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(call_edges(&engine), Vec::<String>::new(), "{:#?}", call_rows(&engine));
    // The two calls are one ledger row: same source, target, mark and count.
    assert_eq!(call_rows(&engine), vec!["f@4 path local-value - unbound".to_string()]);
}

/// A block `using` is read before one of an enclosing namespace or the file:
/// the innermost level holding a `using` of the name decides. (One at the very
/// level of the function's own namespace is read beside it — the same level —
/// and two that disagree bind neither, a missed edge.)
#[cfg(feature = "lang-cpp")]
#[test]
fn a_cpp_block_using_is_read_before_an_enclosing_one() {
    let tmp = tree(&[(
        "src/d.cpp",
        "namespace a {\nvoid f(int x) {}\n}\nnamespace b {\nvoid f(int x) {}\n}\nusing a::f;\n\
         namespace detail {\nvoid one(int x) {\n    using b::f;\n    f(x);\n}\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec!["src/d.cpp:one@9 -> src/d.cpp:f@5".to_string()],
        "{:#?}",
        call_rows(&engine)
    );
}

// ── JavaScript: a local value ────────────────────────────────────────────────

/// `run`'s hoisted `var f` (declared after the call) and `apply`'s parameter
/// `f` are values; `later`'s `let` in a sibling block is not in force at the
/// call; `nested`'s inner function is a declaration, and binds — and so is the
/// function a module wrapper's `var listen = exports.listen = function …`
/// declares, which the graph records at the assignment; `holder`'s `box` holds
/// an object, so `box()` calls a value, never the object's `box` method; and
/// `twice(twice)` calls its parameter, never itself.
#[cfg(feature = "lang-typescript")]
const JS: &str = "\
function f() {}

function run() {
  f();
  var f = pick();
}

function apply(f) {
  return f();
}

function later() {
  if (true) {
    let f = 1;
  }
  f();
}

function nested() {
  function inner() {}
  inner();
}

function pick() {}

function module(exports) {
  var listen = exports.listen = function () {};
  function attach() {
    listen();
  }
}

function holder() {
  var box = { box: function () {} };
  box();
}

function twice(twice) {
  return twice();
}
";

#[cfg(feature = "lang-typescript")]
#[test]
fn a_javascript_local_value_binds_nothing_and_is_counted() {
    let tmp = tree(&[("app.js", JS)]);
    let engine = index(tmp.path());
    let rows = call_rows(&engine);
    assert!(rows.contains(&"f@4 path local-value - unbound".to_string()), "{rows:#?}");
    assert!(rows.contains(&"f@9 path local-value - unbound".to_string()), "{rows:#?}");
    // An object holding a same-named method is a value, never that method.
    assert!(rows.contains(&"box@35 path local-value - unbound".to_string()), "{rows:#?}");
    // A parameter named like its own function is the parameter, never a self-loop.
    assert!(rows.contains(&"twice@39 path local-value - unbound".to_string()), "{rows:#?}");
    assert_eq!(
        call_edges(&engine),
        vec![
            "app.js:attach@28 -> app.js:listen@27".to_string(),
            "app.js:later@12 -> app.js:f@1".to_string(),
            "app.js:nested@19 -> app.js:inner@20".to_string(),
            "app.js:run@3 -> app.js:pick@24".to_string(),
        ],
        "{rows:#?}"
    );
    assert_eq!(residue(&engine, "javascript").get(&R::LocalBinding), Some(&4));
}

// ── Java: calls and values are separate namespaces ───────────────────────────

#[cfg(feature = "lang-java")]
const JAVA: &str = "\
package a;

class Box {
    int size() { return 0; }

    int twice() {
        int size = 2;
        return size() * size;
    }
}
";

/// A Java local named like a method never hides it: `size()` calls the method.
#[cfg(feature = "lang-java")]
#[test]
fn a_java_local_never_shadows_a_call() {
    let tmp = tree(&[("src/main/java/a/Box.java", JAVA)]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec!["src/main/java/a/Box.java:twice@6 -> src/main/java/a/Box.java:size@4".to_string()],
        "{:#?}",
        call_rows(&engine)
    );
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Sync `edits` onto an index of `initial`, and assert the graph equals a
/// fresh index of the edited tree (NFR-RA-06).
#[cfg(feature = "lang-python")]
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> (Engine, TempDir) {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed = Vec::new();
    for (rel, text) in edits {
        fs::write(tmp_a.path().join(rel), text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let mut state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    (engine_a, tmp_a)
}

#[cfg(feature = "lang-python")]
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
#[cfg(feature = "lang-python")]
use graph_fingerprint::graph_fingerprint;

/// A local-import call re-binds when only the module it imports from changes:
/// `app.py` is not re-extracted, and its marked rows are re-selected by the
/// names they spell and the import they go through.
#[cfg(feature = "lang-python")]
#[test]
fn a_local_import_call_syncs_as_a_fresh_index() {
    let base = [("app.py", PY_APP), ("lib.py", PY_LIB)];
    let renamed = PY_LIB.replace("def helper", "def helper2");
    let (engine, _tmp) = synced_equals_reindexed(&base, &[("lib.py", &renamed)]);
    assert!(
        !call_edges(&engine).iter().any(|e| e.starts_with("app.py:g@")),
        "the import names nothing now: {:#?}",
        call_edges(&engine)
    );
    let without_f = "def helper():\n    pass\n";
    let (engine, _tmp) = synced_equals_reindexed(&[("app.py", PY_APP), ("lib.py", without_f)], &[("lib.py", PY_LIB)]);
    assert!(call_edges(&engine).contains(&"app.py:f@1 -> lib.py:f@1".to_string()));
}

// ── Ruby: `f()` with parentheses calls the method ────────────────────────────

/// Ruby declares `separate_call_namespace` too: a parameter or a block
/// parameter named like a method never hides it from `f()` written with
/// parentheses, which calls the method.
#[cfg(feature = "lang-ruby")]
#[test]
fn a_ruby_local_never_shadows_a_call_written_with_parentheses() {
    let tmp = tree(&[(
        "lib/b.rb",
        "class B\n  def f; end\n\n  def g(f)\n    f()\n  end\n\n  def k\n    [1].each { |f| f() }\n  end\nend\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(&engine),
        vec!["lib/b.rb:g@4 -> lib/b.rb:f@2".to_string(), "lib/b.rb:k@8 -> lib/b.rb:f@2".to_string()],
        "{:#?}",
        call_rows(&engine)
    );
}
