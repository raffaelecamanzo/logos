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
//! variable. Since S-825 a name's type is its one binding in scope at the
//! call (CR-256): a parameter or local hides the field of its name, and each
//! re-pinned expectation below names the binding now in or out of scope.
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
use logos_core::models::CallResidueReason;
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

fn index(tmp: &TempDir) -> Engine {
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
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
/// * `disagree` — `twice` is a `Mailer` field and an `Audit` parameter. Not
///   a refusal since S-825: the parameter is the one binding in scope at the
///   call, hiding the field, so the call is `Audit::send` (re-pinned).
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
            row("disagree", "Audit::send", RefForm::Path),
            row("generic", "send", RefForm::Method),
            row("inferred", "send", RefForm::Method),
            row("lambda", "List::forEach", RefForm::Path),
            row("lambda", "send", RefForm::Method),
        ]
    );
    // The near miss, bound: no refused site reaches `T.send` or `Mailer.send`;
    // `disagree`'s parameter binds its own `Audit.send`.
    let edges = call_edges(rt);
    assert!(
        !edges
            .iter()
            .any(|(s, _)| s.starts_with(REFUSE_FILE) && !s.ends_with(":chained") && !s.ends_with(":disagree")),
        "{edges:?}"
    );
    assert!(edges.contains(&(format!("{REFUSE_FILE}:disagree"), format!("{AUDIT_FILE}:send"))), "{edges:?}");
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
/// * `obscuredA` / `obscuredB` — a parameter named `Clock` obscures the
///   imported type `Clock`: a variable is a variable, not a type name. Since
///   S-825 each is typed by its own parameter, the one binding in scope at its
///   call (`Mailer::send`, `Audit::send`; re-pinned) — never `Clock::send`.
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
            row("obscuredA", "Mailer::send", RefForm::Path),
            row("obscuredB", "Audit::send", RefForm::Path),
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
    engine.pipeline().sync(&[SVC_FILE.into()]);
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
    engine.pipeline().sync(&[MAILER_FILE.into()]);
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
    // The near misses still type: a local, an own field — and, since S-825, a
    // parameter whose name a for-each or varargs variable in another method
    // also binds (`Loop.one`, `Varargs.one`; re-pinned): that variable is in
    // scope in its own construct only, no longer poisoning the whole file.
    assert!(rows("Loop").contains(&row("one", "Mailer::send", RefForm::Path)));
    assert!(rows("Varargs").contains(&row("one", "Mailer::send", RefForm::Path)));
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

// ── S-825 (CR-256, FR-RS-10): a binding types only the calls in its scope ────

const LOGGER_FILE: &str = "src/main/java/com/x/base/Logger.java";
const LOGGER: &str = "package com.x.base;\n\npublic class Logger {\n    public void send() {}\n}\n";
/// A base class in another file: its `svc` is a field the subclasses below
/// inherit and cannot see, so a call on it after a closed block proves
/// nothing.
const SCOPED_BASE_FILE: &str = "src/main/java/com/x/base/Base3.java";
const SCOPED_BASE: &str = "package com.x.base;\n\npublic class Base3 {\n    protected Logger svc;\n    protected Logger p;\n    protected Logger e;\n    protected Logger r;\n    protected Logger before;\n}\n";

/// The Java row of the call residue readout: every unresolved-call reason
/// with a count, sorted.
fn java_residue(engine: &Engine) -> Vec<(CallResidueReason, u64)> {
    let residue = engine
        .graph().status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .and_then(|row| row.call_residue)
        .expect("the java row states its call residue");
    let mut reasons: Vec<(CallResidueReason, u64)> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    reasons.sort();
    reasons
}

/// The D1 fixture of CR-256: `m`'s `svc` is the inherited `Logger` field,
/// because the local's block closed before the call; `n` calls inside it.
const CLOSED_FILE: &str = "src/main/java/com/x/svc/C.java";
const CLOSED: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base3;\n\
import com.x.mail.Mailer;\n\
\n\
public class C extends Base3 {\n\
    void m(boolean f) { if (f) { Mailer svc = new Mailer(); } svc.send(); }\n\
    void n(boolean f) { if (f) { Mailer svc = new Mailer(); svc.send(); } }\n\
}\n";

fn closed_block_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in [
        (MAILER_FILE, MAILER),
        (LOGGER_FILE, LOGGER),
        (SCOPED_BASE_FILE, SCOPED_BASE),
        (CLOSED_FILE, CLOSED),
    ] {
        write(tmp.path(), rel, text);
    }
    tmp
}

#[test]
fn a_local_in_a_closed_block_types_no_call_after_it() {
    let tmp = closed_block_fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, CLOSED_FILE),
        [row("m", "send", RefForm::Method), row("n", "Mailer::send", RefForm::Path)]
    );
    let from_c: Vec<_> = call_edges(rt).into_iter().filter(|(s, _)| s.starts_with(CLOSED_FILE)).collect();
    assert_eq!(from_c, [(format!("{CLOSED_FILE}:n"), format!("{MAILER_FILE}:send"))]);
    assert_eq!(java_residue(&engine), [(CallResidueReason::NoReceiverEvidence, 1)]);
}

/// Each typed binding is called once inside its scope and once outside it,
/// where only the inherited `Logger` field of its name is in scope: a typed
/// lambda parameter, a `for` variable, a resource, a local before its
/// declaration. Before S-825 every outside call was typed `Mailer::send`.
const OUT_OF_SCOPE_FILE: &str = "src/main/java/com/x/svc/D.java";
const OUT_OF_SCOPE: &str = "package com.x.svc;\n\
\n\
import com.x.base.Base3;\n\
import com.x.mail.Mailer;\n\
import java.util.List;\n\
\n\
public class D extends Base3 {\n\
    void lambda(List<Mailer> ms) { ms.forEach((Mailer p) -> p.send()); p.send(); }\n\
    void loop() { for (Mailer e = new Mailer(); ; ) { e.send(); break; } e.send(); }\n\
    void resource() { try (Mailer r = new Mailer()) { r.send(); } r.send(); }\n\
    void early() { before.send(); Mailer before = new Mailer(); before.send(); }\n\
}\n";

#[test]
fn a_binding_outside_its_construct_or_before_its_declaration_types_no_call() {
    let tmp = closed_block_fixture();
    write(tmp.path(), OUT_OF_SCOPE_FILE, OUT_OF_SCOPE);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let mut expected = vec![row("lambda", "List::forEach", RefForm::Path)];
    for source in ["early", "lambda", "loop", "resource"] {
        expected.push(row(source, "Mailer::send", RefForm::Path));
        expected.push(row(source, "send", RefForm::Method));
    }
    expected.sort_by(|a, b| (&a.0, &a.1, a.2.as_i32()).cmp(&(&b.0, &b.1, b.2.as_i32())));
    assert_eq!(calls_from(rt, OUT_OF_SCOPE_FILE), expected);
}

/// A parameter or local hides the field of its name, whatever their types —
/// the scope-blind table refused every one of these, two disagreeing
/// declarations of one name — and an inner class's field shadows its outer
/// class's. The near miss: with no parameter or local, the field types.
const HIDES_FILE: &str = "src/main/java/com/x/svc/Hides.java";
const HIDES: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
import java.util.List;\n\
\n\
public class Hides {\n\
    private Mailer svc;\n\
    void param(Audit svc) { svc.send(); }\n\
    void local() { Audit svc = new Audit(); svc.send(); }\n\
    void field() { svc.send(); }\n\
    void untyped(List<Audit> all) { all.forEach(svc -> svc.send()); }\n\
    class Inner {\n\
        private Audit svc;\n\
        void inner() { svc.send(); }\n\
    }\n\
}\n";

#[test]
fn a_parameter_or_local_hides_a_field_and_a_nearer_class_shadows_an_outer_one() {
    let tmp = fixture();
    write(tmp.path(), HIDES_FILE, HIDES);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, HIDES_FILE),
        [
            row("field", "Mailer::send", RefForm::Path),
            row("inner", "Audit::send", RefForm::Path),
            row("local", "Audit::send", RefForm::Path),
            row("param", "Audit::send", RefForm::Path),
            // An untyped lambda parameter hides the field and proves nothing.
            row("untyped", "List::forEach", RefForm::Path),
            row("untyped", "send", RefForm::Method),
        ]
    );
}

#[test]
fn sync_equals_a_full_reindex_after_a_call_moves_out_of_its_block() {
    let tmp = closed_block_fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    write(tmp.path(), CLOSED_FILE, &CLOSED.replace("svc.send(); } }", "} svc.send(); }"));
    engine.pipeline().sync(&[CLOSED_FILE.into()]);
    assert_eq!(
        calls_from(rt, CLOSED_FILE),
        [row("m", "send", RefForm::Method), row("n", "send", RefForm::Method)]
    );
    let cold = TempDir::new().unwrap();
    for rel in [MAILER_FILE, LOGGER_FILE, SCOPED_BASE_FILE, CLOSED_FILE] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(&cold);
    assert_eq!(binding_facts(rt), binding_facts(cold_engine.runtime().unwrap()));
}

/// A local or record class's own field hides the parameter, local or record
/// component of its name declared outside it — Java's nearest declaration —
/// so the call is typed by the field (S-825 review: the outer binding used to
/// win, a wrong edge). The near miss: a local class with no field of the name
/// still reads the enclosing method's local.
const NEAREST_FILE: &str = "src/main/java/com/x/svc/Nearest.java";
const NEAREST: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public class Nearest {\n\
    void param(Mailer x) { class L { Audit x; void run() { x.send(); } } }\n\
    void local() { Mailer y = new Mailer(); class M { Audit y; void go() { y.send(); } } }\n\
    void captured() { Mailer z = new Mailer(); class N { void use() { z.send(); } } }\n\
}\n";
const NEAREST_RECORD_FILE: &str = "src/main/java/com/x/svc/Rec.java";
const NEAREST_RECORD: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public record Rec(Mailer x) {\n\
    class Inner { Audit x; void inner() { x.send(); } }\n\
    void outer() { x.send(); }\n\
}\n";

#[test]
fn a_nearer_classs_field_hides_a_binding_declared_outside_its_class() {
    let tmp = fixture();
    write(tmp.path(), NEAREST_FILE, NEAREST);
    write(tmp.path(), NEAREST_RECORD_FILE, NEAREST_RECORD);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, NEAREST_FILE),
        [
            row("go", "Audit::send", RefForm::Path),
            row("run", "Audit::send", RefForm::Path),
            row("use", "Mailer::send", RefForm::Path),
        ]
    );
    assert_eq!(
        calls_from(rt, NEAREST_RECORD_FILE),
        [row("inner", "Audit::send", RefForm::Path), row("outer", "Mailer::send", RefForm::Path)]
    );
}

/// A local is in scope from where it is written — its own and every later
/// declarator's initializer included — never before it: a case group's
/// local types no call in an earlier group, where the field of its name is
/// in scope (S-825 review: both used to be typed the wrong way round).
const LOCALS_FILE: &str = "src/main/java/com/x/svc/Locals.java";
const LOCALS: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public class Locals {\n\
    Mailer x;\n\
    Mailer a;\n\
    void early(int k) {\n\
        switch (k) {\n\
            case 1: x.self(); break;\n\
            default: Audit x = new Audit(); x.send();\n\
        }\n\
    }\n\
    void init() { Audit a = new Audit(), b = a.copy(); }\n\
    void loop() { for (Audit x = new Audit(); ; ) { x.send(); break; } x.self(); }\n\
}\n";

#[test]
fn a_local_is_in_scope_from_where_it_is_written_and_never_before() {
    let tmp = fixture();
    write(tmp.path(), LOCALS_FILE, LOCALS);
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), LOCALS_FILE),
        [
            row("early", "Audit::send", RefForm::Path),
            row("early", "Mailer::self", RefForm::Path),
            row("init", "Audit::copy", RefForm::Path),
            row("loop", "Audit::send", RefForm::Path),
            row("loop", "Mailer::self", RefForm::Path),
        ]
    );
}

/// A resource is in scope in the rest of its resource list and its `try`
/// block, never in the statement's `catch` or `finally`, where the field of
/// its name is (S-825 review: the resource used to type those calls).
const RESOURCE_FILE: &str = "src/main/java/com/x/svc/Res.java";
const RESOURCE: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public class Res {\n\
    Mailer x;\n\
    void fin() { try (Audit x = new Audit()) { } finally { x.self(); } }\n\
    void cat() { try (Audit x = new Audit()) { } catch (RuntimeException e) { x.self(); } }\n\
    void body() { try (Audit x = new Audit(); Audit y = x.copy()) { x.send(); } }\n\
}\n";

#[test]
fn a_resource_types_its_try_block_and_never_its_catch_or_finally() {
    let tmp = fixture();
    write(tmp.path(), RESOURCE_FILE, RESOURCE);
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), RESOURCE_FILE),
        [
            row("body", "Audit::copy", RefForm::Path),
            row("body", "Audit::send", RefForm::Path),
            row("cat", "Mailer::self", RefForm::Path),
            row("fin", "Mailer::self", RefForm::Path),
        ]
    );
}

/// An interface constant is in scope across its interface's body: a class
/// nested in the interface never reads the outer class's field of the
/// constant's name, which the constant hides — and a constant proves
/// nothing (S-825 review: the walk used to pass the interface and reach the
/// outer field). The near miss: outside the interface the field types.
const IFACE_FILE: &str = "src/main/java/com/x/svc/Iface.java";
const IFACE: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public class Iface {\n\
    Mailer X;\n\
    void outside() { X.send(); }\n\
    interface I { Audit X = null; class N { void inside() { X.send(); } } }\n\
}\n";

#[test]
fn an_interface_constant_hides_an_outer_field_in_its_nested_classes() {
    let tmp = fixture();
    write(tmp.path(), IFACE_FILE, IFACE);
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), IFACE_FILE),
        [row("inside", "send", RefForm::Method), row("outside", "Mailer::send", RefForm::Path)]
    );
}

/// An enum constant's body is an anonymous class: its field is in scope in
/// that body and hides the enum's own field of its name there, and the enum's
/// field is in scope in the enum's other members (S-825 review: the body's
/// field used to be bound to no class, so the enum's field typed its call).
const ENUM_FILE: &str = "src/main/java/com/x/svc/Kinds.java";
const ENUM: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public enum Kinds {\n\
    A { Audit s; void body() { s.send(); } };\n\
    Mailer s;\n\
    void own() { s.send(); }\n\
}\n";

#[test]
fn an_enum_constants_body_field_is_in_scope_in_that_body_only() {
    let tmp = fixture();
    write(tmp.path(), ENUM_FILE, ENUM);
    let engine = index(&tmp);
    assert_eq!(
        calls_from(engine.runtime().unwrap(), ENUM_FILE),
        [row("body", "Audit::send", RefForm::Path), row("own", "Mailer::send", RefForm::Path)]
    );
}

/// An enum constant is a variable of its enum: one named like the imported
/// type `Mailer` obscures it in the enum's body, so `Mailer.send()` there is
/// no static call on `Mailer` (S-825 review: it was typed `Mailer::send`).
const OBSCURE_FILE: &str = "src/main/java/com/x/svc/Obscure.java";
const OBSCURE: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
\n\
public enum Obscure {\n\
    Mailer;\n\
    void obscured() { Mailer.send(); }\n\
    void send() {}\n\
}\n";

#[test]
fn an_enum_constant_obscures_a_type_of_its_name() {
    let tmp = fixture();
    write(tmp.path(), OBSCURE_FILE, OBSCURE);
    let engine = index(&tmp);
    assert_eq!(calls_from(engine.runtime().unwrap(), OBSCURE_FILE), [row("obscured", "send", RefForm::Method)]);
}

/// A variable that proves nothing still obscures a type of its name: an
/// untyped lambda parameter or a pattern variable named `Clock` is never the
/// imported class `Clock` (S-825 review: no test guarded the static-type
/// reading's "declared nowhere in the file" condition once the obscuring
/// parameters above became typed). A C-style array (`Mailer f[]`) is no
/// `Mailer`, and a switch or record pattern variable hides the field of its
/// name in its own scope.
const OBSCURED_FILE: &str = "src/main/java/com/x/svc/Obs.java";
const OBSCURED: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
import com.x.util.Clock;\n\
import java.util.List;\n\
\n\
public class Obs {\n\
    private Mailer m;\n\
    private Mailer f[];\n\
    void lambda(List<Mailer> l) { l.forEach(Clock -> Clock.now()); }\n\
    void pattern(Object o) { if (o instanceof Mailer Clock) { Clock.now(); } }\n\
    void field() { f.send(); }\n\
    void param(Mailer p[]) { p.send(); }\n\
    void local() { Mailer l[] = null; l.send(); }\n\
    void rec(Object o) { if (o instanceof Box(Audit m)) { m.send(); } }\n\
    void sw(Object o) { switch (o) { case Audit m -> m.send(); default -> {} } }\n\
}\n";
const BOX_FILE: &str = "src/main/java/com/x/svc/Box.java";
const BOX: &str = "package com.x.svc;\n\npublic record Box(Audit m) {}\n";

#[test]
fn an_unproven_variable_array_or_pattern_types_no_call() {
    let tmp = fixture();
    write(tmp.path(), OBSCURED_FILE, OBSCURED);
    write(tmp.path(), BOX_FILE, BOX);
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, OBSCURED_FILE),
        [
            row("field", "send", RefForm::Method),
            row("lambda", "List::forEach", RefForm::Path),
            row("lambda", "now", RefForm::Method),
            row("local", "send", RefForm::Method),
            row("param", "send", RefForm::Method),
            row("pattern", "now", RefForm::Method),
            row("rec", "send", RefForm::Method),
            row("sw", "send", RefForm::Method),
        ]
    );
    let wrong: Vec<_> = call_edges(rt)
        .into_iter()
        .filter(|(s, t)| s.starts_with(OBSCURED_FILE) && (t.starts_with(CLOCK_FILE) || t.starts_with(MAILER_FILE)))
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// Every remaining binding form types the calls in its own scope and leaves
/// the field of its name to the calls outside it (S-825 review: a deleted
/// pattern for any of these passed every test). A constructor's local and an
/// anonymous class's field type their calls; after an enhanced `for`, a
/// `catch`, a nested block holding a pattern variable, or a switch rule's
/// pattern, the field types the call.
const FORMS_FILE: &str = "src/main/java/com/x/svc/Forms.java";
const FORMS: &str = "package com.x.svc;\n\
\n\
import com.x.mail.Mailer;\n\
import java.util.List;\n\
\n\
public class Forms {\n\
    private Mailer m;\n\
    private Mailer e;\n\
    Forms() { Mailer c = new Mailer(); c.send(); }\n\
    void anon() { new Runnable() { private Mailer a; public void run() { a.send(); } }; }\n\
    void loop(List<Audit> l) { for (Audit m : l) { } m.send(); }\n\
    void caught() { try { } catch (RuntimeException e) { } e.send(); }\n\
    void block(Object o) { { if (o instanceof Audit m) { } } m.send(); }\n\
    void rule(Object o) { switch (o) { case Audit m -> { } default -> { } } m.send(); }\n\
}\n";

#[test]
fn each_binding_form_types_its_own_scope_and_leaves_the_field_outside_it() {
    let tmp = fixture();
    write(tmp.path(), FORMS_FILE, FORMS);
    let engine = index(&tmp);
    let rows = calls_from(engine.runtime().unwrap(), FORMS_FILE);
    for source in ["Forms", "run", "loop", "caught", "block", "rule"] {
        assert!(rows.contains(&row(source, "Mailer::send", RefForm::Path)), "{source}: {rows:?}");
    }
    assert!(!rows.iter().any(|(_, t, _)| t.starts_with("Audit::")), "{rows:?}");
}
