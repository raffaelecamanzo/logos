//! A static call on a type of the caller's own package binds without an import
//! (S-854, [CR-271], [FR-RS-10], [FR-RS-13], [FR-RS-09], [NFR-RA-05],
//! [NFR-RA-06]) — exercised end to end through the public [`Engine`] façade
//! against real temp-directory fixtures.
//!
//! A Java `T.m(…)` on a type declared in another file of the caller's package
//! is never imported. Before S-854 the extractor typed the receiver `T` only
//! when the file imported or declared it, so the row stayed the bare `m` and
//! the binder's package rung, which already scopes the caller's own package,
//! never saw it (C16, measured on commons-lang: 18,982 calls, 0 bound). Now a
//! type-shaped name no binding of the file declares is written `T::m` and
//! the binder decides its scope: the same package and an import bind; anything
//! else stays unbound with the binder's reason.
//!
//! The C#, Scala and Go fixtures record the same shape in each language. None
//! of them reads the Java check, which is the package-model branch alone, so
//! none is fixed here: C#'s and Scala's bind already, and Go's method
//! expression stays unbound for a reason of its own (a known limitation).
//!
//! [CR-271]: ../../docs/requests/CR-271-a-static-call-on-a-same-package-type-binds-without-an-import.md
//! [FR-RS-10]: ../../docs/specs/requirements/FR-RS-10.md
//! [FR-RS-13]: ../../docs/specs/requirements/FR-RS-13.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md

#![cfg(feature = "lang-java")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &Path) -> Engine {
    coordinate::pin(tmp);
    let engine = Engine::start(tmp).expect("engine starts");
    engine.pipeline().index();
    engine
}

/// A fresh directory holding exactly `files`.
fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    tmp
}

/// Every bound `Calls` edge as `(source file:name, target file:name)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

fn edge(from_file: &str, from: &str, to_file: &str, to: &str) -> (String, String) {
    (format!("{from_file}:{from}"), format!("{to_file}:{to}"))
}

/// The `Calls` edges leaving `file`.
fn edges_from(rt: &Runtime, file: &str) -> Vec<(String, String)> {
    let prefix = format!("{file}:");
    call_edges(rt).into_iter().filter(|(s, _)| s.starts_with(&prefix)).collect()
}

/// The `Calls` ledger targets recorded in `file`, as `(target, resolved)`, sorted.
fn call_rows(rt: &Runtime, file: &str) -> Vec<(String, bool)> {
    let needle = file.to_string();
    rt.submit_read(move |store| {
        let files: HashMap<String, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.symbol.as_str().to_string(), n.file_path.unwrap_or_default()))
            .collect();
        let mut rows: Vec<(String, bool)> = store
            .unresolved_refs()?
            .into_iter()
            .filter(|r| r.kind == EdgeKind::Calls)
            .filter(|r| files.get(&r.source_symbol).is_some_and(|f| *f == needle))
            .map(|r| (r.target, r.resolved))
            .collect();
        rows.sort();
        Ok(rows)
    })
    .expect("read runs")
}

/// The `Calls` residue the `language` row of `status` carries.
fn residue(engine: &Engine, language: &str) -> CallResidue {
    engine
        .graph()
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == language)
        .unwrap_or_else(|| panic!("a {language} row"))
        .call_residue
        .unwrap_or_else(|| panic!("the {language} row states its call residue"))
}

/// The reasons of `residue` whose count is not zero.
fn nonzero(residue: &CallResidue) -> BTreeMap<R, u64> {
    residue.reasons.iter().filter(|(_, n)| **n > 0).map(|(k, n)| (*k, *n)).collect()
}

fn reasons(pairs: &[(R, u64)]) -> BTreeMap<R, u64> {
    pairs.iter().copied().collect()
}

// ── Java: the C16 fixture ──────────────────────────────────────────────────

const HELPER: &str = "src/app/Helper.java";
const USE: &str = "src/app/Use.java";
const BOOT: &str = "src/app/boot/Boot.java";

const HELPER_JAVA: &str = "package app;\n\
\n\
public class Helper {\n\
    public Helper() {}\n\
    public static String slug(String s) { return s.trim(); }\n\
    public String greet(String s) { return \"hi \" + s; }\n\
}\n";

/// The C16 caller: package `app`, no import, a static call on `app.Helper`.
const USE_JAVA: &str = "package app;\n\
\n\
public class Use {\n\
    public String staticCall() {\n\
        return Helper.slug(\" x \");\n\
    }\n\
}\n";

/// The C16 control: another package, importing `app.Helper`.
const BOOT_JAVA: &str = "package app.boot;\n\
\n\
import app.Helper;\n\
\n\
public class Boot {\n\
    public String staticCall() {\n\
        return Helper.slug(\" x \");\n\
    }\n\
}\n";

fn c16() -> [(&'static str, &'static str); 3] {
    [(HELPER, HELPER_JAVA), (USE, USE_JAVA), (BOOT, BOOT_JAVA)]
}

/// The C16 acceptance criterion: `callers slug` counts the same-package caller
/// beside the importing one. On the base it answered total 1 (`Boot` only).
#[test]
fn c16_callers_of_slug_count_the_same_package_static_call_and_the_imported_one() {
    let tmp = tree(&c16());
    let engine = index(tmp.path());
    let callers = engine.navigation().callers("slug", None);
    let mut found: Vec<(String, String)> = callers
        .callers
        .iter()
        .map(|c| (c.file.clone().unwrap_or_default(), c.name.clone()))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [(USE.to_string(), "staticCall".to_string()), (BOOT.to_string(), "staticCall".to_string())],
        "Use#staticCall (same package, no import) and Boot#staticCall (imported)"
    );
    assert_eq!(callers.total, 2);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, USE), [("Helper::slug".to_string(), true)], "the row is written T::m and binds");
}

/// A static call inside a nested class or a lambda body is the same receiver
/// shape: the type is the package's whichever file declares it, and only the
/// binder decides where `Helper` lives.
#[test]
fn a_same_package_static_call_binds_from_a_nested_class_and_a_lambda_body() {
    let caller = "package app;\n\
\n\
import java.util.function.Supplier;\n\
\n\
public class Use {\n\
    static class Inner {\n\
        String viaInner() { return Helper.slug(\"a\"); }\n\
    }\n\
    Supplier<String> viaLambda() { return () -> Helper.slug(\"b\"); }\n\
}\n";
    let tmp = tree(&[(HELPER, HELPER_JAVA), (USE, caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, USE),
        [edge(USE, "viaInner", HELPER, "slug"), edge(USE, "viaLambda", HELPER, "slug")]
    );
}

const CALLER: &str = "src/app/Caller.java";

/// Index `files` and assert no `Calls` edge leaves [`CALLER`]; the tree is
/// returned with the engine so the store outlives the call.
fn unbound_from_caller(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = tree(files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let from_caller = edges_from(rt, CALLER);
    assert!(from_caller.is_empty(), "no edge may leave {CALLER}: {from_caller:?}");
    (tmp, engine)
}

/// A type of another package the file does not import is out of scope: the
/// row is `Other::go`, and the package rung refuses it. The reason is
/// `external-type` — no type of that name is in the caller's scope — where
/// the bare row read `no-receiver-evidence` before.
#[test]
fn an_unimported_type_of_another_package_stays_unbound() {
    let (_tmp, engine) = unbound_from_caller(&[
        ("src/app/other/Other.java", "package app.other;\n\npublic class Other {\n    public static void go() {}\n}\n"),
        (CALLER, "package app;\n\npublic class Caller {\n    public void m() { Other.go(); }\n}\n"),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("Other::go".to_string(), false)]);
    assert_eq!(nonzero(&residue(&engine, "java")), reasons(&[(R::ExternalType, 1)]));
}

/// `java.lang` is never in the graph: `Math.max(…)` is written `Math::max`
/// and stays unbound as `external-type`.
#[test]
fn math_max_stays_unbound_as_external_type() {
    let (_tmp, engine) = unbound_from_caller(&[(
        CALLER,
        "package app;\n\npublic class Caller {\n    public int m() { return Math.max(1, 2); }\n}\n",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("Math::max".to_string(), false)]);
    assert_eq!(nonzero(&residue(&engine, "java")), reasons(&[(R::ExternalType, 1)]));
}

/// A variable named like a type is the variable: a local or a field whose
/// declared type the file proves keeps that type (`Mailer`), never `Helper`.
#[test]
fn a_variable_named_like_a_type_keeps_its_binding_proof() {
    let mailer = "package app;\n\npublic class Mailer {\n    public String slug(String s) { return s; }\n}\n";
    let caller = "package app;\n\
\n\
public class Caller {\n\
    private Mailer Helper = new Mailer();\n\
    public String viaField() { return Helper.slug(\"f\"); }\n\
    public String viaLocal() {\n\
        Mailer Helper = new Mailer();\n\
        return Helper.slug(\"l\");\n\
    }\n\
}\n";
    let tmp = tree(&[(HELPER, HELPER_JAVA), ("src/app/Mailer.java", mailer), (CALLER, caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, CALLER),
        [edge(CALLER, "viaField", "src/app/Mailer.java", "slug"), edge(CALLER, "viaLocal", "src/app/Mailer.java", "slug")],
        "the binding's proof (Mailer), never the same-named type's static"
    );
}

/// A binding named like a type whose type the file cannot read — an untyped
/// lambda parameter — obscures the type of its name anywhere in the file: the
/// call keeps its bare row, as before S-854.
#[test]
fn an_unproven_binding_named_like_a_type_is_not_the_type() {
    let caller = "package app;\n\
\n\
import java.util.function.Function;\n\
\n\
public class Caller {\n\
    Function<Helper, String> f = Helper -> Helper.slug(\"p\");\n\
}\n";
    let (_tmp, engine) = unbound_from_caller(&[(HELPER, HELPER_JAVA), (CALLER, caller)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("slug".to_string(), false)]);
}

/// A single-static-import names a member, and a field it brings in is a
/// variable of the file: it obscures the same-package type of its name, so
/// `Helper.slug(…)` is the field's call, never `Helper::slug` (commons-lang's
/// `import static …Supplementary.CharU20001;` then `CharU20001.indexOf(…)`).
#[test]
fn a_statically_imported_field_named_like_a_type_is_the_field() {
    let consts = "package app;\n\npublic class Consts {\n    public static final String Helper = \"h\";\n}\n";
    let caller = "package app;\n\
\n\
import static app.Consts.Helper;\n\
\n\
public class Caller {\n\
    public int m() { return Helper.indexOf(\"h\"); }\n\
}\n";
    let (_tmp, engine) = unbound_from_caller(&[(HELPER, HELPER_JAVA), ("src/app/Consts.java", consts), (CALLER, caller)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("indexOf".to_string(), false)]);
}

/// A static-on-demand import (`import static app.Consts.*;`) may bring in a
/// field of any name, which obscures a type of its name: a name the file can
/// see no further keeps its bare row rather than the shape proof — only the
/// shape proof, since a single-type import still names its type.
#[test]
fn a_static_wildcard_import_withholds_the_shape_proof() {
    let consts = "package app;\n\npublic class Consts {\n    public static final Mailer Helper = new Mailer();\n}\n";
    let mailer = "package app;\n\npublic class Mailer {\n    public String slug(String s) { return s; }\n}\n";
    let caller = "package app;\n\
\n\
import static app.Consts.*;\n\
\n\
public class Caller {\n\
    public String m() { return Helper.slug(\"w\"); }\n\
}\n";
    let (_tmp, engine) = unbound_from_caller(&[
        (HELPER, HELPER_JAVA),
        ("src/app/Consts.java", consts),
        ("src/app/Mailer.java", mailer),
        (CALLER, caller),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("slug".to_string(), false)]);
}

/// **Known limitation, pinned — deferred for a decision (S-854 review).** A
/// capitalised field the caller's class inherits from another file — from a
/// superclass or an interface, seen from the class itself, an inner class or
/// an anonymous body — obscures a same-package type of its name in Java, but
/// the file cannot see it, so the shape proof writes `Helper::slug` and the
/// package rung binds the type's static `slug`: an invented edge where Java
/// calls `Mailer.slug`. It needs a field name against Java's convention and a
/// same-package type of that name declaring the method. Refusing the proof in
/// every class recording a supertype gives back 2,803 of the 3,037 rows it
/// newly binds on commons-lang (`Serializable` alone); a precise refusal reads
/// the supertypes' fields, which is the binder's (the S-468 limitation
/// `known_limitation_an_inherited_member_type_does_not_shadow_a_same_package_type`
/// is the same trade-off). When it is fixed this fails and states the new rule.
#[test]
fn known_limitation_an_inherited_capitalised_field_reads_as_the_same_package_type() {
    let mailer = "package app;\n\npublic class Mailer {\n    public String slug(String s) { return s; }\n}\n";
    let base = "package app;\n\npublic class Base {\n    protected Mailer Helper = new Mailer();\n}\n";
    let caller = "package app;\n\npublic class Caller extends Base {\n    public String m() { return Helper.slug(\"x\"); }\n}\n";
    let tmp = tree(&[(HELPER, HELPER_JAVA), ("src/app/Mailer.java", mailer), ("src/app/Base.java", base), (CALLER, caller)]);
    let engine = index(tmp.path());
    assert_eq!(
        edges_from(engine.runtime().unwrap(), CALLER),
        [edge(CALLER, "m", HELPER, "slug")],
        "the limitation is closed — expect `Mailer.slug`, or no edge"
    );
}

/// A constant-shaped name (`LOG`, all capitals) is a field by Java's
/// convention — here one the class inherits from another file — never a type:
/// its call keeps its bare row and its `no-receiver-evidence` reason.
#[test]
fn a_constant_shaped_name_is_not_a_type_name() {
    let base = "package app;\n\npublic class Base {\n    protected static final Logger LOG = new Logger();\n}\n";
    let caller = "package app;\n\npublic class Caller extends Base {\n    public void m() { LOG.info(\"x\"); }\n}\n";
    let (_tmp, engine) = unbound_from_caller(&[("src/app/Base.java", base), (CALLER, caller)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, CALLER), [("info".to_string(), false)]);
    assert_eq!(nonzero(&residue(&engine, "java")), reasons(&[(R::NoReceiverEvidence, 1)]));
}

/// A type an on-demand package import (`import app.*;`) brings in reaches
/// the binder as `Helper::slug` too, but no rung scopes a type-qualified row
/// through a package wildcard: the static call stays unbound exactly as a
/// typed receiver of the same type does (`Helper h; h.greet()`), both read
/// `external-type`. The binder's wildcard reading is unchanged by S-854
/// (CR-271 §3.3) — a pre-existing gap, listed for a follow-up.
#[test]
fn known_limitation_a_package_wildcard_scopes_neither_a_static_call_nor_a_typed_receiver() {
    let caller = "package app.boot;\n\nimport app.*;\n\npublic class Caller {\n    public String m() { return Helper.slug(\"w\"); }\n    public String t(Helper h) { return h.greet(\"t\"); }\n}\n";
    let path = "src/app/boot/Caller.java";
    let tmp = tree(&[(HELPER, HELPER_JAVA), (path, caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, path).is_empty());
    assert_eq!(call_rows(rt, path), [("Helper::greet".to_string(), false), ("Helper::slug".to_string(), false)]);
}

/// The Java query's `type_name` marker is the type-name convention: a
/// capitalised name, one letter included, is a type; an all-capitals one
/// (`URL`, `MAX_SIZE`) is a constant's, and a lowercase one a variable's. Each
/// name sits one rule from its neighbour; none is declared or imported, so
/// the marker alone decides the row.
#[test]
fn the_type_name_convention_is_capitalised_and_never_all_capitals() {
    let caller = "package app;\n\
\n\
public class Caller {\n\
    public void m() {\n\
        Helper.a(); T.b(); IOUtils.c(); Outer$Inner.d();\n\
        URL.e(); MAX_SIZE.f(); helper.g(); _Foo.h();\n\
    }\n\
}\n";
    let (_tmp, engine) = unbound_from_caller(&[(CALLER, caller)]);
    let rt = engine.runtime().unwrap();
    let rows: Vec<String> = call_rows(rt, CALLER).into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        rows,
        ["Helper::a", "IOUtils::c", "Outer$Inner::d", "T::b", "e", "f", "g", "h"],
        "typed: Helper, T, IOUtils, Outer$Inner; bare: URL, MAX_SIZE, helper, _Foo"
    );
}

/// A qualified head (`app.Helper.slug(…)`) is a field access, which no
/// receiver marker reads: its row is the one it was before S-854.
#[test]
fn a_qualified_head_is_unchanged() {
    let caller = "package app.boot;\n\npublic class Caller {\n    public String m() { return app.Helper.slug(\"q\"); }\n}\n";
    let path = "src/app/boot/Caller.java";
    let tmp = tree(&[(HELPER, HELPER_JAVA), (path, caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(call_rows(rt, path), [("slug".to_string(), false)]);
}

// ── Java: sync ≡ reindex ───────────────────────────────────────────────────

/// Every edge and every ledger row, by symbol — the store's binding state.
fn binding_facts(rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    rt.submit_read(|store| {
        let sym: HashMap<NodeId, String> =
            store.all_nodes()?.into_iter().map(|n| (n.id, n.symbol.as_str().to_string())).collect();
        let mut edges: Vec<(String, String, String)> = store
            .all_edges()?
            .into_iter()
            .map(|e| (sym[&e.source].clone(), sym[&e.target].clone(), e.kind.as_str().to_string()))
            .collect();
        edges.sort();
        let mut refs: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .map(|r| format!("{} {} {:?} {:?} {}", r.source_symbol, r.target, r.form, r.kind, r.resolved))
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// The binding state of a cold index over the files of `paths` `tmp` holds now.
fn cold_facts(tmp: &TempDir, paths: &[&str]) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in paths {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(cold.path());
    binding_facts(engine.runtime().unwrap())
}

const C16_PATHS: [&str; 3] = [HELPER, USE, BOOT];

/// The type arrives after its caller was indexed: the stored `Helper::slug`
/// row binds on the sync that adds `Helper.java`, as a cold index binds it.
#[test]
fn sync_equals_a_full_reindex_when_the_same_package_type_arrives() {
    let tmp = tree(&[(USE, USE_JAVA), (BOOT, BOOT_JAVA)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, USE).is_empty(), "precondition: Helper is declared nowhere yet");
    write(tmp.path(), HELPER, HELPER_JAVA);
    engine.pipeline().sync(&[HELPER.into()]);
    assert_eq!(edges_from(rt, USE), [edge(USE, "staticCall", HELPER, "slug")]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &C16_PATHS));
}

/// The type leaves the package: the edge goes, the row stays `Helper::slug`
/// unbound, as a cold index leaves it.
#[test]
fn sync_equals_a_full_reindex_when_the_same_package_type_is_deleted() {
    let tmp = tree(&c16());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, USE), [edge(USE, "staticCall", HELPER, "slug")]);
    fs::remove_file(tmp.path().join(HELPER)).unwrap();
    engine.pipeline().sync(&[HELPER.into()]);
    assert!(edges_from(rt, USE).is_empty());
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &C16_PATHS));
}

/// The caller is edited to make a second same-package static call: its
/// re-extracted rows are `Helper::slug` and both bind, as a cold index binds
/// them — an edit the sync must read, so a sync that re-extracted nothing
/// would leave the new caller's edge out.
#[test]
fn sync_equals_a_full_reindex_when_the_caller_is_edited() {
    let tmp = tree(&c16());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let edited = USE_JAVA.replace(
        "public class Use {\n",
        "public class Use {\n    public String second() { return Helper.slug(\"y\"); }\n",
    );
    assert_ne!(edited, USE_JAVA, "precondition: the edit applies");
    write(tmp.path(), USE, &edited);
    engine.pipeline().sync(&[USE.into()]);
    assert_eq!(
        edges_from(rt, USE),
        [edge(USE, "second", HELPER, "slug"), edge(USE, "staticCall", HELPER, "slug")]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &C16_PATHS));
}

// ── C#, Scala, Go: the same shape, recorded ────────────────────────────────

/// C#: a static call on a type of the caller's namespace, declared in another
/// file, with no `using`. The namespace model binds it already (S-553).
#[cfg(feature = "lang-c-sharp")]
#[test]
fn csharp_a_same_namespace_static_call_binds_without_a_using() {
    let helper = "namespace App\n{\n    public class Helper\n    {\n        public static string Slug(string s) { return s; }\n    }\n}\n";
    let caller = "namespace App\n{\n    public class Use\n    {\n        public string StaticCall() { return Helper.Slug(\" x \"); }\n    }\n}\n";
    let tmp = tree(&[("src/App/Helper.cs", helper), ("src/App/Use.cs", caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "src/App/Use.cs"), [edge("src/App/Use.cs", "StaticCall", "src/App/Helper.cs", "Slug")]);
}

/// Scala: a call on an `object` of the caller's package, declared in another
/// file, with no import (S-573's companion objects). It binds already.
#[cfg(feature = "lang-scala")]
#[test]
fn scala_a_same_package_object_call_binds_without_an_import() {
    let helper = "package app\n\nobject Helper {\n  def slug(s: String): String = s.trim\n}\n";
    let caller = "package app\n\nclass Use {\n  def staticCall(): String = Helper.slug(\" x \")\n}\n";
    let tmp = tree(&[("src/app/Helper.scala", helper), ("src/app/Use.scala", caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/app/Use.scala"),
        [edge("src/app/Use.scala", "staticCall", "src/app/Helper.scala", "slug")]
    );
}

/// Go: a method expression `Helper.Slug(h, …)` on a type of the caller's
/// package, declared in another file — Go's one spelling of a call through a
/// type name. Recorded, not fixed: Go is a path-model language, which never
/// reads the Java check, and its query marks no receiver of this call, so the
/// row is the bare `Slug` — as it is for `var h Helper; h.Slug(…)`. A Go
/// receiver-typing gap of its own, listed for a follow-up.
#[cfg(feature = "lang-go")]
#[test]
fn known_limitation_go_a_same_package_method_expression_stays_unbound() {
    let helper = "package app\n\ntype Helper struct{}\n\nfunc (Helper) Slug(s string) string { return s }\n";
    let caller = "package app\n\nfunc StaticCall() string { return Helper.Slug(Helper{}, \" x \") }\n";
    let tmp = tree(&[("go.mod", "module example.com/app\n\ngo 1.22\n"), ("app/helper.go", helper), ("app/use.go", caller)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, "app/use.go").is_empty());
    assert_eq!(call_rows(rt, "app/use.go"), [("Slug".to_string(), false)]);
}
