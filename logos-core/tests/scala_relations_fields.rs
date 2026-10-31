//! Scala class parameters, relations and field access are captured, and bodied
//! traits count for Cohesion (S-575, CR-183, FR-EX-28, FR-RS-15, FR-QM-11,
//! NFR-RA-05, NFR-RA-06) — exercised end to end through the public [`Engine`]
//! façade against the SHIPPED Scala `references.scm`, `symbols.scm` and
//! `plugin.toml`, never a query override.
//!
//! The fixtures follow the two inspection corpora's dominant shapes:
//! gitbucket's Scala 2 cake pattern (`class AccountController extends
//! AccountControllerBase with AccountService with …`, every `with` a trait), and
//! ox's Scala 3 `case class FixedWindow(rate: Int, per: FiniteDuration) extends
//! RateLimiterAlgorithm:`, whose methods share its fields through bare reads.
//!
//! What the fixtures pin:
//!
//! - every entry of an `extends … with …` clause binds the one in-repository
//!   type it names, and its edge kind follows that type: `Extends` to a class,
//!   `Implements` to a trait. A library supertype and a companion pair stay
//!   unbound; a self-type (`self: A with B =>`) records no relation;
//! - a class's, case class's and trait's constructor parameters are `Field`s;
//! - a bare name a method reads in a value position, which its class declares
//!   as a field and the method does not declare, records `Accesses` and binds to
//!   that field; a parameter, local, lambda or pattern variable of the name hides
//!   it, and a named argument's label, another object's member, a method and an
//!   undeclared name record nothing;
//! - Scala declares `member_access`, so its classes are scored for Cohesion, and
//!   a trait with a bodied method counts as a container.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
#![cfg(feature = "lang-scala")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::{Engine, Granularity, Runtime};
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

/// Every bound edge of `kind`, as `source name -> target name:kind`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let (s, t) = (&nodes[&e.source], &nodes[&e.target]);
                format!("{} -> {}:{}", s.name, t.name, t.kind.as_str())
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's unbound rows of `kind`, as their targets, sorted and
/// deduplicated.
fn unbound(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    let mut rows: Vec<String> = rt
        .submit_read(move |store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == kind && r.form != RefForm::Symbol && !r.resolved)
                .map(|r| r.target)
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows.dedup();
    rows
}

/// Every `Accesses` ledger row, bound or not, as `(method name, field name)`.
fn access_rows(rt: &Runtime) -> Vec<(String, String)> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), n.name))
                .collect())
        })
        .expect("read runs");
    let mut rows: Vec<(String, String)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Accesses)
        .filter_map(|r| Some((by_symbol.get(&r.source_symbol)?.clone(), r.target)))
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// The names of the nodes of `kind` the graph holds, sorted.
fn names_of(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    let mut out: Vec<String> = rt
        .submit_read(move |store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == kind)
                .map(|n| n.name)
                .collect())
        })
        .expect("read runs");
    out.sort();
    out
}

fn strings(items: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    out.sort();
    out
}

// ── Relations: gitbucket's cake pattern ──────────────────────────────────────

const CONTROLLER_BASE: (&str, &str) = (
    "src/main/scala/app/controller/ControllerBase.scala",
    "\
package app.controller

abstract class ControllerBase {
  def start(): Unit = ()
}
",
);

const SERVICES: (&str, &str) = (
    "src/main/scala/app/service/AccountService.scala",
    "\
package app.service

trait AccountService {
  def getAccount(name: String): String = name
}

object AccountService {
  val Admin = \"admin\"
}

trait RepositoryService {
  def getRepository(): Unit = ()
}

trait Paged[A] {
  def page(a: A): A = a
}

class Pair

object Pair
",
);

const ACCOUNT_CONTROLLER: (&str, &str) = (
    "src/main/scala/app/controller/AccountController.scala",
    "\
package app.controller

import app.service._

class AccountController
    extends AccountControllerBase
    with AccountService
    with RepositoryService
    with Paged[Int]

trait AccountControllerBase extends ControllerBase {
  self: AccountService with RepositoryService =>
  def show(): Unit = start()
}

class Library extends munit.FunSuite with Serializable

class Sub extends ControllerBase {
  def go(): Unit = super.start()
}

class UsesPair extends Pair
",
);

/// FR-EX-28 AC (gitbucket): the cake pattern's `with` mixins record
/// `Implements` — `AccountService` too, which, as gitbucket's services do, has
/// a companion `object`: an object is never a supertype — and a trait or class
/// extending the abstract `ControllerBase` records `Extends`. Each entry's kind
/// follows the type it binds, through the wildcard import or the declared
/// package. A self-type records no relation.
#[test]
fn cake_pattern_mixins_implement_their_traits_and_a_base_class_is_extended() {
    let tmp = tree(&[CONTROLLER_BASE, SERVICES, ACCOUNT_CONTROLLER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "AccountControllerBase -> ControllerBase:class",
            "Sub -> ControllerBase:class",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "AccountController -> AccountControllerBase:trait",
            "AccountController -> AccountService:trait",
            "AccountController -> Paged:trait",
            "AccountController -> RepositoryService:trait",
        ])
    );
}

/// NFR-RA-05: a library supertype (`munit.FunSuite`, `Serializable`) binds
/// nothing, and a name a class and its companion `object` share (`Pair`) names
/// two classes the graph cannot tell apart, so `extends Pair` stays unbound
/// rather than guessed. A trait's companion (`object AccountService`) is no
/// trait, so `with AccountService` binds the trait (above).
#[test]
fn a_library_supertype_and_a_companion_pair_stay_unbound() {
    let tmp = tree(&[CONTROLLER_BASE, SERVICES, ACCOUNT_CONTROLLER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        unbound(rt, EdgeKind::Extends),
        strings(&["Pair", "Serializable", "munit::FunSuite"])
    );
    assert!(unbound(rt, EdgeKind::Implements).is_empty());
}

/// A proven `Extends` is what a call on the current instance climbs: the
/// trait's bare `start()` and the subclass's `super.start()` reach the base
/// class's `start`.
#[test]
fn a_call_on_the_current_instance_climbs_a_proven_base() {
    let tmp = tree(&[CONTROLLER_BASE, SERVICES, ACCOUNT_CONTROLLER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&["go -> start:method", "show -> start:method"])
    );
}

/// FR-EX-28 (review): a trait beside its companion `object` binds as a
/// supertype wherever the name is read — the same file, a scope nested in an
/// object, a member path (`M.S`) and a member wildcard (`import p.M._`) — as it
/// does through a package import above.
#[test]
fn a_trait_with_a_companion_binds_on_every_rung_a_supertype_reads() {
    let tmp = tree(&[
        (
            "src/main/scala/p/A.scala",
            "\
package p

trait Logging { def log(): Unit = () }
object Logging { val x = 1 }
class Svc extends Logging

object Registry {
  trait Component { def c(): Int = 1 }
  object Component { val x = 1 }
  class Impl extends Component
}

object M {
  trait S { def s(): Unit = () }
  object S { val x = 1 }
}

class X extends M.S
",
        ),
        (
            "src/main/scala/q/B.scala",
            "package q\n\nimport p.M._\n\nclass Y extends S\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        strings(&["Impl -> Component:trait", "Svc -> Logging:trait", "X -> S:trait", "Y -> S:trait"])
    );
}

/// FR-EX-28 (review): an object's, an enum's and a trait's supertypes record
/// the relation too, a generic one (`G[Int]`) by its head type.
#[test]
fn objects_enums_and_generic_supertypes_relate() {
    let tmp = tree(&[
        (
            "src/main/scala/app/svc/T1.scala",
            "package app.svc\n\ntrait T1 { def t(): Unit = () }\ntrait G[A] { def g(): Unit = () }\n",
        ),
        (
            "src/main/scala/app/svc/Uses.scala",
            "\
package app.svc

object O1 extends T1
object O2 extends G[Int]
trait T2 extends G[Int]
enum E1 extends T1 { case X }
enum E2 extends G[Int] { case Y }
",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        strings(&["E1 -> T1:trait", "E2 -> G:trait", "O1 -> T1:trait", "O2 -> G:trait", "T2 -> G:trait"])
    );
}

/// FR-EX-28 (review): a trait with a companion `object` that a class of its own
/// package extends binds through the package — the same-package rung, no
/// import — and leaves no supertype row unbound.
#[test]
fn a_same_package_trait_with_a_companion_binds() {
    let tmp = tree(&[
        (
            "src/main/scala/app/svc/Audited.scala",
            "package app.svc\n\ntrait Audited { def audit(): Unit = () }\nobject Audited { val Name = \"a\" }\n",
        ),
        ("src/main/scala/app/svc/Page.scala", "package app.svc\n\nclass Page extends Audited\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::Implements), strings(&["Page -> Audited:trait"]));
    assert!(unbound(rt, EdgeKind::Extends).is_empty(), "{:?}", unbound(rt, EdgeKind::Extends));
}

// ── Class parameters are Fields ──────────────────────────────────────────────

/// FR-EX-28 AC: a class's, a case class's and a trait's constructor parameters
/// are `Field`s of it — `val`, `var`, `private` and plain alike, every
/// parameter list — and a method's parameter is not.
#[test]
fn class_parameters_are_fields() {
    let tmp = tree(&[(
        "src/main/scala/app/model/Model.scala",
        "\
package app.model

class Account(val name: String, var mail: String, private val id: Int, plain: Int)(implicit ctx: Ctx)

case class Point(x: Int, y: Int)

trait Named(label: String) {
  def show(prefix: String): String = prefix
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        names_of(rt, NodeKind::Field),
        strings(&["ctx", "id", "label", "mail", "name", "plain", "x", "y"])
    );
}

// ── Bare field access: ox's FixedWindow ──────────────────────────────────────

const RATE_LIMITER: (&str, &str) = (
    "core/src/main/scala/ox/resilience/StartTimeRateLimiterAlgorithm.scala",
    "\
package ox.resilience

import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.Semaphore

trait RateLimiterAlgorithm:
  def acquire(permits: Int): Unit
  def update(): Unit

object StartTimeRateLimiterAlgorithm:
  case class FixedWindow(rate: Int, per: FiniteDuration) extends RateLimiterAlgorithm:
    private val lastUpdate = new AtomicLong(System.nanoTime())
    private val semaphore = new Semaphore(rate)

    def acquire(permits: Int): Unit =
      semaphore.acquire(permits)

    def tryAcquire(permits: Int): Boolean =
      semaphore.tryAcquire(permits)

    def getNextUpdate: Long =
      val waitTime = lastUpdate.get() + per.toNanos - System.nanoTime()
      if waitTime > 0 then waitTime else 0L

    def update(): Unit =
      val now = System.nanoTime()
      lastUpdate.set(now)
      semaphore.release(rate - semaphore.availablePermits())
    end update

    def runOperation[T](operation: => T, permits: Int): T = operation

  end FixedWindow
",
);

/// FR-EX-28: a bare read of a field the class declares — a class parameter or
/// a member `val` — records `Accesses` and binds to that field: as the receiver
/// of a call (`semaphore.acquire(…)`) or a selection (`per.toNanos`), and as an
/// operand (`rate - …`). A local (`waitTime`, `now`) and a parameter
/// (`operation`, `permits`) are not fields and record nothing.
#[test]
fn fixed_window_reads_its_fields_bare() {
    let tmp = tree(&[RATE_LIMITER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Accesses),
        strings(&[
            "acquire -> semaphore:field",
            "getNextUpdate -> lastUpdate:field",
            "getNextUpdate -> per:field",
            "tryAcquire -> semaphore:field",
            "update -> lastUpdate:field",
            "update -> rate:field",
            "update -> semaphore:field",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&["FixedWindow -> RateLimiterAlgorithm:trait"])
    );
}

const COUNTER: (&str, &str) = (
    "src/main/scala/app/model/Counter.scala",
    "\
package app.model

class Counter(val start: Int, step: Int) {
  private var count = start
  val label: String = \"c\"
  val items: Array[Int] = Array(1)

  def next(): Int = { count = count + step; count }
  def show = s\"$label ${count}\"
  def first(): Int = items(0)
  def cond(): Int = if (count > 0) step else 0
  def matcher(): Int = count match { case _ => 1 }
  def pair(): (Int, Int) = (start, -step)
  def body = label

  def shadowParam(count: Int): Int = count + 1
  def shadowLocal(): String = { val label = \"x\"; label }
  def shadowLambda(): Seq[Int] = Seq(1).map(step => step + 1)
  def shadowCase(o: Option[Int]): Int = o match { case Some(count) => count; case None => 0 }
  def shadowFor(): Seq[Int] = for (step <- Seq(1)) yield step

  def named(): Int = make(start = 2)
  def make(start: Int): Int = 0
  def other(o: Counter): Int = o.count
  def method(): Int = next()
  def unknown(): Int = missing
}
",
);

/// FR-EX-28: a bare read in each value position real code writes one in — an
/// assignment's both sides, an operand, the last expression of a block, an
/// interpolation, an applied field (`items(0)`), a condition and its branches,
/// a scrutinee, a tuple, a unary operand and an expression body — is one
/// `Accesses` row of the method.
#[test]
fn a_bare_read_is_an_access_in_each_value_position() {
    let tmp = tree(&[COUNTER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Accesses),
        strings(&[
            "body -> label:field",
            "cond -> count:field",
            "cond -> step:field",
            "first -> items:field",
            "matcher -> count:field",
            "next -> count:field",
            "next -> step:field",
            "pair -> start:field",
            "pair -> step:field",
            "show -> count:field",
            "show -> label:field",
        ])
    );
}

/// NFR-RA-05: a parameter, a local, a lambda parameter, a pattern variable or a
/// `for` generator of the field's name hides it in that method; a named
/// argument's label, another object's member, a method and a name the class
/// does not declare record no access at all.
#[test]
fn a_shadowing_name_and_the_near_misses_record_nothing() {
    let tmp = tree(&[COUNTER]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let rows = access_rows(rt);
    for method in ["shadowParam", "shadowLocal", "shadowLambda", "shadowCase", "shadowFor", "named", "make", "other", "method", "unknown"] {
        assert!(
            rows.iter().all(|(m, _)| m != method),
            "{method} records an access: {rows:?}"
        );
    }
}

/// NFR-RA-05 (review): a local `def`, `object` or `class` the method declares
/// under a field's name is what a bare read of the name reads, so the field
/// records no access there — while a sibling method's read of it still does.
#[test]
fn a_local_definition_of_the_fields_name_hides_the_field() {
    let tmp = tree(&[(
        "src/main/scala/p/A.scala",
        "\
package p

class A(rate: Int) {
  def f(): Int = { def rate: Int = 5; rate + 1 }
  def g(): Int = { def rate(): Int = 5; rate() }
  def h(): Int = { object rate { val x = 1 }; rate.x }
  def k(): Int = { class rate { val y = 1 }; rate }
  def read(): Int = rate
}
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(edges(engine.runtime().unwrap(), EdgeKind::Accesses), strings(&["read -> rate:field"]));
}

/// NFR-RA-05 (review): each name of a multi-name local (`val a, rate = 0`,
/// `var b, step = 0`) hides the field of its name.
#[test]
fn each_name_of_a_multi_name_local_hides_the_field() {
    let tmp = tree(&[(
        "src/main/scala/p/A.scala",
        "\
package p

class A(rate: Int, step: Int) {
  def f(): Int = { val a, rate = 0; rate + 1 }
  def g(): Int = { var b, step = 0; step }
  def read(): Int = rate + step
}
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Accesses),
        strings(&["read -> rate:field", "read -> step:field"])
    );
}

/// NFR-RA-05 (review): inside a class, an `extension (a: Int)`'s parameter is
/// the receiver its methods read, never the class's field `a`; a method outside
/// the extension still reads the field.
#[test]
fn an_extension_parameter_hides_the_field_inside_its_methods() {
    let tmp = tree(&[(
        "src/main/scala/p/A.scala",
        "\
package p

class A(a: Int) {
  extension (a: Int) def twice: Int = a * 2
  extension (b: Int)(using c: Int) def plus: Int = a + b
  def read(): Int = a
}
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Accesses),
        strings(&["plus -> a:field", "read -> a:field"])
    );
}

const POSITIONS: (&str, &str) = (
    "src/main/scala/app/model/Positions.scala",
    "\
package app.model

class P(var a: Int, var b: Int, var c: Int, flag: Boolean, xs: Seq[Int]) {
  def pArg(): Int = make(a)
  def pParen(): Int = (b)
  def pAscr(): Any = a: Int
  def pAssignRight(): Unit = { var z = 0; z = b }
  def pReturn(): Int = return a
  def pVal(): Int = { val q = a; q }
  def pVar(): Int = { var q = b; q }
  def pIfAlt(): Int = if (c > 0) 1 else a
  def pWhileParen(): Unit = while (flag) {}
  def pCase(): Int = 1 match { case _ => a }
  def pLambda(): Seq[Int] = Seq(1).map(x => a)
  def pForBody(): Seq[Int] = for (x <- Seq(1)) yield b
  def pEnum(): Seq[Int] = for (x <- xs) yield x
  def pAssignBody(): Unit = c = 1
  def pAssignBlock(): Unit = { c = 2 }
  def make(n: Int): Int = n

  def sVar(): Int = { var a = 1; a }
  def sTuple(): Int = { val (a, b) = (1, 2); a + b }
  def sCaseId(o: Int): Int = o match { case a => a }
  def sTyped(o: Any): Any = o match { case a: Int => a }
  def sCapture(o: Option[Int]): Any = o match { case a @ Some(_) => a }
  def sInfix(l: List[Int]): Int = l match { case a :: b => a; case _ => 0 }
  def sGiven(): Int = { given a: Int = 1; a }
  def sBinding(): Seq[Int] = Seq(1).map((a: Int) => a)
  def sRepeat(l: Seq[Int]): Any = l match { case Seq(a*) => a }
}

class Q(var a: Int, var c: Int, flag: Boolean):
  def qIndent(): Int =
    val z = 1
    a
  def qIndentAssign(): Unit =
    val z = 1
    c = 2
  def qIfCond(): Int = if flag then 1 else 2
  def qWhileCond(): Unit = while flag do ()
",
);

/// FR-EX-28 (review): one method per further value position — an argument, a
/// parenthesised or ascribed name, an assignment's source and a statement-level
/// target (in a block, a `def` body and an indented block), a `return`, a local's
/// initialiser, an `if` condition and alternative, a `while` condition (brace and
/// Scala 3 forms), a `case` body, a lambda body, a `for` body and generator
/// source, and an indented block's result — records one `Accesses` edge.
#[test]
fn a_bare_read_is_an_access_in_every_further_value_position() {
    let tmp = tree(&[POSITIONS]);
    let engine = index(tmp.path());
    let got = edges(engine.runtime().unwrap(), EdgeKind::Accesses);
    let positions: Vec<String> = got.iter().filter(|e| e.starts_with('p') || e.starts_with('q')).cloned().collect();
    assert_eq!(
        positions,
        strings(&[
            "pArg -> a:field",
            "pAscr -> a:field",
            "pAssignBlock -> c:field",
            "pAssignBody -> c:field",
            "pAssignRight -> b:field",
            "pCase -> a:field",
            "pEnum -> xs:field",
            "pForBody -> b:field",
            "pIfAlt -> a:field",
            "pIfAlt -> c:field",
            "pLambda -> a:field",
            "pParen -> b:field",
            "pReturn -> a:field",
            "pVal -> a:field",
            "pVar -> b:field",
            "pWhileParen -> flag:field",
            "qIfCond -> flag:field",
            "qIndent -> a:field",
            "qIndentAssign -> c:field",
            "qWhileCond -> flag:field",
        ])
    );
}

/// NFR-RA-05 (review): each further form that binds a name in a method — a
/// local `var`, a tuple, an identifier, typed, `@`-capture, infix and repeated
/// pattern, a named `given` and a typed lambda parameter — hides the field of
/// its name there.
#[test]
fn every_further_binding_form_hides_the_field() {
    let tmp = tree(&[POSITIONS]);
    let engine = index(tmp.path());
    let rows = access_rows(engine.runtime().unwrap());
    let shadowed: Vec<&(String, String)> = rows.iter().filter(|(m, _)| m.starts_with('s')).collect();
    assert!(shadowed.is_empty(), "{shadowed:?}");
}

// ── Cohesion: Scala declares member_access; bodied traits are containers ─────

fn snapshot(tmp: &TempDir) -> RecordedSnapshot {
    let engine = index(tmp.path());
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    let member_access = engine
        .registry()
        .expect("a started engine holds its registry")
        .member_access_extensions();
    metrics::snapshot(
        rt,
        &view,
        None,
        metrics::Thresholds::default(),
        &member_access,
    )
    .expect("snapshot runs")
}

/// FR-EX-28 AC (ox): `FixedWindow`'s LCOM4 is 2 — its four methods that touch
/// `semaphore`, `lastUpdate`, `per` and `rate` are one component, and
/// `runOperation`, which reads only its own parameter, is the other. Before
/// S-575 no bare read was captured and Scala declared no `member_access`: its
/// five methods were five components, and the class was not scored.
#[test]
fn fixed_window_is_scored_for_cohesion_at_lcom4_two() {
    let tmp = tree(&[RATE_LIMITER]);
    let s = snapshot(&tmp);
    let lcom4: Vec<(&str, &str)> = s
        .worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.detail.as_str()))
        .collect();
    assert_eq!(lcom4, [("FixedWindow", "LCOM4 2")]);
}

/// FR-QM-11 (CR-183): a Scala trait with a bodied method is a Cohesion
/// container — five such traits, each with two methods sharing no field, are
/// scored at LCOM4 2 each — while a trait of abstract declarations only is no
/// container at all.
#[test]
fn a_bodied_trait_is_scored_and_an_abstract_one_is_no_container() {
    let mut files: Vec<(String, String)> = (0..5)
        .map(|i| {
            (
                format!("src/main/scala/app/Svc{i}.scala"),
                format!("package app\n\ntrait Svc{i} {{\n  val a = 1\n  val b = 2\n  def one(): Int = a\n  def two(): Int = b\n}}\n"),
            )
        })
        .collect();
    files.push((
        "src/main/scala/app/Port.scala".to_string(),
        "package app\n\ntrait Port {\n  val a = 1\n  def one(): Int\n  def two(): Int\n}\n".to_string(),
    ));
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let tmp = tree(&refs);
    let s = snapshot(&tmp);
    let cohesion = s.metrics.cohesion.expect("five bodied traits: Cohesion is scored, not n/a");
    assert_eq!(cohesion.raw, 0.5, "each trait is LCOM4 2; the abstract Port enters no mean");
    let mut lcom4: Vec<(&str, &str)> = s
        .worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.detail.as_str()))
        .collect();
    lcom4.sort();
    assert_eq!(
        lcom4,
        [
            ("Svc0", "LCOM4 2"),
            ("Svc1", "LCOM4 2"),
            ("Svc2", "LCOM4 2"),
            ("Svc3", "LCOM4 2"),
            ("Svc4", "LCOM4 2"),
        ]
    );
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// NFR-RA-06: a sync that adds a field read, a mixin and a class parameter
/// leaves the graph a cold index of the edited tree builds.
#[test]
fn sync_matches_a_cold_reindex() {
    let before = "\
package app.model

trait Audited {
  def audit(): Unit = ()
}

class Box(size: Int) {
  def get(): Int = 0
}
";
    let after = "\
package app.model

trait Audited {
  def audit(): Unit = ()
}

class Box(size: Int, weight: Int) extends Audited {
  def get(): Int = size + weight
}
";
    let rel = "src/main/scala/app/model/Box.scala";
    let tmp = tree(&[(rel, before)]);
    let engine = index(tmp.path());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    write(tmp.path(), rel, after);
    engine.sync(&[root.join(rel)] as &[PathBuf]);
    let synced = graph_fingerprint(engine.runtime().unwrap());

    let cold = tree(&[(rel, after)]);
    let fresh = index(cold.path());
    assert_eq!(synced, graph_fingerprint(fresh.runtime().unwrap()));
    assert_eq!(
        edges(fresh.runtime().unwrap(), EdgeKind::Accesses),
        strings(&["get -> size:field", "get -> weight:field"])
    );
}

/// NFR-RA-06 across files: a mixin whose trait a later sync adds binds then,
/// as a cold index of the final tree binds it.
#[test]
fn a_supertype_added_in_another_file_binds_on_sync_as_on_a_cold_index() {
    let sub = (
        "src/main/scala/app/web/Page.scala",
        "package app.web\n\nimport app.svc.Audited\n\nclass Page extends Base with Audited\n\nclass Base\n",
    );
    let audited = (
        "src/main/scala/app/svc/Audited.scala",
        "package app.svc\n\ntrait Audited {\n  def audit(): Unit = ()\n}\n",
    );
    let tmp = tree(&[sub]);
    let engine = index(tmp.path());
    assert!(edges(engine.runtime().unwrap(), EdgeKind::Implements).is_empty());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    write(tmp.path(), audited.0, audited.1);
    engine.sync(&[root.join(audited.0)] as &[PathBuf]);
    let synced = graph_fingerprint(engine.runtime().unwrap());

    let cold = tree(&[sub, audited]);
    let fresh = index(cold.path());
    assert_eq!(synced, graph_fingerprint(fresh.runtime().unwrap()));
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Implements),
        strings(&["Page -> Audited:trait"])
    );
    assert_eq!(
        edges(engine.runtime().unwrap(), EdgeKind::Extends),
        strings(&["Page -> Base:class"])
    );
}

/// NFR-RA-06: a trait that gains or loses its companion `object` keeps its
/// mixins bound, on a sync as on a cold index.
#[test]
fn a_companion_added_beside_a_mixed_in_trait_keeps_it_bound_on_sync() {
    let page = (
        "src/main/scala/app/web/Page.scala",
        "package app.web\n\nimport app.svc.Audited\n\nclass Page extends Audited\n",
    );
    let rel = "src/main/scala/app/svc/Audited.scala";
    let bare = "package app.svc\n\ntrait Audited {\n  def audit(): Unit = ()\n}\n";
    let paired = "package app.svc\n\ntrait Audited {\n  def audit(): Unit = ()\n}\n\nobject Audited {\n  val Name = \"a\"\n}\n";
    let tmp = tree(&[page, (rel, bare)]);
    let engine = index(tmp.path());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    for text in [paired, bare] {
        write(tmp.path(), rel, text);
        engine.sync(&[root.join(rel)] as &[PathBuf]);
        let cold = tree(&[page, (rel, text)]);
        let fresh = index(cold.path());
        assert_eq!(graph_fingerprint(engine.runtime().unwrap()), graph_fingerprint(fresh.runtime().unwrap()));
        assert_eq!(
            edges(engine.runtime().unwrap(), EdgeKind::Implements),
            strings(&["Page -> Audited:trait"])
        );
    }
}
