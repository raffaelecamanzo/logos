//! A Kotlin call on a typed receiver binds among that type's members (S-563,
//! CR-178, FR-RS-32, FR-RS-10, FR-RS-12, NFR-RA-05) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! the SHIPPED `references.scm` and `plugin.toml` of the Kotlin plugin (no
//! query override).
//!
//! What the fixtures pin, over methods of `shop.api` classes calling the
//! `shop.domain` types through their imports:
//!
//! - a receiver whose one binding in scope at the call proves its type — a
//!   constructor property, a typed or constructed property, a parameter, a
//!   typed local, `val x = T()`, a typed lambda parameter, `T?` — is the
//!   Path-form `T::m` of shape `other`, bound among `T`'s instance members up
//!   its proven supertypes, never among its companion's;
//! - a receiver with no binding that proves a type — an untyped local or
//!   lambda parameter, `it`, a destructured name, a `vararg`, a type
//!   parameter, a qualified type, a plain constructor parameter read in a
//!   method — and one whose typed binding a nearer one hides (a parameter, a
//!   local, a lambda, `for` or `catch` variable), or whose class property a
//!   receiver lambda's own `this` may hide, stays the Method-form row of shape
//!   `other`, bound nowhere (`no-receiver-evidence`);
//! - a typed call no member of its type answers still reaches the one
//!   extension function of the name in scope (S-560).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be
//! indexed into its own graph.

#![cfg(feature = "lang-kotlin")]

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

/// The `Calls` rows sourced in `file` whose target ends in one of `names`,
/// sorted.
fn rows(rt: &Runtime, file: &str, names: &[&str]) -> Vec<Row> {
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
        .filter(|r| names.iter().any(|n| r.target == *n || r.target.ends_with(&format!("::{n}"))))
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

/// Every call-residue reason the Kotlin row states, with its count, sorted.
fn kotlin_residue(engine: &Engine) -> Vec<(CallResidueReason, u64)> {
    let residue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "kotlin")
        .and_then(|row| row.call_residue)
        .expect("the kotlin row states its call residue");
    let mut reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    reasons.sort();
    reasons
}

const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

const DOMAIN_PATH: &str = "src/main/kotlin/shop/domain/Domain.kt";

/// `Order`'s companion declares a `ship` too: a call on the class reaches it,
/// a call on an instance never does.
const DOMAIN: &str = "\
package shop.domain

interface OrderService {
    fun addOrderItem(id: Int)
}
open class Order {
    fun ship() {}
    fun addOrderItem(id: Int) {}
    companion object {
        fun ship() {}
    }
}
class SpecialOrder : Order()
class Mailer {
    fun send() {}
}
";

const HANDLER_PATH: &str = "src/main/kotlin/shop/api/Handler.kt";

const HANDLER: &str = "\
package shop.api

import shop.domain.Order
import shop.domain.OrderService
import shop.domain.SpecialOrder

class Handler(private val orderService: OrderService, plain: Order) {
    private val current: Order = Order()
    private var maybe: Order? = null
    private val made = Order()
    fun byConstructorProperty() { orderService.addOrderItem(1) }
    fun byProperty() { current.ship() }
    fun byNullable() { maybe?.ship() }
    fun byConstructed() { made.addOrderItem(2) }
    fun byParameter(order: Order) { order.ship() }
    fun byNullableParameter(order: Order?) { order?.ship() }
    fun byLocal() { val o: Order = make(); o.ship() }
    fun byNew() { val n = Order(); n.ship() }
    fun byBase(s: SpecialOrder) { s.ship() }
    fun byLambda(xs: List<Order>) { xs.forEach { y: Order -> y.ship() } }
    fun byClass() { Order.ship() }
    fun untyped() { val got = make(); got.ship() }
    fun implicitIt(xs: List<Order>) { xs.forEach { it.ship() } }
    fun untypedLambda(xs: List<Order>) { xs.forEach { z -> z.ship() } }
    fun plainParameter() { plain.ship() }
    fun make(): Order = Order()
}
";

#[test]
fn a_call_on_a_typed_receiver_binds_among_that_types_members() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (HANDLER_PATH, HANDLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let h = |line: u32, name: &str| format!("{HANDLER_PATH}:{name}@{line}");
    let d = |line: u32, name: &str| format!("{DOMAIN_PATH}:{name}@{line}");
    let typed = |source: String, target: &str| row(&source, target, RefForm::Path, OTHER, true);
    let untyped = |source: String, name: &str| row(&source, name, RefForm::Method, OTHER, false);
    assert_eq!(
        rows(rt, HANDLER_PATH, &["ship", "addOrderItem"]),
        sorted(vec![
            typed(h(11, "byConstructorProperty"), "OrderService::addOrderItem"),
            typed(h(12, "byProperty"), "Order::ship"),
            typed(h(13, "byNullable"), "Order::ship"),
            typed(h(14, "byConstructed"), "Order::addOrderItem"),
            typed(h(15, "byParameter"), "Order::ship"),
            typed(h(16, "byNullableParameter"), "Order::ship"),
            typed(h(17, "byLocal"), "Order::ship"),
            typed(h(18, "byNew"), "Order::ship"),
            typed(h(19, "byBase"), "SpecialOrder::ship"),
            typed(h(20, "byLambda"), "Order::ship"),
            // A call on the class: the companion's, as before (I4-INT).
            row(&h(21, "byClass"), "Order::ship", RefForm::Path, None, true),
            untyped(h(22, "untyped"), "ship"),
            untyped(h(23, "implicitIt"), "ship"),
            untyped(h(24, "untypedLambda"), "ship"),
            untyped(h(25, "plainParameter"), "ship"),
        ])
    );
    let edges = calls(rt);
    let add_interface = d(4, "addOrderItem");
    let add_class = d(8, "addOrderItem");
    let ship = d(7, "ship");
    let companion_ship = d(10, "ship");
    for expected in [
        edge(&h(11, "byConstructorProperty"), &add_interface),
        edge(&h(12, "byProperty"), &ship),
        edge(&h(13, "byNullable"), &ship),
        edge(&h(14, "byConstructed"), &add_class),
        edge(&h(15, "byParameter"), &ship),
        edge(&h(16, "byNullableParameter"), &ship),
        edge(&h(17, "byLocal"), &ship),
        edge(&h(18, "byNew"), &ship),
        // Inheritance: `SpecialOrder` declares no `ship`; its base does.
        edge(&h(19, "byBase"), &ship),
        edge(&h(20, "byLambda"), &ship),
        edge(&h(21, "byClass"), &companion_ship),
    ] {
        assert!(edges.contains(&expected), "missing {expected:?} in {edges:#?}");
    }
    // An instance call never reaches the companion's `ship`.
    let to_companion: Vec<&(String, String)> = edges.iter().filter(|(_, to)| *to == companion_ship).collect();
    assert_eq!(to_companion, vec![&edge(&h(21, "byClass"), &companion_ship)]);
}

const FORMS_PATH: &str = "src/main/kotlin/shop/api/Forms.kt";

/// One typed receiver per remaining declaration form: a `catch`, `when` subject
/// and `for` variable, a local inside a lambda, a nullable constructor
/// property, and a property of an enum class, an object and a companion.
const FORMS: &str = "\
package shop.api

import shop.domain.Order

fun byCatch() {
    try {
        make()
    } catch (e: Order) {
        e.ship()
    }
}
fun byWhen() {
    when (val o: Order = pick()) {
        else -> o.ship()
    }
}
fun byLoop(xs: List<Order>) {
    for (o: Order in xs) {
        o.ship()
    }
}
fun byLambdaLocal(xs: List<Int>) {
    xs.forEach {
        val o: Order = Order()
        o.ship()
    }
}
class ByNullableProperty(private val o: Order?) {
    fun f() = o?.ship()
}
enum class ByEnum {
    A;
    val o: Order = Order()
    fun f() = o.ship()
}
object ByObject {
    val o: Order = Order()
    fun f() = o.ship()
}
class ByCompanion {
    companion object {
        val o: Order = Order()
        fun f() = o.ship()
    }
}
";

#[test]
fn every_other_declaration_form_kotlin_writes_proves_its_type() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (FORMS_PATH, FORMS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let f = |line: u32, name: &str| format!("{FORMS_PATH}:{name}@{line}");
    let ship = format!("{DOMAIN_PATH}:ship@7");
    let mut from_file: Edges = calls(rt)
        .into_iter()
        .filter(|(from, to)| from.starts_with(FORMS_PATH) && to.starts_with(DOMAIN_PATH))
        .collect();
    from_file.sort();
    let mut expected = vec![
        edge(&f(5, "byCatch"), &ship),
        edge(&f(12, "byWhen"), &ship),
        edge(&f(17, "byLoop"), &ship),
        edge(&f(22, "byLambdaLocal"), &ship),
        edge(&f(29, "f"), &ship),
        edge(&f(34, "f"), &ship),
        edge(&f(38, "f"), &ship),
        edge(&f(43, "f"), &ship),
    ];
    expected.sort();
    assert_eq!(from_file, expected);
}

/// A repository type named like a Rust wrapper: a Kotlin type is read as
/// written, so `Box<Order>` is a `Box`, never peeled to the `Order` inside.
#[test]
fn a_generic_type_is_its_own_type_never_its_argument() {
    let box_path = "src/main/kotlin/shop/domain/Box.kt";
    let caller = "src/main/kotlin/shop/api/Boxed.kt";
    let tmp = tree(&[
        (DOMAIN_PATH, DOMAIN),
        (box_path, "package shop.domain\n\nclass Box<T>(val v: T) {\n    fun ship() {}\n}\n"),
        (
            caller,
            "package shop.api\n\nimport shop.domain.Box\nimport shop.domain.Order\n\nfun boxed(b: Box<Order>) = b.ship()\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let from_caller: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(caller)).collect();
    assert_eq!(from_caller, vec![edge(&format!("{caller}:boxed@6"), &format!("{box_path}:ship@4"))]);
}

// ── What the file does not prove ────────────────────────────────────────────

const SHADOW_PATH: &str = "src/main/kotlin/shop/api/Shadow.kt";

/// Every `ship()` below names a receiver the file proves no type for, though
/// a typed `repo: Order` property, or a plain reading, would hand it one. Each
/// binding form appears where it hides a typed binding of its name (the
/// property, or `byIt`'s parameter); the rest declare a type the file cannot
/// prove, or read the property where another receiver may declare it.
///
/// Members are spelt with expression bodies or on lines of their own: the
/// Kotlin grammar misreads a statement-initial `a.b()` in some one-line bodies
/// as a function type, which would fail the fixture's parse.
const SHADOW: &str = "\
package shop.api

import shop.domain.Order
import shop.domain.Mailer

class Generic<Order>(private val g: Order) {
    fun go() = g.ship()
}
class Ctx {
    val repo: Mailer = Mailer()
}
fun build(block: Ctx.() -> Unit) = Ctx().block()
class Shadow(private val repo: Order) {
    fun byBuilder() = build { repo.ship() }
    fun byArgument() = build({ repo.ship() })
    fun byStored(): Ctx.() -> Unit = { repo.ship() }
    fun byParameter(repo: Mailer) = repo.ship()
    fun byLocal() { val repo = make(); repo.ship() }
    fun byLambda(xs: List<Mailer>) = xs.forEach { repo -> repo.ship() }
    fun byFor(xs: List<Mailer>) { for (repo in xs) { repo.ship() } }
    fun byCatch() = try { } catch (repo: Exception) { repo.ship() }
    fun byDestructuring(p: Pair<Mailer, Int>) { val (repo, n) = p; repo.ship() }
    fun byForPair(xs: List<Pair<Mailer, Int>>) {
        for ((repo, n) in xs) {
            repo.ship()
        }
    }
    fun byLambdaPair(xs: List<Pair<Mailer, Int>>) = xs.forEach { (repo, n) -> repo.ship() }
    fun byApply(x: Mailer) = x.apply { repo.ship() }
    fun byWith(x: Mailer) = with(x) { repo.ship() }
    fun byRun(x: Mailer) = x.run { repo.ship() }
    fun byObject() = object : Runnable {
        override fun run() {
            repo.ship()
        }
    }
    inner class Inner {
        fun g() {
            repo.ship()
        }
    }
    companion object {
        fun c() = repo.ship()
    }
    fun byIt(it: Order, xs: List<Mailer>) = xs.forEach { it.ship() }
    fun byVararg(vararg vs: Order) = vs.ship()
    fun <Order> byTypeParameter(t: Order) = t.ship()
    fun byQualified(q: shop.domain.Order) {
        q.ship()
    }
    fun byFunctionType(f: () -> Order) = f.ship()
    fun byAssertion(o: Order?) = o!!.ship()
    fun make(): Mailer = Mailer()
}
class Plain(repo: Mailer) {
    private val repo: Order = Order()
    private val n: Unit = repo.ship()
    init {
        repo.ship()
    }
}
";

#[test]
fn a_receiver_the_file_proves_no_type_for_binds_nowhere() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (SHADOW_PATH, SHADOW)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, SHADOW_PATH, &["ship"]);
    // One row per `ship()` call the fixture writes — `Plain`'s initialiser and
    // `init` calls are both the class's, one row, which a typed call would split.
    assert_eq!(receiver_calls.len(), 25, "{receiver_calls:#?}");
    let typed: Vec<&Row> = receiver_calls
        .iter()
        .filter(|r| (r.1.as_str(), r.2, r.3, r.4) != ("ship", RefForm::Method, OTHER, false))
        .collect();
    assert!(typed.is_empty(), "every row must stay untyped: {typed:#?}");
    let edges = calls(rt);
    assert!(
        !edges.iter().any(|(_, to)| to.starts_with(DOMAIN_PATH)),
        "nothing here proves a domain type: {edges:#?}"
    );
}

/// An untyped receiver states why it binds nowhere: `no-receiver-evidence`
/// (FR-RS-32), the reason a call on a receiver of unproven type has always
/// carried.
#[test]
fn an_untyped_receiver_stays_no_receiver_evidence() {
    let path = "src/main/kotlin/shop/api/Untyped.kt";
    let source = "package shop.api\n\nfun untyped() {\n    val got = make()\n    got.ship()\n}\nfun make(): Int = 0\n";
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (path, source)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        rows(rt, path, &["ship"]),
        vec![row(&format!("{path}:untyped@3"), "ship", RefForm::Method, OTHER, false)]
    );
    assert_eq!(kotlin_residue(&engine), vec![(CallResidueReason::NoReceiverEvidence, 1)]);
}

/// A binding that hides nothing proves its type: the same class with every
/// hiding form above written under another name.
const UNSHADOWED: &str = "\
package shop.api

import shop.domain.Order

class Unshadowed(private val repo: Order) {
    fun byParameter(other: Order) { repo.ship() }
    fun before() { repo.ship(); val repo = 1 }
    fun byLambda(xs: List<Order>) { xs.forEach { o -> repo.ship() } }
}
";

#[test]
fn a_binding_hides_only_where_it_is_in_scope() {
    let path = "src/main/kotlin/shop/api/Unshadowed.kt";
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (path, UNSHADOWED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let u = |line: u32, name: &str| format!("{path}:{name}@{line}");
    let ship = format!("{DOMAIN_PATH}:ship@7");
    let mut from_file: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(path)).collect();
    from_file.sort();
    assert_eq!(
        from_file,
        vec![
            // A local hides the property only after its declaration.
            edge(&u(7, "before"), &ship),
            edge(&u(8, "byLambda"), &ship),
            edge(&u(6, "byParameter"), &ship),
        ]
    );
}

const IMPLICIT_PATH: &str = "src/main/kotlin/shop/api/Implicit.kt";

/// A typed top-level `repo` beside members of its name that an implicit
/// receiver brings into view — an inherited property, a companion's, an outer
/// class's, an extension receiver's — none of which the file reads: each
/// outranks the top-level property, so none of these calls is typed.
const IMPLICIT: &str = "\
package shop.api

import shop.domain.Order
import shop.domain.Mailer

val repo: Order = Order()

open class Base {
    val repo: Mailer = Mailer()
}
class Ctx {
    val repo: Mailer = Mailer()
}
class Sub : Base() {
    fun inherited() = repo.ship()
}
class WithCompanion {
    fun viaCompanion() = repo.ship()
    companion object {
        val repo: Mailer = Mailer()
    }
}
fun Ctx.viaExtension() = repo.ship()
class Outer {
    val repo: Mailer = Mailer()
    inner class In {
        fun g() = repo.ship()
    }
}
fun topLevel() = repo.ship()
";

#[test]
fn a_top_level_property_never_types_a_call_an_implicit_receiver_may_answer() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (IMPLICIT_PATH, IMPLICIT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let receiver_calls = rows(rt, IMPLICIT_PATH, &["ship"]);
    assert_eq!(receiver_calls.len(), 5, "{receiver_calls:#?}");
    let typed: Vec<&Row> = receiver_calls
        .iter()
        .filter(|r| (r.1.as_str(), r.2, r.3, r.4) != ("ship", RefForm::Method, OTHER, false))
        .collect();
    assert!(typed.is_empty(), "every row must stay untyped: {typed:#?}");
    assert!(
        !calls(rt).iter().any(|(_, to)| to.starts_with(DOMAIN_PATH)),
        "nothing here proves a domain type: {:#?}",
        calls(rt)
    );
}

const FACTORY_PATH: &str = "src/main/kotlin/shop/api/Factory.kt";

/// A same-package function `Order()` beside the class a wildcard brings in:
/// `Order()` calls the function, so `made` is a `Mailer`, never an `Order`.
const FACTORY: &str = "\
package shop.api

import shop.domain.*

fun Order(): Mailer = Mailer()

fun viaFactory() {
    val made = Order()
    made.ship()
}
";

#[test]
fn a_type_a_top_level_function_also_spells_proves_nothing() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (FACTORY_PATH, FACTORY)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        !calls(rt).iter().any(|(_, to)| to.starts_with(DOMAIN_PATH)),
        "the factory's `Order` is no class: {:#?}",
        calls(rt)
    );
    assert_eq!(
        rows(rt, FACTORY_PATH, &["ship"]),
        vec![row(&format!("{FACTORY_PATH}:viaFactory@7"), "Order::ship", RefForm::Path, OTHER, false)]
    );
    assert!(kotlin_residue(&engine).contains(&(CallResidueReason::TypeAmbiguous, 1)), "{:?}", kotlin_residue(&engine));
}

// ── Extension functions (S-560) ─────────────────────────────────────────────

const EXTENSIONS_PATH: &str = "src/main/kotlin/shop/api/Slugger.kt";

const EXTENSIONS: &str = "\
package shop.api

import shop.domain.Order

fun String.toSlug(): String = this
fun Order.audit() {}

class Slugger(private val title: String, private val order: Order) {
    fun slug() { title.toSlug() }
    fun doAudit() { order.audit() }
    fun ship() { order.ship() }
}
";

#[test]
fn a_typed_call_no_member_answers_still_reaches_its_extension() {
    let tmp = tree(&[(DOMAIN_PATH, DOMAIN), (EXTENSIONS_PATH, EXTENSIONS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let s = |line: u32, name: &str| format!("{EXTENSIONS_PATH}:{name}@{line}");
    let mut from_file: Edges = calls(rt).into_iter().filter(|(from, _)| from.starts_with(EXTENSIONS_PATH)).collect();
    from_file.sort();
    assert_eq!(
        from_file,
        vec![
            // `Order` declares no `audit`: the extension is the one called.
            edge(&s(10, "doAudit"), &s(6, "audit")),
            edge(&s(11, "ship"), &format!("{DOMAIN_PATH}:ship@7")),
            // `String` is no repository type: the extension, as before.
            edge(&s(9, "slug"), &s(5, "toSlug")),
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

const CALLER_PATH: &str = "src/main/kotlin/shop/api/Caller.kt";

const CALLER: &str = "\
package shop.api

import shop.domain.OrderService
import shop.domain.Order

class Caller(private val service: OrderService) {
    fun run() { service.addOrderItem(1) }
}
";

#[test]
fn a_synced_edit_to_the_receivers_type_matches_a_fresh_reindex() {
    let initial = [(DOMAIN_PATH, DOMAIN), (CALLER_PATH, CALLER)];
    let run = format!("{CALLER_PATH}:run@7");
    // The member is renamed away: the edge goes, though the caller's file
    // never changes.
    let renamed = DOMAIN.replace("    fun addOrderItem(id: Int)\n}", "    fun add(id: Int)\n}");
    let edges = synced_equals_reindexed(&initial, &[(DOMAIN_PATH, &renamed)]);
    assert!(edges.is_empty(), "{edges:?}");

    // The type arrives after the caller: the call binds on sync.
    let edges = synced_equals_reindexed(&[(CALLER_PATH, CALLER)], &[(DOMAIN_PATH, DOMAIN)]);
    assert_eq!(edges, vec![edge(&run, &format!("{DOMAIN_PATH}:addOrderItem@4"))]);

    // The property's declared type changes: the caller's row is re-typed.
    let retyped = CALLER.replace("service: OrderService", "service: Order");
    let edges = synced_equals_reindexed(&initial, &[(CALLER_PATH, &retyped)]);
    assert_eq!(edges, vec![edge(&run, &format!("{DOMAIN_PATH}:addOrderItem@8"))]);

    // The receiver's type gains, in another file, the supertype that declares
    // the member: the call binds on sync, though its file never changes.
    let special = "\
package shop.api

import shop.domain.SpecialOrder

class Special(private val s: SpecialOrder) {
    fun go() = s.ship()
}
";
    let orphan = DOMAIN.replace("class SpecialOrder : Order()", "class SpecialOrder");
    let edges = synced_equals_reindexed(&[(DOMAIN_PATH, &orphan), (CALLER_PATH, special)], &[(DOMAIN_PATH, DOMAIN)]);
    assert_eq!(edges, vec![edge(&format!("{CALLER_PATH}:go@6"), &format!("{DOMAIN_PATH}:ship@7"))]);
}
