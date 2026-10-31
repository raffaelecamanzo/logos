//! Scala calls resolve through imports, the declared package and the objects
//! they name (S-573, CR-183, FR-RS-36, FR-RS-13, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against the SHIPPED Scala
//! `references.scm`, `symbols.scm` and `plugin.toml`, never a query override.
//!
//! The fixtures follow the two inspection corpora's dominant shapes: gitbucket's
//! Scala 2 `object Util { def m }` called as `Util.m(…)` through an explicit,
//! wildcard or same-package import, and ox's Scala 3 companion pairs
//! (`class Chain` + `object Chain`) and top-level `def`s reached through
//! `import ox.*`.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
#![cfg(feature = "lang-scala")]

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

/// Every bound edge of `kind` whose source lies in the file `rel`, as
/// `source label -> target label`, sorted.
fn edges(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let label = labels(rt);
    let prefix = format!("{rel}:");
    let mut out: Vec<String> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind && label[&e.source].starts_with(&prefix))
        .map(|e| format!("{} -> {}", label[&e.source], label[&e.target]))
        .collect();
    out.sort();
    out
}

/// The reference rows of `kind` the file `rel` records, as `(target, form,
/// receiver shape, arg count, resolved)`, sorted.
type Row = (String, RefForm, Option<ReceiverShape>, Option<u32>, bool);

fn rows(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<Row> {
    let rel = rel.to_string();
    let mut out: Vec<Row> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == kind && r.file_id.and_then(|id| files.get(&id)) == Some(&rel))
                .map(|r| (r.target, r.form, r.receiver, r.arg_count, r.resolved))
                .collect())
        })
        .expect("read runs");
    out.sort_by_key(|row| format!("{row:?}"));
    out
}

fn strings(items: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    out.sort();
    out
}

// ── The library side: a Scala 2 utility object, a companion pair, a lone
//    class, and Scala 3 top-level definitions ──────────────────────────────────

const STRING_UTIL: (&str, &str) = (
    "src/main/scala/app/util/StringUtil.scala",
    "\
package app.util

object StringUtil {
  def decode(s: String): String = s
  def size: Int = 0
  val limit: Int = 1
  def max(other: Int): Int = other
}
",
);

const CHAIN: (&str, &str) = (
    "src/main/scala/app/util/Chain.scala",
    "\
package app.util

class Chain {
  def size(): Int = 0
}

object Chain {
  def empty(): Chain = new Chain
  val inner: Int = 0
}
",
);

const BOX: (&str, &str) = (
    "src/main/scala/app/util/Box.scala",
    "\
package app.util

class Box(n: Int)
",
);

const CONTROL: (&str, &str) = (
    "core/src/main/scala/ox/control.scala",
    "\
package ox

def forever(f: => Unit): Nothing = ???

def discard(x: Any): Unit = ()
",
);

/// FR-RS-36: `Obj.m()` records `Obj::m` and binds among the object's members —
/// through an explicit import, a wildcard import and the declared package
/// alike — and an external object stays unbound.
#[test]
fn an_object_qualified_call_binds_the_objects_member_through_every_import_form() {
    let explicit = "src/main/scala/app/web/Ctl.scala";
    let wildcard = "src/main/scala/app/web/Wild.scala";
    let same_package = "src/main/scala/app/util/Near.scala";
    let tmp = tree(&[
        STRING_UTIL,
        (
            explicit,
            "\
package app.web

import app.util.StringUtil
import scala.util.Try

object Ctl {
  def run(t: String): Unit = {
    StringUtil.decode(t)
    Try.apply(t)
  }
}
",
        ),
        (
            wildcard,
            "\
package app.web

import app.util._

class Wild {
  def run(t: String): String = StringUtil.decode(t)
}
",
        ),
        (
            same_package,
            "\
package app.util

class Near {
  def run(t: String): String = StringUtil.decode(t)
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let decode = "src/main/scala/app/util/StringUtil.scala:decode@4";
    assert_eq!(edges(rt, explicit, EdgeKind::Calls), strings(&[&format!("{explicit}:run@7 -> {decode}")]));
    assert_eq!(edges(rt, wildcard, EdgeKind::Calls), strings(&[&format!("{wildcard}:run@6 -> {decode}")]));
    assert_eq!(edges(rt, same_package, EdgeKind::Calls), strings(&[&format!("{same_package}:run@4 -> {decode}")]));
    // The external object's row is recorded qualified, and stays unbound.
    assert_eq!(
        rows(rt, explicit, EdgeKind::Calls),
        vec![
            ("StringUtil::decode".to_string(), RefForm::Path, None, Some(1), true),
            ("Try::apply".to_string(), RefForm::Path, None, Some(1), false),
        ]
    );
}

/// FR-RS-36: a parameterless member selection `Obj.m` binds only a method — a
/// `val` of the name is never a call target — and records no argument count.
#[test]
fn a_parameterless_selection_on_an_object_binds_only_a_method() {
    let file = "src/main/scala/app/web/Sel.scala";
    let tmp = tree(&[
        STRING_UTIL,
        (
            file,
            "\
package app.web

import app.util.StringUtil

object Sel {
  def run(): Int = StringUtil.size + StringUtil.limit
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:run@6 -> src/main/scala/app/util/StringUtil.scala:size@5")])
    );
    assert_eq!(
        rows(rt, file, EdgeKind::Calls),
        vec![
            ("StringUtil::limit".to_string(), RefForm::Path, None, None, false),
            ("StringUtil::size".to_string(), RefForm::Path, None, None, true),
        ]
    );
}

/// A class and its companion object are one name (S-573): a call on the name
/// binds among the OBJECT's members — the class's instance method of the name
/// is never reached through it — and a wildcard of the pair, or of a value
/// inside it, no longer makes every other import of the file ambiguous.
#[test]
fn a_call_on_a_companion_pair_binds_the_objects_member_and_never_the_classs() {
    let file = "src/main/scala/app/web/Comp.scala";
    let tmp = tree(&[
        STRING_UTIL,
        CHAIN,
        (
            file,
            "\
package app.web

import app.util.Chain
import app.util.Chain._
import app.util.Chain.inner._
import app.util.StringUtil

object Comp {
  def run(t: String): Unit = {
    Chain.empty()
    Chain.size()
    StringUtil.decode(t)
  }
}

def viaWildcard(): Chain = empty()
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:run@9 -> src/main/scala/app/util/Chain.scala:empty@8"),
            &format!("{file}:run@9 -> src/main/scala/app/util/StringUtil.scala:decode@4"),
            // `import app.util.Chain._` brings the object's `empty` into view.
            &format!("{file}:viaWildcard@16 -> src/main/scala/app/util/Chain.scala:empty@8"),
        ])
    );
}

/// FR-RS-36: a Scala 3 top-level `def` is a member of its declared package — an
/// import names it, a wildcard brings it into view, and a free call in the
/// same package reaches it; an `import` of a name no file declares stays
/// unbound.
#[test]
fn a_top_level_def_binds_through_its_import_its_wildcard_and_its_package() {
    let named = "core/src/main/scala/ox/channels/actor.scala";
    let star = "core/src/main/scala/ox/flow/flow.scala";
    let same = "core/src/main/scala/ox/fork.scala";
    let tmp = tree(&[
        CONTROL,
        (
            named,
            "\
package ox.channels

import ox.discard
import ox.missing

def create(x: Int): Unit = discard(x)
",
        ),
        (
            star,
            "\
package ox.flow

import ox.*

def run(): Nothing = forever(())
",
        ),
        (
            same,
            "\
package ox

def forkDiscard(x: Int): Unit = discard(x)
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let control = "core/src/main/scala/ox/control.scala";
    assert_eq!(
        edges(rt, named, EdgeKind::Imports),
        strings(&[&format!("{named}:actor@1 -> {control}:discard@5")])
    );
    assert_eq!(
        rows(rt, named, EdgeKind::Imports),
        vec![
            ("ox::discard".to_string(), RefForm::Path, None, None, true),
            ("ox::missing".to_string(), RefForm::Path, None, None, false),
        ]
    );
    assert_eq!(edges(rt, named, EdgeKind::Calls), strings(&[&format!("{named}:create@6 -> {control}:discard@5")]));
    assert_eq!(edges(rt, star, EdgeKind::Calls), strings(&[&format!("{star}:run@5 -> {control}:forever@3")]));
    assert_eq!(edges(rt, same, EdgeKind::Calls), strings(&[&format!("{same}:forkDiscard@3 -> {control}:discard@5")]));
}

/// FR-RS-36: `new T` records `Instantiates` and binds the one class its scope
/// names; `new` of a companion pair names the pair, which stays unbound.
#[test]
fn new_records_instantiates_and_binds_the_class_its_scope_names() {
    let file = "src/main/scala/app/web/Make.scala";
    let tmp = tree(&[
        BOX,
        CHAIN,
        (
            file,
            "\
package app.web

import app.util.{Box, Chain}

object Make {
  def run(): Unit = {
    new Box(1)
    new Chain()
    new java.util.ArrayList[Int]()
  }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Instantiates),
        strings(&[&format!("{file}:run@6 -> src/main/scala/app/util/Box.scala:Box@3")])
    );
    assert_eq!(
        rows(rt, file, EdgeKind::Instantiates),
        vec![
            ("Box".to_string(), RefForm::Path, None, None, true),
            ("Chain".to_string(), RefForm::Path, None, None, false),
            ("java::util::ArrayList".to_string(), RefForm::Path, None, None, false),
        ]
    );
}

/// FR-RS-36: an infix call `a op b` records a call to `op` on the receiver `a`
/// — `other`, which binds only once the receiver's type is proven (FR-RS-37);
/// `this op b` is a call on the current instance and binds its own `op`. A
/// symbolic operator records nothing.
#[test]
fn an_infix_call_records_its_operator_on_its_left_operand() {
    let file = "src/main/scala/app/web/Inf.scala";
    let tmp = tree(&[(
        file,
        "\
package app.web

class Inf {
  def max(o: Int): Int = o
  def run(a: Int, b: Int): Int = (a max b) + (this max b)
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        rows(rt, file, EdgeKind::Calls),
        vec![
            ("max".to_string(), RefForm::Method, Some(ReceiverShape::Other), None, false),
            ("max".to_string(), RefForm::Method, Some(ReceiverShape::SelfInstance), None, true),
        ]
    );
    assert_eq!(edges(rt, file, EdgeKind::Calls), strings(&[&format!("{file}:run@5 -> {file}:max@4")]));
}

/// A generic call `Obj.m[T](x)` is the call `Obj.m(x)`, and a capitalised value
/// the file declares is a value, never an object to qualify a call by: its
/// qualified row is dropped (S-553's shadow rule), never bound as `Codec::length`.
#[test]
fn a_generic_call_is_qualified_and_a_capitalised_value_is_never_an_object() {
    let file = "src/main/scala/app/web/Gen.scala";
    let tmp = tree(&[
        STRING_UTIL,
        (
            file,
            "\
package app.web

import app.util.StringUtil

object Gen {
  def run(t: String): String = StringUtil.decode[String](t)
  def other(Codec: String): Int = Codec.length()
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:run@6 -> src/main/scala/app/util/StringUtil.scala:decode@4")])
    );
    assert_eq!(
        rows(rt, file, EdgeKind::Calls),
        vec![("StringUtil::decode".to_string(), RefForm::Path, None, Some(1), true)]
    );
}

/// Sync ≡ reindex (NFR-RA-06): giving an object the method a call names, adding
/// a companion object, and adding a top-level `def` each re-bind the calls
/// that read them exactly as a fresh index binds them.
#[test]
fn sync_equals_reindex_for_object_companion_and_top_level_edits() {
    let caller = (
        "src/main/scala/app/web/Ctl.scala",
        "\
package app.web

import app.util.StringUtil
import app.util.Chain
import ox.*

object Ctl {
  def run(t: String): Unit = {
    StringUtil.decode(t)
    Chain.empty()
  }
}

def top(): Unit = discard(1)
",
    );
    let initial = [
        ("src/main/scala/app/util/StringUtil.scala", "package app.util\n\nobject StringUtil\n"),
        ("src/main/scala/app/util/Chain.scala", "package app.util\n\nclass Chain\n"),
        ("core/src/main/scala/ox/control.scala", "package ox\n\ndef forever(): Unit = ()\n"),
        caller,
    ];
    let edits = [STRING_UTIL, CHAIN, CONTROL];
    let tmp_a = tree(&initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let changed: Vec<PathBuf> = edits
        .iter()
        .map(|(rel, text)| {
            write(tmp_a.path(), rel, text);
            root_a.join(rel)
        })
        .collect();
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(graph_fingerprint(rt_a), graph_fingerprint(engine_b.runtime().unwrap()));
    let ctl = caller.0;
    assert_eq!(
        edges(rt_a, ctl, EdgeKind::Calls),
        strings(&[
            &format!("{ctl}:run@8 -> src/main/scala/app/util/Chain.scala:empty@8"),
            &format!("{ctl}:run@8 -> src/main/scala/app/util/StringUtil.scala:decode@4"),
            &format!("{ctl}:top@14 -> core/src/main/scala/ox/control.scala:discard@5"),
        ])
    );
}

/// FR-RS-36: a bare call inside an object no member of which answers it is the
/// free call the imports and the package bind (ox's `forever { … }` inside
/// `object Actor`); every case the file cannot rule a member out stays on the
/// instance — a body that inherits, one the compiler adds members to, a name
/// every instance has — and a member of the name still answers it. A name the
/// caller binds as a value at the call is a call through that value (S-594):
/// `step(1)` applies the parameter, a `local-value` row that binds nothing.
#[test]
fn a_bare_call_no_member_answers_is_free_and_every_other_stays_on_the_instance() {
    let file = "core/src/main/scala/ox/channels/actor.scala";
    let tmp = tree(&[
        CONTROL,
        (
            file,
            "\
package ox.channels

import ox.*

object Actor {
  def create(): Unit = forever(discard(1))
}

class Base

class Inherits extends Base {
  def run(): Unit = forever(())
}

case class Copied(n: Int) {
  def run(): Unit = discard(n)
}

class Applies {
  def run(step: Int => Unit): Unit = step(1)
}

def step(x: Int): Unit = ()

class Own {
  def discard(x: Any): Unit = ()
  def run(): Unit = discard(1)
  def text(): String = toString()
}

class Nested {
  def outer(): Unit = {
    object Inner {
      def run(): Unit = forever(())
    }
  }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let control = "core/src/main/scala/ox/control.scala";
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:create@6 -> {control}:discard@5"),
            &format!("{file}:create@6 -> {control}:forever@3"),
            &format!("{file}:run@27 -> {file}:discard@26"),
            &format!("{file}:run@34 -> {control}:forever@3"),
        ])
    );
    let on_instance: Vec<String> = rows(rt, file, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, form, receiver, _, _)| *form == RefForm::Method && *receiver == Some(ReceiverShape::SelfInstance))
        .map(|(target, ..)| target)
        .collect();
    // `Inherits.run`, `Copied.run`, `Own.run`, `Own.text`.
    assert_eq!(on_instance, strings(&["discard", "discard", "forever", "toString"]));
    let step: Vec<Row> = rows(rt, file, EdgeKind::Calls).into_iter().filter(|(target, ..)| target == "step").collect();
    assert_eq!(step, vec![("step".to_string(), RefForm::Path, None, Some(1), false)], "`Applies.run` calls its parameter");
}

/// A pattern variable and a named `given` are values the file binds (S-573
/// review): a call of one applies it, so it never turns into the free call
/// that would reach a same-named top-level `def`.
#[test]
fn a_call_of_a_pattern_variable_or_a_given_never_turns_free() {
    let file = "src/main/scala/app/Use.scala";
    let tmp = tree(&[
        ("src/main/scala/app/Lib.scala", "package app\n\ndef f(x: Int): Int = x\n\ndef t(): Int = 0\n\ndef g(x: Int): Int = x\n"),
        (
            file,
            "\
package app

class K {
  def go(xs: List[Int => Int]): Int = xs match {
    case f :: rest => f(1)
    case _ => 0
  }
  def each(xs: List[List[Int => Int]]): Unit = for (f :: _ <- xs) f(1)
  def local(): Int = {
    val h :: t = List(1)
    t()
  }
}

class G {
  given g: (Int => Int) = x => x
  def use(): Int = g(1)
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, file, EdgeKind::Calls), Vec::<String>::new());
}

/// A bare call inside `new T { … }` may name `T`'s inherited member, which the
/// file cannot see (S-573 review): it never reaches a same-named top-level
/// `def`, while one the body declares itself still binds; and a top-level
/// call that applies a parameter never reaches the package's `def` of that
/// name.
#[test]
fn a_bare_call_in_an_anonymous_body_or_of_a_value_never_reaches_the_package() {
    let file = "src/main/scala/app/Use.scala";
    let tmp = tree(&[
        ("src/main/scala/app/Lib.scala", "package app\n\ndef helper(): Int = 0\n\ndef f(x: Int): Int = x\n"),
        ("src/main/scala/app/Task.scala", "package app\n\nabstract class Task {\n  def helper(): Int = 1\n  def run(): Int\n}\n"),
        (
            file,
            "\
package app

object O {
  val t = new Task { def run(): Int = helper() }
  val u = new Task {
    def own(): Int = 2
    def run(): Int = own()
  }
}

def apply2(f: Int => Int): Int = f(1)
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, file, EdgeKind::Calls), strings(&[&format!("{file}:run@7 -> {file}:own@6")]));
}

/// A top-level `def` is a member of a Scala package only (S-573 review): a
/// Kotlin file of the same `jvm` family never reaches one by name or by
/// import, and a class of the same name keeps its import and its `new`.
#[cfg(feature = "lang-kotlin")]
#[test]
fn a_top_level_def_is_read_by_scala_only_and_never_hides_a_class_of_its_name() {
    let kotlin = "src/main/kotlin/app/web/K.kt";
    let scala = "src/main/scala/app/web/Use.scala";
    let tmp = tree(&[
        ("src/main/scala/app/Lib.scala", "package app\n\ndef helper(): Int = 0\n"),
        ("src/main/scala/app/Foo.scala", "package app\n\nclass Foo(n: Int)\n"),
        ("src/main/scala/app/FooFactory.scala", "package app\n\ndef Foo(): Foo = new Foo(1)\n"),
        (kotlin, "package app.web\n\nimport app.helper\n\nfun top(): Int = helper()\n"),
        (scala, "package app.web\n\nimport app.Foo\n\nclass Bar {\n  def m(): Foo = new Foo(2)\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, kotlin, EdgeKind::Calls), Vec::<String>::new());
    assert_eq!(edges(rt, kotlin, EdgeKind::Imports), Vec::<String>::new());
    let foo = "src/main/scala/app/Foo.scala:Foo@3";
    assert_eq!(edges(rt, scala, EdgeKind::Imports), strings(&[&format!("{scala}:Use@1 -> {foo}")]));
    assert_eq!(edges(rt, scala, EdgeKind::Instantiates), strings(&[&format!("{scala}:m@6 -> {foo}")]));
}

/// Scala's precedence (S-573 review): a name a wildcard import brings into view
/// outranks one the file's package declares in another file — a top-level
/// `def` of `lib2` over the package's own, an object's method over the
/// package's top-level `def` — while the file's own declaration outranks both.
#[test]
fn a_wildcard_import_outranks_the_packages_other_files_and_the_own_file_outranks_both() {
    let file = "src/main/scala/app/Use.scala";
    let tmp = tree(&[
        ("src/main/scala/app/Lib.scala", "package app\n\ndef helper(): Int = 0\n\ndef t(): Int = 0\n\ndef own(): Int = 0\n"),
        ("src/main/scala/lib2/H.scala", "package lib2\n\ndef helper(): Int = 2\n\ndef own(): Int = 2\n"),
        ("src/main/scala/app/Other.scala", "package app\n\nobject Other {\n  def t(): Int = 3\n}\n"),
        (
            file,
            "package app\n\nimport lib2.*\nimport app.Other._\n\ndef top(): Int = helper()\n\ndef top2(): Int = t()\n\ndef top3(): Int = own()\n\ndef own(): Int = 4\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:top2@8 -> src/main/scala/app/Other.scala:t@4"),
            &format!("{file}:top3@10 -> {file}:own@12"),
            &format!("{file}:top@6 -> src/main/scala/lib2/H.scala:helper@3"),
        ])
    );
}

/// A call on a companion pair that binds records no reason, and one no member
/// of the object admits records why (S-573 review): the object's `make(a)`
/// takes one argument, so `Pair.make()` is `no-applicable-overload`, and
/// `Pair.size()` — the class's instance method, never the object's — is
/// `supertype-unreached`.
#[test]
fn an_unbound_call_on_a_companion_pair_states_its_reason() {
    use logos_core::models::CallResidueReason as R;
    let tmp = tree(&[
        (
            "src/main/scala/app/Pair.scala",
            "package app\n\nclass Pair {\n  def size(): Int = 0\n}\n\nobject Pair {\n  def make(a: Int): Pair = new Pair\n}\n",
        ),
        (
            "src/main/scala/app/web/Use.scala",
            "package app.web\n\nimport app.Pair\n\ndef a(): Pair = Pair.make(1)\n\ndef b(): Pair = Pair.make()\n\ndef c(): Int = Pair.size()\n",
        ),
    ]);
    let engine = index(tmp.path());
    let residue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "scala")
        .and_then(|row| row.call_residue)
        .expect("the scala row states its call residue");
    let reasons: Vec<(R, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    assert_eq!(reasons, vec![(R::NoApplicableOverload, 1), (R::SupertypeUnreached, 1)]);
}

/// A companion pair is one file's two declarations under one parent (S-573
/// review): a class's nested `object Inner` and its object's nested `class
/// Inner` are two names, and a call on `Inner` binds neither; and one class
/// in `DupA.scala` beside one object in `DupB.scala` is no pair either.
#[test]
fn two_same_named_types_under_two_parents_or_in_two_files_are_no_pair() {
    let nested = "src/main/scala/app/web/N.scala";
    let files = "src/main/scala/app/web/D.scala";
    let tmp = tree(&[
        (
            "src/main/scala/app/util/P.scala",
            "package app.util\n\nclass P {\n  object Inner {\n    def m(): Int = 0\n  }\n}\n\nobject P {\n  class Inner {\n    def m(): Int = 0\n  }\n}\n",
        ),
        (nested, "package app.web\n\nimport app.util.P.Inner\n\nobject N {\n  def r(): Int = Inner.m()\n}\n"),
        ("src/main/scala/app/util/DupA.scala", "package app.util\n\nclass Dup {\n  def make(): Int = 0\n}\n"),
        ("src/main/scala/app/util/DupB.scala", "package app.util\n\nobject Dup {\n  def make(): Int = 0\n}\n"),
        (files, "package app.web\n\nimport app.util.Dup\n\nobject D {\n  def r(): Int = Dup.make()\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, nested, EdgeKind::Calls), Vec::<String>::new());
    assert_eq!(edges(rt, files, EdgeKind::Calls), Vec::<String>::new());
}

/// A path through a companion pair reaches a type nested in the object
/// (S-573 review): `import app.util.P.Inner` then `Inner.m()` binds the
/// nested object's `m`.
#[test]
fn a_path_through_a_companion_pair_reaches_a_nested_object() {
    let file = "src/main/scala/app/web/N.scala";
    let tmp = tree(&[
        (
            "src/main/scala/app/util/P.scala",
            "package app.util\n\nclass P\n\nobject P {\n  object Inner {\n    def m(): Int = 0\n  }\n}\n",
        ),
        (file, "package app.web\n\nimport app.util.P.Inner\n\nobject N {\n  def r(): Int = Inner.m()\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:r@6 -> src/main/scala/app/util/P.scala:m@7")])
    );
}

/// The package `def` the value forms below would wrongly reach.
const STEP: (&str, &str) = ("core/src/main/scala/ox/step.scala", "package ox\n\ndef step(x: Int): Unit = ()\n");

/// The `Calls` edges `body` — a file of package `ox.v` importing `ox.*`, alone
/// beside [`STEP`] — makes to the top-level `step`.
fn calls_to_step(name: &str, body: &str) -> Vec<String> {
    let file = format!("core/src/main/scala/ox/v/{name}.scala");
    let text = format!("package ox.v\n\nimport ox.*\n\n{body}");
    let tmp = tree(&[STEP, (file.as_str(), text.as_str())]);
    let engine = index(tmp.path());
    edges(engine.runtime().unwrap(), &file, EdgeKind::Calls)
        .into_iter()
        .filter(|e| e.ends_with(":step@3"))
        .collect()
}

/// Every form that binds a value is read as one (S-573 review): a call of the
/// value applies it, so it never turns into the free call that would reach the
/// package's `def step` — one file per form, since the guard is file-wide.
#[test]
fn every_value_form_keeps_a_call_of_it_off_the_package() {
    let forms = [
        ("lambda", "object L {\n  def r(fs: List[Int => Unit]): Unit = fs.foreach(step => step(1))\n}\n"),
        ("case", "object K {\n  def r(x: Int => Unit): Unit = x match {\n    case step => step(1)\n  }\n}\n"),
        ("typed", "object K {\n  def r(x: Any): Unit = x match {\n    case step: (Int => Unit) => step(1)\n  }\n}\n"),
        ("capture", "object K {\n  def r(x: Int => Unit): Unit = x match {\n    case step @ _ => step(1)\n  }\n}\n"),
        ("for", "object F {\n  def r(fs: List[Int => Unit]): Unit = for (step <- fs) step(1)\n}\n"),
        ("for3", "object F:\n  def r(fs: List[Int => Unit]): Unit =\n    for step <- fs do step(1)\n"),
        ("tuple", "object T {\n  def r(p: (Int => Unit, Int)): Unit = {\n    val (step, n) = p\n    step(n)\n  }\n}\n"),
        ("val", "object V {\n  def r(f: Int => Unit): Unit = {\n    val step = f\n    step(1)\n  }\n}\n"),
        ("var", "object W {\n  def r(f: Int => Unit): Unit = {\n    var step = f\n    step(1)\n  }\n}\n"),
        ("class_param", "class P(step: Int => Unit) {\n  def r(): Unit = step(1)\n}\n"),
        ("member", "object M {\n  val step: Int => Unit = _ => ()\n  def r(): Unit = step(1)\n}\n"),
        ("abstract", "trait D {\n  val step: Int => Unit\n  def r(): Unit = step(1)\n}\n"),
    ];
    for (name, body) in forms {
        assert_eq!(calls_to_step(name, body), Vec::<String>::new(), "{name}");
    }
    // The control: no value of the name, so the call is free and binds.
    assert_eq!(
        calls_to_step("free", "object G {\n  def r(): Unit = step(1)\n}\n"),
        strings(&["core/src/main/scala/ox/v/free.scala:r@6 -> core/src/main/scala/ox/step.scala:step@3"])
    );
}

/// Every body whose members the file cannot all see is open (S-573 review): a
/// bare call inside one stays on the instance and never reaches the package's
/// `def step` — in brace and in Scala 3 indentation syntax alike.
#[test]
fn every_open_body_keeps_a_bare_call_on_the_instance() {
    let bodies = [
        ("case_object", "case object Done {\n  def d(): Unit = step(1)\n}\n"),
        ("enum", "enum Color {\n  case Red\n  def c(): Unit = step(1)\n}\n"),
        ("object_extends", "class Base\n\nobject ObjExt extends Base {\n  def e(): Unit = step(1)\n}\n"),
        ("trait_extends", "class Base\n\ntrait TExt extends Base {\n  def f(): Unit = step(1)\n}\n"),
        ("self_type", "class Base\n\ntrait SelfT { self: Base =>\n  def g(): Unit = step(1)\n}\n"),
        ("class_extends_indent", "class Base\n\nclass Inh extends Base:\n  def a(): Unit = step(1)\n"),
        ("case_class_indent", "case class Cc(n: Int):\n  def b(): Unit = step(1)\n"),
        ("enum_indent", "enum Color:\n  case Red\n  def c(): Unit = step(1)\n"),
        ("object_extends_indent", "class Base\n\nobject ObjExt extends Base:\n  def e(): Unit = step(1)\n"),
        ("self_type_indent", "class Base\n\ntrait SelfT:\n  self: Base =>\n  def g(): Unit = step(1)\n"),
    ];
    for (name, body) in bodies {
        assert_eq!(calls_to_step(name, body), Vec::<String>::new(), "{name}");
    }
}

/// Every form that declares a capitalised value shadows an object of that name
/// (S-573 review): `Codec.length()` on a `val`, `var` or class parameter `Codec`
/// is a call on the value, never on the imported `object Codec`, which an
/// unshadowed call binds.
#[test]
fn every_capitalised_value_form_shadows_an_object_of_its_name() {
    let codec = ("src/main/scala/app/util/Codec.scala", "package app.util\n\nobject Codec {\n  def length(): Int = 0\n}\n");
    let calls = |name: &str, body: &str| {
        let file = format!("src/main/scala/app/web/{name}.scala");
        let text = format!("package app.web\n\nimport app.util.Codec\n\n{body}");
        let tmp = tree(&[codec, (file.as_str(), text.as_str())]);
        let engine = index(tmp.path());
        edges(engine.runtime().unwrap(), &file, EdgeKind::Calls)
    };
    let forms = [
        ("val", "object S {\n  def r(): Int = {\n    val Codec = \"x\"\n    Codec.length()\n  }\n}\n"),
        ("var", "object S {\n  def r(): Int = {\n    var Codec = \"x\"\n    Codec.length()\n  }\n}\n"),
        ("class_param", "class S(Codec: String) {\n  def r(): Int = Codec.length()\n}\n"),
    ];
    for (name, body) in forms {
        assert_eq!(calls(name, body), Vec::<String>::new(), "{name}");
    }
    assert_eq!(
        calls("plain", "object S {\n  def r(): Int = Codec.length()\n}\n"),
        strings(&["src/main/scala/app/web/plain.scala:r@6 -> src/main/scala/app/util/Codec.scala:length@4"])
    );
}

/// ox's dominant shape, Scala 3 indentation syntax (S-573 review): `object
/// Util:` and the pair `class Pair:` + `object Pair:` bind as their brace
/// twins do — `Pair.size()`, the class's, stays unbound — and a bare call,
/// generic or not, inside `object Ind:` is free and reaches the top-level
/// `def`s `import ox.*` brings into view.
#[test]
fn indentation_syntax_binds_objects_pairs_and_free_calls_as_braces_do() {
    let caller = "src/main/scala/app/web/Ind.scala";
    let tmp = tree(&[
        CONTROL,
        (
            "src/main/scala/app/util/Util.scala",
            "package app.util\n\nobject Util:\n  def decode(s: String): String = s\n\nclass Pair:\n  def size(): Int = 0\n\nobject Pair:\n  def empty(): Int = 0\n",
        ),
        (
            caller,
            "package app.web\n\nimport app.util.{Util, Pair}\nimport ox.*\n\nobject Ind:\n  def run(t: String): Unit =\n    Util.decode(t)\n    Pair.empty()\n    Pair.size()\n    forever(())\n    discard[Int](1)\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let control = "core/src/main/scala/ox/control.scala";
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[
            &format!("{caller}:run@7 -> src/main/scala/app/util/Util.scala:decode@4"),
            &format!("{caller}:run@7 -> src/main/scala/app/util/Util.scala:empty@10"),
            &format!("{caller}:run@7 -> {control}:discard@5"),
            &format!("{caller}:run@7 -> {control}:forever@3"),
        ])
    );
}

/// A companion pair's name names both halves (S-573 review): a lookup by kind
/// never picks one out — `new FlowStage {}` beside `trait FlowStage` and
/// `object FlowStage` binds neither, through the package or a wildcard, as
/// ox's `FlowCompanionOps` writes it — while a lone class's `new` binds.
#[test]
fn a_lookup_by_kind_never_picks_one_half_of_a_companion_pair() {
    let same = "src/main/scala/app/Make.scala";
    let wild = "src/main/scala/web/Wild.scala";
    let tmp = tree(&[
        (
            "src/main/scala/app/Flow.scala",
            "package app\n\ntrait FlowStage {\n  def run(): Unit\n}\n\nobject FlowStage {\n  def make(): Int = 0\n}\n\nclass Box\n",
        ),
        (same, "package app\n\nobject Make {\n  def a(): FlowStage = new FlowStage { def run(): Unit = () }\n  def b(): Box = new Box()\n}\n"),
        (wild, "package web\n\nimport app._\n\nobject Wild {\n  def a(): FlowStage = new FlowStage { def run(): Unit = () }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, same, EdgeKind::Instantiates),
        strings(&[&format!("{same}:b@5 -> src/main/scala/app/Flow.scala:Box@11")])
    );
    assert_eq!(edges(rt, wild, EdgeKind::Instantiates), Vec::<String>::new());
}
