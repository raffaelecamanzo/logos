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
//!   not in scope;
//! - a binding types only the calls in its own scope (S-824, CR-256): a local
//!   `val` in a block that has closed, a nested `def`'s parameter and a typed
//!   lambda parameter type no call outside them, and a name a pattern,
//!   generator, untyped lambda, `catch`, `given` or self-type alias binds is
//!   refused inside its own construct alone.
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
    let result = engine.pipeline().index();
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
        .graph().status()
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
    engine_a.pipeline().sync(&changed);
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

// ── Scope-precise bindings (S-824, CR-256) ─────────────────────────────────

/// Two types of one method name, and a base class in its own file whose `svc`
/// is a `SLogger` — a member the class inheriting it never sees declared.
const SCOPED_DOMAIN_PATH: &str = "src/main/scala/shop/s/Shop.scala";
const SCOPED_DOMAIN: &str = "\
package shop.s

class SLogger { def send(): Unit = {} }
class SMailer { def send(): Unit = {} }
";

const SBASE_PATH: &str = "src/main/scala/shop/s/SBase.scala";
const SBASE: &str = "\
package shop.s

class SBase { val svc: SLogger = null }
";

const SC_PATH: &str = "src/main/scala/shop/s/SC.scala";
const SC: &str = "\
package shop.s

class SC extends SBase {
  def m(f: Boolean): Unit = { if (f) { val svc: SMailer = new SMailer(); () }; svc.send() }
  def n(f: Boolean): Unit = { if (f) { val svc: SMailer = new SMailer(); svc.send() } }
}
";

/// The status readout's Scala call residue, by reason, the empty ones dropped.
fn scala_residue(engine: &Engine) -> Vec<(CallResidueReason, u64)> {
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "scala")
        .and_then(|row| row.call_residue)
        .expect("the scala row states its call residue");
    let mut reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    reasons.sort_by_key(|(reason, _)| format!("{reason:?}"));
    reasons
}

/// A `val` in a block that has closed types no later call of its name (S-824):
/// `m`'s `svc` is the inherited `SLogger` member, which no binding in this
/// file states, so the call reads `no-receiver-evidence`. Before, the file-wide
/// reading typed it `SMailer` and bound `SC#m() → SMailer#send()`. The same
/// call inside the block still binds the local's type.
#[test]
fn a_val_in_a_closed_block_types_no_call_after_it() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (SBASE_PATH, SBASE), (SC_PATH, SC)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("{SC_PATH}:{name}@{line}");
    assert_eq!(
        rows(rt, SC_PATH, &["send"]),
        sorted(vec![
            row(&c(4, "m"), "send", RefForm::Method, OTHER, false),
            row(&c(5, "n"), "SMailer::send", RefForm::Path, OTHER, true),
        ])
    );
    let from_sc: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(SC_PATH)).collect();
    assert_eq!(from_sc, vec![edge(&c(5, "n"), &format!("{SCOPED_DOMAIN_PATH}:send@4"))]);
    assert_eq!(scala_residue(&engine), vec![(CallResidueReason::NoReceiverEvidence, 1)]);
}

/// A nested `def`'s parameter and a typed lambda parameter are in scope in
/// their own declaration alone (S-824): the call after each is on the
/// inherited `svc`, which no binding states. Before, the outermost callable
/// declaring `svc: SMailer` anywhere typed both.
const SN_PATH: &str = "src/main/scala/shop/s/SN.scala";
const SN: &str = "\
package shop.s

class SN extends SBase {
  def nested(): Unit = { def inner(svc: SMailer): Unit = svc.send(); svc.send() }
  def lambda(ms: List[SMailer]): Unit = { ms.foreach((svc: SMailer) => svc.send()); svc.send() }
}
";

#[test]
fn a_nested_defs_or_typed_lambdas_parameter_types_no_call_outside_it() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (SBASE_PATH, SBASE), (SN_PATH, SN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let s = |line: u32, name: &str| format!("{SN_PATH}:{name}@{line}");
    let mailer = |source: String| row(&source, "SMailer::send", RefForm::Path, OTHER, true);
    let untyped = |source: String| row(&source, "send", RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, SN_PATH, &["send"]),
        sorted(vec![
            // `inner`'s own parameter; after `inner`, `svc` is the inherited one.
            mailer(s(4, "inner")),
            untyped(s(4, "nested")),
            // The lambda's own parameter; after the lambda, the inherited one.
            mailer(s(5, "lambda")),
            untyped(s(5, "lambda")),
        ])
    );
}

/// Every name a pattern, a generator, an untyped lambda, a `catch`, a `given`,
/// a destructuring `val`, a `new T { … }` value or a self-type alias binds is
/// refused inside its own construct alone (S-824): each `cN` is called there,
/// and again in a sibling method whose typed parameter of that name types the
/// call. Before, each such name was refused in the whole file.
const REFUSED_PATH: &str = "src/main/scala/shop/s/Refused.scala";
const REFUSED: &str = "\
package shop.s

class Refused {
  def caseVar(v: Any): Unit = v match { case c1 => c1.send() }
  def ok1(c1: SMailer): Unit = c1.send()
  def generator(xs: List[Any]): Unit = for (c2 <- xs) c2.send()
  def ok2(c2: SMailer): Unit = c2.send()
  def lambda(xs: List[Any]): Unit = xs.foreach(c3 => c3.send())
  def ok3(c3: SMailer): Unit = c3.send()
  def catchVar(): Unit = try make() catch case c4 => c4.send()
  def ok4(c4: SMailer): Unit = c4.send()
  def given(): Unit = { given c5: SMailer = null; c5.send() }
  def ok5(c5: SMailer): Unit = c5.send()
  def tuple(): Unit = { val (c6, n) = pair(); c6.send() }
  def ok6(c6: SMailer): Unit = c6.send()
  def anon(): Unit = { val c7 = new SMailer { }; c7.send() }
  def ok7(c7: SMailer): Unit = c7.send()
  def typedCase(v: Any): Unit = v match { case c8: SMailer => c8.send() }
  def ok8(c8: SMailer): Unit = c8.send()
  def make(): Any = null
  def pair(): (Any, Int) = (null, 0)
}

class Aliased { c9 => def go(): Unit = c9.send() }
class Typed9 { def ok9(c9: SMailer): Unit = c9.send() }
";

#[test]
fn a_name_a_pattern_binds_is_refused_only_inside_its_construct() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (REFUSED_PATH, REFUSED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let r = |line: u32, name: &str| format!("{REFUSED_PATH}:{name}@{line}");
    let mailer = |source: String| row(&source, "SMailer::send", RefForm::Path, OTHER, true);
    let untyped = |source: String| row(&source, "send", RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, REFUSED_PATH, &["send"]),
        sorted(vec![
            untyped(r(4, "caseVar")),
            mailer(r(5, "ok1")),
            untyped(r(6, "generator")),
            mailer(r(7, "ok2")),
            untyped(r(8, "lambda")),
            mailer(r(9, "ok3")),
            untyped(r(10, "catchVar")),
            mailer(r(11, "ok4")),
            untyped(r(12, "given")),
            mailer(r(13, "ok5")),
            untyped(r(14, "tuple")),
            mailer(r(15, "ok6")),
            untyped(r(16, "anon")),
            mailer(r(17, "ok7")),
            // A typed `case` pattern proves nothing (a stated ceiling).
            untyped(r(18, "typedCase")),
            mailer(r(19, "ok8")),
            untyped(r(24, "go")),
            mailer(r(25, "ok9")),
        ])
    );
}

/// The forms of `new` a local is initialised with: `new T` and `new T()`
/// prove `T`; `new T() { … }` builds an anonymous subclass and `new T(a)(b)`
/// is a curried constructor, which prove nothing (a stated ceiling).
const NEWS_PATH: &str = "src/main/scala/shop/s/News.scala";
const NEWS: &str = "\
package shop.s

class News {
  def bare(): Unit = { val a = new SMailer; a.send() }
  def applied(): Unit = { var b = new SMailer(); b.send() }
  def anon(): Unit = { val c = new SMailer() { }; c.send() }
  def curried(): Unit = { val d = new SMailer(1)(2); d.send() }
}
";

#[test]
fn a_new_instance_proves_its_class_unless_it_has_a_body_or_two_argument_lists() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (NEWS_PATH, NEWS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let n = |line: u32, name: &str| format!("{NEWS_PATH}:{name}@{line}");
    assert_eq!(
        rows(rt, NEWS_PATH, &["send"]),
        sorted(vec![
            row(&n(4, "bare"), "SMailer::send", RefForm::Path, OTHER, true),
            row(&n(5, "applied"), "SMailer::send", RefForm::Path, OTHER, true),
            row(&n(6, "anon"), "send", RefForm::Method, OTHER, false),
            row(&n(7, "curried"), "send", RefForm::Method, OTHER, false),
        ])
    );
}

/// A call moved into the block that declares its receiver binds on sync, and
/// the synced graph equals a fresh index of the edited tree (NFR-RA-06).
#[test]
fn a_synced_move_into_the_locals_block_matches_a_fresh_reindex() {
    let initial = [(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (SBASE_PATH, SBASE), (SC_PATH, SC)];
    let mailer_send = format!("{SCOPED_DOMAIN_PATH}:send@4");
    // Before the edit, only `n`'s call is inside the local's block.
    let before = tree(&initial);
    let engine = index(before.path());
    let from_sc: Edges = calls(engine.runtime().unwrap()).into_iter().filter(|(from, _)| from.starts_with(SC_PATH)).collect();
    assert_eq!(from_sc, vec![edge(&format!("{SC_PATH}:n@5"), &mailer_send)]);

    let moved = SC.replace("new SMailer(); () }; svc.send() }", "new SMailer(); svc.send() } }");
    assert_ne!(moved, SC, "the edit moves the call");
    let edges = synced_equals_reindexed(&initial, &[(SC_PATH, &moved)]);
    let from_sc: Edges = edges.into_iter().filter(|(from, _)| from.starts_with(SC_PATH)).collect();
    assert_eq!(
        from_sc,
        vec![edge(&format!("{SC_PATH}:m@4"), &mailer_send), edge(&format!("{SC_PATH}:n@5"), &mailer_send)]
    );
}

/// A member `val` is in scope in its own type's members alone (S-824's
/// `.member` scope, a stated ceiling): `own` types its call, while a nested
/// class's method — which Scala lets read the outer member — and a top-level
/// `def` reading a top-level `val` type none, and neither does a call on a
/// lowercase `object` the file declares, which no binding names. Before, the
/// outer-class walk typed the nested class's call and the `name` marker's
/// static-name reading typed `util.send()`; at ox and gitbucket no edge
/// depended on either.
const MEMBERS_PATH: &str = "src/main/scala/shop/s/Members.scala";
const MEMBERS: &str = "\
package shop.s

val top: SMailer = null
def topLevel(): Unit = top.send()

class Outer {
  private val svc: SMailer = null
  def own(): Unit = svc.send()
  class Inner { def go(): Unit = svc.send() }
  def viaObject(): Unit = util.send()
}

object util { def send(): Unit = {} }
";

#[test]
fn a_member_is_in_scope_in_its_own_types_members_alone() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (MEMBERS_PATH, MEMBERS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let m = |line: u32, name: &str| format!("{MEMBERS_PATH}:{name}@{line}");
    assert_eq!(
        rows(rt, MEMBERS_PATH, &["send"]),
        sorted(vec![
            row(&m(4, "topLevel"), "send", RefForm::Method, OTHER, false),
            row(&m(8, "own"), "SMailer::send", RefForm::Path, OTHER, true),
            row(&m(9, "go"), "send", RefForm::Method, OTHER, false),
            row(&m(10, "viaObject"), "send", RefForm::Method, OTHER, false),
        ])
    );
}

/// A declaration no proof types still hides an outer binding of its name
/// (S-824 review): a local `object` is the receiver of the call in its block,
/// a `given … with` body's own `val` is a second binding of the name, and so
/// is an anonymous or local class's member `def` or `object` in its body, so
/// the enclosing parameter or local is not the one binding in scope there.
const HIDDEN_PATH: &str = "src/main/scala/shop/s/Hidden.scala";
const HIDDEN: &str = "\
package shop.s

class I(svc: SMailer) {
  def localObject(): Unit = { object svc { def send(): Unit = {} }; svc.send() }
  def outside(): Unit = svc.send()
  given Ordering[Int] with
    val svc: SLogger = null
    def compare(a: Int, b: Int): Int = { svc.send(); 0 }
}

class E {
  def anonDef(svc: SMailer): Runnable = new Runnable { def svc: SLogger = null; def run(): Unit = svc.send() }
  def localDef(svc: SMailer): Unit = { class L { def svc(): SLogger = null; def go(): Unit = svc.send() }; () }
  def localObj(): Unit = { val svc: SMailer = null; class L { object svc { def send(): Unit = {} }; def go(): Unit = svc.send() }; svc.send() }
}
";

#[test]
fn a_declaration_no_proof_types_hides_an_outer_binding_of_its_name() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (HIDDEN_PATH, HIDDEN)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("{HIDDEN_PATH}:{name}@{line}");
    let typed: Vec<Row> =
        rows(rt, HIDDEN_PATH, &["send"]).into_iter().filter(|r| r.2 == RefForm::Path).collect();
    // Only the calls outside every hiding declaration are typed by `svc: SMailer`.
    assert_eq!(
        typed,
        vec![
            row(&h(14, "localObj"), "SMailer::send", RefForm::Path, OTHER, true),
            row(&h(5, "outside"), "SMailer::send", RefForm::Path, OTHER, true),
        ]
    );
}

/// A nearer binding with no proof of its own — or another type's — refuses
/// the call an outer `SLogger` binding of its name would type (S-824 review):
/// an extension's or `given`'s parameter, a `new T { … }` body's `val`, `var`
/// or `given`, a brace-less or `case`-body local, a local `def`, a Scala 3
/// colon lambda's parameter and a self-type alias. Each is the binding whose
/// pattern alone keeps the outer one from being the one in scope.
const NEARER_PATH: &str = "src/main/scala/shop/s/Nearer.scala";
const NEARER: &str = "\
package shop.s

trait Tr { def send(): Unit }

object Ext { private val svc: SLogger = null; extension (svc: SMailer) def go(): Unit = svc.send() }
object Giv { private val svc: SLogger = null; given ord(using svc: SMailer): Tr = { svc.send(); null } }

class Anon {
  def f1(a1: SLogger): Any = new Tr { val a1 = make(); def send(): Unit = a1.send() }
  def f2(a2: SLogger): Any = new Tr { var a2 = make(); def send(): Unit = a2.send() }
  def f3(a3: SLogger): Any = new Tr { given a3: Tr = null; def send(): Unit = a3.send() }
  def make(): Any = null
}

class Ib(i1: SLogger, i2: SLogger, i3: SLogger) {
  def f(): Unit =
    val i1 = make()
    i1.send()
  def g(): Unit =
    var i2 = make()
    i2.send()
  def h(): Unit =
    given i3: Tr = null
    i3.send()
  def make(): Any = null
}

class Cc(k1: SLogger, k2: SLogger, k3: SLogger) {
  def f(v: Any): Unit = v match { case _ => val k1 = make(); k1.send() }
  def g(v: Any): Unit = v match { case _ => var k2 = make(); k2.send() }
  def h(v: Any): Unit = v match { case _ => given k3: Tr = null; k3.send() }
  def make(): Any = null
}

class LocalDef(d1: SLogger, d2: SLogger) {
  def f(): Unit = { def d1: Any = null; d1.send() }
  def g(): Unit =
    def d2: Any = null
    d2.send()
}

class Colon(co: SLogger) {
  def f(xs: List[Any]): Unit = xs.foreach: co =>
    co.send()
}

class SelfAlias {
  def m(sf: SLogger): Unit = { class In { sf => def go(): Unit = sf.send() }; () }
}
";

#[test]
fn a_nearer_binding_refuses_the_call_an_outer_one_would_type() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (NEARER_PATH, NEARER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let n = |line: u32, name: &str| format!("{NEARER_PATH}:{name}@{line}");
    let untyped = |source: String| row(&source, "send", RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, NEARER_PATH, &["send"]),
        sorted(vec![
            untyped(n(5, "go")),
            // A `given` with a parameter list is no declaration: its object's.
            untyped(n(6, "Giv")),
            untyped(n(9, "send")),
            untyped(n(10, "send")),
            untyped(n(11, "send")),
            untyped(n(16, "f")),
            untyped(n(19, "g")),
            untyped(n(22, "h")),
            untyped(n(29, "f")),
            untyped(n(30, "g")),
            untyped(n(31, "h")),
            untyped(n(36, "f")),
            untyped(n(37, "g")),
            untyped(n(43, "f")),
            untyped(n(48, "go")),
        ])
    );
}

/// Every member, class-parameter and proof form a typed call reads, beyond
/// the ones the main fixtures write (S-824 review): a trait's and an enum's
/// class parameter; a class `var`; an object's, a trait's and an enum's
/// `val` and `var`; a generic class parameter, lambda parameter, `val` and
/// `var` (`Box[Int]` reads `Box`); and `var x = new T`.
const FORMS_PATH: &str = "src/main/scala/shop/s/Forms.scala";
const FORMS: &str = "\
package shop.s

class Box[A] { def send(): Unit = {} }

trait TP(tp: SMailer) { def go(): Unit = tp.send() }
enum EP(ep: SMailer) { case A extends EP(null); def go(): Unit = ep.send() }
class CV { var cv: SMailer = null; def go(): Unit = cv.send() }
object OV { val ov: SMailer = null; def go(): Unit = ov.send() }
object OW { var ow: SMailer = null; def go(): Unit = ow.send() }
trait TV { val tv: SMailer = null; def go(): Unit = tv.send() }
trait TW { var tw: SMailer = null; def go(): Unit = tw.send() }
enum EV { case B; val ev: SMailer = null; def go(): Unit = ev.send() }
enum EW { case C; var ew: SMailer = null; def go(): Unit = ew.send() }

class Gen(cb: Box[Int]) {
  def a(): Unit = cb.send()
  def b(xs: List[Box[Int]]): Unit = xs.foreach((lb: Box[Int]) => lb.send())
  def c(): Unit = { val vb: Box[Int] = null; vb.send() }
  def d(): Unit = { var wb: Box[Int] = null; wb.send() }
  def e(): Unit = { var nb = new SMailer; nb.send() }
}
";

#[test]
fn every_member_parameter_and_proof_form_types_its_call() {
    let tmp = tree(&[(SCOPED_DOMAIN_PATH, SCOPED_DOMAIN), (FORMS_PATH, FORMS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("{FORMS_PATH}:{name}@{line}");
    let mailer = |source: String| row(&source, "SMailer::send", RefForm::Path, OTHER, true);
    let boxed = |source: String| row(&source, "Box::send", RefForm::Path, OTHER, true);
    assert_eq!(
        rows(rt, FORMS_PATH, &["send"]),
        sorted(vec![
            mailer(f(5, "go")),
            mailer(f(6, "go")),
            mailer(f(7, "go")),
            mailer(f(8, "go")),
            mailer(f(9, "go")),
            mailer(f(10, "go")),
            mailer(f(11, "go")),
            mailer(f(12, "go")),
            mailer(f(13, "go")),
            boxed(f(16, "a")),
            boxed(f(17, "b")),
            boxed(f(18, "c")),
            boxed(f(19, "d")),
            mailer(f(20, "e")),
        ])
    );
}
