//! A Java call site carries its receiver's proven type (S-467, CR-150 §3.2 A,
//! FR-EX-08, FR-RS-06, NFR-RA-05, NFR-MA-01) — exercised end-to-end through the
//! public [`Engine`] façade against real temp-directory fixtures.
//!
//! Before S-467 every Java receiver call recorded its bare name as a Method-form
//! `send`, and the receiver was discarded, so the binder could not tell one
//! `send()` from another. Each fixture here pins one receiver shape of CR-150
//! §3.2 A to the type-qualified `T::send` Path-form row it now records — typed
//! field, parameter, local, `this.x`, `this`, a bare call, `super`, a static
//! type name — and each refusal shape to the bare Method-form row it recorded
//! before: a chained call, an untyped lambda parameter, a generic type
//! variable, a name the file declares with two disagreeing types.
//!
//! `T` is written as the file names it; its scope — the file's single-type
//! imports, then the same package — is the binder's package rung (S-465). The
//! qualification fixtures pin that end to end: an imported `Mailer` beside a
//! same-package `Mailer` binds to the imported one.
//!
//! [FR-EX-08]: ../../docs/specs/requirements/FR-EX-08.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md

#![cfg(all(feature = "lang-java", feature = "lang-rust"))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// One `Calls` ledger row: `(source name, target, form)`.
type CallRow = (String, String, RefForm);

/// The `Calls` rows whose source declaration lies in `file`, sorted.
fn calls_from(rt: &Runtime, file: &str) -> Vec<CallRow> {
    let needle = file.to_string();
    let mut rows: Vec<CallRow> = rt
        .submit_read(move |store| {
            let sources: HashMap<String, (String, String)> = store
                .all_nodes()?
                .into_iter()
                .map(|n| {
                    let file = n.file_path.unwrap_or_default();
                    (n.symbol.as_str().to_string(), (file, n.name))
                })
                .collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
                .filter_map(|r| {
                    let (f, name) = sources.get(&r.source_symbol)?;
                    (*f == needle).then(|| (name.clone(), r.target, r.form))
                })
                .collect())
        })
        .expect("read runs");
    rows.sort_by(|a, b| (&a.0, &a.1, a.2.as_i32()).cmp(&(&b.0, &b.1, b.2.as_i32())));
    rows
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

fn row(source: &str, target: &str, form: RefForm) -> CallRow {
    (source.to_string(), target.to_string(), form)
}

const MAILER_FILE: &str = "src/main/java/com/x/mail/Mailer.java";
/// The same-package decoy: `Svc` imports `com.x.mail.Mailer`, so this one is
/// shadowed — a single-type import is read before the package.
const DECOY_FILE: &str = "src/main/java/com/x/svc/Mailer.java";
const AUDIT_FILE: &str = "src/main/java/com/x/svc/Audit.java";
const CLOCK_FILE: &str = "src/main/java/com/x/util/Clock.java";
const BASE_FILE: &str = "src/main/java/com/x/base/Base.java";
const SVC_FILE: &str = "src/main/java/com/x/svc/Svc.java";

const MAILER: &str = "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n}\n";
const DECOY: &str = "package com.x.svc;\n\npublic class Mailer {\n    public void send() {}\n}\n";
const AUDIT: &str = "package com.x.svc;\n\npublic class Audit {\n    public void send() {}\n}\n";
const CLOCK: &str = "package com.x.util;\n\npublic class Clock {\n    public static long now() { return 0; }\n}\n";
const BASE: &str = "package com.x.base;\n\npublic class Base {\n    public void start() {}\n}\n";

/// One method per receiver shape of CR-150 §3.2 A, each with one call site.
const SVC: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base;\n\
import com.x.mail.Mailer;\n\
import com.x.util.Clock;\n\
\n\
public class Svc extends Base {\n\
    private Mailer mailer;\n\
    private Audit audit;\n\
    public void viaField() { mailer.send(); }\n\
    public void viaParam(Mailer given) { given.send(); }\n\
    public void viaLocal() { Mailer local = new Mailer(); local.send(); }\n\
    public void viaThisField() { this.mailer.send(); }\n\
    public void viaThis() { this.helper(); }\n\
    public void viaBare() { helper(); }\n\
    public void viaSuper() { super.start(); }\n\
    public void viaStatic() { Clock.now(); }\n\
    public void viaSamePackage() { audit.send(); }\n\
    void helper() {}\n\
}\n";

fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in [
        (MAILER_FILE, MAILER),
        (DECOY_FILE, DECOY),
        (AUDIT_FILE, AUDIT),
        (CLOCK_FILE, CLOCK),
        (BASE_FILE, BASE),
        (SVC_FILE, SVC),
    ] {
        write(tmp.path(), rel, text);
    }
    tmp
}

#[test]
fn every_proven_receiver_shape_records_a_type_qualified_path_row() {
    let tmp = fixture();
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), SVC_FILE),
        [
            row("viaBare", "Svc::helper", RefForm::Path),
            row("viaField", "Mailer::send", RefForm::Path),
            row("viaLocal", "Mailer::send", RefForm::Path),
            row("viaParam", "Mailer::send", RefForm::Path),
            row("viaSamePackage", "Audit::send", RefForm::Path),
            row("viaStatic", "Clock::now", RefForm::Path),
            row("viaSuper", "Base::start", RefForm::Path),
            row("viaThis", "Svc::helper", RefForm::Path),
            row("viaThisField", "Mailer::send", RefForm::Path),
        ]
    );
}

#[test]
fn the_type_is_qualified_through_the_single_type_import_then_the_same_package() {
    // `Mailer` is imported from `com.x.mail` AND declared in the caller's own
    // package: the import is read first, so every typed `Mailer` call binds to
    // the imported class and none to the decoy. `Audit` has no import, so the
    // same package supplies it.
    let tmp = fixture();
    let engine = index(&tmp);
    let edges = call_edges(engine.runtime().unwrap());
    let into = |target: &str| -> Vec<String> {
        edges
            .iter()
            .filter(|(_, t)| t == target)
            .map(|(s, _)| s.clone())
            .collect()
    };
    assert_eq!(
        into(&format!("{MAILER_FILE}:send")),
        [
            format!("{SVC_FILE}:viaField"),
            format!("{SVC_FILE}:viaLocal"),
            format!("{SVC_FILE}:viaParam"),
            format!("{SVC_FILE}:viaThisField"),
        ]
    );
    assert!(into(&format!("{DECOY_FILE}:send")).is_empty(), "{edges:?}");
    assert_eq!(
        into(&format!("{AUDIT_FILE}:send")),
        [format!("{SVC_FILE}:viaSamePackage")]
    );
    assert_eq!(into(&format!("{CLOCK_FILE}:now")), [format!("{SVC_FILE}:viaStatic")]);
}

const REFUSE_FILE: &str = "src/main/java/com/x/svc/Refuse.java";
/// Every shape CR-150 §3.2 A refuses, each beside the declaration that would
/// type it were the refusal missing (the near miss):
///
/// * `chained` — `make().send()`: the receiver is an expression, not a name.
/// * `lambda` — `x` is an untyped lambda parameter; a FIELD `x` typed `Mailer`
///   is what a scope-blind reading would answer with.
/// * `generic` — `item` is declared `T`, the class's type parameter, beside a
///   same-package class named `T`.
/// * `disagree` — `twice` is a `Mailer` field and an `Audit` parameter.
/// * `array` — `many` is a `Mailer[]`: an array has no `send`.
/// * `inferred` — `var v`: `var` names no type.
const REFUSE: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
import java.util.List;\n\
\n\
public class Refuse<T> {\n\
    private Mailer x;\n\
    private T item;\n\
    private Mailer twice;\n\
    private Mailer[] many;\n\
    private List<Mailer> all;\n\
    public void chained() { make().send(); }\n\
    public void lambda() { all.forEach(x -> x.send()); }\n\
    public void generic() { item.send(); }\n\
    public void disagree(Audit twice) { twice.send(); }\n\
    public void array() { many.clone(); }\n\
    public void inferred() { var v = new Mailer(); v.send(); }\n\
    Mailer make() { return x; }\n\
}\n";
const T_FILE: &str = "src/main/java/com/x/svc/T.java";
const T_CLASS: &str = "package com.x.svc;\n\npublic class T {\n    public void send() {}\n}\n";

#[test]
fn an_unprovable_receiver_keeps_the_bare_method_form_row() {
    let tmp = fixture();
    write(tmp.path(), REFUSE_FILE, REFUSE);
    write(tmp.path(), T_FILE, T_CLASS);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, REFUSE_FILE),
        [
            row("array", "clone", RefForm::Method),
            row("chained", "Refuse::make", RefForm::Path),
            row("chained", "send", RefForm::Method),
            row("disagree", "send", RefForm::Method),
            row("generic", "send", RefForm::Method),
            row("inferred", "send", RefForm::Method),
            row("lambda", "List::forEach", RefForm::Path),
            row("lambda", "send", RefForm::Method),
        ]
    );
    // The near miss, bound: no refused site reaches `T.send` or `Mailer.send`.
    let edges = call_edges(rt);
    assert!(
        !edges.iter().any(|(s, _)| s.starts_with(REFUSE_FILE) && !s.ends_with(":chained")),
        "{edges:?}"
    );
}

/// The enclosing class is `this` only where the file can name it, and a bare
/// call is the enclosing class's only where the class declares the name
/// itself or nothing else in scope could supply it:
///
/// * an anonymous class body's `this` is the anonymous class — no name;
/// * a nested class's bare `outerOnly()` is the OUTER class's method, which a
///   lexical scope walk finds and `Inner::outerOnly` would not;
/// * a statically imported `now()` is the import's, when the class declares no
///   `now` of its own; the class's own `tick()`, which no import names, is
///   the class's. (An own member shadowing an import is `EDGE`'s `own`.)
const SCOPE_FILE: &str = "src/main/java/com/x/svc/Scope.java";
const SCOPE: &str = "package com.x.svc;\n\
\n\
import static com.x.util.Clock.now;\n\
\n\
public class Scope {\n\
    public void anonymous() {\n\
        Runnable r = new Runnable() {\n\
            public void run() { helper(); this.helper(); }\n\
        };\n\
    }\n\
    public void imported() { now(); }\n\
    public void own() { tick(); }\n\
    void tick() {}\n\
    void helper() {}\n\
    void outerOnly() {}\n\
    class Inner {\n\
        void go() { outerOnly(); mine(); }\n\
        void mine() {}\n\
    }\n\
}\n";

#[test]
fn a_bare_or_this_call_is_typed_only_where_the_enclosing_class_is_provable() {
    let tmp = fixture();
    write(tmp.path(), SCOPE_FILE, SCOPE);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, SCOPE_FILE),
        [
            row("go", "Inner::mine", RefForm::Path),
            row("go", "outerOnly", RefForm::Path),
            row("imported", "now", RefForm::Path),
            row("own", "Scope::tick", RefForm::Path),
            row("run", "helper", RefForm::Path),
            row("run", "helper", RefForm::Method),
        ]
    );
    // The untyped shapes still bind as they did: the outer method by lexical
    // scope, the import by its static import.
    let edges = call_edges(rt);
    for (source, target) in [
        ("go", format!("{SCOPE_FILE}:outerOnly")),
        ("go", format!("{SCOPE_FILE}:mine")),
        ("imported", format!("{CLOCK_FILE}:now")),
        ("own", format!("{SCOPE_FILE}:tick")),
    ] {
        let edge = (format!("{SCOPE_FILE}:{source}"), target);
        assert!(edges.contains(&edge), "{edge:?} not in {edges:?}");
    }
}

/// A static type name is a type only when the file proves it is one — its
/// single-type import or its own declaration — and no variable of that name
/// obscures it. A STATIC import names a member, never a type.
const STATIC_FILE: &str = "src/main/java/com/x/svc/Statics.java";
const STATICS: &str = "package com.x.svc;\n\
\n\
import static com.x.util.Clock.INSTANCE;\n\
import com.x.util.Clock;\n\
\n\
public class Statics {\n\
    public void own() { Nested.make(); }\n\
    public void constant() { INSTANCE.tick(); }\n\
    public void undeclared() { Audit.make(); }\n\
    static class Nested {\n\
        static void make() {}\n\
    }\n\
}\n";

#[test]
fn a_static_type_name_is_typed_through_the_files_import_or_own_declaration_only() {
    let tmp = fixture();
    write(tmp.path(), STATIC_FILE, STATICS);
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), STATIC_FILE),
        [
            row("constant", "tick", RefForm::Method),
            row("own", "Nested::make", RefForm::Path),
            row("undeclared", "make", RefForm::Method),
        ]
    );
}

#[test]
fn a_site_records_one_row_and_the_ledger_count_is_unchanged() {
    // One row per call site across every fixture file — no method here calls a
    // name through two receivers the ledger's dedup key would split or merge
    // (`Scope.run`'s `helper()` and `this.helper()` stay two rows by form) — so
    // the `Calls` count is exactly what it was when every receiver call was a
    // bare row (measured against the pre-S-467 extraction: 9 + 8 + 6 + 3).
    let tmp = fixture();
    write(tmp.path(), REFUSE_FILE, REFUSE);
    write(tmp.path(), T_FILE, T_CLASS);
    write(tmp.path(), SCOPE_FILE, SCOPE);
    write(tmp.path(), STATIC_FILE, STATICS);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let counts: Vec<usize> = [SVC_FILE, REFUSE_FILE, SCOPE_FILE, STATIC_FILE]
        .iter()
        .map(|f| calls_from(rt, f).len())
        .collect();
    assert_eq!(counts, [9, 8, 6, 3]);
    // Never both: no method holds a typed `T::name` row beside a bare `name`.
    for file in [SVC_FILE, REFUSE_FILE, SCOPE_FILE, STATIC_FILE] {
        let rows = calls_from(rt, file);
        for (source, target, _) in &rows {
            if let Some((_, name)) = target.rsplit_once("::") {
                assert!(
                    !rows.iter().any(|(s, t, _)| s == source && t == name),
                    "{file}: {source} records both {target} and {name}"
                );
            }
        }
    }
}

#[test]
fn no_java_row_reaches_the_method_form_dyn_dispatch_branch() {
    // The binder reads a Method-form `Calls` target containing `::` as a Rust
    // `&dyn T` dispatch (S-281, FR-RS-08). Every typed Java row is Path form by
    // construction, so no Java row may ever be Method form AND qualified.
    let tmp = fixture();
    write(tmp.path(), REFUSE_FILE, REFUSE);
    write(tmp.path(), SCOPE_FILE, SCOPE);
    write(tmp.path(), STATIC_FILE, STATICS);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let mut typed = 0;
    for file in [SVC_FILE, REFUSE_FILE, SCOPE_FILE, STATIC_FILE] {
        for (source, target, form) in calls_from(rt, file) {
            assert!(
                !(form == RefForm::Method && target.contains("::")),
                "{file}: {source} → {target} is Method form and qualified"
            );
            typed += usize::from(target.contains("::"));
        }
    }
    // Not vacuous: the corpus does record qualified rows.
    assert!(typed >= 12, "only {typed} qualified rows");
}

/// The guards the shapes above cannot tell apart, each as its near miss:
///
/// * `shadowed` — `this.mailer` is the FIELD, though a parameter `mailer` of
///   another type poisons the scope-blind name (S-398's field map).
/// * `wildcard` — `now()` is what `import static Clock.*` supplies; the class
///   declares no `now`.
/// * `own` — the class's own `tick()` shadows the static wildcard.
/// * `obscuredA` / `obscuredB` — a variable named `Clock`, declared with two
///   disagreeing types, still obscures the imported type `Clock`: a poisoned
///   variable is a variable, not a type name.
const EDGE_FILE: &str = "src/main/java/com/x/svc/Edge.java";
const EDGE: &str = "package com.x.svc;\n\
\n\
import static com.x.util.Clock.*;\n\
import com.x.mail.Mailer;\n\
import com.x.util.Clock;\n\
\n\
public class Edge {\n\
    private Mailer mailer;\n\
    public void shadowed(Audit mailer) { this.mailer.send(); }\n\
    public void wildcard() { now(); }\n\
    public void own() { tick(); }\n\
    public void obscuredA(Mailer Clock) { Clock.send(); }\n\
    public void obscuredB(Audit Clock) { Clock.send(); }\n\
    long tick() { return 0; }\n\
}\n";

#[test]
fn a_field_a_wildcard_import_an_own_member_and_an_obscuring_variable_each_decide_their_site() {
    let tmp = fixture();
    write(tmp.path(), EDGE_FILE, EDGE);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, EDGE_FILE),
        [
            row("obscuredA", "send", RefForm::Method),
            row("obscuredB", "send", RefForm::Method),
            row("own", "Edge::tick", RefForm::Path),
            row("shadowed", "Mailer::send", RefForm::Path),
            row("wildcard", "now", RefForm::Path),
        ]
    );
    let edges = call_edges(rt);
    for (source, target) in [
        ("shadowed", format!("{MAILER_FILE}:send")),
        ("wildcard", format!("{CLOCK_FILE}:now")),
        ("own", format!("{EDGE_FILE}:tick")),
    ] {
        let edge = (format!("{EDGE_FILE}:{source}"), target);
        assert!(edges.contains(&edge), "{edge:?} not in {edges:?}");
    }
}

/// Every edge and every non-Symbol ledger row, by symbol — the store's whole
/// binding state.
fn binding_facts(rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    rt.submit_read(|store| {
        let sym: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.symbol.as_str().to_string()))
            .collect();
        let mut edges: Vec<(String, String, String)> = store
            .all_edges()?
            .into_iter()
            .map(|e| {
                (
                    sym[&e.source].clone(),
                    sym[&e.target].clone(),
                    e.kind.as_str().to_string(),
                )
            })
            .collect();
        edges.sort();
        let mut refs: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .map(|r| {
                format!(
                    "{} {} {:?} {:?} {}",
                    r.source_symbol, r.target, r.form, r.kind, r.resolved
                )
            })
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// The binding state of a cold index over the files `tmp` holds now.
fn cold_facts(tmp: &TempDir) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in [MAILER_FILE, DECOY_FILE, AUDIT_FILE, CLOCK_FILE, BASE_FILE, SVC_FILE] {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(&cold);
    binding_facts(engine.runtime().unwrap())
}

#[test]
fn sync_equals_a_full_reindex_after_editing_the_calling_file() {
    // `Svc` drops its `Mailer` import: every `Mailer` receiver now names the
    // same-package decoy, and `super` has no `extends` to read.
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    write(
        tmp.path(),
        SVC_FILE,
        &SVC.replace("import com.x.mail.Mailer;\n", "")
            .replace(" extends Base", ""),
    );
    engine.sync(&[SVC_FILE.into()]);
    assert!(calls_from(rt, SVC_FILE).contains(&row("viaSuper", "start", RefForm::Method)));
    assert!(call_edges(rt).contains(&(
        format!("{SVC_FILE}:viaField"),
        format!("{DECOY_FILE}:send")
    )));
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

#[test]
fn sync_equals_a_full_reindex_after_the_receivers_type_gains_the_method() {
    // The imported `Mailer` first declares no `send`: every typed call into it
    // stays unbound. Adding `send` binds them, although `Svc` did not change —
    // the row's `send` token is what re-selects it.
    let tmp = fixture();
    write(
        tmp.path(),
        MAILER_FILE,
        "package com.x.mail;\n\npublic class Mailer {\n    public void other() {}\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        call_edges(rt).iter().all(|(_, t)| !t.starts_with(MAILER_FILE)),
        "precondition: nothing binds into Mailer yet"
    );
    write(tmp.path(), MAILER_FILE, MAILER);
    engine.sync(&[MAILER_FILE.into()]);
    let into_mailer = call_edges(rt)
        .into_iter()
        .filter(|(_, t)| *t == format!("{MAILER_FILE}:send"))
        .count();
    assert_eq!(into_mailer, 4);
    assert_eq!(binding_facts(rt), cold_facts(&tmp));
}

/// A file-wide type is a variable's type only where the variable is declared
/// in scope and every declaration of its name is one the file reads — each
/// row here was typed `Mailer::…` by a scope-blind lookup, and bound to
/// `Mailer.send`, before review (S-467 review, [NFR-RA-05]):
///
/// * `Loop.two` — a for-each variable `m` of type `Audit`, beside a `Mailer m`
///   parameter elsewhere;
/// * `Catch.go` — a catch parameter `e`, beside a `Mailer e` field;
/// * `Pattern.go` — an `instanceof Audit m` pattern variable, beside a field;
/// * `Varargs.two` — `Mailer... xs` is an array, beside a `Mailer xs`;
/// * `Inherit.two` — `m` is `Base2`'s inherited `Audit m`, beside a local
///   `Mailer m` in another method;
/// * `InheritF.Sub.two` — `this.m` is `Base2`'s field, not the outer class's.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
const SCOPED: [(&str, &str); 8] = [
    (
        "src/main/java/com/x/svc/Oops.java",
        "package com.x.svc;\n\npublic class Oops extends RuntimeException {\n    public void send() {}\n}\n",
    ),
    (
        "src/main/java/com/x/base/Base2.java",
        "package com.x.base;\n\nimport com.x.svc.Audit;\n\npublic class Base2 {\n    protected Audit m;\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Loop.java",
        "package com.x.svc;\n\nimport com.x.mail.Mailer;\nimport java.util.List;\n\npublic class Loop {\n    void one(Mailer m) { m.send(); }\n    void two(List<Audit> audits) { for (Audit m : audits) { m.send(); } }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Catch.java",
        "package com.x.svc;\n\nimport com.x.mail.Mailer;\n\npublic class Catch {\n    private Mailer e;\n    void go() { try { first(); } catch (Oops e) { e.send(); } }\n    void first() {}\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Pattern.java",
        "package com.x.svc;\n\nimport com.x.mail.Mailer;\n\npublic class Pattern {\n    private Mailer m;\n    void go(Object o) { if (o instanceof Audit m) { m.send(); } }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Varargs.java",
        "package com.x.svc;\n\nimport com.x.mail.Mailer;\n\npublic class Varargs {\n    void one(Mailer xs) { xs.send(); }\n    void two(Mailer... xs) { xs.toString(); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Inherit.java",
        "package com.x.svc;\n\nimport com.x.base.Base2;\nimport com.x.mail.Mailer;\n\npublic class Inherit extends Base2 {\n    void one() { Mailer m = new Mailer(); m.send(); }\n    void two() { m.send(); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/InheritF.java",
        "package com.x.svc;\n\nimport com.x.base.Base2;\nimport com.x.mail.Mailer;\n\npublic class InheritF {\n    private Mailer m;\n    void one() { m.send(); }\n    static class Sub extends Base2 {\n        void two() { this.m.send(); }\n    }\n}\n",
    ),
];

#[test]
fn a_variable_is_typed_only_where_it_is_declared_in_scope_and_readably() {
    let tmp = fixture();
    for (rel, text) in SCOPED {
        write(tmp.path(), rel, text);
    }
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let rows = |file: &str| calls_from(rt, &format!("src/main/java/com/x/svc/{file}.java"));
    // The near misses still type: a local, an own field. (`Loop.one`'s `m` does
    // not: a for-each `m` poisons the name for the whole file, like a
    // disagreeing declaration — the refusing direction.)
    assert!(rows("Loop").contains(&row("one", "send", RefForm::Method)));
    assert!(rows("Inherit").contains(&row("one", "Mailer::send", RefForm::Path)));
    assert!(rows("InheritF").contains(&row("one", "Mailer::send", RefForm::Path)));
    // The refusals keep the bare row.
    for (file, source, name) in [
        ("Loop", "two", "send"),
        ("Catch", "go", "send"),
        ("Pattern", "go", "send"),
        ("Varargs", "two", "toString"),
        ("Inherit", "two", "send"),
        ("InheritF", "two", "send"),
    ] {
        assert!(
            rows(file).contains(&row(source, name, RefForm::Method)),
            "{file}.{source}: {:?}",
            rows(file)
        );
    }
    // And none of them binds to `Mailer.send`.
    let wrong: Vec<_> = call_edges(rt)
        .into_iter()
        .filter(|(s, t)| {
            *t == format!("{MAILER_FILE}:send")
                && ["Loop.java:two", "Catch.java:go", "Pattern.java:go", "Inherit.java:two", "InheritF.java:two"]
                    .iter()
                    .any(|w| s.ends_with(w))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// A declared type the file QUALIFIES keeps no typing through its simple name:
/// `com.b.Mailer other` beside `import com.x.mail.Mailer` would be re-qualified
/// by the import to the other class, and `Map.Entry e` by the same package to
/// an in-house `Entry` (S-467 review, [NFR-RA-05]). The simple declarations
/// beside them still type.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
const QUALIFIED_FILE: &str = "src/main/java/com/x/svc/Qualified.java";
const QUALIFIED: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
import java.util.Map;\n\
\n\
public class Qualified {\n\
    private Mailer mine;\n\
    private com.x.other.Mailer other;\n\
    void a() { other.send(); }\n\
    void b(Map.Entry<String, String> e) { e.getValue(); }\n\
    void c() { com.x.other.Mailer local = null; local.send(); }\n\
    void d() { mine.send(); }\n\
}\n";

#[test]
fn a_qualified_declared_type_is_never_re_qualified_through_its_simple_name() {
    let tmp = fixture();
    write(tmp.path(), QUALIFIED_FILE, QUALIFIED);
    write(
        tmp.path(),
        "src/main/java/com/x/other/Mailer.java",
        "package com.x.other;\n\npublic class Mailer {\n    public void send() {}\n}\n",
    );
    write(
        tmp.path(),
        "src/main/java/com/x/svc/Entry.java",
        "package com.x.svc;\n\npublic class Entry {\n    public String getValue() { return null; }\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, QUALIFIED_FILE),
        [
            row("a", "send", RefForm::Method),
            row("b", "getValue", RefForm::Method),
            row("c", "send", RefForm::Method),
            row("d", "Mailer::send", RefForm::Path),
        ]
    );
    let wrong: Vec<_> = call_edges(rt)
        .into_iter()
        .filter(|(s, _)| s.starts_with(QUALIFIED_FILE) && !s.ends_with(":d"))
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// `Outer.super.start()` calls `Outer`'s SUPERCLASS's `start`, but its receiver
/// parses as the name `Outer` followed by `super`: read as a simple name it was
/// typed `Outer::start`, a type proof naming `Outer`'s own override (S-467
/// review, [NFR-RA-05]). It keeps its bare row; a plain `super.start()` still
/// types.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
const OUTER_FILE: &str = "src/main/java/com/x/svc/Outer.java";
const OUTER: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base;\n\
\n\
public class Outer extends Base {\n\
    public void start() {}\n\
    void plain() { super.start(); }\n\
    class Inner {\n\
        void go() { Outer.super.start(); }\n\
    }\n\
}\n";

#[test]
fn a_qualified_super_call_is_never_typed_as_its_qualifying_class() {
    let tmp = fixture();
    write(tmp.path(), OUTER_FILE, OUTER);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, OUTER_FILE),
        [
            row("go", "start", RefForm::Method),
            row("plain", "Base::start", RefForm::Path),
        ]
    );
    // No edge is asserted: the bare row still takes the binder's pre-existing
    // lexical receiver-method rung (the CR-066 path this story leaves as it
    // was), which is a binding question, not this extraction's.
}

/// Refusals the fixtures above reach only through ANOTHER shape, each pinned
/// where the review's mutation sweep found it unguarded (S-467 review):
///
/// * `GenF` — `this.item` of a type-parameter type and `this.many` of an array
///   type refuse through the field shape too, not only the simple-name one;
/// * `Lam` — a two-parameter lambda's `(a, b)` poisons `a` like `x -> …` does;
/// * `Logr` — `log.info()` where `log` is a METHOD: a method is no type name;
/// * `Fld` — an inner class's FIELD `ping` is no member a bare `ping()` calls;
/// * `LocalC` — a class local to a method is nested too, so its bare
///   `helper()` is the outer class's.
const GAP_FILES: [(&str, &str); 5] = [
    (
        "src/main/java/com/x/svc/GenF.java",
        "package com.x.svc;\n\npublic class GenF<T> {\n    private T item;\n    private Audit[] many;\n    void a() { this.item.send(); }\n    void b() { this.many.clone(); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Lam.java",
        "package com.x.svc;\n\nimport java.util.Map;\n\npublic class Lam {\n    private Audit a;\n    void go(Map<String, String> m) { m.forEach((a, b) -> a.send()); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Logr.java",
        "package com.x.svc;\n\npublic class Logr {\n    void log(String s) {}\n    void go() { log.info(\"x\"); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Fld.java",
        "package com.x.svc;\n\npublic class Fld {\n    void ping() {}\n    class Inner {\n        int ping;\n        void go() { ping(); }\n    }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/LocalC.java",
        "package com.x.svc;\n\npublic class LocalC {\n    void helper() {}\n    public void go() {\n        class Local {\n            void run() { helper(); }\n        }\n    }\n}\n",
    ),
];

#[test]
fn each_refusal_holds_through_every_shape_that_reaches_it() {
    let tmp = fixture();
    write(tmp.path(), T_FILE, T_CLASS);
    for (rel, text) in GAP_FILES {
        write(tmp.path(), rel, text);
    }
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let rows = |file: &str| calls_from(rt, &format!("src/main/java/com/x/svc/{file}.java"));
    assert_eq!(
        rows("GenF"),
        [row("a", "send", RefForm::Method), row("b", "clone", RefForm::Method)]
    );
    assert_eq!(
        rows("Lam"),
        [row("go", "Map::forEach", RefForm::Path), row("go", "send", RefForm::Method)]
    );
    assert_eq!(rows("Logr"), [row("go", "info", RefForm::Method)]);
    assert_eq!(rows("Fld"), [row("go", "ping", RefForm::Path)]);
    assert_eq!(rows("LocalC"), [row("run", "helper", RefForm::Path)]);
}

/// Proofs that must still TYPE, each pinned where the review's mutation sweep
/// found a refusal could creep in unnoticed (S-467 review):
///
/// * `Wild` — a NON-static wildcard (`import java.util.*`) supplies no method,
///   so a bare inherited `start()` is still the class's;
/// * `Impl` — `super` is the one `extends`, however many interfaces the class
///   also `implements`;
/// * `Digits` — a type name with digits (`S3Client`) or a `$` is one
///   identifier.
const TYPED_FILES: [(&str, &str); 3] = [
    (
        "src/main/java/com/x/svc/Wild.java",
        "package com.x.svc;\n\nimport java.util.*;\nimport com.x.base.Base;\n\npublic class Wild extends Base {\n    void go() { start(); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Impl.java",
        "package com.x.svc;\n\nimport com.x.base.Base;\n\npublic class Impl extends Base implements Runnable {\n    public void run() { super.start(); }\n}\n",
    ),
    (
        "src/main/java/com/x/svc/Digits.java",
        "package com.x.svc;\n\npublic class Digits {\n    private S3Client s3;\n    private $Proxy px;\n    void a() { s3.send(); }\n    void b() { px.send(); }\n}\n",
    ),
];

#[test]
fn a_proof_still_types_beside_the_shapes_that_look_like_refusals() {
    let tmp = fixture();
    for (rel, text) in TYPED_FILES {
        write(tmp.path(), rel, text);
    }
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let rows = |file: &str| calls_from(rt, &format!("src/main/java/com/x/svc/{file}.java"));
    assert_eq!(rows("Wild"), [row("go", "Wild::start", RefForm::Path)]);
    assert_eq!(rows("Impl"), [row("run", "Base::start", RefForm::Path)]);
    assert_eq!(
        rows("Digits"),
        [row("a", "S3Client::send", RefForm::Path), row("b", "$Proxy::send", RefForm::Path)]
    );
}

/// An inner class reads its outer class's field — a `@Nested` test class and
/// its outer test's collaborators — and that field is in scope, unless the
/// inner class inherits: a supertype may declare a same-named field this file
/// cannot see, which shadows the outer one (S-467 review). An interface is a
/// supertype too — its constant `m` would shadow the field just as a
/// superclass's would (sprint-81 review).
const NESTED_FILE: &str = "src/main/java/com/x/svc/NestedT.java";
const NESTED: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base2;\n\
import com.x.mail.Mailer;\n\
\n\
public class NestedT {\n\
    private Mailer m;\n\
    class Plain {\n\
        void go() { m.send(); }\n\
    }\n\
    class Inherits extends Base2 {\n\
        void go() { m.send(); }\n\
    }\n\
    class Implements implements Runnable {\n\
        void go() { m.send(); }\n\
    }\n\
}\n";

#[test]
fn an_outer_classs_field_is_in_scope_through_inner_classes_that_inherit_nothing() {
    let tmp = fixture();
    write(tmp.path(), NESTED_FILE, NESTED);
    write(tmp.path(), SCOPED[1].0, SCOPED[1].1);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let rows = calls_from(rt, NESTED_FILE);
    assert_eq!(
        rows,
        [
            row("go", "Mailer::send", RefForm::Path),
            row("go", "send", RefForm::Method),
            row("go", "send", RefForm::Method)
        ]
    );
    // The typed row is `Plain.go`'s: it binds to the imported `Mailer`.
    let edges = call_edges(rt);
    let from_go: Vec<_> = edges.iter().filter(|(s, _)| s.starts_with(NESTED_FILE)).collect();
    assert_eq!(from_go.len(), 1, "{from_go:?}");
    assert_eq!(from_go[0].1, format!("{MAILER_FILE}:send"));
}
