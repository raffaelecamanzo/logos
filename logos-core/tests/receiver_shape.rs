//! A receiver call records its receiver's shape and binds by it (S-514, CR-169,
//! FR-EX-13, FR-RS-12, FR-RS-06, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures.
//!
//! The seam under test is the one every language plugs into with query
//! captures alone: `@ref.receiver.self` / `.super` / `.other` / `.self_name`
//! name a method call's receiver shape, and the binder dispatches on it —
//! `self` among the caller's own class's members (or through its recorded self
//! type), `super` only through a proven `Extends`, `other` and none never
//! through the caller's scope. Most fixtures here drive it through an on-disk
//! **query override** (`.logos/plugins/<lang>/queries/`), the synthetic plugin a
//! language's own markers will later replace: an override pins the vocabulary
//! these tests exercise whatever the shipped query of that language says.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(all(
    feature = "lang-python",
    feature = "lang-rust",
    feature = "lang-go",
    feature = "lang-java"
))]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

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

/// One `Calls` ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` ledger rows sourced in `file`, sorted.
fn call_rows(rt: &Runtime, file: &str) -> Vec<Row> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| {
                    let file = n.file_path.unwrap_or_default();
                    let label = format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0));
                    (n.symbol.as_str().to_string(), label)
                })
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

/// `rows` in the order [`call_rows`] returns them, so an expectation can be
/// written in source order.
fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

/// `edges` in the order [`call_edges`] returns them.
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

const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);
const SUPER: Option<ReceiverShape> = Some(ReceiverShape::Super);
const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

// ── A class-nested language (Python grammar) ──────────────────────────────────

const PY_FILE: &str = "pkg/a.py";

/// `A.m` / `B.m`, the four call shapes of CR-169 §6, a class with no `m` of its
/// own, and a module-level `m` the methods are named like.
const PY: &str = "\
def m():
    return 0


class A:
    def m(self):
        return other.m()

    def n(self):
        return self.m()

    def k(self):
        return super().m()


class B:
    def m(self):
        return 1


class C:
    def n(self):
        return self.m()


def free_caller():
    return m()
";

const PY_OVERRIDE: &str = ".logos/plugins/python/queries/references.scm";

/// The synthetic **marker-less** plugin: Python's calls and method calls, and
/// no receiver marker at all.
const PY_MARKERLESS: &str = "\
(call function: (identifier) @ref.call)
(call function: (attribute attribute: (identifier) @ref.method))
";

/// The same plugin with the shape vocabulary: `self` for `self.`, `super` for
/// `super().`, `other` for every receiver (outranked by the two).
const PY_MARKED: &str = "\
(call function: (identifier) @ref.call)
(call function: (attribute attribute: (identifier) @ref.method))
((attribute object: (identifier) @ref.receiver.self) (#eq? @ref.receiver.self \"self\"))
((attribute object: (call function: (identifier) @_super) @ref.receiver.super) (#eq? @_super \"super\"))
(attribute object: (_) @ref.receiver.other)
";

#[test]
fn a_marker_less_plugin_leaves_every_method_form_call_unbound() {
    let tmp = tree(&[(PY_FILE, PY), (PY_OVERRIDE, PY_MARKERLESS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{PY_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, PY_FILE),
        sorted(vec![
            row(&a(6, "m"), "m", RefForm::Method, None, false),
            row(&a(9, "n"), "m", RefForm::Method, None, false),
            row(&a(12, "k"), "m", RefForm::Method, None, false),
            row(&a(12, "k"), "super", RefForm::Path, None, false),
            row(&a(22, "n"), "m", RefForm::Method, None, false),
            row(&a(26, "free_caller"), "m", RefForm::Path, None, true),
        ]),
        "no marker, no shape: every method-form call is read as `other` and stays unbound"
    );
    // Only the free call binds, through the scope hierarchy as before.
    assert_eq!(call_edges(rt), vec![edge(&a(26, "free_caller"), &a(1, "m"))]);
}

#[test]
fn self_binds_to_the_callers_own_class_only_and_other_and_super_never_bind() {
    let tmp = tree(&[(PY_FILE, PY), (PY_OVERRIDE, PY_MARKED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{PY_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, PY_FILE),
        sorted(vec![
            // `other.m()` inside `A.m`: no self-loop.
            row(&a(6, "m"), "m", RefForm::Method, OTHER, false),
            // `self.m()` inside `A.n`: `A.m`.
            row(&a(9, "n"), "m", RefForm::Method, SELF, true),
            // `super().m()` with no proven base.
            row(&a(12, "k"), "m", RefForm::Method, SUPER, false),
            row(&a(12, "k"), "super", RefForm::Path, None, false),
            // `self.m()` in `C`, which has no `m`: never the module-level `m`,
            // never `A.m` or `B.m`.
            row(&a(22, "n"), "m", RefForm::Method, SELF, false),
            row(&a(26, "free_caller"), "m", RefForm::Path, None, true),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&a(26, "free_caller"), &a(1, "m")),
            edge(&a(9, "n"), &a(6, "m")),
        ])
    );
}

// ── A module-level language with a recorded self type (Rust grammar) ─────────

const RS_OVERRIDE: &str = ".logos/plugins/rust/queries/references.scm";

/// Rust's call captures with the S-493 `@ref.method.self` capture replaced by
/// the shape vocabulary — what a module-level language writes to opt in.
const RS_MARKED: &str = "\
(call_expression function: (identifier) @ref.call)
(call_expression function: (scoped_identifier) @ref.call)
(call_expression function: (field_expression field: (field_identifier) @ref.method))
(field_expression value: (self) @ref.receiver.self)
(field_expression value: (_) @ref.receiver.other)
";

const RS: &str = "\
pub struct A;
pub struct B;

impl A {
    fn helper(&self) {}
    fn run(&self, other: &B) {
        self.helper();
        other.helper();
    }
}

impl B {
    fn helper(&self) {}
}

fn helper() {}

fn free(x: &A) {
    x.helper();
    helper();
}
";

#[test]
fn a_self_call_in_a_module_level_method_binds_through_its_recorded_self_type() {
    let tmp = tree(&[("src/lib.rs", RS), (RS_OVERRIDE, RS_MARKED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("src/lib.rs:{name}@{line}");
    assert_eq!(
        call_rows(rt, "src/lib.rs"),
        sorted(vec![
            // `x.helper()` in a free function: never `A`'s, `B`'s or the free one.
            row(&a(18, "free"), "helper", RefForm::Method, OTHER, false),
            row(&a(18, "free"), "helper", RefForm::Path, None, true),
            // `self.helper()` is the S-493 row, `Self::helper`.
            row(&a(6, "run"), "Self::helper", RefForm::Path, None, true),
            // `other.helper()` beside the caller's own `helper`.
            row(&a(6, "run"), "helper", RefForm::Method, OTHER, false),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&a(18, "free"), &a(16, "helper")),
            edge(&a(6, "run"), &a(5, "helper")),
        ])
    );
}

// ── Go: the own receiver identifier is `self` (`self_name`) ───────────────────

const GO_REFS_OVERRIDE: &str = ".logos/plugins/go/queries/references.scm";
const GO_SYMBOLS_OVERRIDE: &str = ".logos/plugins/go/queries/symbols.scm";

/// A receiver `x.f()` is `other` unless `x` is the identifier the enclosing
/// method's receiver parameter declares (`self_name`).
const GO_MARKED_REFS: &str = "\
(call_expression function: (identifier) @ref.call)
(call_expression function: (selector_expression field: (field_identifier) @ref.method))
(selector_expression operand: (_) @ref.receiver.other)
(method_declaration receiver: (parameter_list (parameter_declaration name: (identifier) @ref.receiver.self_name)))
";

/// Go's declarations with the receiver's base type recorded as the method's
/// self type (S-493's `@symbol.self_type`), so this suite does not depend on
/// the shipped Go query.
const GO_SELF_TYPED_SYMBOLS: &str = "\
(function_declaration name: (identifier) @symbol.function)
(method_declaration
  receiver: (parameter_list
    (parameter_declaration type: [(type_identifier) @symbol.self_type
                                  (pointer_type (type_identifier) @symbol.self_type)]))
  name: (field_identifier) @symbol.method)
(type_declaration (type_spec name: (type_identifier) @symbol.struct type: (struct_type)))
";

const GO: &str = "\
package svc

type Svc struct{}
type Other struct{}

func (s *Svc) F() {}

func (s *Svc) Run(x *Other) {
\ts.F()
\tx.F()
}

func (o Other) F() {}

func F() {}

func Free(s *Svc) {
\ts.F()
}
";

#[test]
fn a_call_on_the_methods_own_receiver_binds_its_types_method_and_no_other_does() {
    let tmp = tree(&[
        ("svc/svc.go", GO),
        (GO_REFS_OVERRIDE, GO_MARKED_REFS),
        (GO_SYMBOLS_OVERRIDE, GO_SELF_TYPED_SYMBOLS),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("svc/svc.go:{name}@{line}");
    assert_eq!(
        call_rows(rt, "svc/svc.go"),
        sorted(vec![
            // `s.F()` with `s` a parameter, not a receiver: `other`, unbound —
            // and never the free `func F`.
            row(&a(17, "Free"), "F", RefForm::Method, OTHER, false),
            // `s.F()` on the own receiver: through the self type `Svc`.
            row(&a(8, "Run"), "Self::F", RefForm::Path, None, true),
            // `x.F()`: `other`.
            row(&a(8, "Run"), "F", RefForm::Method, OTHER, false),
        ])
    );
    assert_eq!(call_edges(rt), vec![edge(&a(8, "Run"), &a(6, "F"))]);
}

// ── Java: the typing markers map onto the shape ───────────────────────────────

const JAVA_BASE_FILE: &str = "src/main/java/com/x/base/Base.java";
const JAVA_MAILER_FILE: &str = "src/main/java/com/x/mail/Mailer.java";
const JAVA_SVC_FILE: &str = "src/main/java/com/x/svc/Svc.java";
const JAVA_BASE: &str = "package com.x.base;\n\npublic class Base {\n    public void start() {}\n}\n";
const JAVA_MAILER: &str =
    "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n    public Mailer next() { return this; }\n}\n";
/// Every receiver S-467 proves, and three it cannot: a chained call, a `this`
/// inside an anonymous class body (to a method that body declares), a `super`
/// call of a class extending nothing.
const JAVA_SVC: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base;\n\
import com.x.mail.Mailer;\n\
\n\
public class Svc extends Base {\n\
    private Mailer mailer;\n\
    public void viaField() { mailer.send(); }\n\
    public void viaParam(Mailer given) { given.send(); }\n\
    public void viaThis() { this.helper(); }\n\
    public void viaBare() { helper(); }\n\
    public void viaSuper() { super.start(); }\n\
    public void chained() { mailer.next().send(); }\n\
    public void anon() { new Runnable() { public void run() { this.helper(); } void helper() {} }; }\n\
    void helper() {}\n\
}\n\
\n\
class Plain {\n\
    void up() { super.hashCode(); }\n\
}\n";

#[test]
fn java_proven_receivers_bind_as_before_and_every_other_receiver_records_its_shape() {
    let tmp = tree(&[
        (JAVA_BASE_FILE, JAVA_BASE),
        (JAVA_MAILER_FILE, JAVA_MAILER),
        (JAVA_SVC_FILE, JAVA_SVC),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let svc = |line: u32, name: &str| format!("{JAVA_SVC_FILE}:{name}@{line}");
    let mailer = |line: u32, name: &str| format!("{JAVA_MAILER_FILE}:{name}@{line}");
    // Every proven receiver's edge, exactly as S-467/S-468 bind them.
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&svc(10, "viaThis"), &svc(15, "helper")),
            edge(&svc(11, "viaBare"), &svc(15, "helper")),
            edge(&svc(12, "viaSuper"), &format!("{JAVA_BASE_FILE}:start@4")),
            edge(&svc(13, "chained"), &mailer(5, "next")),
            edge(&svc(8, "viaField"), &mailer(4, "send")),
            edge(&svc(9, "viaParam"), &mailer(4, "send")),
            // The anonymous body's own `helper`, reached as a free call through
            // the lexical scope — as the scope walk bound it before S-514.
            edge(&svc(14, "run"), &svc(14, "helper")),
        ])
    );
    let shaped: Vec<Row> = call_rows(rt, JAVA_SVC_FILE)
        .into_iter()
        .filter(|r| r.2 == RefForm::Method)
        .collect();
    assert_eq!(
        shaped,
        sorted(vec![
            // The chained receiver proves nothing.
            row(&svc(13, "chained"), "send", RefForm::Method, OTHER, false),
            // A class extending nothing: no level for `super` to bind at.
            row(&svc(19, "up"), "hashCode", RefForm::Method, SUPER, false),
        ])
    );
}

// ── Determinism: index twice, sync ≡ reindex ──────────────────────────────────

#[test]
fn reindexing_is_byte_identical_and_a_synced_edit_matches_a_fresh_reindex() {
    let b_file = "pkg/b.py";
    let b = "class D:\n    def m(self):\n        return self.m()\n";
    let tmp = tree(&[(PY_FILE, PY), (b_file, b), (PY_OVERRIDE, PY_MARKED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let (edges, rows_a, rows_b) = (call_edges(rt), call_rows(rt, PY_FILE), call_rows(rt, b_file));
    engine.index();
    assert_eq!(
        (call_edges(rt), call_rows(rt, PY_FILE), call_rows(rt, b_file)),
        (edges, rows_a.clone(), rows_b),
        "a second index yields the identical edges and ledger"
    );

    // `C` gains its own `m`: its `self.m()` now binds it.
    let edited = PY.replace(
        "class C:\n    def n(self):\n        return self.m()\n",
        "class C:\n    def m(self):\n        return 2\n\n    def n(self):\n        return self.m()\n",
    );
    write(tmp.path(), PY_FILE, &edited);
    let changed: Vec<PathBuf> = vec![PY_FILE.into()];
    engine.sync(&changed);

    let fresh_tmp = tree(&[(PY_FILE, &edited), (b_file, b), (PY_OVERRIDE, PY_MARKED)]);
    let fresh = index(fresh_tmp.path());
    let fresh_rt = fresh.runtime().unwrap();
    assert_eq!(call_edges(rt), call_edges(fresh_rt), "sync ≡ reindex: edges");
    assert!(
        call_edges(rt).contains(&edge(&format!("{PY_FILE}:n@25"), &format!("{PY_FILE}:m@22"))),
        "the edited class's own `m` binds"
    );
    for file in [PY_FILE, b_file] {
        assert_eq!(call_rows(rt, file), call_rows(fresh_rt, file), "sync ≡ reindex: {file} rows");
    }
}

#[test]
fn a_callers_self_and_other_calls_of_one_name_are_two_rows_that_bind_differently() {
    // The shape is part of the ledger identity (migration 29): one caller's
    // `self.m()` and `other.m()` share source, target, form and kind, and only
    // the first binds.
    let src = "class A:\n    def m(self):\n        return 0\n\n    def n(self, other):\n        self.m()\n        other.m()\n";
    let tmp = tree(&[(PY_FILE, src), (PY_OVERRIDE, PY_MARKED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let n = format!("{PY_FILE}:n@5");
    assert_eq!(
        call_rows(rt, PY_FILE),
        sorted(vec![
            row(&n, "m", RefForm::Method, SELF, true),
            row(&n, "m", RefForm::Method, OTHER, false),
        ])
    );
    assert_eq!(call_edges(rt), vec![edge(&n, &format!("{PY_FILE}:m@2"))]);
}
