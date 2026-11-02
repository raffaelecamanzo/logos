//! Kotlin calls resolve through imports, the package, extensions and companions
//! (S-560, CR-178, FR-RS-31, FR-RS-13, FR-RS-12, NFR-RA-05) — exercised end to
//! end through the public [`Engine`] façade against the SHIPPED Kotlin and Java
//! queries and descriptors, never a query override.
//!
//! The fixtures follow mixit's dominant shapes: an extension on a library type
//! (`fun LocalDateTime.formatTalkTime(…)`, `fun String.toSlug()`) called on a
//! value through an explicit import, a top-level function called bare from a
//! handler class, a companion member called on its class (`Ticket.empty()`),
//! and koin's Java test calling a top-level function through its file facade
//! (`DefaultContextExtKt.startKoin(…)`).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
#![cfg(all(feature = "lang-kotlin", feature = "lang-java"))]

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

/// The `Calls` rows the file `rel` records, as `(target, form, receiver shape,
/// resolved)`, sorted.
type Row = (String, RefForm, Option<ReceiverShape>, bool);

fn calls(rt: &Runtime, rel: &str) -> Vec<Row> {
    let rel = rel.to_string();
    let mut out: Vec<Row> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Calls && r.file_id.and_then(|id| files.get(&id)) == Some(&rel))
                .map(|r| (r.target, r.form, r.receiver, r.resolved))
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

// ── The library side: mixit's util package and a companion ───────────────────

const UTILS: (&str, &str) = (
    "src/main/kotlin/mixit/util/Utils.kt",
    "\
package mixit.util

import java.time.LocalDateTime

fun enumMatcher(s: String): Boolean = s.isNotEmpty()

fun LocalDateTime.formatTalkTime(language: String): String = language

fun String.toSlug(): String = lowercase()
",
);

const TICKET: (&str, &str) = (
    "src/main/kotlin/mixit/ticket/Ticket.kt",
    "\
package mixit.ticket

data class Ticket(val id: String) {
    companion object {
        fun empty(): Ticket = Ticket(\"\")
    }
}
",
);

const TALK_UTILS: (&str, &str) = (
    "src/main/kotlin/mixit/talk/TalkUtils.kt",
    "\
package mixit.talk

fun talkHelper(): Int = 1
",
);

const HANDLER: &str = "src/main/kotlin/mixit/talk/TalkHandler.kt";
const HANDLER_TEXT: &str = "\
package mixit.talk

import mixit.ticket.Ticket
import mixit.util.enumMatcher
import mixit.util.formatTalkTime
import mixit.util.toSlug
import java.time.LocalDateTime

class TalkHandler {
    fun view(start: LocalDateTime, title: String) {
        enumMatcher(title)
        start.formatTalkTime(\"fr\")
        title.toSlug()
        Ticket.empty()
        talkHelper()
    }
}

fun route(start: LocalDateTime, title: String) {
    enumMatcher(title)
    start.formatTalkTime(\"fr\")
    Ticket.empty()
    talkHelper()
}
";

/// FR-RS-31: inside a class and at the top level alike, an imported top-level
/// function, an imported extension, a companion member and a same-package
/// function each bind to the one declaration in another file.
#[test]
fn imported_functions_extensions_companions_and_package_functions_bind_across_files() {
    let tmp = tree(&[UTILS, TICKET, TALK_UTILS, (HANDLER, HANDLER_TEXT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let (utils, ticket, talk) = (UTILS.0, TICKET.0, TALK_UTILS.0);
    assert_eq!(
        edges(rt, HANDLER, EdgeKind::Calls),
        strings(&[
            &format!("{HANDLER}:route@19 -> {talk}:talkHelper@3"),
            &format!("{HANDLER}:route@19 -> {ticket}:empty@5"),
            &format!("{HANDLER}:route@19 -> {utils}:enumMatcher@5"),
            &format!("{HANDLER}:route@19 -> {utils}:formatTalkTime@7"),
            &format!("{HANDLER}:view@10 -> {talk}:talkHelper@3"),
            &format!("{HANDLER}:view@10 -> {ticket}:empty@5"),
            &format!("{HANDLER}:view@10 -> {utils}:enumMatcher@5"),
            &format!("{HANDLER}:view@10 -> {utils}:formatTalkTime@7"),
            &format!("{HANDLER}:view@10 -> {utils}:toSlug@9"),
        ])
    );
    // A call on a class is the Path its class qualifies (`Ticket::empty`); a
    // receiver-less call inside a class that declares none of its name and
    // inherits nothing is the free Path call.
    let rows = calls(rt, HANDLER);
    assert!(rows.contains(&("Ticket::empty".to_string(), RefForm::Path, None, true)), "{rows:?}");
    // One row per call: never a second, receiver-shaped `empty` beside it.
    assert_eq!(rows.iter().filter(|(t, _, _, _)| t.ends_with("empty")).count(), 2, "{rows:?}");
    assert!(rows.contains(&("enumMatcher".to_string(), RefForm::Path, None, true)), "{rows:?}");
}

/// FR-RS-12, NFR-RA-05: a bare call stays on the instance wherever a member
/// may answer it — the class declares the name, inherits (a supertype, a
/// `data` or `enum class`), or the file binds the name as a value, which the
/// call invokes — and `this.f()` is never a free call.
#[test]
fn a_bare_call_a_member_or_a_value_may_answer_stays_on_the_instance() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let tmp = tree(&[
        (
            "src/main/kotlin/app/web/Top.kt",
            "package app.web\n\nfun helper(): Int = 0\nfun block(): Int = 0\nfun copyOf(): Int = 0\n",
        ),
        (
            caller,
            "\
package app.web

class Own {
    fun helper(): Int = 1
    fun run() { helper() }
}

class Values(val block: () -> Int) {
    fun run() { block() }
}

data class Data(val id: Int) {
    fun run() { copyOf() }
}

class Explicit {
    fun run() { this.helper() }
}

fun top(block: () -> Int) {
    block()
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:run@5 -> {caller}:helper@4")])
    );
    let rows = calls(rt, caller);
    assert!(rows.iter().all(|(t, form, _, _)| t != "copyOf" || *form == RefForm::Method), "{rows:?}");
    // `Values.run` invokes the property on the instance; `top` invokes its
    // parameter — a call through a local value (S-594), the one Path row.
    let block: Vec<_> = rows.iter().filter(|(t, ..)| t == "block").collect();
    assert_eq!(
        block,
        [&("block".to_string(), RefForm::Method, Some(ReceiverShape::SelfInstance), false), &("block".to_string(), RefForm::Path, None, false)],
        "{rows:?}"
    );
}

/// FR-RS-31, NFR-RA-05: two extensions of one name in scope whose receiver
/// type both apply bind neither, while the receiver's proven type tells a
/// `String` extension from an `Int` one (S-833); a call on a value never binds
/// a top-level function that is no extension.
#[test]
fn an_ambiguous_extension_and_a_plain_function_named_by_a_receiver_call_stay_unbound() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let tmp = tree(&[
        ("src/main/kotlin/app/a/Text.kt", "package app.a\n\nfun String.pretty(): String = this\n"),
        ("src/main/kotlin/app/b/Numbers.kt", "package app.b\n\nfun String.pretty(): String = \"\"\n"),
        (
            "src/main/kotlin/app/web/Same.kt",
            "package app.web\n\nfun String.shout(): String = this\nfun Int.shout(): String = \"\"\nfun find(): Int = 0\nfun String.collectList(): Int = 0\n",
        ),
        (
            caller,
            "\
package app.web

import app.a.pretty
import app.b.pretty
import kotlinx.coroutines.flow.collectList

class Ctl {
    fun run(s: String, repo: Repo) {
        s.pretty()
        s.shout()
        repo.find()
        s.collectList()
    }
}

class Repo
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `collectList` is imported from a library: the import is in force, and
    // the same-package extension of its name is never reached past it.
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&["src/main/kotlin/app/web/Ctl.kt:run@8 -> src/main/kotlin/app/web/Same.kt:shout@3"])
    );
    // Each receiver's type is proven (S-563): the rows are typed, and the
    // ambiguous, missing and library ones unbound.
    let rows = calls(rt, caller);
    assert!(rows.contains(&("String::shout".to_string(), RefForm::Path, Some(ReceiverShape::Other), true)), "{rows:?}");
    for target in ["String::pretty", "Repo::find", "String::collectList"] {
        assert!(
            rows.contains(&(target.to_string(), RefForm::Path, Some(ReceiverShape::Other), false)),
            "{target}: {rows:?}"
        );
    }
    // Two extensions in scope say why they bind neither. `Repo` declares no
    // `find` and no extension of the name is in scope; `String` is no
    // repository type and the import names a library's extension.
    assert_eq!(
        kotlin_residue(&engine),
        vec![
            (CallResidueReason::ExternalType, 1),
            (CallResidueReason::OverloadAmbiguous, 1),
            (CallResidueReason::SupertypeUnreached, 1),
        ]
    );
}

/// Every call-residue reason the Kotlin row states, with its count, sorted.
fn kotlin_residue(engine: &Engine) -> Vec<(CallResidueReason, u64)> {
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "kotlin")
        .and_then(|row| row.call_residue)
        .expect("the kotlin row states its call residue");
    let mut reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    reasons.sort();
    reasons
}

/// FR-RS-31: a single extension of the name binds from the package alone, an
/// extension whose parameters do not admit the call is no candidate, and only
/// a callable member of the source's family rules an extension out — not a
/// property of the name, nor a method of another language's class.
#[test]
fn an_extension_binds_from_the_package_by_its_parameters_past_non_members() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let ext = "src/main/kotlin/app/web/Ext.kt";
    let mut files = vec![
        (
            ext,
            "package app.web\n\nfun String.whisper(): String = this\nfun String.pad(): String = this\nfun Int.pad(width: Int): Int = width\nfun String.size2(): Int = 0\nfun String.save2(): Int = 0\n",
        ),
        ("src/main/kotlin/app/web/Holder.kt", "package app.web\n\nclass Holder {\n    val size2: Int = 0\n}\n"),
        (
            caller,
            "package app.web\n\nclass Ctl {\n    fun run(s: String) {\n        s.whisper()\n        s.pad()\n        s.size2()\n        s.save2()\n    }\n}\n",
        ),
    ];
    if cfg!(feature = "lang-python") {
        files.push(("py/repo.py", "class Repo:\n    def save2(self):\n        return 1\n"));
    }
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[
            &format!("{caller}:run@4 -> {ext}:pad@4"),
            &format!("{caller}:run@4 -> {ext}:save2@7"),
            &format!("{caller}:run@4 -> {ext}:size2@6"),
            &format!("{caller}:run@4 -> {ext}:whisper@3"),
        ])
    );
}

/// FR-RS-31: `import app.text.*` brings the package's top-level functions and
/// extensions into view, as its types.
#[test]
fn a_wildcard_import_brings_top_level_functions_and_extensions_into_view() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let text = "src/main/kotlin/app/text/Text.kt";
    let tmp = tree(&[
        (text, "package app.text\n\nfun String.shout(): String = this\n\nfun topFn(): Int = 0\n"),
        (
            caller,
            "package app.web\n\nimport app.text.*\n\nclass Ctl {\n    fun run(s: String) = s.shout()\n}\n\nfun top(): Int = topFn()\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:run@6 -> {text}:shout@3"), &format!("{caller}:top@9 -> {text}:topFn@5")])
    );
}

/// FR-RS-12, NFR-RA-05: each form that lets a member or a value answer a bare
/// call keeps it off the top-level `helper` — an `enum class`, an `object`
/// with a supertype, an object expression, a `catch`, local or parameter of
/// the name, and `Any`'s `toString` — while an `inner` class, which inherits
/// nothing, binds it. One file per form, since a value is read file-wide.
#[test]
fn every_form_a_member_or_value_may_answer_keeps_a_bare_call_on_the_instance() {
    let dir = "src/main/kotlin/app/cases";
    let cases = [
        ("Enum.kt", "enum class Color {\n    RED;\n    fun run() { helper() }\n}\n"),
        ("Obj.kt", "open class Base0\n\nobject Reg : Base0() {\n    fun run() { helper() }\n}\n"),
        ("Literal.kt", "fun make(): Runnable = object : Runnable {\n    override fun run() { helper() }\n}\n"),
        ("Catch.kt", "class C1 {\n    fun a() { try { } catch (helper: Exception) { } }\n    fun run() { helper() }\n}\n"),
        ("Local.kt", "class C2 {\n    fun run() {\n        val helper = { 1 }\n        helper()\n    }\n}\n"),
        ("Param.kt", "class C3 {\n    fun run(helper: () -> Int) { helper() }\n}\n"),
        ("Any.kt", "class C4 {\n    fun run() = toString()\n}\n"),
    ];
    let paths: Vec<String> = cases.iter().map(|(f, _)| format!("{dir}/{f}")).collect();
    let texts: Vec<String> = cases.iter().map(|(_, t)| format!("package app.cases\n\n{t}")).collect();
    let inner = format!("{dir}/Inner.kt");
    let mut files: Vec<(&str, &str)> = paths.iter().map(String::as_str).zip(texts.iter().map(String::as_str)).collect();
    let top = format!("{dir}/Top.kt");
    files.push((&top, "package app.cases\n\nfun helper(): Int = 0\n\nfun toString(): String = \"\"\n"));
    files.push((&inner, "package app.cases\n\nclass Outer {\n    inner class In {\n        fun run() { helper() }\n    }\n}\n"));
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    for path in &paths {
        assert!(edges(rt, path, EdgeKind::Calls).is_empty(), "{path}: {:?}", edges(rt, path, EdgeKind::Calls));
    }
    assert_eq!(edges(rt, &inner, EdgeKind::Calls), vec![format!("{inner}:run@5 -> {top}:helper@3")]);
}

/// NFR-RA-05: a capitalised local or constructor property shadows a class name
/// as a parameter does — no class-qualified row, no edge.
#[test]
fn a_capitalised_local_or_constructor_property_shadows_a_class_name() {
    let local = "src/main/kotlin/mixit/talk/Local.kt";
    let prop = "src/main/kotlin/mixit/talk/Prop.kt";
    let tmp = tree(&[
        TICKET,
        (local, "package mixit.talk\n\nimport mixit.ticket.Ticket\n\nfun run() {\n    val Ticket = Any()\n    Ticket.empty()\n}\n"),
        (prop, "package mixit.talk\n\nimport mixit.ticket.Ticket\n\nclass C(val Ticket: Any) {\n    fun run() { Ticket.empty() }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    for file in [local, prop] {
        assert!(edges(rt, file, EdgeKind::Calls).is_empty(), "{file}: {:?}", edges(rt, file, EdgeKind::Calls));
        assert!(calls(rt, file).iter().all(|(t, _, _, _)| t != "Ticket::empty"), "{file}: {:?}", calls(rt, file));
    }
}

/// FR-RS-31: a member of the name wins over an extension. A receiver whose
/// type the file proves binds that type's member (S-563); one it proves no
/// type for may be any class's, so a callable member of the name on any class
/// of the family keeps the call off the extension.
#[test]
fn an_extension_named_like_a_member_stays_unbound() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let tmp = tree(&[
        ("src/main/kotlin/app/web/Ext.kt", "package app.web\n\nfun Ctl.save(): Int = 0\n"),
        ("src/main/kotlin/app/repo/Repo.kt", "package app.repo\n\nclass Repo {\n    fun save(): Int = 1\n}\n"),
        (
            caller,
            "\
package app.web

import app.repo.Repo

class Ctl {
    fun run(repo: Repo) {
        repo.save()
    }
    fun untyped(xs: List<Repo>) {
        xs.forEach { r -> r.save() }
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:run@6 -> src/main/kotlin/app/repo/Repo.kt:save@4")])
    );
    assert!(
        calls(rt, caller).contains(&("save".to_string(), RefForm::Method, Some(ReceiverShape::Other), false)),
        "{:?}",
        calls(rt, caller)
    );
}

/// FR-RS-12, FR-RS-31: a bare call inside a class binds the class's member, an
/// inherited one included, before a top-level function of the name; a class
/// whose base the graph does not hold may inherit the name, so its bare call
/// binds no top-level function.
#[test]
fn a_bare_call_binds_a_member_first_and_an_open_hierarchy_never_falls_through() {
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let tmp = tree(&[
        ("src/main/kotlin/app/web/Top.kt", "package app.web\n\nfun helper(): Int = 0\nfun other(): Int = 0\n"),
        ("src/main/kotlin/app/web/Base.kt", "package app.web\n\nopen class Base {\n    fun helper(): Int = 1\n}\n"),
        (
            caller,
            "\
package app.web

import org.springframework.Controller

class Ctl : Base() {
    fun run() {
        helper()
    }
}

class Spring : Controller() {
    fun run() {
        other()
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:run@6 -> src/main/kotlin/app/web/Base.kt:helper@4")])
    );
}

/// FR-RS-31: Java reaches a Kotlin file's top-level functions through its file
/// facade `<File>Kt` through its import, and the import binds the facade's
/// file. Kotlin itself never names the facade.
#[test]
fn java_calls_a_top_level_function_through_its_file_facade() {
    let java = "src/main/java/mixit/boot/Boot.java";
    let kotlin = "src/main/kotlin/mixit/boot/Kt.kt";
    let tmp = tree(&[
        UTILS,
        (
            java,
            "\
package mixit.boot;

import mixit.util.UtilsKt;

class Boot {
    void start() {
        UtilsKt.enumMatcher(\"x\");
    }

    void slug() {
        UtilsKt.toSlug(\"Title\");
    }
}
",
        ),
        (kotlin, "package mixit.boot\n\nimport mixit.util.UtilsKt\n\nfun k() {\n    UtilsKt.enumMatcher(\"k\")\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // An extension takes its receiver as the first argument through the
    // facade: `toSlug()` declares none, and Java passes one.
    assert_eq!(
        edges(rt, java, EdgeKind::Calls),
        strings(&[
            &format!("{java}:slug@10 -> {}:toSlug@9", UTILS.0),
            &format!("{java}:start@6 -> {}:enumMatcher@5", UTILS.0),
        ])
    );
    assert_eq!(edges(rt, java, EdgeKind::Imports), vec![format!("{java}:Boot@1 -> {}:Utils@1", UTILS.0)]);
    assert!(edges(rt, kotlin, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, kotlin, EdgeKind::Calls));
    assert!(edges(rt, kotlin, EdgeKind::Imports).is_empty(), "{:?}", edges(rt, kotlin, EdgeKind::Imports));
}

/// FR-RS-31, NFR-RA-05: a bare call reaches an extension only through an
/// implicit receiver the graph sees — an extension's body, whose `this` may be
/// its receiver, or a class of the extension's receiver type (S-833) — so
/// `Holder` reaches no `KoinTest` extension; and never one named like a
/// member — koin's `single { X(get()) }` calls the lambda receiver's member
/// `Scope.get()`, never the same-package `fun KoinTest.get()`.
#[test]
fn a_bare_call_binds_an_extension_only_through_an_implicit_receiver_and_never_past_a_member() {
    let file = "src/main/kotlin/app/di/Di.kt";
    let tmp = tree(&[(
        file,
        "\
package app.di

interface KoinTest

class Scope {
    fun get(): Int = 1
}

fun KoinTest.get(): Int = 0

fun KoinTest.inject(): Int = 0

fun String.shout(): String = this

class Holder {
    fun build(): Int = inject()
}

fun module(): Int = inject()

fun String.loud(): String = shout()

fun Scope.make(): Int = get()
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:loud@21 -> {file}:shout@13")])
    );
}

/// FR-RS-43: a bare call reaches a top-level function through the package or
/// an import only when the function's parameters admit the call, as the
/// lexical rungs already require — `size(1, 2, 3)` never binds `fun size()`.
#[test]
fn a_package_or_imported_function_binds_only_a_call_its_parameters_admit() {
    let caller = "src/main/kotlin/app/web/Use.kt";
    let tmp = tree(&[
        ("src/main/kotlin/app/web/Top.kt", "package app.web\n\nfun size(): Int = 0\n"),
        ("src/main/kotlin/app/lib/Lib.kt", "package app.lib\n\nfun width(): Int = 0\n"),
        (
            caller,
            "package app.web\n\nimport app.lib.width\n\nfun run(): Int = size(1, 2, 3) + width(4)\n\nfun ok(): Int = size() + width()\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[
            &format!("{caller}:ok@7 -> src/main/kotlin/app/lib/Lib.kt:width@3"),
            &format!("{caller}:ok@7 -> src/main/kotlin/app/web/Top.kt:size@3"),
        ])
    );
}

/// FR-RS-12, NFR-RA-05: inside an extension's body the receiver's own members
/// answer a bare call first — `fun Box.twice() = size() * 2` calls `Box.size`,
/// never a top-level `size`, whether the receiver is written plain or
/// qualified (`fun app.box.Box.thrice()`) — and a companion with a supertype
/// is an implicit receiver of its whole class.
#[test]
fn a_receivers_member_hides_a_top_level_function_from_an_extension_or_a_companion() {
    let file = "src/main/kotlin/app/box/Ext.kt";
    let tmp = tree(&[
        (
            "src/main/kotlin/app/box/Top.kt",
            "package app.box\n\nfun size(): Int = 0\nfun log(msg: String): Int = 0\nfun other(): Int = 0\n",
        ),
        (
            file,
            "\
package app.box

class Box {
    fun size(): Int = 1
}

fun Box.twice(): Int = size() * 2

fun Box.more(): Int = other()

fun app.box.Box.thrice(): Int = size() * 3

interface Logging {
    fun log(msg: String) {}
}

class Svc {
    companion object : Logging
    fun run() { log(\"x\") }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:more@9 -> src/main/kotlin/app/box/Top.kt:other@5")])
    );
}

/// FR-RS-31 (I4-INT): a class's companion is the implicit receiver after the
/// class itself, so a bare call inside the class reaches the companion's
/// member — `class C { fun a() { make() } companion object { fun make() } }`,
/// a free call to the extractor since `C` declares no `make`, and mixit's enum
/// calling its companion's private functions, a call on the instance. A member
/// of the class's own that the call can take still comes first, and one it
/// cannot take is passed over to the companion's. A call on the class,
/// `Own.build()`, binds the companion's member over an instance member of the
/// same name.
#[test]
fn a_bare_call_inside_a_class_reaches_its_companions_member() {
    let file = "src/main/kotlin/app/tabs/Tabs.kt";
    let tmp = tree(&[(
        file,
        "\
package app.tabs

class C {
    fun a(): C = make()
    companion object {
        fun make(): C = C()
    }
}

enum class TalksTabs {
    Current, Old;
    fun tabs(): List<TalksTabs> = listOf(current(), old())
    companion object {
        private fun current(): TalksTabs = Current
        private fun old(): TalksTabs = Old
    }
}

class Own {
    fun build(): Int = 1
    fun a(): Int = build()
    fun b(): Int = Own.build()
    companion object {
        fun build(): Int = 2
    }
}

class Pick {
    fun make(x: Int): Int = x
    fun a(): Int = make()
    companion object {
        fun make(): Int = 0
    }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:a@4 -> {file}:make@6"),
            &format!("{file}:a@21 -> {file}:build@20"),
            &format!("{file}:a@30 -> {file}:make@32"),
            &format!("{file}:b@22 -> {file}:build@24"),
            &format!("{file}:tabs@12 -> {file}:current@14"),
            &format!("{file}:tabs@12 -> {file}:old@15"),
        ])
    );
}

/// FR-RS-12, FR-RS-43 (I4-INT): a bare call inside a subclass reaches its
/// superclass's companion — Kotlin links a superclass's companion scope into
/// its subclasses — before a top-level function, even past a member of its own
/// that the call cannot take; a class nested in a class reaches the outer
/// class's companion; and a class whose base the graph does not hold reaches
/// no companion, since that base may declare the name.
#[test]
fn a_bare_call_reaches_a_superclass_or_outer_companion_but_never_past_an_unseen_base() {
    let base = "src/main/kotlin/app/s/Base.kt";
    let top = "src/main/kotlin/app/s/Top.kt";
    let file = "src/main/kotlin/app/s/Use.kt";
    let tmp = tree(&[
        (base, "package app.s\n\nopen class Base {\n    companion object {\n        fun make(): Int = 0\n    }\n}\n"),
        (top, "package app.s\n\nfun make(): Int = 1\nfun module(): Int = 1\n"),
        (
            file,
            "\
package app.s

import org.springframework.Controller

class D : Base() {
    fun make(x: Int): Int = x
    fun a(): Int = make()
}

class Outer {
    class Nested {
        fun b(): Int = build()
    }
    companion object {
        fun build(): Int = 2
    }
}

class Ext : Controller() {
    fun c(): Int = build()
    companion object {
        fun build(): Int = 3
    }
}

enum class E {
    X;
    fun module(a: Int, b: Int): Int = a + b
    fun d(): Int = module()
    companion object {
        fun other(): Int = 4
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:a@7 -> {base}:make@5"),
            &format!("{file}:b@12 -> {file}:build@15"),
            &format!("{file}:d@29 -> {top}:module@4"),
        ])
    );
}

/// FR-RS-43, NFR-RA-05 (I4-INT): a call on a class with a companion is decided
/// by the companion alone — an instance member of the class is never what
/// `W.make(1)` calls. A companion holding the name with no overload the call
/// admits is `no-applicable-overload`, one holding two it admits is
/// `overload-ambiguous`, and one holding none — its supertype is not recorded
/// — leaves the call unbound rather than reaching the instance member.
#[test]
fn a_call_on_a_class_with_a_companion_never_reaches_an_instance_member() {
    let file = "src/main/kotlin/app/w/W.kt";
    let cases: [(&str, Option<CallResidueReason>); 3] = [
        (
            "class W {\n    fun make(x: Int): Int = x\n    companion object {\n        fun make(): Int = 0\n    }\n}\n\nfun use(): Int = W.make(1)\n",
            Some(CallResidueReason::NoApplicableOverload),
        ),
        (
            "class W {\n    fun make(): Int = 1\n    companion object {\n        fun make(): Int = 0\n        fun make(x: Int = 1): Int = x\n    }\n}\n\nfun use(): Int = W.make()\n",
            Some(CallResidueReason::OverloadAmbiguous),
        ),
        (
            "open class Factory {\n    fun make(): Int = 2\n}\n\nclass W {\n    fun make(): Int = 1\n    companion object : Factory()\n}\n\nfun use(): Int = W.make()\n",
            None,
        ),
    ];
    for (text, reason) in cases {
        let tmp = tree(&[(file, &format!("package app.w\n\n{text}"))]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        let from_use: Vec<String> = edges(rt, file, EdgeKind::Calls).into_iter().filter(|e| e.contains(":use@")).collect();
        assert!(from_use.is_empty(), "{text}: {from_use:?}");
        if let Some(reason) = reason {
            assert_eq!(kotlin_residue(&engine), vec![(reason, 1)], "{text}");
        }
    }
}

/// NFR-RA-05 (I4-INT): only a class has a companion. A top-level type named
/// like one — an `object Companion` or a `class Companion` — lends the file no
/// scope: a bare call beside it binds the imported or same-package function
/// Kotlin calls, never the type's member.
#[test]
fn a_top_level_type_named_companion_is_no_files_companion() {
    let file = "src/main/kotlin/app/p/A.kt";
    let lib = "src/main/kotlin/app/q/B.kt";
    let other = "src/main/kotlin/app/p/C.kt";
    let tmp = tree(&[
        (lib, "package app.q\n\nfun make(): Int = 2\n"),
        (other, "package app.p\n\nfun stray(): Int = 3\n"),
        (
            file,
            "\
package app.p

import app.q.make

object Companion {
    fun make(): Int = 1
    fun stray(): Int = 1
}

fun a(): Int = make()
fun b(): Int = stray()
",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        edges(engine.runtime().unwrap(), file, EdgeKind::Calls),
        strings(&[&format!("{file}:a@10 -> {lib}:make@3"), &format!("{file}:b@11 -> {other}:stray@3")])
    );
}

/// FR-RS-31: an extension of a generic or nullable receiver records its
/// receiver too — `fun <T> List<T>.second()`, `fun String?.orBlank()` — and a
/// receiver call binds it.
#[test]
fn an_extension_of_a_generic_or_nullable_receiver_binds() {
    let caller = "src/main/kotlin/app/web/Use.kt";
    let lib = "src/main/kotlin/app/util/Ext.kt";
    let tmp = tree(&[
        (lib, "package app.util\n\nfun <T> List<T>.second(): T = this[1]\n\nfun String?.orBlank(): String = this ?: \"\"\n"),
        (
            caller,
            "package app.web\n\nimport app.util.orBlank\nimport app.util.second\n\nfun run(xs: List<Int>, s: String?): Int = xs.second() + s.orBlank().length\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:run@6 -> {lib}:orBlank@5"), &format!("{caller}:run@6 -> {lib}:second@3")])
    );
}

/// NFR-RA-05: a capitalised value the file declares is no class — a call on it
/// records no class-qualified row, so it never binds the class's member.
#[test]
fn a_call_on_a_capitalised_value_is_never_a_call_on_a_class() {
    let caller = "src/main/kotlin/mixit/talk/Ctl.kt";
    let tmp = tree(&[
        TICKET,
        (
            caller,
            "package mixit.talk\n\nimport mixit.ticket.Ticket\n\nfun run(Ticket: Any) {\n    Ticket.empty()\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, caller, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, caller, EdgeKind::Calls));
    assert!(calls(rt, caller).iter().all(|(t, _, _, _)| t != "Ticket::empty"), "{:?}", calls(rt, caller));
}

/// NFR-RA-05: Kotlin and Scala share the `jvm` family and both declare
/// top-level functions, but neither names the other's by its package — a
/// Kotlin call of `helper` in a package where a Scala file declares one binds
/// only the Kotlin `helper`, and the Scala call only the Scala one.
#[cfg(feature = "lang-scala")]
#[test]
fn kotlin_and_scala_top_level_functions_of_one_package_never_bind_across_languages() {
    let kotlin = "src/main/kotlin/app/Caller.kt";
    let scala = "src/main/scala/app/Caller.scala";
    let tmp = tree(&[
        ("src/main/kotlin/app/Helpers.kt", "package app\n\nfun kotlinOnly(): Int = 0\nfun helper(): Int = 0\n"),
        ("src/main/scala/app/helpers.scala", "package app\n\ndef scalaOnly(): Int = 0\ndef helper(): Int = 0\n"),
        (kotlin, "package app\n\nfun k() {\n    helper()\n    scalaOnly()\n}\n"),
        (scala, "package app\n\ndef s(): Unit = {\n  helper()\n  kotlinOnly()\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, kotlin, EdgeKind::Calls),
        vec![format!("{kotlin}:k@3 -> src/main/kotlin/app/Helpers.kt:helper@4")]
    );
    assert_eq!(
        edges(rt, scala, EdgeKind::Calls),
        vec![format!("{scala}:s@3 -> src/main/scala/app/helpers.scala:helper@4")]
    );
}

/// FR-RS-31, NFR-RA-05: two files of one facade class — a Multiplatform
/// `common` and `jvm` `Utils.kt` of one package — bind neither, and a facade's
/// overloads bind the one the Java call's arguments admit.
#[test]
fn a_facade_two_files_name_binds_neither_and_its_overloads_bind_by_arity() {
    let java = "src/main/java/app/boot/Boot.java";
    let tmp = tree(&[
        ("common/src/main/kotlin/app/util/Utils.kt", "package app.util\n\nfun f(a: Int): Int = a\n"),
        ("jvm/src/main/kotlin/app/util/Utils.kt", "package app.util\n\nfun g(a: Int): Int = a\n"),
        ("src/main/kotlin/app/math/Math.kt", "package app.math\n\nfun f(a: Int): Int = a\n\nfun f(a: Int, b: Int): Int = a + b\n"),
        (
            java,
            "package app.boot;\n\nimport app.util.UtilsKt;\nimport app.math.MathKt;\n\nclass Boot {\n    void start() {\n        UtilsKt.f(1);\n        MathKt.f(1);\n    }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, java, EdgeKind::Calls),
        vec![format!("{java}:start@7 -> src/main/kotlin/app/math/Math.kt:f@3")]
    );
}

/// FR-RS-43: a facade call no function of its name admits — `enumMatcher` takes
/// one argument, the call passes none — binds nothing and says so,
/// `no-applicable-overload`, as every arity-filtered arm does.
#[test]
fn a_facade_call_no_function_admits_is_no_applicable_overload() {
    let java = "src/main/java/mixit/boot/Boot.java";
    let tmp = tree(&[
        UTILS,
        (
            java,
            "package mixit.boot;\n\nimport mixit.util.UtilsKt;\n\nclass Boot {\n    void start() {\n        UtilsKt.enumMatcher();\n    }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert!(edges(engine.runtime().unwrap(), java, EdgeKind::Calls).is_empty());
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .and_then(|row| row.call_residue)
        .expect("the java row states its call residue");
    let reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    assert_eq!(reasons, vec![(CallResidueReason::NoApplicableOverload, 1)]);
}

/// NFR-RA-06: a facade is named after its file, which none of its nodes spells,
/// but its path spells the file's package — so deleting the file, from a
/// directory that does not spell that package, unbinds Java's import of the
/// facade as a fresh index of the final tree does.
#[test]
fn sync_equals_reindex_when_a_facades_file_is_deleted() {
    let utils = "src/main/kotlin/shared/Utils.kt";
    let java = "src/main/java/app/boot/Boot.java";
    let files = [
        (utils, "package com.acme.text\n\nfun enumMatcher(s: String): Boolean = s.isNotEmpty()\n"),
        (java, "package app.boot;\n\nimport com.acme.text.UtilsKt;\n\nclass Boot {\n}\n"),
    ];
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, java, EdgeKind::Imports), vec![format!("{java}:Boot@1 -> {utils}:Utils@1")]);
    let root = tmp.path().canonicalize().expect("canonicalize root");
    fs::remove_file(root.join(utils)).unwrap();
    engine.pipeline().sync(&[root.join(utils)]);
    let fresh = tree(&files[1..]);
    let engine_b = index(fresh.path());
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(engine_b.runtime().unwrap()));
}

/// NFR-RA-06: a sync that adds a top-level function, an extension, a companion
/// member and a facade's file binds what a fresh index of the final tree binds.
#[test]
fn sync_equals_reindex_for_top_level_extension_companion_and_facade_edits() {
    let java = (
        "src/main/java/mixit/boot/Boot.java",
        "package mixit.boot;\n\nimport mixit.util.UtilsKt;\n\nclass Boot {\n    void start() {\n        UtilsKt.enumMatcher(\"x\");\n    }\n}\n",
    );
    let initial = [
        ("src/main/kotlin/mixit/ticket/Ticket.kt", "package mixit.ticket\n\ndata class Ticket(val id: String)\n"),
        ("src/main/kotlin/mixit/talk/Other.kt", "package mixit.talk\n\nfun unrelated(): Int = 0\n"),
        (HANDLER, HANDLER_TEXT),
        java,
    ];
    let edits = [UTILS, TICKET, TALK_UTILS];
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
    engine_a.pipeline().sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(graph_fingerprint(rt_a), graph_fingerprint(engine_b.runtime().unwrap()));
    assert_eq!(edges(rt_a, HANDLER, EdgeKind::Calls).len(), 9, "{:?}", edges(rt_a, HANDLER, EdgeKind::Calls));
    assert_eq!(edges(rt_a, java.0, EdgeKind::Calls).len(), 1, "{:?}", edges(rt_a, java.0, EdgeKind::Calls));
    assert_eq!(edges(rt_a, java.0, EdgeKind::Imports).len(), 1, "{:?}", edges(rt_a, java.0, EdgeKind::Imports));
}

/// NFR-RA-06 (I4-INT): a bare call inside a class reaches its companion only
/// after the class's supertypes, so a superclass in another file gaining the
/// name takes the call from the companion — and a sync of that file binds what
/// a fresh index of the final tree binds.
#[test]
fn sync_equals_reindex_when_a_superclass_gains_a_companions_member_name() {
    let base = "src/main/kotlin/app/tabs/Base.kt";
    let child = "src/main/kotlin/app/tabs/Child.kt";
    let child_text = "\
package app.tabs

class Child : Base() {
    fun a(): Int = make()
    companion object {
        fun make(): Int = 0
    }
}
";
    let before = "package app.tabs\n\nopen class Base {\n    fun other(): Int = 1\n}\n";
    let after = "package app.tabs\n\nopen class Base {\n    fun make(): Int = 1\n}\n";
    let tmp_a = tree(&[(base, before), (child, child_text)]);
    let engine_a = index(tmp_a.path());
    let rt_a = engine_a.runtime().unwrap();
    assert_eq!(edges(rt_a, child, EdgeKind::Calls), vec![format!("{child}:a@4 -> {child}:make@6")]);
    write(tmp_a.path(), base, after);
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    engine_a.pipeline().sync(&[root_a.join(base)]);
    assert_eq!(edges(rt_a, child, EdgeKind::Calls), vec![format!("{child}:a@4 -> {base}:make@4")]);
    let tmp_b = tree(&[(base, after), (child, child_text)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(graph_fingerprint(rt_a), graph_fingerprint(engine_b.runtime().unwrap()));
}

// ── S-833: an extension binds only through a receiver of its type ────────────

/// FR-RS-31, FR-RS-32, NFR-RA-05 (S-833): a call on a proven receiver reaches
/// an extension only when the extension's receiver type is the proven type or
/// an in-repository supertype of it — `d.format("x")` on a `LocalDate` never
/// binds `fun LocalDateTime.format`, while `s.toSlug()` on a `String` still
/// binds `fun String.toSlug()`, and a `Box` reaches its superclass's and its
/// interface's extensions but never an unrelated class's.
#[test]
fn a_proven_receiver_reaches_only_an_extension_of_its_type_or_a_supertype() {
    let ext = "src/main/kotlin/app/time/Ext.kt";
    let caller = "src/main/kotlin/app/time/Use.kt";
    let tmp = tree(&[
        (
            ext,
            "\
package app.time

import java.time.LocalDateTime

fun LocalDateTime.format(f: String): String = f

fun String.toSlug(): String = this

fun Base.lift(): Int = 1

fun Named.greet(): Int = 2

fun Other.boxed(): Int = 3
",
        ),
        (
            caller,
            "\
package app.time

import java.time.LocalDate

open class Base

interface Named

class Box : Base(), Named

class Other

fun dated(d: LocalDate): String = d.format(\"x\")

fun slug(s: String): String = s.toSlug()

fun lifted(b: Box): Int = b.lift() + b.greet()

fun unrelated(b: Box): Int = b.boxed()
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[
            &format!("{caller}:lifted@17 -> {ext}:greet@11"),
            &format!("{caller}:lifted@17 -> {ext}:lift@9"),
            &format!("{caller}:slug@15 -> {ext}:toSlug@7"),
        ])
    );
    // The typed rows stay typed, and unbound.
    let rows = calls(rt, caller);
    for target in ["LocalDate::format", "Box::boxed"] {
        assert!(
            rows.contains(&(target.to_string(), RefForm::Path, Some(ReceiverShape::Other), false)),
            "{target}: {rows:?}"
        );
    }
}

/// FR-RS-31, NFR-RA-05 (S-833): a receiver whose type nothing proves reaches an
/// extension only when the extension's receiver type is a class of the
/// repository — only there can "no member of the name" be checked. mixit's
/// `Base64.getDecoder().decode(s)` never binds `fun ServerRequest.decode`, nor
/// an untyped lambda parameter `fun String.toSlug()`, while one calling
/// `fun Repo.audit()` on an in-repository `Repo` still binds.
#[test]
fn an_unproven_receiver_reaches_only_an_extension_of_a_repository_class() {
    let ext = "src/main/kotlin/app/web/Ext.kt";
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let tmp = tree(&[
        (
            ext,
            "\
package app.web

import org.springframework.web.reactive.function.server.ServerRequest

fun ServerRequest.decode(s: String): String = s

fun String.toSlug(): String = this

fun Repo.audit(): Int = 0
",
        ),
        (
            caller,
            "\
package app.web

class Repo

class Ctl {
    fun decoded(s: String): ByteArray = java.util.Base64.getDecoder().decode(s)

    fun slugs(xs: List<String>) {
        xs.forEach { x -> x.toSlug() }
    }

    fun audits(xs: List<Repo>) {
        xs.forEach { r -> r.audit() }
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, caller, EdgeKind::Calls),
        strings(&[&format!("{caller}:audits@12 -> {ext}:audit@9")])
    );
    let rows = calls(rt, caller);
    for target in ["decode", "toSlug"] {
        assert!(
            rows.contains(&(target.to_string(), RefForm::Method, Some(ReceiverShape::Other), false)),
            "{target}: {rows:?}"
        );
    }
}

/// FR-RS-31, FR-RS-12, NFR-RA-05 (S-833): inside a class `Box`, whose implicit
/// `this` is a `Box`, a bare `twice()` binds `fun Box.twice()` when no member
/// answers — and so does a bare call inside a subclass of an in-repository
/// `Base` reach `fun Base.up()`. A member of the class or of its
/// in-repository supertype wins over the extension; a class whose base the
/// graph does not hold may inherit the name, and an unrelated class reaches
/// no other class's extension.
#[test]
fn a_bare_call_inside_a_class_reaches_its_own_extension_when_no_member_answers() {
    let ext = "src/main/kotlin/app/box/Ext.kt";
    let file = "src/main/kotlin/app/box/Box.kt";
    let tmp = tree(&[
        (
            ext,
            "\
package app.box

fun Box.twice(): Int = 2

fun Base.up(): Int = 1

fun Mem.again(): Int = 2

fun Q.pp(): Int = 2

fun Ctl.render(): Int = 0

fun Box.boxOnly(): Int = 0
",
        ),
        (
            file,
            "\
package app.box

import org.springframework.Controller

class Box {
    fun size(): Int = twice()
}

open class Base

class Sub : Base() {
    fun run(): Int = up()
}

class Mem {
    fun again(): Int = 1
    fun run(): Int = again()
}

open class P {
    fun pp(): Int = 1
}

class Q : P() {
    fun run(): Int = pp()
}

class Ctl : Controller() {
    fun run(): Int = render()
}

class Holder {
    fun build(): Int = boxOnly()
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:run@12 -> {ext}:up@5"),
            &format!("{file}:run@17 -> {file}:again@16"),
            &format!("{file}:run@25 -> {file}:pp@21"),
            &format!("{file}:size@6 -> {ext}:twice@3"),
        ])
    );
}

/// NFR-RA-06 (S-833): a sync of `edit` binds what a fresh index of the final
/// tree binds; the synced caller's `Calls` edges are returned.
fn synced_caller_edges(initial: &[(&'static str, &'static str)], edit: (&'static str, &'static str), caller: &str) -> Vec<String> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let rt_a = engine_a.runtime().unwrap();
    assert!(edges(rt_a, caller, EdgeKind::Calls).is_empty(), "{:?}", edges(rt_a, caller, EdgeKind::Calls));
    write(tmp_a.path(), edit.0, edit.1);
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    engine_a.pipeline().sync(&[root_a.join(edit.0)]);
    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.insert(edit.0, edit.1);
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(graph_fingerprint(rt_a), graph_fingerprint(engine_b.runtime().unwrap()));
    edges(rt_a, caller, EdgeKind::Calls)
}

/// NFR-RA-06 (S-833): an unproven receiver's extension `fun Repo.audit()`
/// binds once a class `Repo` is declared in another file — a name the row
/// `audit` does not spell — and a sync of that file binds what a fresh index
/// of the final tree binds.
#[test]
fn sync_equals_reindex_when_an_extensions_receiver_class_is_declared() {
    let ext = "src/main/kotlin/app/web/Ext.kt";
    let caller = "src/main/kotlin/app/web/Ctl.kt";
    let declared = "src/main/kotlin/app/web/Repo.kt";
    let initial = [
        (ext, "package app.web\n\nfun Repo.audit(): Int = 0\n"),
        (
            caller,
            "\
package app.web

class Ctl {
    fun audits(xs: List<Any>) {
        xs.forEach { r -> r.audit() }
    }
}
",
        ),
        (declared, "package app.web\n\nfun unrelated(): Int = 0\n"),
    ];
    let edit = (declared, "package app.web\n\nclass Repo\n");
    assert_eq!(
        synced_caller_edges(&initial, edit, caller),
        strings(&[&format!("{caller}:audits@4 -> {ext}:audit@3")])
    );
}

/// NFR-RA-06 (S-833): a bare call inside a subclass reaches its base's
/// extension `fun Base.up()` once the base is declared in another file, and a
/// sync of that file binds what a fresh index of the final tree binds.
#[test]
fn sync_equals_reindex_when_a_subclass_gains_the_extensions_base() {
    let ext = "src/main/kotlin/app/web/Ext.kt";
    let caller = "src/main/kotlin/app/web/Sub.kt";
    let declared = "src/main/kotlin/app/web/Base.kt";
    let initial = [
        (ext, "package app.web\n\nfun Base.up(): Int = 1\n"),
        (caller, "package app.web\n\nclass Sub : Base() {\n    fun run(): Int = up()\n}\n"),
        (declared, "package app.web\n\nfun unrelated(): Int = 0\n"),
    ];
    let edit = (declared, "package app.web\n\nopen class Base\n");
    assert_eq!(
        synced_caller_edges(&initial, edit, caller),
        strings(&[&format!("{caller}:run@4 -> {ext}:up@3")])
    );
}

/// NFR-RA-05 (S-833 review): an extension of a nested type records only the
/// type's last segment — `fun Box.Companion.make()` records `Companion`, `fun
/// Bar.Builder.build()` records `Builder` — so a nested class's simple name
/// never makes it the extension's receiver: a bare call inside another class's
/// companion or a same-named nested class, with or without a supertype, and a
/// call on a proven `Foo.Builder`, bind no such extension.
#[test]
fn a_nested_types_simple_name_never_makes_it_an_extensions_receiver() {
    let ext = "src/main/kotlin/app/c/Ext.kt";
    let file = "src/main/kotlin/app/c/Use.kt";
    let tmp = tree(&[
        (ext, "package app.c\n\nfun Box.Companion.make(): Int = 1\n\nfun Bar.Builder.build(): Int = 1\n"),
        (
            file,
            "\
package app.c

import lib.factory.*

open class Factory

class Box {
    companion object {
        fun other(): Int = 0
    }
}

class A {
    companion object {
        fun x(): Int = make()
    }
}

class B {
    companion object : Factory() {
        fun y(): Int = make()
    }
}

class Bar {
    class Builder
}

class Foo {
    class Builder {
        fun b(): Int = build()
    }
}

fun built(f: Foo.Builder): Int = f.build()
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, file, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, file, EdgeKind::Calls));
}

/// FR-RS-31, NFR-RA-05 (S-833 review): a member wins over an extension, and
/// where the call might sit inside a lambda whose receiver the graph does not
/// read, a callable member of the name on any class of the family keeps the
/// call off the extension — on a proven receiver, inside the extended class,
/// and inside a subclass of it alike.
#[test]
fn a_member_of_the_name_on_any_class_keeps_every_receiver_off_the_extension() {
    let ext = "src/main/kotlin/app/d/Ext.kt";
    let file = "src/main/kotlin/app/d/Use.kt";
    let tmp = tree(&[
        (ext, "package app.d\n\nfun Box.twice(): Int = 2\n\nfun Base.up(): Int = 1\n\nfun Box.lift(): Int = 3\n"),
        (
            file,
            "\
package app.d

class Other {
    fun twice(): Int = 0
    fun up(): Int = 0
    fun lift(): Int = 0
}

class Box {
    fun size(): Int = twice()
}

open class Base

class Sub : Base() {
    fun run(): Int = up()
}

fun lifted(b: Box): Int = b.lift()
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, file, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, file, EdgeKind::Calls));
}

/// FR-RS-31, FR-RS-32 (S-833 review): an extension of a grandparent type is
/// reached through every level of the receiver's in-repository ancestry — on a
/// proven `C` and from inside `C` alike.
#[test]
fn an_extension_of_a_grandparent_is_reached_through_the_whole_ancestry() {
    let ext = "src/main/kotlin/app/e/Ext.kt";
    let file = "src/main/kotlin/app/e/Use.kt";
    let tmp = tree(&[
        (ext, "package app.e\n\nfun A.top(): Int = 1\n"),
        (
            file,
            "\
package app.e

open class A

open class B : A()

class C : B() {
    fun r(): Int = top()
}

fun use(c: C): Int = c.top()
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[&format!("{file}:r@8 -> {ext}:top@3"), &format!("{file}:use@11 -> {ext}:top@3")])
    );
}

/// NFR-RA-05 (S-833 review): an unproven receiver reaches an extension only
/// through a class of the source's own family — a Python `class Repo` never
/// makes Kotlin's `fun Repo.audit()` reachable from an untyped receiver.
#[test]
fn another_familys_class_never_makes_an_unproven_extension_reachable() {
    let caller = "src/main/kotlin/app/f/Ctl.kt";
    let mut files = vec![
        ("src/main/kotlin/app/f/Ext.kt", "package app.f\n\nfun Repo.audit(): Int = 0\n"),
        (
            caller,
            "\
package app.f

class Ctl {
    fun audits(xs: List<Any>) {
        xs.forEach { r -> r.audit() }
    }
}
",
        ),
    ];
    if cfg!(feature = "lang-python") {
        files.push(("py/repo.py", "class Repo:\n    def other(self):\n        return 1\n"));
    }
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, caller, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, caller, EdgeKind::Calls));
    assert!(
        calls(rt, caller).contains(&("audit".to_string(), RefForm::Method, Some(ReceiverShape::Other), false)),
        "{:?}",
        calls(rt, caller)
    );
}

// ── S-834: a bare call inside a lambda with a receiver ──────────────────────

/// The package's top-level functions, a class and an extension of a
/// repository class the receiver lambdas below name bare (S-834).
const LAMBDA_TOP: (&str, &str) = (
    "src/main/kotlin/app/lam/Top.kt",
    "package app.lam\n\nfun append(s: String) {}\n\nfun log(s: String) {}\n\nclass Thing\n\nclass Repo\n\nfun Repo.audit() {}\n",
);

const LAMBDAS: &str = "src/main/kotlin/app/lam/Lambdas.kt";

/// `buildString { … }` and a DSL builder (`module { single { … } }`) take a
/// lambda whose receiver nothing in the file proves; `let`, `forEach` and `map`
/// take one with no receiver; `Holder` declares its own `append`; `local`
/// declares a local `log`, which Kotlin reads before any implicit receiver;
/// `audits` names an extension of the repository class `Repo`, which no
/// receiver in scope is.
const LAMBDAS_TEXT: &str = "\
package app.lam

fun built(): String = buildString { append(\"x\") }
fun dsl() { module { single { log(\"x\") } } }
fun made(): String = buildString { Thing(); append(\"y\") }
fun viaLet(s: String) { s.let { log(it) } }
fun viaForEach(xs: List<String>) { xs.forEach { log(it) } }
fun viaMap(xs: List<String>) = xs.map { log(it) }
fun plain() { log(\"z\") }

class Holder {
    fun append(s: String) {}
    fun keeps() = buildString { append(\"w\") }
}

fun local(): String {
    fun log(s: String) {}
    return buildString { log(\"q\") }
}

fun audits(): String = buildString { audit() }
";

/// FR-RS-31 (S-834, CR-261 K4): a bare call inside a lambda S-563 marks as
/// rebinding `this`, whose receiver nothing proves, binds no top-level,
/// imported or package function — before the change `buildString { append(…) }`
/// bound the top-level `append`. A trailing lambda of a non-receiver function,
/// a constructor call, a member of the enclosing class and a local function are
/// read as before. An extension is a top-level function too: `buildString {
/// audit() }` never reaches `fun Repo.audit()`, whose receiver no implicit
/// receiver in scope is (review of S-834).
#[test]
fn a_bare_call_inside_an_unproven_receiver_lambda_binds_no_top_level_function() {
    let top = LAMBDA_TOP.0;
    let tmp = tree(&[LAMBDA_TOP, (LAMBDAS, LAMBDAS_TEXT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, LAMBDAS, EdgeKind::Calls),
        strings(&[
            &format!("{LAMBDAS}:keeps@13 -> {LAMBDAS}:append@12"),
            &format!("{LAMBDAS}:local@16 -> {LAMBDAS}:log@17"),
            &format!("{LAMBDAS}:plain@9 -> {top}:log@5"),
            &format!("{LAMBDAS}:viaForEach@7 -> {top}:log@5"),
            &format!("{LAMBDAS}:viaLet@6 -> {top}:log@5"),
            &format!("{LAMBDAS}:viaMap@8 -> {top}:log@5"),
        ])
    );
    assert_eq!(
        edges(rt, LAMBDAS, EdgeKind::Instantiates),
        strings(&[&format!("{LAMBDAS}:made@5 -> {top}:Thing@7")])
    );
    // The refused rows keep the bare call's shape: a sync re-reads them.
    let refused = calls(rt, LAMBDAS)
        .into_iter()
        .filter(|row| row == &("append".to_string(), RefForm::Path, None, false))
        .count();
    assert_eq!(refused, 2, "{:?}", calls(rt, LAMBDAS));
}

/// FR-RS-31 (S-834): the refused call states why — `no-receiver-evidence`, the
/// reason a call on a receiver of unproven type carries. `buildString` itself,
/// a library function, records its own reason either way.
#[test]
fn a_bare_call_an_unproven_receiver_lambda_refuses_records_no_receiver_evidence() {
    let file = "src/main/kotlin/app/lam/Built.kt";
    let control = "src/main/kotlin/app/lam/Control.kt";
    let tmp = tree(&[LAMBDA_TOP, (file, "package app.lam\n\nfun built(): String = buildString { append(\"x\") }\n")]);
    let engine = index(tmp.path());
    let tmp_control = tree(&[LAMBDA_TOP, (control, "package app.lam\n\nfun built(): String = buildString { }\n")]);
    let engine_control = index(tmp_control.path());
    let mut expected = kotlin_residue(&engine_control);
    match expected.iter_mut().find(|(reason, _)| *reason == CallResidueReason::NoReceiverEvidence) {
        Some((_, n)) => *n += 1,
        None => expected.push((CallResidueReason::NoReceiverEvidence, 1)),
    }
    expected.sort();
    assert_eq!(kotlin_residue(&engine), expected);
}

/// FR-RS-31 (S-834, review of I3-INT): a bare call an unproven receiver lambda
/// refuses records the lambda's reason alone. Its own bind's miss — here an
/// argument count the top-level `one` does not admit — is not the row's
/// reason, so the row reads `no-receiver-evidence`, never `no-applicable-overload`.
#[test]
fn a_refused_bare_call_records_no_receiver_evidence_not_its_own_bind_miss() {
    let file = "src/main/kotlin/app/lam/Arity.kt";
    let control = "src/main/kotlin/app/lam/Control.kt";
    let one = "package app.lam\n\nfun one(s: String) {}\n\n";
    let tmp = tree(&[(file, &format!("{one}fun a(): String = buildString {{ one(\"a\", \"b\") }}\n"))]);
    let engine = index(tmp.path());
    let tmp_control = tree(&[(control, &format!("{one}fun a(): String = buildString {{ }}\n"))]);
    let engine_control = index(tmp_control.path());
    let mut expected = kotlin_residue(&engine_control);
    match expected.iter_mut().find(|(reason, _)| *reason == CallResidueReason::NoReceiverEvidence) {
        Some((_, n)) => *n += 1,
        None => expected.push((CallResidueReason::NoReceiverEvidence, 1)),
    }
    expected.sort();
    assert_eq!(kotlin_residue(&engine), expected);
}

/// FR-RS-31 (S-834, review of I3-INT): when a proven receiver whose whole
/// hierarchy the repository holds declares no such member, the bare call
/// inside its lambda reads as the same bare call outside it — the receiver
/// type's own miss is not carried into the bare call's reason.
#[test]
fn a_proven_receivers_miss_is_not_the_bare_calls_reason() {
    let file = "src/main/kotlin/app/lam/Proven.kt";
    let order = "package app.lam\n\nclass Order {\n    fun ship() {}\n}\n\n";
    let inside = format!("{order}fun a(o: Order) {{ with(o) {{ nothere() }} }}\n");
    let outside = format!("{order}fun a(o: Order) {{ with(o) {{ }} }}\n\nfun b() {{ nothere() }}\n");
    let tmp = tree(&[(file, &inside)]);
    let tmp_outside = tree(&[(file, &outside)]);
    assert_eq!(kotlin_residue(&index(tmp.path())), kotlin_residue(&index(tmp_outside.path())));
}

/// FR-RS-31 (S-834 review): the enclosing class's own companion is the
/// class's scope, not a top-level function: a bare call to its member inside
/// an unproven receiver lambda binds it, as one to an instance member does —
/// and as `Builder().apply { create() }` reaches the companion's `create`
/// (configuration.md, `companion_name`). Another class's companion member,
/// imported, is an imported function: refused.
#[test]
fn an_unproven_receiver_lambda_keeps_the_enclosing_classs_companion_members() {
    let file = "src/main/kotlin/app/lam/Comp.kt";
    let other = "src/main/kotlin/app/other/Config.kt";
    let tmp = tree(&[
        (other, "package app.other\n\nclass Config {\n    companion object {\n        fun media() {}\n    }\n}\n"),
        (
            file,
            "\
package app.lam

import app.other.Config.Companion.media

class Comp {
    companion object {
        fun make() {}
    }
    fun inst() {}
    fun outside() { make() }
    fun unproven() = buildString { make(); inst(); media() }
    fun applied() = Builder().apply { make() }
}

class Builder
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, file, EdgeKind::Calls),
        strings(&[
            &format!("{file}:applied@12 -> {file}:make@7"),
            &format!("{file}:outside@10 -> {file}:make@7"),
            &format!("{file}:unproven@11 -> {file}:inst@9"),
            &format!("{file}:unproven@11 -> {file}:make@7"),
        ])
    );
}

/// FR-RS-44 / FR-RS-31 (S-834 review): a bare call through a local value is
/// decided by that binding (S-594) inside a receiver lambda too — it keeps its
/// `local-binding` reason, never the receiver lambda's.
#[test]
fn a_call_through_a_local_value_inside_a_receiver_lambda_stays_the_local_binding() {
    let file = "src/main/kotlin/app/lam/Local.kt";
    let control = "src/main/kotlin/app/lam/Control.kt";
    let tmp = tree(&[(file, "package app.lam\n\nfun lv(): String {\n    val audit = { }\n    return buildString { audit() }\n}\n")]);
    let engine = index(tmp.path());
    let tmp_control = tree(&[(control, "package app.lam\n\nfun lv(): String {\n    return buildString { }\n}\n")]);
    let engine_control = index(tmp_control.path());
    let mut expected = kotlin_residue(&engine_control);
    expected.push((CallResidueReason::LocalBinding, 1));
    expected.sort();
    assert_eq!(kotlin_residue(&engine), expected);
}

// ── `@file:JvmName` names the facade (S-832, CR-261 K2) ──────────────────────

const SLUGS: (&str, &str) = (
    "src/main/kotlin/app/text/Strings.kt",
    "@file:JvmName(\"Slugs\")\n\npackage app.text\n\nfun slug(s: String): String = s.lowercase()\n",
);

/// The Java caller of the facade tests: each facade call in a method of its
/// own, so an edge that survives a sync it should not is visible by source.
const FACADE_CALLER: (&str, &str) = (
    "src/main/java/app/boot/Boot.java",
    "\
package app.boot;

import app.text.Slugs;
import app.text.StringsKt;

class Boot {
    void declared() {
        Slugs.slug(\"x\");
    }

    void derived() {
        StringsKt.slug(\"y\");
    }
}
",
);

/// FR-RS-31 (S-832): a file's `@file:JvmName("Slugs")` names its facade, so
/// Java's `Slugs.slug("x")` binds and `StringsKt.slug(…)` no longer does; the
/// import of the declared facade binds the file and the derived one nothing.
/// A file without the annotation keeps `<Stem>Kt`, another file annotation
/// (`@file:Suppress("Gone")`) names nothing, and the qualified
/// `@file:kotlin.jvm.JvmName("Routes")` names its facade too. Each call sits
/// in a method of its own, so each binding is told apart by its source.
#[test]
fn a_jvm_name_annotation_names_the_files_facade_and_the_stem_name_no_longer_binds() {
    let java = "src/main/java/app/boot/Calls.java";
    let plain = "src/main/kotlin/app/text/Plain.kt";
    let paths = "src/main/kotlin/app/text/Paths.kt";
    let tmp = tree(&[
        SLUGS,
        (plain, "@file:Suppress(\"Gone\")\n\npackage app.text\n\nfun plain(): Int = 0\n"),
        (paths, "@file:kotlin.jvm.JvmName(\"Routes\")\n\npackage app.text\n\nfun route(): Int = 1\n"),
        (
            java,
            "\
package app.boot;

import app.text.Slugs;
import app.text.StringsKt;
import app.text.PlainKt;
import app.text.Routes;

class Calls {
    void declared() {
        Slugs.slug(\"x\");
    }

    void derived() {
        StringsKt.slug(\"y\");
    }

    void plain() {
        PlainKt.plain();
    }

    void gone() {
        Gone.plain();
    }

    void routes() {
        Routes.route();
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, java, EdgeKind::Calls),
        strings(&[
            &format!("{java}:declared@9 -> {}:slug@5", SLUGS.0),
            &format!("{java}:plain@17 -> {plain}:plain@5"),
            &format!("{java}:routes@25 -> {paths}:route@5"),
        ])
    );
    // The imports share one source, so their rows are read one by one.
    let java_rel = java.to_string();
    let mut imports: Vec<(String, bool)> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> = store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Imports && r.file_id.and_then(|id| files.get(&id)) == Some(&java_rel))
                .map(|r| (r.target, r.resolved))
                .collect())
        })
        .expect("read runs");
    imports.sort();
    assert_eq!(
        imports,
        vec![
            ("app::text::PlainKt".to_string(), true),
            ("app::text::Routes".to_string(), true),
            ("app::text::Slugs".to_string(), true),
            ("app::text::StringsKt".to_string(), false),
        ]
    );
}

/// FR-RS-43 (S-832): the residue `status` states walks the facade the file
/// declares, as the binder does — the unbound `StringsKt.slug(…)` is classified
/// with a reason, never read as bindable and left unclassified.
#[test]
fn the_call_residue_walks_the_declared_facade() {
    let tmp = tree(&[SLUGS, FACADE_CALLER]);
    let engine = index(tmp.path());
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .and_then(|row| row.call_residue)
        .expect("the java row states its call residue");
    let reasons: u64 = residue.reasons.values().sum();
    assert_eq!((residue.unbound, residue.unclassified, reasons), (1, 0, 1), "{residue:?}");
}

/// FR-RS-31, NFR-RA-05 (S-832): two files declaring one facade bind neither,
/// as two files deriving one do — koin's `KoinWaitExt.kt` and
/// `KoinWaitExt.jvm.kt` both declare `KoinWaitExtKt`, which before the
/// annotation was read named the first file alone.
#[test]
fn two_files_declaring_one_facade_bind_neither() {
    let java = "src/main/java/app/boot/Wait.java";
    let tmp = tree(&[
        (
            "src/commonMain/kotlin/app/core/KoinWaitExt.kt",
            "@file:JvmName(\"KoinWaitExtKt\")\n@file:JvmMultifileClass\n\npackage app.core\n\nfun awaitAll(a: Int): Int = a\n",
        ),
        (
            "src/jvmMain/kotlin/app/core/KoinWaitExt.jvm.kt",
            "@file:JvmName(\"KoinWaitExtKt\")\n@file:JvmMultifileClass\n\npackage app.core\n\nfun awaitOne(a: Int): Int = a\n",
        ),
        (
            java,
            "package app.boot;\n\nimport app.core.KoinWaitExtKt;\n\nclass Wait {\n    void start() {\n        KoinWaitExtKt.awaitAll(1);\n        KoinWaitExtKt.awaitOne(1);\n    }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, java, EdgeKind::Calls).is_empty(), "{:?}", edges(rt, java, EdgeKind::Calls));
    assert!(edges(rt, java, EdgeKind::Imports).is_empty(), "{:?}", edges(rt, java, EdgeKind::Imports));
}

/// NFR-RA-06 (S-832): a sync that adds or removes a file's `@file:JvmName`
/// binds what a fresh index of the final tree binds — the file's own facade,
/// and a second file starting and stopping to declare it, which no node of
/// that file spells.
#[test]
fn sync_equals_reindex_when_a_jvm_name_annotation_is_added_or_removed() {
    let strings = SLUGS.0;
    let other = "src/main/kotlin/app/text/Other.kt";
    let derived = "package app.text\n\nfun slug(s: String): String = s.lowercase()\n";
    let other_plain = "package app.text\n\nfun other(): Int = 2\n";
    let other_slugs = "@file:JvmName(\"Slugs\")\n\npackage app.text\n\nfun other(): Int = 2\n";
    let java = FACADE_CALLER.0;
    let declared = format!("{java}:declared@7 -> {strings}:slug@5");
    let steps: [(&str, &str, Vec<String>); 4] = [
        (strings, SLUGS.1, vec![declared.clone()]),
        (other, other_slugs, vec![]),
        (other, other_plain, vec![declared]),
        (strings, derived, vec![format!("{java}:derived@11 -> {strings}:slug@3")]),
    ];
    let mut state: BTreeMap<&str, &str> = BTreeMap::from([(strings, derived), (other, other_plain), FACADE_CALLER]);
    let tmp_a = tree(&state.iter().map(|(p, t)| (*p, *t)).collect::<Vec<_>>());
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let rt_a = engine_a.runtime().unwrap();
    assert_eq!(edges(rt_a, java, EdgeKind::Calls), vec![format!("{java}:derived@11 -> {strings}:slug@3")]);
    for (i, (rel, text, bound)) in steps.iter().enumerate() {
        write(tmp_a.path(), rel, text);
        engine_a.pipeline().sync(&[root_a.join(rel)]);
        state.insert(rel, text);
        let tmp_b = tree(&state.iter().map(|(p, t)| (*p, *t)).collect::<Vec<_>>());
        let engine_b = index(tmp_b.path());
        assert_eq!(edges(rt_a, java, EdgeKind::Calls), *bound, "step {i}");
        assert_eq!(graph_fingerprint(rt_a), graph_fingerprint(engine_b.runtime().unwrap()), "step {i}");
    }
}
