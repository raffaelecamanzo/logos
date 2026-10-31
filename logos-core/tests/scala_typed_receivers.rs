//! A Scala call on a receiver of declared type binds among that type's members
//! (S-576, CR-183, FR-RS-37, FR-RS-10, FR-RS-12, NFR-RA-05) — exercised end
//! to end through the public [`Engine`] façade against temp-directory
//! fixtures, with the SHIPPED `references.scm`, `symbols.scm` and
//! `plugin.toml` of the Scala plugin (no query override).
//!
//! What the fixture pins, over methods of `shop.api.Handler` calling the
//! `shop.domain` types through an import:
//!
//! - a receiver whose type the file declares — a class parameter
//!   (`orderService`), a `val` class parameter read through `this.`, a member
//!   `val`, a parameter, a typed local `val`, `val made = new Order()`, a typed
//!   lambda parameter — is the Path-form `T::m` of shape `other`, bound among
//!   `T`'s members: a trait's own, and a base class's through the proven
//!   hierarchy (`SpecialOrder` → `Order.ship`) — and a class's own, never its
//!   companion `object`'s, which only the written `T.m()` reaches;
//! - a receiver the file proves no type for — `val got = make()`, an untyped
//!   lambda parameter, a generator variable, a `val (a, b)` name, a pattern
//!   variable, a name declared with a qualified type, whose simple name the
//!   imports may re-qualify to another type — stays the Method-form row of
//!   shape `other`, bound nowhere (`no-receiver-evidence`), though the class
//!   declares a same-named member `val` whose type it does prove;
//! - a parameterless selection `order.ship` is not typed: it reads a `val`
//!   and a parameterless `def` alike, and a case class's field beside its
//!   same-named copy-builder `def` would bind the builder;
//! - a type parameter (`class Box[T](t: T)`, `def gen[A](a: A)`) is no type,
//!   even beside a class of that name, and a member a base class declares is
//!   not in scope.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`scala_calls.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-scala")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::models::CallResidueReason;
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

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn calls(rt: &Runtime) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The `Calls` rows sourced in `file` whose target names one of `methods`,
/// sorted.
fn rows(rt: &Runtime, file: &str, methods: &[&str]) -> Vec<Row> {
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
        .filter(|r| methods.iter().any(|m| r.target == *m || r.target.ends_with(&format!("::{m}"))))
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

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

/// The methods the fixtures call on a receiver.
const TYPED_CALLS: &[&str] = &["ship", "addOrderItem"];

const DOMAIN_PATH: &str = "src/main/scala/shop/domain/Domain.scala";
const DOMAIN: &str = "\
package shop.domain

trait OrderService {
  def addOrderItem(id: Int): Unit
}

class Order {
  def addOrderItem(id: Int): Unit = {}
  def ship(): Unit = {}
}

class SpecialOrder extends Order

class Mailer {
  def ship(): Unit = {}
}

class Box[A] {
  def put[B](b: B): Unit = {}
}

object Util {
  def ship(): Unit = {}
  def ping[A](a: A): Unit = {}
}
";

const HANDLER_PATH: &str = "src/main/scala/shop/api/Handler.scala";
const HANDLER: &str = "\
package shop.api

import shop.domain.{Mailer, Order, OrderService, SpecialOrder}

class Handler(orderService: OrderService, val current: Order) {
  private val maybe: Order = null
  private var x: Mailer = null
  private val m: Mailer = null
  private val it: Mailer = null
  private val e: Mailer = null
  private val got: Mailer = null
  def byClassParameter(): Unit = orderService.addOrderItem(1)
  def byThis(): Unit = this.current.ship()
  def byVal(): Unit = maybe.ship()
  def byParameter(order: Order): Unit = order.ship()
  def byLocal(): Unit = { val o: Order = make(); o.ship() }
  def byNew(): Unit = { val made = new Order(); made.addOrderItem(2) }
  def byBase(s: SpecialOrder): Unit = s.ship()
  def bySelection(order: Order): Unit = order.ship
  def byTypedLambda(xs: List[Order]): Unit = xs.foreach((each: Order) => each.ship())
  def untyped(): Unit = { val got = make(); got.ship() }
  def lambda(xs: List[Order]): Unit = xs.foreach(x => x.ship())
  def loop(xs: List[Order]): Unit = for (m <- xs) m.ship()
  def tuple(): Unit = { val (it, n) = pair(); it.ship() }
  def qualified(u: shop.domain.Order): Unit = u.ship()
  def pattern(v: Any): Unit = v match { case e: Order => e.ship() }
  def make(): Order = null
  def pair(): (Order, Int) = (null, 0)
}
";

#[test]
fn a_call_on_a_receiver_of_declared_type_binds_among_that_types_members() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (HANDLER_PATH, HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("{HANDLER_PATH}:{name}@{line}");
    let d = |line: u32, name: &str| format!("{DOMAIN_PATH}:{name}@{line}");
    let path = |source: String, target: &str| row(&source, target, RefForm::Path, OTHER, true);
    let untyped = |source: String, name: &str| row(&source, name, RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, HANDLER_PATH, TYPED_CALLS),
        sorted(vec![
            path(h(12, "byClassParameter"), "OrderService::addOrderItem"),
            path(h(13, "byThis"), "Order::ship"),
            path(h(14, "byVal"), "Order::ship"),
            path(h(15, "byParameter"), "Order::ship"),
            path(h(16, "byLocal"), "Order::ship"),
            path(h(17, "byNew"), "Order::addOrderItem"),
            path(h(18, "byBase"), "SpecialOrder::ship"),
            untyped(h(19, "bySelection"), "ship"),
            path(h(20, "byTypedLambda"), "Order::ship"),
            untyped(h(21, "untyped"), "ship"),
            untyped(h(22, "lambda"), "ship"),
            untyped(h(23, "loop"), "ship"),
            untyped(h(24, "tuple"), "ship"),
            untyped(h(25, "qualified"), "ship"),
            untyped(h(26, "pattern"), "ship"),
        ])
    );
    let edges = calls(rt);
    let add_trait = d(4, "addOrderItem");
    let add_class = d(8, "addOrderItem");
    let ship = d(9, "ship");
    for expected in [
        edge(&h(12, "byClassParameter"), &add_trait),
        edge(&h(13, "byThis"), &ship),
        edge(&h(14, "byVal"), &ship),
        edge(&h(15, "byParameter"), &ship),
        edge(&h(16, "byLocal"), &ship),
        edge(&h(17, "byNew"), &add_class),
        // Inheritance: `SpecialOrder` declares no `ship`; its base does.
        edge(&h(18, "byBase"), &ship),
        edge(&h(20, "byTypedLambda"), &ship),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    // Nothing reaches `Mailer.ship`: every same-named member `val` is another
    // variable, whose `Mailer` type a leak would bind.
    let mailer_ship = d(15, "ship");
    assert!(!edges.iter().any(|(_, to)| *to == mailer_ship), "{edges:#?}");
}

#[test]
fn impact_of_the_called_method_lists_every_typed_caller() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (HANDLER_PATH, HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let callers: Vec<String> = calls(rt)
        .into_iter()
        .filter(|(_, to)| to.starts_with(&format!("{DOMAIN_PATH}:addOrderItem@")))
        .map(|(from, _)| from)
        .collect();
    assert_eq!(
        callers,
        vec![format!("{HANDLER_PATH}:byClassParameter@12"), format!("{HANDLER_PATH}:byNew@17")]
    );
}

/// The declaration forms the main fixture does not write: a typed `var`,
/// `var x = new T()`, `val x = new T[A]()`, and a generic call `m[T](…)` on a
/// `this.` field and on a parameter. A capitalised receiver is S-573's object
/// call, never a typed name; `other.held` is no `this.held`, though `Typed`
/// declares `held`.
const TYPED_PATH: &str = "src/main/scala/shop/api/Typed.scala";
const TYPED: &str = "\
package shop.api

import shop.domain.{Box, Mailer, Order, Util}

class Typed(val box: Box[Int]) {
  def byVar(): Unit = { var v: Order = null; v.ship() }
  def byVarNew(): Unit = { var w = new Order(); w.ship() }
  def byGenericNew(): Unit = { val g = new Box[Int](); g.put(1) }
  def byGenericField(): Unit = this.box.put[Int](1)
  def byGenericCall(b: Box[Int]): Unit = b.put[Int](2)
  private val held: Order = null
  private val held2: Box[Int] = null
  def onObject(): Unit = Util.ship()
  def onObjectGeneric(): Unit = Util.ping[Int](1)
  def viaOther(other: Mailer): Unit = { other.held.ship(); other.held2.put[Int](1) }
  def onLocalObject(): Unit = { Local.ship(); Local.ping[Int](1) }
}

object Local {
  def ship(): Unit = {}
  def ping[A](a: A): Unit = {}
}
";

#[test]
fn every_declaration_form_scala_writes_proves_its_type() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (TYPED_PATH, TYPED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let t = |line: u32, name: &str| format!("{TYPED_PATH}:{name}@{line}");
    let (ship, put) = (format!("{DOMAIN_PATH}:ship@9"), format!("{DOMAIN_PATH}:put@19"));
    let from_typed: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(TYPED_PATH)).collect();
    assert_eq!(
        from_typed,
        vec![
            edge(&t(10, "byGenericCall"), &put),
            edge(&t(9, "byGenericField"), &put),
            edge(&t(8, "byGenericNew"), &put),
            edge(&t(6, "byVar"), &ship),
            edge(&t(7, "byVarNew"), &ship),
            edge(&t(16, "onLocalObject"), &t(21, "ping")),
            edge(&t(16, "onLocalObject"), &t(20, "ship")),
            edge(&t(13, "onObject"), &format!("{DOMAIN_PATH}:ship@23")),
            edge(&t(14, "onObjectGeneric"), &format!("{DOMAIN_PATH}:ping@24")),
        ]
    );
}

// ── What the file does not prove ────────────────────────────────────────────

/// A base class in another file, whose `repo` is no member of the class that
/// inherits it as far as this file can see.
const BASE_PATH: &str = "src/main/scala/shop/api/Base.scala";
const BASE: &str = "\
package shop.api

import shop.domain.Order

class Base {
  val repo: Order = null
}
";

/// Every call below names `ship` on a receiver the file proves no type for,
/// though a plain reading would hand it one: a type parameter — plain,
/// covariant or contravariant — beside a class
/// of its name, an inherited member, a `new T { … }` value, a `given`, a
/// pattern variable that is not its pattern's first.
const UNPROVEN_PATH: &str = "src/main/scala/shop/api/Unproven.scala";
const UNPROVEN: &str = "\
package shop.api

import shop.domain.Order

class T { def ship(): Unit = {} }
class A { def ship(): Unit = {} }

class Box[T](t: T) {
  def run(): Unit = t.ship()
}

class Gen {
  def gen[A](a: A): Unit = a.ship()
}

class Child extends Base {
  def run(): Unit = repo.ship()
}

class Anon {
  private val made: T = null
  def run(): Unit = { val made = new Order { }; made.ship() }
}

class Given {
  private val ord: T = null
  def run(): Unit = { given ord: Order = null; ord.ship() }
}

class Matched {
  private val b: T = null
  def run(x: Any): Unit = x match { case Pair(_, c, b) => b.ship() }
}

class Cov[+T](t: T) { def run(): Unit = t.ship() }
class Contra[-T](t: T) { def run(): Unit = t.ship() }
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (BASE_PATH, BASE), (UNPROVEN_PATH, UNPROVEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, UNPROVEN_PATH, &["ship"]);
    assert_eq!(receiver_calls.len(), 8, "the fixture makes eight receiver calls: {receiver_calls:#?}");
    for r in &receiver_calls {
        assert_eq!((r.2, r.3, r.4), (RefForm::Method, OTHER, false), "{r:?} must stay untyped");
    }
    let edges = calls(rt);
    assert!(
        !edges.iter().any(|(from, _)| from.starts_with(UNPROVEN_PATH)),
        "nothing here proves a receiver type: {edges:#?}"
    );
}

/// A parameterless selection reads a field as readily as a method: a case
/// class's field `schedule` sits beside its copy-builder `def schedule(s)`
/// (ox's `RetryConfig`), and S-573's selection binds only a method. Only the
/// applied call is typed.
const BUILDER_PATH: &str = "src/main/scala/shop/api/Builder.scala";
const BUILDER: &str = "\
package shop.api

import shop.domain.Config

class Builder(c: Config) {
  def read(): Unit = sleep(c.schedule)
  def rebuild(): Config = c.schedule(2)
  def sleep(n: Int): Unit = {}
}
";

const CONFIG_PATH: &str = "src/main/scala/shop/domain/Config.scala";
const CONFIG: &str = "\
package shop.domain

case class Config(schedule: Int) {
  def schedule(n: Int): Config = copy(schedule = n)
}
";

#[test]
fn a_parameterless_selection_is_not_typed_and_an_applied_call_is() {
    let tmp = tree(&[(CONFIG_PATH, CONFIG), (BUILDER_PATH, BUILDER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let b = |line: u32, name: &str| format!("{BUILDER_PATH}:{name}@{line}");
    assert_eq!(
        rows(rt, BUILDER_PATH, &["schedule"]),
        sorted(vec![
            row(&b(6, "read"), "schedule", RefForm::Method, OTHER, false),
            row(&b(7, "rebuild"), "Config::schedule", RefForm::Path, OTHER, true),
        ])
    );
    let builder_calls: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(BUILDER_PATH)).collect();
    assert_eq!(
        builder_calls,
        vec![edge(&b(6, "read"), &b(8, "sleep")), edge(&b(7, "rebuild"), &format!("{CONFIG_PATH}:schedule@4"))]
    );
}

/// Index `src` at `path` beside the domain: its `ship` calls number `expected`,
/// every one stays the untyped `other` row, binding nothing in the domain, and
/// they are the Scala calls the status readout classifies `no-receiver-evidence`.
fn assert_every_ship_call_stays_untyped(path: &str, src: &str, expected: usize) {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (path, src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, path, &["ship"]);
    assert_eq!(receiver_calls.len(), expected, "{receiver_calls:#?}");
    for r in &receiver_calls {
        assert_eq!((r.2, r.3, r.4), (RefForm::Method, OTHER, false), "{r:?} must stay untyped");
    }
    let edges = calls(rt);
    assert!(!edges.iter().any(|(from, to)| from.starts_with(path) && to.starts_with(DOMAIN_PATH)), "{edges:#?}");
    let residue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "scala")
        .and_then(|row| row.call_residue)
        .expect("the scala row states its call residue");
    assert_eq!(residue.reasons.get(&CallResidueReason::NoReceiverEvidence), Some(&(expected as u64)));
}

/// Every binding form the declaration walk never reads, each naming a member
/// `val` the class declares `Order`: a form the query does not refuse is typed
/// by that member and binds `Order.ship`.
const LEAKS_PATH: &str = "src/main/scala/shop/api/Leaks.scala";
const LEAKS: &str = "\
package shop.api

import shop.domain.Order

class Leaks {
  private val c1: Order = null
  private val c2: Order = null
  private val c3: Order = null
  private val c4: Order = null
  private val c5: Order = null
  private val c6: Order = null
  private val c7: Order = null
  private val c8: Order = null
  private val c9: Order = null
  private val c10: Order = null
  private val c11: Order = null
  private val c12: Order = null
  private val c13: Order = null
  private val c14: Order = null
  private val c15: Order = null
  private val c16: Order = null
  private val c17: Order = null
  def caseVar(v: Any): Unit = v match { case c1 => c1.ship() }
  def capture(v: Any): Unit = v match { case c2 @ Some(_) => c2.ship() }
  def infixLeft(v: List[Any]): Unit = v match { case c3 :: rest => c3.ship() }
  def infixRight(v: List[Any]): Unit = v match { case h :: c4 => c4.ship() }
  def binding(xs: List[Any]): Unit = xs.foreach((c5) => c5.ship())
  def identifiers(): Unit = { val c6, p = make(); c6.ship() }
  def varIdentifiers(): Unit = { var c7, q = make(); c7.ship() }
  def repeat(v: Any): Unit = v match { case List(c8*) => c8.ship() }
  def named(v: Any): Unit = v match { case P(a = c9) => c9.ship() }
  def anon(): Unit = { val c10 = new Order { }; c10.ship() }
  def varAnon(): Unit = { var c11 = new Order { }; c11.ship() }
  def catchVar(): Unit = try make() catch case c12 => c12.ship()
  def implicitLambda(xs: List[Any]): Unit = xs.foreach { implicit c13 => c13.ship() }
  def interpolated(s: String): Unit = s match { case s\"#$c14\" => c14.ship() }
  def nestedInterpolated(v: Any): Unit = v match { case Some(s\"#$c15\") => c15.ship() }
  class SelfAlias { c16 => def go(): Unit = c16.ship() }
  def varUntyped(): Unit = { var c17 = make(); c17.ship() }
  def make(): Any = null
}
";

#[test]
fn every_binding_form_the_walk_never_reads_refuses_its_name() {
    assert_every_ship_call_stays_untyped(LEAKS_PATH, LEAKS, 17);
}

/// A name declared with a type whose simple name is not the type: qualified,
/// by-name, generic over a qualified name, compound, a function, a union. The
/// walk reads each as `Order` and would bind `Order.ship`; every declaration
/// form refuses the name, a member `val`/`var` declaration file-wide.
const QUALIFIED_PATH: &str = "src/main/scala/shop/api/Qualified.scala";
const QUALIFIED: &str = "\
package shop.api

import shop.domain.Order

class Q1(q1: shop.domain.Order) { def run(): Unit = q1.ship() }

class Qualified {
  def byName(q2: => shop.domain.Order): Unit = q2.ship()
  def generic(q3: shop.domain.Order[Int]): Unit = q3.ship()
  def compound(q4: Other with shop.domain.Order): Unit = q4.ship()
  def function(q5: Int => shop.domain.Order): Unit = q5.ship()
  def union(q6: Other | shop.domain.Order): Unit = q6.ship()
  def lambda(xs: List[Any]): Unit = xs.foreach((q7: shop.domain.Order) => q7.ship())
  def local(): Unit = { val q8: shop.domain.Order = null; q8.ship() }
  def localVar(): Unit = { var q9: shop.domain.Order = null; q9.ship() }
}

trait QualifiedMembers {
  val q10: shop.domain.Order
  var q11: shop.domain.Order
  def f(q10: Order): Unit = q10.ship()
  def g(q11: Order): Unit = q11.ship()
}
";

#[test]
fn a_name_declared_with_a_type_that_is_not_its_simple_name_is_refused() {
    assert_every_ship_call_stays_untyped(QUALIFIED_PATH, QUALIFIED, 11);
}

// ── A class with a companion object ─────────────────────────────────────────

/// A class and its companion `object`, both declaring `ship`, and only the
/// class declaring `pack`: the written `Order.ship()` is the object's (S-573),
/// an instance's `o.ship()` the class's.
const PAIR_PATH: &str = "src/main/scala/shop/pair/Order.scala";
const PAIR: &str = "\
package shop.pair

class Order {
  def ship(): Unit = {}
  def pack(): Unit = {}
}

object Order {
  def ship(): Unit = {}
}
";

const PAIR_CALLER_PATH: &str = "src/main/scala/shop/pair/C.scala";
const PAIR_CALLER: &str = "\
package shop.pair

class C {
  def m(o: Order): Unit = { o.ship(); o.pack() }
  def onObject(): Unit = Order.ship()
}
";

/// A typed receiver of a class that has a companion `object` binds among the
/// class's own methods (S-576 against S-573): `o.ship()` is the class's
/// `ship`, never the object's, and `o.pack()`, which only the class declares,
/// binds too. The written `Order.ship()` stays the object's. The typed row
/// keeps its `other` shape, as a Kotlin one does (S-563).
#[test]
fn a_typed_receiver_of_a_class_with_a_companion_binds_the_classs_method() {
    let tmp = tree(&[(PAIR_PATH, PAIR), (PAIR_CALLER_PATH, PAIR_CALLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("{PAIR_CALLER_PATH}:{name}@{line}");
    let o = |line: u32, name: &str| format!("{PAIR_PATH}:{name}@{line}");
    assert_eq!(
        rows(rt, PAIR_CALLER_PATH, &["ship", "pack"]),
        sorted(vec![
            row(&c(4, "m"), "Order::pack", RefForm::Path, OTHER, true),
            row(&c(4, "m"), "Order::ship", RefForm::Path, OTHER, true),
            row(&c(5, "onObject"), "Order::ship", RefForm::Path, None, true),
        ])
    );
    let from_caller: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(PAIR_CALLER_PATH)).collect();
    assert_eq!(
        from_caller,
        vec![
            edge(&c(4, "m"), &o(5, "pack")),
            edge(&c(4, "m"), &o(4, "ship")),
            // The written call on the name is the object's (S-573).
            edge(&c(5, "onObject"), &o(9, "ship")),
        ]
    );
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Edges {
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
    calls(rt_a)
}

const CALLER: &str = "\
package shop.api

import shop.domain.{Order, OrderService}

class Handler(orderService: OrderService) {
  def byClassParameter(): Unit = orderService.addOrderItem(1)
}
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [(DOMAIN_PATH, DOMAIN), (HANDLER_PATH, CALLER)];
    let caller = format!("{HANDLER_PATH}:byClassParameter@6");
    // The member is renamed away: the edge goes, though the caller's file
    // never changes.
    let renamed = DOMAIN.replace("def addOrderItem(id: Int): Unit\n", "def add(id: Int): Unit\n");
    let edges = synced_equals_reindexed(&initial, &[(DOMAIN_PATH, &renamed)]);
    assert!(edges.is_empty(), "{edges:?}");

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(&[(HANDLER_PATH, CALLER)], &[(DOMAIN_PATH, DOMAIN)]);
    assert_eq!(edges, vec![edge(&caller, &format!("{DOMAIN_PATH}:addOrderItem@4"))]);

    // The class parameter's declared type changes: the caller's row is re-typed.
    let retyped = CALLER.replace("orderService: OrderService", "orderService: Order");
    let edges = synced_equals_reindexed(&initial, &[(HANDLER_PATH, &retyped)]);
    assert_eq!(edges, vec![edge(&caller, &format!("{DOMAIN_PATH}:addOrderItem@8"))]);
}
