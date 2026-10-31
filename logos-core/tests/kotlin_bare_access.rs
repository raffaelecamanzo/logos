//! A bare Kotlin property read is an access (S-562, CR-178, FR-EX-25,
//! FR-QM-11, NFR-RA-05, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! `references.scm`, `symbols.scm` and `plugin.toml` of the Kotlin plugin (no
//! query override).
//!
//! The fixtures follow mixit's dominant shapes: a class whose primary
//! constructor declares `val`s and whose methods read them, and the body's
//! `val`/`var` properties, bare — `balance + amount`, `log.add(x)`, `"$name"`.
//!
//! What the fixtures pin, over methods of `app.Account`:
//!
//! - a bare `balance`, `limit` or `name` that the enclosing class declares as a
//!   property — a constructor `val`/`var` included — in the value positions real
//!   code spells them in (an operand, an argument, a receiver, an assigned,
//!   returned or thrown expression, an expression body, a `${…}` interpolation,
//!   a loop's source, a condition, a branch), is an `Accesses` row of the method,
//!   and binds to the one property;
//! - a local, parameter, lambda parameter, destructured name, `for` or `catch`
//!   variable of the same name in that method shadows the property: it records
//!   nothing, and the property read beside it in another method still does;
//! - the near misses sit one token from matching and record nothing: a member
//!   of another value (`o.count`), a call's callee, a named argument's label, a
//!   plain constructor parameter, a name the class does not declare or only a
//!   base class does, a property an outer class declares, the body of an
//!   `object : T { … }` and of an extension function, a companion's constant;
//! - Kotlin declares `member_access`, so Cohesion is computed from those links.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`kotlin_calls.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-kotlin")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Granularity, Runtime};
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

/// Every bound `Accesses` edge as `(method name, property name)`, sorted.
fn accesses(rt: &Runtime) -> Vec<(String, String)> {
    let names: HashMap<NodeId, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, n.name))
                .collect())
        })
        .expect("read runs");
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Accesses)
        .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// Every `Accesses` ledger row, bound or not, as `(method name, property name)`.
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

fn pair(method: &str, property: &str) -> (String, String) {
    (method.to_string(), property.to_string())
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = items.iter().map(|(m, p)| pair(m, p)).collect();
    out.sort();
    out
}

const ACCOUNT: &str = "\
package app

class Account(val balance: Int, private var limit: Int) {
    private val log = mutableListOf<Int>()
    var name: String = \"\"

    fun deposit(amount: Int): Int { log.add(amount); return balance + amount }
    fun show(): String = name + limit
    fun say(): String = \"${name}\"
    fun over(x: Int): Boolean = x > limit && check(limit)
    fun check(v: Int): Boolean = v > 0
    fun pick(b: Boolean): Int = if (b) balance else limit
    fun ref(): Int = wrap(limit) + wrap(value = balance)
    fun wrap(value: Int): Int = value
    fun twice(): Int {
        val x = balance
        return x + balance
    }
}
";

#[test]
fn a_bare_property_read_is_an_access() {
    let tmp = tree(&[("src/Account.kt", ACCOUNT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `check`, `wrap` and the parameter reads (`v`, `value`, `x`, `amount`,
    // `b`) record nothing, and neither does `value = balance`'s label.
    let expected = pairs(&[
        ("deposit", "balance"),
        ("deposit", "log"),
        ("over", "limit"),
        ("pick", "balance"),
        ("pick", "limit"),
        ("ref", "balance"),
        ("ref", "limit"),
        ("say", "name"),
        ("show", "limit"),
        ("show", "name"),
        ("twice", "balance"),
    ]);
    assert_eq!(accesses(rt), expected);
}

/// One value position per member, each reading a property of its own that no
/// other member reads, so a position the query lost is a missing pair rather
/// than one another member's read still supplies. `@` is the member's name.
const POSITIONS: &[(&str, &str)] = &[
    ("f01", "fun @() { take(f01) }"),
    ("f02", "fun @() { take(v = f02) }"),
    ("f03", "fun @() { f03.toString() }"),
    ("f04", "fun @() { f04?.toString() }"),
    ("f05", "fun @() { val x = f05[0] }"),
    ("f06", "fun @() { f06 = 1 }"),
    ("f07", "fun @() { var q: Any = 0; q = f07 }"),
    ("f08", "fun @() { val x = f08 + 1 }"),
    ("f09", "fun @() { val x = 1 + f09 }"),
    ("f10", "fun @() { val x = if (f10) 1 else 2 }"),
    ("f11", "fun @() { val x = if (true) f11 else 2 }"),
    ("f12", "fun @() { val x = if (false) 1 else f12 }"),
    ("f13", "fun @() { val x = !f13 }"),
    ("f14", "fun @() { f14++ }"),
    ("f15", "fun @() { val x = (f15) }"),
    ("f16", "fun @() { val x = f16 as String }"),
    ("f17", "fun @() { val x = f17 is String }"),
    ("f18", "fun @() { val x = f18 in 1..3 }"),
    ("f19", "fun @() { val x = 1 in f19 }"),
    ("f20", "fun @() { val x = f20..3 }"),
    ("f21", "fun @() { val x = 1..f21 }"),
    ("f22", "fun @() { val x = f22 }"),
    ("f23", "fun @() { val x = listOf(f23) }"),
    ("f24", "fun @() { val g = { f24 } }"),
    ("f25", "fun @(): Any = f25"),
    ("f26", "fun @(): Any { return f26 }"),
    ("f27", "fun @() { throw f27 }"),
    ("f28", "fun @() { val x = f28 ?: 1 }"),
    ("f29", "fun @() { if (f29) { } }"),
    ("f30", "fun @() { while (f30) { } }"),
    ("f31", "fun @() { do { } while (f31) }"),
    ("f32", "fun @() { when (f32) { else -> 1 } }"),
    ("f33", "fun @() { when (1) { f33 -> 1 else -> 2 } }"),
    ("f34", "fun @() { when (1) { 1 -> f34 else -> 2 } }"),
    ("f35", "fun @() { for (x in f35) { } }"),
    ("f36", "fun @() { val s = \"x ${f36}\" }"),
    ("f37", "fun @() { val s = \"${f37.length}\" }"),
    ("f38", "fun @() { take(*f38) }"),
    ("f39", "fun @() { val x = f39!! }"),
    ("f40", "fun @() { val x = 1 shl f40 }"),
    ("f41", "fun @() { val x = f41 shl 1 }"),
    ("f42", "fun @() { f42 += 1 }"),
    ("f43", "fun @() { val x = -f43 }"),
    (
        "f44",
        "fun @() { val x = try { f44 } catch (e: Exception) { 0 } }",
    ),
    ("f45", "fun @() { while (true) f45 }"),
    ("f46", "fun @() { val x = arr[f46] }"),
    ("f47", "fun @() { val x = f47 == null }"),
    ("f48", "fun @() { run { f48 } }"),
];

#[test]
fn a_name_is_an_access_in_each_position_a_value_is_written() {
    let fields: Vec<&str> = POSITIONS.iter().map(|(field, _)| *field).collect();
    let mut src = String::from("package app\n\nclass Forms {\n");
    for field in &fields {
        src.push_str(&format!("    var {field}: Any = 0\n"));
    }
    src.push_str("    fun take(v: Any) { }\n");
    let mut expected: Vec<(String, String)> = Vec::new();
    for (i, (field, member)) in POSITIONS.iter().enumerate() {
        let name = format!("m{i:02}");
        src.push_str(&format!("    {}\n", member.replace('@', &name)));
        expected.push(pair(&name, field));
    }
    src.push_str("}\n");
    expected.sort();
    let tmp = tree(&[("src/Forms.kt", &src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(accesses(rt), expected);
}

/// Each read is one ledger row on the line it is written on, however often
/// the method repeats it (`twice` reads `balance` on two lines: one row, at the
/// first).
#[test]
fn a_bare_read_is_one_row_at_its_line() {
    let tmp = tree(&[("src/Account.kt", ACCOUNT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let line_of =
        |needle: &str| 1 + ACCOUNT.lines().position(|l| l.contains(needle)).unwrap() as u32;
    let mut got: Vec<(String, Option<i64>)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| {
            r.kind == EdgeKind::Accesses
                && r.target == "balance"
                && r.source_symbol.contains("deposit")
        })
        .map(|r| (r.target, r.line))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![(
            "balance".to_string(),
            Some(i64::from(line_of("fun deposit")))
        )]
    );

    let mut twice: Vec<Option<i64>> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| {
            r.kind == EdgeKind::Accesses
                && r.target == "balance"
                && r.source_symbol.contains("twice")
        })
        .map(|r| r.line)
        .collect();
    twice.sort();
    assert_eq!(twice, vec![Some(i64::from(line_of("val x = balance")))]);
}

const SHADOWS: &str = "\
package app

class Shadows(val rate: Int) {
    var count = 0
    var name = \"\"

    fun param(count: Int): Int { return count }
    fun local(): Int { val name = \"x\"; return name.length }
    fun lambda(): Int { val f = { count: Int -> count + 1 }; return f(1) + rate }
    fun loop(xs: List<Int>): Int { var n = 0; for (count in xs) { n += count }; return n }
    fun destructure(p: Pair<Int, Int>): Int { val (count, other) = p; return count + other }
    fun caught(): Int { try { return 1 } catch (name: Exception) { return name.message!!.length } }
    fun later(b: Boolean): Int { if (b) { return count } else { val count = 3; return count } }
    fun localFn(): Int { fun inner(count: Int) = count; return inner(1) }
    fun lambdaDestructure(m: Map<Int, Int>) = m.map { (count, v) -> count + v }
    fun closure(): Int { val count = 1; fun inner() = count; return inner() }
    fun untouched(): Int { return count + rate }
}
";

#[test]
fn a_local_or_parameter_of_the_same_name_records_nothing() {
    let tmp = tree(&[("src/Shadows.kt", SHADOWS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        access_rows(rt),
        pairs(&[
            // The lambda's own parameter shadows `count`; `rate` is not.
            ("lambda", "rate"),
            // `untouched` reads both, with nothing to shadow either.
            ("untouched", "count"),
            ("untouched", "rate"),
        ])
    );
}

const NEAR_MISSES: &str = "\
package app

open class Base {
    var x = 0
}

class Derived : Base() {
    fun go(): Int { return x }
}

class Outer {
    var o = 0
    class Nested {
        fun go(): Int { return o }
    }
    inner class Inner {
        fun go(): Int { return o }
    }
}

class Companioned {
    var v = 0
    fun cap() = MAX
    companion object {
        const val MAX = 1
    }
}

class Misses(val id: Int, plain: Int) {
    private val repo = Repo()
    val label = plain + id
    var count = 0

    fun member(o: Misses): Int = o.count
    fun callee(): Int = count()
    fun count(): Int = 1
    fun named(): Foo = Foo(count = 1)
    fun usesPlain() = plain
    fun unknown() = ghost + Other.value
    fun thisForm() = this.count
    fun short() = \"$count\"
    fun maker(): Repo = Repo()
    fun listener() = object : Runnable {
        override fun run() { count++ }
    }
    fun String.slug(): String = this + count
    fun String?.slug2(): String = this + count
    val doubled: Int get() = count * 2
}

class Infix {
    val to = 1
    val shl = 2

    fun p() = 1 to 2
    fun q() = 3 shl 1
}

class Labelled {
    var lbl = 0

    fun r(xs: List<Int>) {
        xs.forEach lbl@{ if (it > 0) return@lbl }
    }
}

class Initialisers {
    var o = 0
    var p = 0

    class Nested {
        val x = o
        init {
            println(o)
        }
    }

    object Holder {
        val h = p
    }

    companion object {
        val c = o
    }
}
";

#[test]
fn the_near_misses_record_nothing() {
    let tmp = tree(&[("src/Misses.kt", NEAR_MISSES)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // The written `this.count` — the pre-existing own-property form — and
    // nothing else. The grammar reads a short `"$count"` as string text, so
    // `short` records nothing: a stated ceiling, never a fabricated row.
    assert_eq!(access_rows(rt), pairs(&[("thisForm", "count")]));
}

/// A name read where another receiver is in scope is not the class's: the
/// trailing lambda of `x.apply { }`, `x.run { }` and `with(x) { }` has `x` for
/// `this`, so `age = 3` writes the DTO's `age`. A bare `run { }` has no receiver,
/// and `let { }` names its value `it`: both read the class's own property.
const RECEIVER_LAMBDAS: &str = "\
package app

class Dto {
    var age: Int = 0
}

class Mapper(val age: Int) {
    fun toDto(): Dto = Dto().apply {
        age = 3
    }
    fun withIt(d: Dto): Int = with(d) { age }
    fun runIt(d: Dto): Int = d.run { age + 1 }
    fun safe(d: Dto?): Int? = d?.run { age }
    fun letIt(d: Dto): Int = d.let { it.age + age }
    fun plainRun(): Int = run { age }
}
";

#[test]
fn a_receiver_lambda_names_its_receivers_property_not_the_classs() {
    let tmp = tree(&[("src/Mapper.kt", RECEIVER_LAMBDAS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        access_rows(rt),
        pairs(&[("letIt", "age"), ("plainRun", "age")])
    );
}

/// The query's `class_body` capture also takes a property declared in the body
/// of an `object : T { … }` or an enum entry, so the class would seem to declare
/// it: a bare read in the class's own methods is of the base class's or the
/// file's, which no row records.
const ANONYMOUS_BODY_PROPERTIES: &str = "\
package app

val count = 5

open class Base {
    val label = 1
}

interface Named {
    val label: Int
}

class Holder : Base() {
    val named = object : Named {
        override val label = 2
    }
    fun read() = label
}

enum class Level {
    HIGH {
        val count = 1
        override fun weight() = count
    },
    LOW;

    open fun weight() = count
}

class Own {
    val label = 3
    val named = object : Named {
        override val label = 4
    }
    fun read() = label
}
";

#[test]
fn a_property_only_an_anonymous_body_declares_is_not_the_classs() {
    let tmp = tree(&[("src/Holder.kt", ANONYMOUS_BODY_PROPERTIES)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `Own` declares `label` itself as well, so its read stays.
    assert_eq!(access_rows(rt), pairs(&[("read", "label")]));
}

/// `this.x` names the class's own instance only when unlabelled.
#[test]
fn a_labelled_this_names_another_instance() {
    let src = "\
package app

class Outer(val count: Int) {
    inner class In(val count: Int) {
        fun own() = this.count
        fun outer() = this@Outer.count
        fun inner() = this@In.count
    }
}
";
    let tmp = tree(&[("src/Outer.kt", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(access_rows(rt), pairs(&[("own", "count")]));
}

/// A class declared inside an extension function's body has its own receiver:
/// a bare read in its method is of its own property.
#[test]
fn a_local_class_in_an_extension_reads_its_own_property() {
    let src = "\
package app

class Host {
    fun Int.other(): Int {
        class Local {
            var q = 0

            fun m() = q
        }
        return 1
    }
}
";
    let tmp = tree(&[("src/Host.kt", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(access_rows(rt), pairs(&[("m", "q")]));
}

/// An extension function's receiver is written before its name, in each form
/// the grammar has: a plain, nullable, generic, qualified and parenthesised type.
#[test]
fn an_extension_body_reads_its_receiver_in_every_spelling() {
    let src = "\
package app

class Host(val count: Int) {
    fun Other.plain() = count
    fun Other?.nullable() = count
    fun <T> List<T>.generic() = count
    fun app.Other.qualified() = count
    fun (Other).parenthesised() = count
    fun member() = count
}
";
    let tmp = tree(&[("src/Host.kt", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(access_rows(rt), pairs(&[("member", "count")]));
}

#[test]
fn a_shadowing_parameter_records_nothing_and_binds_nothing() {
    let src = "\
package app

class Total(val sum: Int) {
    fun plus(sum: Int): Int = sum + 1
    fun own(): Int = sum + 1
}
";
    let tmp = tree(&[("src/Total.kt", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(accesses(rt), pairs(&[("own", "sum")]));
}

// ── Cohesion ─────────────────────────────────────────────────────────────────

/// A class of two property groups, each read and written by two methods: with
/// the bare accesses captured the methods of one property are one LCOM4
/// component and the class has two; with only `this.x` captured it has four.
fn split_class(name: &str) -> String {
    format!(
        "package app\n\nclass {name} {{ private var a = 0; private var b = 0\n\
         fun getA() = a\nfun setA(v: Int) {{ a = v }}\n\
         fun getB() = b\nfun setB(v: Int) {{ b = v }}\n}}\n"
    )
}

fn scan_classes(n: usize) -> (TempDir, RecordedSnapshot) {
    let files: Vec<(String, String)> = (0..n)
        .map(|i| (format!("src/Box{i}.kt"), split_class(&format!("Box{i}"))))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let tmp = tree(&refs);
    let engine = index(tmp.path());
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    let member_access = engine
        .registry()
        .expect("a started engine holds its registry")
        .member_access_extensions();
    let snapshot = metrics::snapshot(
        rt,
        &view,
        None,
        metrics::Thresholds::default(),
        &member_access,
    )
    .expect("snapshot runs");
    (tmp, snapshot)
}

#[test]
fn kotlin_cohesion_is_computed_from_the_bare_accesses() {
    let (_tmp, s) = scan_classes(5);
    assert!(
        s.metrics.cohesion.is_some(),
        "five Kotlin classes: Cohesion is scored, not n/a"
    );
    assert!(s.metrics.cohesion_not_applicable.is_none());
    let lcom4: Vec<(&str, &str)> = s
        .worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.detail.as_str()))
        .collect();
    assert_eq!(lcom4.len(), 5, "{lcom4:?}");
    assert!(lcom4.iter().all(|(_, d)| *d == "LCOM4 2"), "{lcom4:?}");
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

const BEFORE: &str = "\
package app

class A { private var n = 0
    fun get(): Int { return n }
}
";
const AFTER: &str = "\
package app

class A { private var n = 0; private var m = 0
    fun get(): Int { return n + m }
}
";

#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    let tmp_a = tree(&[("src/A.kt", BEFORE)]);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("src/A.kt"), AFTER).unwrap();
    engine_a.sync(&[root_a.join("src/A.kt")] as &[PathBuf]);
    let rt_a = engine_a.runtime().unwrap();

    let tmp_b = tree(&[("src/A.kt", AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    assert_eq!(accesses(rt_a), pairs(&[("get", "m"), ("get", "n")]));

    // And back: the edit that drops `m` leaves no stale row or edge of it.
    fs::write(tmp_a.path().join("src/A.kt"), BEFORE).unwrap();
    engine_a.sync(&[root_a.join("src/A.kt")] as &[PathBuf]);
    let tmp_c = tree(&[("src/A.kt", BEFORE)]);
    let engine_c = index(tmp_c.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_c.runtime().unwrap()),
        "sync back to the earlier state must equal index-of-state"
    );
    assert_eq!(
        accesses(engine_a.runtime().unwrap()),
        pairs(&[("get", "n")])
    );
}
