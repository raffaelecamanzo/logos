//! A PHP static call keeps its class, and `new` records instantiation (S-547,
//! CR-176, FR-RS-25, FR-RS-12, FR-RS-13, NFR-RA-05) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! the SHIPPED `references.scm` and `plugin.toml` of the PHP plugin (no query
//! override).
//!
//! What the fixture pins, over methods of a caller calling `Mono\Utils`:
//!
//! - `Utils::getClass()` — a static call on a named class — is the Path-form
//!   `Utils::getClass` (no receiver shape: it is a written qualified call), read
//!   through the file's `use` (also an aliased one and a fully-qualified
//!   `\Mono\Utils::`) or its own namespace, and binds `Utils::getClass` in
//!   another file;
//! - a class two files declare (`Twin`), or none does (`Ghost`), records its
//!   Path row and stays unbound; a qualified name without a leading `\` (PHP reads
//!   it under the caller's own namespace) records no Path row, only a bare `other`
//!   Method row, and never binds a global class of that name;
//! - `self::` and `static::` bind the caller's own class's method, `parent::` the
//!   proven base's — the rows keep their shape and are not Path rows;
//! - a class held in a variable (`$cls::m()`), a call result and a
//!   `namespace\Foo::` scope are `other` Method rows that bind nothing;
//! - `new Utils()` is an `Instantiates` row bound to the class (the same class
//!   through a `use`, an alias, or a fully-qualified name), and `new self`,
//!   `new static`, `new $cls` and `new class { … }` record none.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_call_forms.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-php")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

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

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// Every bound edge of `kind` as `(source label, target label)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The rows of `kind` sourced in `file`, sorted.
fn rows(rt: &Runtime, file: &str, kind: EdgeKind) -> Vec<Row> {
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
        .filter(|r| r.kind == kind && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

fn row(source: &str, target: &str, form: RefForm, receiver: Option<ReceiverShape>, resolved: bool) -> Row {
    (source.to_string(), target.to_string(), form, receiver, resolved)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);
const SUPER: Option<ReceiverShape> = Some(ReceiverShape::Super);
const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

const UTILS: &str = "\
<?php
namespace Mono;

class Utils {
    public static function getClass($o) { return 1; }
    public static function other() { return 2; }
}
";

const BASE: &str = "\
<?php
namespace Mono;

class Base {
    public static function make() { return 1; }
}
";

const TWIN_A: &str = "\
<?php
namespace Mono;

class Twin {
    public static function go() { return 1; }
}
";

const TWIN_B: &str = "\
<?php
namespace Mono;

class Twin {
    public static function go() { return 2; }
}
";

/// A caller in the callee's own namespace, with no `use`.
const SAME_NAMESPACE: &str = "\
<?php
namespace Mono;

class Handler extends Base {
    public function helper() { return 0; }
    public function a() { return Utils::getClass($this); }
    public function b() { return Twin::go(); }
    public function c() { return Ghost::run(); }
    public function d() { return self::helper() + static::helper(); }
    public function e() { return parent::make(); }
    public function f($cls) { return $cls::getClass($this); }
    public function g() { return Utils::build()::other(); }
    public function h() { return namespace\\Utils::other(); }
    public function i() { return \\Mono\\Utils::other(); }
    public function j() { return f()::other(); }
    public function k() { return (new Utils)::other(); }
    public function l($a) { return $a->p::other(); }
    public function m() { return Utils::$x::other(); }
    public function n() { return $this->r()::other(); }
}
";

/// A caller in another namespace, through `use`, an aliased `use` and a
/// fully-qualified name.
const OTHER_NAMESPACE: &str = "\
<?php
namespace App;

use Mono\\Utils;
use Mono\\Utils as U;
use Mono\\Base;

class Service {
    public function a() { return Utils::getClass($this); }
    public function b() { return U::other(); }
    public function c() { return \\Mono\\Utils::other(); }
    public function d() { return Base::make(); }
    public function e() { return Missing::run(); }
}
";

#[test]
fn a_static_call_on_a_named_class_is_a_path_row_that_binds_among_the_classs_methods() {
    let tmp = tree(&[
        ("src/Utils.php", UTILS),
        ("src/Base.php", BASE),
        ("src/TwinA.php", TWIN_A),
        ("src/TwinB.php", TWIN_B),
        ("src/Handler.php", SAME_NAMESPACE),
        ("src/Service.php", OTHER_NAMESPACE),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("src/Handler.php:{name}@{line}");
    assert_eq!(
        rows(rt, "src/Handler.php", EdgeKind::Calls),
        sorted(vec![
            row(&h(6, "a"), "Utils::getClass", RefForm::Path, None, true),
            // Two classes `Mono\Twin`: it names neither (NFR-RA-05).
            row(&h(7, "b"), "Twin::go", RefForm::Path, None, false),
            row(&h(8, "c"), "Ghost::run", RefForm::Path, None, false),
            // `self::` and `static::`: one `self` row; `parent::`: the base's.
            row(&h(9, "d"), "helper", RefForm::Method, SELF, true),
            row(&h(10, "e"), "make", RefForm::Method, SUPER, true),
            // A class held in a variable, and a call result, are `other`; the
            // inner `Utils::build()` is itself a call on a named class.
            row(&h(11, "f"), "getClass", RefForm::Method, OTHER, false),
            row(&h(12, "g"), "Utils::build", RefForm::Path, None, false),
            row(&h(12, "g"), "other", RefForm::Method, OTHER, false),
            // `namespace\Utils` is a prefix the imports cannot read.
            row(&h(13, "h"), "other", RefForm::Method, OTHER, false),
            row(&h(14, "i"), "\\::Mono::Utils::other", RefForm::Path, None, true),
            // A call result, a parenthesised expression, a property and a
            // property of a class are scopes no class name spells: `other`.
            row(&h(15, "j"), "f", RefForm::Path, None, false),
            row(&h(15, "j"), "other", RefForm::Method, OTHER, false),
            row(&h(16, "k"), "other", RefForm::Method, OTHER, false),
            row(&h(17, "l"), "other", RefForm::Method, OTHER, false),
            row(&h(18, "m"), "other", RefForm::Method, OTHER, false),
            row(&h(19, "n"), "other", RefForm::Method, OTHER, false),
            row(&h(19, "n"), "r", RefForm::Method, SELF, false),
        ])
    );
}

#[test]
fn a_static_call_binds_through_use_alias_and_a_fully_qualified_name() {
    let tmp = tree(&[
        ("src/Utils.php", UTILS),
        ("src/Base.php", BASE),
        ("src/TwinA.php", TWIN_A),
        ("src/TwinB.php", TWIN_B),
        ("src/Handler.php", SAME_NAMESPACE),
        ("src/Service.php", OTHER_NAMESPACE),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let s = |line: u32, name: &str| format!("src/Service.php:{name}@{line}");
    assert_eq!(
        rows(rt, "src/Service.php", EdgeKind::Calls),
        sorted(vec![
            row(&s(9, "a"), "Utils::getClass", RefForm::Path, None, true),
            row(&s(10, "b"), "U::other", RefForm::Path, None, true),
            row(&s(11, "c"), "\\::Mono::Utils::other", RefForm::Path, None, true),
            row(&s(12, "d"), "Base::make", RefForm::Path, None, true),
            // No class `Missing` here: the global namespace is no fallback.
            row(&s(13, "e"), "Missing::run", RefForm::Path, None, false),
        ])
    );
    let h = |line: u32, name: &str| format!("src/Handler.php:{name}@{line}");
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            edge(&h(6, "a"), "src/Utils.php:getClass@5"),
            edge(&h(9, "d"), &h(5, "helper")),
            edge(&h(10, "e"), "src/Base.php:make@5"),
            edge(&h(14, "i"), "src/Utils.php:other@6"),
            edge(&s(9, "a"), "src/Utils.php:getClass@5"),
            edge(&s(10, "b"), "src/Utils.php:other@6"),
            edge(&s(11, "c"), "src/Utils.php:other@6"),
            edge(&s(12, "d"), "src/Base.php:make@5"),
        ]
    );
}

// ── `new` ────────────────────────────────────────────────────────────────────

const NEWS: &str = "\
<?php
namespace App;

use Mono\\Utils;
use Mono\\Utils as U;

class Factory {
    public function a() { return new Utils(); }
    public function b() { return new U; }
    public function c() { return new \\Mono\\Utils(1, 2); }
    public function d() { return new Missing(); }
    public function e() { return new \\Mono\\Twin(); }
    public function f() { return new self(); }
    public function g() { return new static(); }
    public function h($cls) { return new $cls(); }
    public function i() { return new class { public function run() { return 1; } }; }
    public function j() { return new Local(); }
    public function k() { return new Selfish(); }
    public function l() { return new STATIC(); }
    public function m() { return new parent(); }
}

class Local { }
";

#[test]
fn new_records_instantiates_and_only_for_a_named_class() {
    let tmp = tree(&[
        ("src/Utils.php", UTILS),
        ("src/TwinA.php", TWIN_A),
        ("src/TwinB.php", TWIN_B),
        ("src/Factory.php", NEWS),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("src/Factory.php:{name}@{line}");
    assert_eq!(
        rows(rt, "src/Factory.php", EdgeKind::Instantiates),
        sorted(vec![
            row(&f(8, "a"), "Utils", RefForm::Path, None, true),
            row(&f(9, "b"), "U", RefForm::Path, None, true),
            row(&f(10, "c"), "\\::Mono::Utils", RefForm::Path, None, true),
            row(&f(11, "d"), "Missing", RefForm::Path, None, false),
            // Two classes `Mono\Twin`: it names neither (NFR-RA-05).
            row(&f(12, "e"), "\\::Mono::Twin", RefForm::Path, None, false),
            row(&f(17, "j"), "Local", RefForm::Path, None, true),
            // A name that only starts like `self` is a class; `STATIC` is the keyword.
            row(&f(18, "k"), "Selfish", RefForm::Path, None, false),
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        vec![
            edge(&f(8, "a"), "src/Utils.php:Utils@4"),
            edge(&f(9, "b"), "src/Utils.php:Utils@4"),
            edge(&f(10, "c"), "src/Utils.php:Utils@4"),
            edge(&f(17, "j"), &f(23, "Local")),
        ]
    );
    // A construction is not a call: no `Calls` row names a class.
    assert!(rows(rt, "src/Factory.php", EdgeKind::Calls).is_empty());
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` and `Instantiates` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> (Edges, Edges) {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        let path = tmp_a.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    (edges(rt_a, EdgeKind::Calls), edges(rt_a, EdgeKind::Instantiates))
}

const CALLER: &str = "\
<?php
namespace App;

use Mono\\Utils;

class A {
    public function p() { return Utils::other(); }
    public function q() { return \\Mono\\Utils::other(); }
    public function u() { return new Utils(); }
}
";

#[test]
fn a_synced_edit_to_the_called_class_matches_a_fresh_reindex() {
    let initial = [("src/A.php", CALLER), ("src/Utils.php", UTILS)];
    // The caller moves down a line: its rows are re-extracted and re-bound.
    let moved = format!("// moved\n{CALLER}");
    let moved = moved.replacen("// moved\n<?php", "<?php\n// moved", 1);
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/A.php", &moved)]);
    assert_eq!(
        calls,
        vec![
            edge("src/A.php:p@8", "src/Utils.php:other@6"),
            edge("src/A.php:q@9", "src/Utils.php:other@6"),
        ]
    );
    assert_eq!(news, vec![edge("src/A.php:u@10", "src/Utils.php:Utils@4")]);

    // The method is renamed away and the class removed: both edges go, though
    // the caller's file never changes.
    let renamed = UTILS.replace("function other()", "function another()");
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/Utils.php", &renamed)]);
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(news, vec![edge("src/A.php:u@9", "src/Utils.php:Utils@4")]);
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/Utils.php", "<?php\nnamespace Mono;\n")]);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(news.is_empty(), "{news:?}");

    // A second `Mono\Utils` makes the call name neither.
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/Twin.php", UTILS)]);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(news.is_empty(), "{news:?}");

    // And the class arrives after the caller: the calls bind on sync.
    let (calls, news) = synced_equals_reindexed(&[("src/A.php", CALLER)], &[("src/Utils.php", UTILS)]);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(news.len(), 1, "{news:?}");

    // A namespace moved away from under the caller's `use` unbinds all three.
    let moved_ns = UTILS.replace("namespace Mono;", "namespace Elsewhere;");
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/Utils.php", &moved_ns)]);
    assert!(calls.is_empty() && news.is_empty(), "{calls:?} {news:?}");
}

// ── A namespace-relative qualified name is never read as a global one ───────

const SUB_FOO: &str = "\
<?php
namespace Sub;

class Foo {
    public static function bar() { return 1; }
}
";

const RELATIVE_CALLER: &str = "\
<?php
namespace App;

class C {
    public function a() { return Sub\\Foo::bar(); }
    public function b() { return new Sub\\Foo(); }
    public function c() { return \\Sub\\Foo::bar(); }
    public function d() { return new \\Sub\\Foo(); }
}
";

/// PHP reads `Sub\Foo` in `namespace App` as `App\Sub\Foo`, never the global
/// `Sub\Foo`: the relative form binds nothing (not even beside an `App\Sub\Foo`
/// that would be the right answer — the binder has no own-namespace rung for
/// PHP), and only the rooted `\Sub\Foo` binds the global class (NFR-RA-05).
#[test]
fn a_relative_qualified_class_name_is_never_read_as_a_global_one() {
    let twin = "<?php\nnamespace App\\Sub;\n\nclass Foo {\n    public static function bar() { return 2; }\n}\n";
    for files in [
        vec![("src/Foo.php", SUB_FOO), ("src/Caller.php", RELATIVE_CALLER)],
        vec![("src/Foo.php", SUB_FOO), ("src/AppFoo.php", twin), ("src/Caller.php", RELATIVE_CALLER)],
    ] {
        let tmp = tree(&files);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        let c = |line: u32, name: &str| format!("src/Caller.php:{name}@{line}");
        assert_eq!(
            rows(rt, "src/Caller.php", EdgeKind::Calls),
            sorted(vec![
                row(&c(5, "a"), "bar", RefForm::Method, OTHER, false),
                row(&c(7, "c"), "\\::Sub::Foo::bar", RefForm::Path, None, true),
            ]),
            "{} file(s)",
            files.len()
        );
        // The rooted `\Sub\Foo::bar()` binds the global class's method — in
        // both trees, whatever `App\Sub\Foo` is.
        assert_eq!(
            edges(rt, EdgeKind::Calls),
            vec![edge(&c(7, "c"), "src/Foo.php:bar@5")]
        );
        assert_eq!(
            rows(rt, "src/Caller.php", EdgeKind::Instantiates),
            vec![row(&c(8, "d"), "\\::Sub::Foo", RefForm::Path, None, true)]
        );
    }
}
