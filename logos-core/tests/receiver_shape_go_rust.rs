//! Go and Rust method calls bind only through the caller's own receiver type
//! (S-517, CR-169, FR-EX-13, FR-EX-12, FR-RS-11, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against temp-directory fixtures.
//!
//! `receiver_shape.rs` pins the receiver-shape vocabulary (S-514) through
//! on-disk query **overrides**; this suite pins the **shipped** `go` and `rust`
//! plugins, which opt in with those same markers and no override anywhere:
//!
//! - **Go** — a call on the identifier the enclosing method's receiver parameter
//!   declares (`s.F()` in `func (s *Svc) …`) is `self` and binds among the
//!   receiver type's methods; every other operand (`x.F()`, `s.next.F()`, a
//!   parameter that merely shares the receiver's name in another function) is
//!   `other` and binds nowhere — never the same-named free `func F`.
//! - **Rust** — `self.f()` is `self` (S-493's binding, unchanged); every other
//!   receiver (`other.f()`, `self.field.f()`) is `other` and binds nowhere.
//!
//! A bare call (`F()`, `helper()`) is a free call in both languages and keeps
//! binding through the scope walk: neither plugin declares an implicit receiver.
//!
//! The reason a shape-`other` call stays unbound (`no-receiver-evidence`) is a
//! fixed function of the shape, pinned by the binder's unit tests; a fixture of a
//! non-package-shaped language reads the shape and `resolved == false` instead
//! (`logos status` reports `call_residue` for Java and Kotlin rows only).
//!
//! Fixtures are written inline into temp directories, like every sibling binding
//! suite: a fixture tree checked into this repository would be indexed into its
//! own graph.

#![cfg(all(feature = "lang-go", feature = "lang-rust"))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

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

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

// ── Go ────────────────────────────────────────────────────────────────────────

const GO_FILE: &str = "svc/svc.go";

/// `Svc.F`, `Other.F` and a free `func F` — three callables one name — and the
/// receiver shapes against them: the own receiver, a parameter, a field of the
/// own receiver, and a free function whose parameter is *named* like a receiver.
const GO: &str = "\
package svc

type Svc struct{ next *Svc }
type Other struct{}

func (s *Svc) F() {}

func (s *Svc) Run(x *Other) {
\ts.F()
\tx.F()
}

func (s *Svc) Again() {
\ts.Again()
\ts.next.Again()
}

func (o Other) F() {}

func F() {}

func Free(s *Svc) {
\ts.F()
}

func (s *Svc) Bare() {
\tF()
}
";

#[test]
fn a_go_call_on_the_methods_own_receiver_binds_its_types_method_and_no_other_does() {
    let tmp = tree(&[(GO_FILE, GO)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{GO_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, GO_FILE),
        sorted(vec![
            // `s.F()` on the own receiver: the S-493 row `Self::F`, bound.
            row(&a(8, "Run"), "Self::F", RefForm::Path, None, true),
            // `x.F()`: another object — `other`, unbound, and never the free `F`.
            row(&a(8, "Run"), "F", RefForm::Method, OTHER, false),
            // `s.Again()` is genuine recursion; `s.next.Again()` is a call on a
            // field of the receiver, which is not the receiver.
            row(&a(13, "Again"), "Self::Again", RefForm::Path, None, true),
            row(&a(13, "Again"), "Again", RefForm::Method, OTHER, false),
            // `s.F()` with `s` a parameter of a free function, not a receiver.
            row(&a(22, "Free"), "F", RefForm::Method, OTHER, false),
            // A bare `F()` is a free call: the scope walk, the free `func F`.
            row(&a(26, "Bare"), "F", RefForm::Path, None, true),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&a(8, "Run"), &a(6, "F")),
            edge(&a(13, "Again"), &a(13, "Again")),
            edge(&a(26, "Bare"), &a(20, "F")),
        ]),
        "own-receiver calls bind to the receiver type's methods; `x.F()`, `s.next.Again()` and \
         the free function's `s.F()` bind to nothing, and no method-form call reaches `func F`"
    );
}

/// A call inside a local `var` initialiser. Go's `symbols` query captures every
/// `var_spec`, so the call is attributed to the variable `v`, not to the method:
/// it misses the method's `self_name` and self type.
const GO_LOCAL: &str = "\
package svc

type Svc struct{}

func (s *Svc) F() int { return 0 }

func (s *Svc) Local() {
\tvar v = s.F()
\t_ = v
}
";

/// A **known limitation**, pinned so a change to it is deliberate (the S-514 seam
/// notes name it): the own-receiver call in a local `var` initialiser records
/// `other` and stays unbound. It loses a real edge and never invents one.
#[test]
fn a_go_own_receiver_call_in_a_local_var_initialiser_stays_unbound() {
    let tmp = tree(&[(GO_FILE, GO_LOCAL)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows(rt, GO_FILE),
        vec![row(&format!("{GO_FILE}:v@8"), "F", RefForm::Method, OTHER, false)]
    );
    assert!(call_edges(rt).is_empty());
}

// ── Rust ──────────────────────────────────────────────────────────────────────

const RS_FILE: &str = "src/lib.rs";

/// `A::helper`, `B::helper` and a free `fn helper` — three callables one name —
/// with `other.helper()` inside `A::helper` itself (the self-loop shape), beside
/// `self.helper(..)`, and `self.peer.again(..)` (a call on a field of `self`).
const RS: &str = "\
pub struct A { peer: B }
pub struct B;

impl A {
    fn helper(&self, other: &B) {
        other.helper();
    }
    fn run(&self, other: &B) {
        self.helper(other);
        other.helper();
    }
    fn again(&self, n: u32) {
        self.again(n - 1);
        self.peer.again(n);
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
fn a_rust_self_call_binds_through_the_impl_and_every_other_receiver_is_unbound() {
    let tmp = tree(&[(RS_FILE, RS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("{RS_FILE}:{name}@{line}");
    assert_eq!(
        call_rows(rt, RS_FILE),
        sorted(vec![
            // `other.helper()` inside `A::helper`: another object, no self-loop.
            row(&a(5, "helper"), "helper", RefForm::Method, OTHER, false),
            // `self.helper(..)` is S-493's `Self::helper`; `other.helper()` beside
            // it is `other` — neither the caller's own `helper` nor `B`'s.
            row(&a(8, "run"), "Self::helper", RefForm::Path, None, true),
            row(&a(8, "run"), "helper", RefForm::Method, OTHER, false),
            // Genuine recursion binds; a call on `self.peer` does not.
            row(&a(12, "again"), "Self::again", RefForm::Path, None, true),
            row(&a(12, "again"), "again", RefForm::Method, OTHER, false),
            // `x.helper()` in a free function: not the free `helper` beside it.
            row(&a(24, "free"), "helper", RefForm::Method, OTHER, false),
            row(&a(24, "free"), "helper", RefForm::Path, None, true),
        ])
    );
    assert_eq!(
        call_edges(rt),
        sorted_edges(vec![
            edge(&a(8, "run"), &a(5, "helper")),
            edge(&a(12, "again"), &a(12, "again")),
            edge(&a(24, "free"), &a(22, "helper")),
        ]),
        "the only self-loop is the genuine `self.again(..)`; `other.helper()` inside `helper` \
         is not one, and no method-form call reaches the free `fn helper`"
    );
}
