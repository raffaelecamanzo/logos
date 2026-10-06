//! A type-qualified Java call binds among its type's members and in-repository
//! supertypes (S-468, CR-150 §3.2 B–C, FR-RS-10, UAT-RS-05, FR-RS-09,
//! NFR-RA-05, NFR-RA-06) — exercised end to end through the public [`Engine`]
//! façade against real temp-directory fixtures.
//!
//! S-467 records a call whose receiver the file proves as `T::m` (Path form).
//! Before S-468 the package rung bound such a row only to a callable `T`
//! declares itself; an inherited `m`, a `super.m()` into a class that inherits
//! it, or a typed receiver whose class inherits the method stayed unbound. Here
//! the binder walks `T`'s in-repository `Extends` chain (S-466), nearest first,
//! bounded and cycle-guarded, and binds on the first level holding exactly one
//! callable `m`.
//!
//! What stays unbound is stated by reason on the Java row of the per-language
//! readout ([FR-RS-09]): `no-receiver-evidence`, `external-type`,
//! `type-in-another-member`, `overload-ambiguous`, `type-ambiguous`,
//! `supertype-unreached` — one no-edge fixture each, every one asserted on a
//! **cold** index. On `sync`, a bound row whose target gains a same-named
//! sibling keeps its old edge where a cold index leaves it unbound — the
//! pre-existing commit-semantics gap (S-439), pinned here by the test named for
//! it rather than mistaken for a regression.
//!
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md

#![cfg(all(feature = "lang-java", feature = "lang-rust"))]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{discover, workspace_status, EngineRegistry, RegistryMode};
use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::models::{CallResidue, CallResidueReason as R, ResidueScope};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &Path) -> Engine {
    let engine = Engine::start(tmp).expect("engine starts");
    engine.index();
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

/// The `Calls` ledger rows sourced in `file`: `(source name, target, form,
/// resolved)`, sorted.
fn call_rows(rt: &Runtime, file: &str) -> Vec<(String, String, RefForm, bool)> {
    let needle = file.to_string();
    let mut rows: Vec<(String, String, RefForm, bool)> = rt
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
                    (*f == needle).then(|| (name.clone(), r.target, r.form, r.resolved))
                })
                .collect())
        })
        .expect("read runs");
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

fn edge(from_file: &str, from: &str, to_file: &str, to: &str) -> (String, String) {
    (format!("{from_file}:{from}"), format!("{to_file}:{to}"))
}

/// The `Calls` residue the Java row of `status` carries.
fn java_residue(engine: &Engine) -> CallResidue {
    engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .expect("a java row")
        .call_residue
        .expect("the java row states its call residue")
}

/// The reasons of `residue` whose count is not zero.
fn nonzero(residue: &CallResidue) -> BTreeMap<R, u64> {
    residue
        .reasons
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| (*k, *n))
        .collect()
}

fn reasons(pairs: &[(R, u64)]) -> BTreeMap<R, u64> {
    pairs.iter().copied().collect()
}

// ── UAT-RS-05: two classes each declaring `send()` ───────────────────────

const MAILER_FILE: &str = "src/main/java/com/x/mail/Mailer.java";
const PAGER_FILE: &str = "src/main/java/com/x/mail/Pager.java";
const CLIENT_FILE: &str = "src/main/java/com/x/app/Client.java";

const MAILER: &str = "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n}\n";
const PAGER: &str = "package com.x.mail;\n\npublic class Pager {\n    public void send() {}\n}\n";
const CLIENT: &str = "package com.x.app;\n\
\n\
import com.x.mail.Mailer;\n\
import com.x.mail.Pager;\n\
\n\
public class Client {\n\
    private Mailer mailer;\n\
    private Pager pager;\n\
    public void viaMailer() { mailer.send(); }\n\
    public void viaPager() { pager.send(); }\n\
    public void viaParam(Pager given) { given.send(); }\n\
    public void viaLocal() { Mailer local = new Mailer(); local.send(); }\n\
    public void viaThisField() { this.pager.send(); }\n\
    public void viaUndeclared() { x.send(); }\n\
}\n";

#[test]
fn uat_rs_05_each_typed_call_binds_its_own_classs_send_and_an_untyped_one_stays_unresolved() {
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let from_client: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(s, _)| s.starts_with(CLIENT_FILE))
        .collect();
    assert_eq!(
        from_client,
        [
            edge(CLIENT_FILE, "viaLocal", MAILER_FILE, "send"),
            edge(CLIENT_FILE, "viaMailer", MAILER_FILE, "send"),
            edge(CLIENT_FILE, "viaPager", PAGER_FILE, "send"),
            edge(CLIENT_FILE, "viaParam", PAGER_FILE, "send"),
            edge(CLIENT_FILE, "viaThisField", PAGER_FILE, "send"),
        ]
    );
    // The undeclared receiver keeps its bare Method-form row, unresolved.
    assert!(call_rows(rt, CLIENT_FILE).contains(&(
        "viaUndeclared".to_string(),
        "send".to_string(),
        RefForm::Method,
        false
    )));
    assert_eq!(nonzero(&java_residue(&engine)), reasons(&[(R::NoReceiverEvidence, 1)]));
}

// ── the in-repository supertype walk ──────────────────────────────────────

const BASE_FILE: &str = "src/main/java/com/x/base/Base.java";
const MID_FILE: &str = "src/main/java/com/x/base/Mid.java";
const LEAF_FILE: &str = "src/main/java/com/x/app/Leaf.java";
const USER_FILE: &str = "src/main/java/com/x/app/User.java";

const BASE: &str = "package com.x.base;\n\
\n\
public class Base {\n\
    public void start() {}\n\
    public void stop() {}\n\
}\n";
const MID: &str = "package com.x.base;\n\
\n\
public class Mid extends Base {\n\
    public void stop() {}\n\
}\n";
const LEAF: &str = "package com.x.app;\n\
\n\
import com.x.base.Mid;\n\
\n\
public class Leaf extends Mid {\n\
    public void viaSuper() { super.start(); }\n\
    public void viaInherited() { start(); }\n\
    public void viaThis() { this.start(); }\n\
    public void nearest() { stop(); }\n\
}\n";
const USER: &str = "package com.x.app;\n\
\n\
public class User {\n\
    private Leaf leaf;\n\
    public void run() { leaf.start(); }\n\
    public void halt() { leaf.stop(); }\n\
}\n";

fn hierarchy() -> TempDir {
    tree(&[(BASE_FILE, BASE), (MID_FILE, MID), (LEAF_FILE, LEAF), (USER_FILE, USER)])
}

#[test]
fn super_and_inherited_calls_bind_to_the_in_repo_superclass_method_nearest_first() {
    let tmp = hierarchy();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let edges = call_edges(rt);
    for expected in [
        // `super.start()` in `Leaf` is `Mid::start`: `Mid` inherits it from `Base`.
        edge(LEAF_FILE, "viaSuper", BASE_FILE, "start"),
        // An unqualified inherited call, two levels up.
        edge(LEAF_FILE, "viaInherited", BASE_FILE, "start"),
        edge(LEAF_FILE, "viaThis", BASE_FILE, "start"),
        // Nearest first: `Mid` overrides `stop`, so `Base.stop` is never reached.
        edge(LEAF_FILE, "nearest", MID_FILE, "stop"),
        // A typed receiver whose class inherits the method, from another file.
        edge(USER_FILE, "run", BASE_FILE, "start"),
        edge(USER_FILE, "halt", MID_FILE, "stop"),
    ] {
        assert!(edges.contains(&expected), "{expected:?} not in {edges:?}");
    }
    assert!(
        !edges.contains(&edge(LEAF_FILE, "nearest", BASE_FILE, "stop"))
            && !edges.contains(&edge(USER_FILE, "halt", BASE_FILE, "stop")),
        "an overridden method binds the nearest level only: {edges:?}"
    );
    assert_eq!(nonzero(&java_residue(&engine)), BTreeMap::new());
}

#[test]
fn a_cycle_in_extends_terminates_and_binds_nothing() {
    let tmp = tree(&[
        (
            "src/main/java/com/x/A.java",
            "package com.x;\n\npublic class A extends B {}\n",
        ),
        (
            "src/main/java/com/x/B.java",
            "package com.x;\n\npublic class B extends A {}\n",
        ),
        (
            "src/main/java/com/x/C.java",
            "package com.x;\n\npublic class C {\n    private A a;\n    public void m() { a.go(); }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        call_edges(rt).is_empty(),
        "a cyclic hierarchy declares no `go`: {:?}",
        call_edges(rt)
    );
    assert_eq!(nonzero(&java_residue(&engine)), reasons(&[(R::SupertypeUnreached, 1)]));
}

/// An interface's super-interfaces form one level: a name only one of them
/// declares binds to it, and a name both declare is ambiguous — never a pick
/// of the first ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn an_interfaces_super_interfaces_are_one_level_and_a_name_both_declare_is_ambiguous() {
    const USE_FILE: &str = "src/main/java/com/x/i/Use.java";
    let tmp = tree(&[
        (
            "src/main/java/com/x/i/A.java",
            "package com.x.i;\n\npublic interface A {\n    void a();\n    void same();\n}\n",
        ),
        (
            "src/main/java/com/x/i/B.java",
            "package com.x.i;\n\npublic interface B {\n    void b();\n    void same();\n}\n",
        ),
        (
            "src/main/java/com/x/i/C.java",
            "package com.x.i;\n\npublic interface C extends A, B {}\n",
        ),
        (
            USE_FILE,
            "package com.x.i;\n\
             \n\
             public class Use {\n\
                 private C c;\n\
                 public void ua() { c.a(); }\n\
                 public void ub() { c.b(); }\n\
                 public void us() { c.same(); }\n\
             }\n",
        ),
    ]);
    let engine = index(tmp.path());
    let from_use: Vec<(String, String)> = call_edges(engine.runtime().unwrap())
        .into_iter()
        .filter(|(s, _)| s.starts_with(USE_FILE))
        .collect();
    assert_eq!(
        from_use,
        [
            edge(USE_FILE, "ua", "src/main/java/com/x/i/A.java", "a"),
            edge(USE_FILE, "ub", "src/main/java/com/x/i/B.java", "b"),
        ]
    );
    assert_eq!(nonzero(&java_residue(&engine)), reasons(&[(R::OverloadAmbiguous, 1)]));
}

// ── one no-edge fixture per residue reason, each on a cold index ──────────

/// Index `files`, assert no `Calls` edge leaves `caller`, and return the Java
/// row's residue.
fn residue_of(files: &[(&str, &str)], caller: &str) -> CallResidue {
    let tmp = tree(files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let from_caller: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(s, _)| s.starts_with(caller))
        .collect();
    assert!(from_caller.is_empty(), "no edge may leave {caller}: {from_caller:?}");
    let residue = java_residue(&engine);
    assert_eq!(residue.scope, ResidueScope::Repository);
    assert_eq!(
        residue.unbound,
        residue.reasons.values().sum::<u64>() + residue.unclassified,
        "the reasons partition the unbound rows"
    );
    residue
}

const CALLER: &str = "src/main/java/com/x/app/Caller.java";

#[test]
fn an_unproven_receiver_is_no_receiver_evidence() {
    let residue = residue_of(
        &[(
            CALLER,
            "package com.x.app;\n\npublic class Caller {\n    public void m() { x.send(); x.make().done(); }\n}\n",
        )],
        CALLER,
    );
    // An undeclared receiver `x` (`send`, `make`) and a chained call (`done`).
    assert_eq!(nonzero(&residue), reasons(&[(R::NoReceiverEvidence, 3)]));
    assert_eq!(residue.unbound, 3);
}

#[test]
fn a_jdk_or_library_type_and_its_super_call_are_external_type() {
    let residue = residue_of(
        &[(
            CALLER,
            "package com.x.app;\n\
             \n\
             import java.util.List;\n\
             import org.springframework.Thing;\n\
             \n\
             public class Caller extends Thing {\n\
                 private List<String> items;\n\
                 public void m() { items.clear(); }\n\
                 public void s() { super.init(); }\n\
             }\n",
        )],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::ExternalType, 2)]));
}

#[test]
fn an_overloaded_target_on_the_type_or_on_a_supertype_is_overload_ambiguous() {
    let residue = residue_of(
        &[
            (
                "src/main/java/com/x/mail/Mailer.java",
                "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n    public void send(String to) {}\n}\n",
            ),
            (
                "src/main/java/com/x/mail/Base.java",
                "package com.x.mail;\n\npublic class Base {\n    public void go() {}\n    public void go(int n) {}\n}\n",
            ),
            (
                "src/main/java/com/x/mail/Kid.java",
                "package com.x.mail;\n\npublic class Kid extends Base {}\n",
            ),
            (
                CALLER,
                "package com.x.app;\n\
                 \n\
                 import com.x.mail.Kid;\n\
                 import com.x.mail.Mailer;\n\
                 \n\
                 public class Caller {\n\
                     private Mailer mailer;\n\
                     private Kid kid;\n\
                     public void m() { mailer.send(); }\n\
                     public void k() { kid.go(); }\n\
                 }\n",
            ),
        ],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::OverloadAmbiguous, 2)]));
}

#[test]
fn a_type_declared_twice_under_one_name_is_type_ambiguous() {
    let residue = residue_of(
        &[
            (
                "src/main/java/com/x/mail/Mailer.java",
                "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n}\n",
            ),
            (
                "src/test/java/com/x/mail/Mailer.java",
                "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n}\n",
            ),
            (
                CALLER,
                "package com.x.app;\n\nimport com.x.mail.Mailer;\n\npublic class Caller {\n    private Mailer mailer;\n    public void m() { mailer.send(); }\n}\n",
            ),
        ],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::TypeAmbiguous, 1)]));
}

#[test]
fn an_inherited_call_through_an_external_superclass_and_an_interface_without_the_method_are_supertype_unreached(
) {
    let residue = residue_of(
        &[
            (
                "src/main/java/com/x/app/Port.java",
                "package com.x.app;\n\npublic interface Port {\n    void open();\n}\n",
            ),
            (
                CALLER,
                "package com.x.app;\n\
                 \n\
                 import org.springframework.Thing;\n\
                 \n\
                 public class Caller extends Thing {\n\
                     private Port port;\n\
                     public void m() { init(); }\n\
                     public void p() { port.close(); }\n\
                 }\n",
            ),
        ],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::SupertypeUnreached, 2)]));
}

/// A bare call two static imports (or two static wildcards) each supply is
/// ambiguous between two same-named callables — `overload-ambiguous`, not
/// `no-receiver-evidence`: the call names imports, just two of them.
#[test]
fn a_bare_call_two_static_imports_supply_is_overload_ambiguous() {
    let clock = |pkg: &str| {
        format!("package com.x.{pkg};\n\npublic class Clock {{\n    public static long now() {{ return 0; }}\n}}\n")
    };
    let (a, b) = (clock("a"), clock("b"));
    let residue = residue_of(
        &[
            ("src/main/java/com/x/a/Clock.java", &a),
            ("src/main/java/com/x/b/Clock.java", &b),
            (
                CALLER,
                "package com.x.app;\n\
                 \n\
                 import static com.x.a.Clock.now;\n\
                 import static com.x.b.Clock.now;\n\
                 \n\
                 public class Caller {\n\
                     public void m() { now(); }\n\
                 }\n",
            ),
            (
                "src/main/java/com/x/app/Other.java",
                "package com.x.app;\n\
                 \n\
                 import static com.x.a.Clock.*;\n\
                 import static com.x.b.Clock.*;\n\
                 \n\
                 public class Other {\n\
                     public void m() { now(); }\n\
                 }\n",
            ),
        ],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::OverloadAmbiguous, 2)]));
}

/// A nested type its in-graph outer type does not declare — a generated
/// Lombok builder — is a type no file here declares: `external-type`.
#[test]
fn a_nested_type_its_in_graph_outer_type_does_not_declare_is_external_type() {
    let residue = residue_of(
        &[
            (
                "src/main/java/com/x/m/Outer.java",
                "package com.x.m;\n\npublic class Outer {\n    public void own() {}\n}\n",
            ),
            (
                CALLER,
                "package com.x.app;\n\nimport com.x.m.Outer.Builder;\n\npublic class Caller {\n    private Builder builder;\n    public void c() { builder.build(); }\n}\n",
            ),
        ],
        CALLER,
    );
    assert_eq!(nonzero(&residue), reasons(&[(R::ExternalType, 1)]));
}

/// `Outer` declared twice under one name (a `src/main` and a `src/test`
/// class), each with a nested `Mailer` and a static `now()`.
fn duplicated_outer() -> Vec<(&'static str, String)> {
    let body = "package com.x.o;\n\npublic class Outer {\n    public static class Mailer {\n        public void send() {}\n    }\n    public static long now() { return 0; }\n}\n";
    vec![
        ("src/main/java/com/x/o/Outer.java", body.to_string()),
        ("src/test/java/com/x/o/Outer.java", body.to_string()),
    ]
}

/// A wildcard over a type declared twice, which could supply the call's head
/// type or its bare name, is `type-ambiguous` — for a typed receiver and for a
/// statically imported bare call alike.
#[test]
fn a_wildcard_over_a_type_declared_twice_is_type_ambiguous() {
    let outer = duplicated_outer();
    let mut files: Vec<(&str, &str)> = outer.iter().map(|(p, t)| (*p, t.as_str())).collect();
    // Two files: the ledger keeps one `Glob` row per written target, so a
    // static and a non-static wildcard of one type in one file are one row.
    files.push((
        CALLER,
        "package com.x.app;\n\
         \n\
         import com.x.o.Outer.*;\n\
         \n\
         public class Caller {\n\
             private Mailer mailer;\n\
             public void typed() { mailer.send(); }\n\
         }\n",
    ));
    files.push((
        "src/main/java/com/x/app/Bare.java",
        "package com.x.app;\n\
         \n\
         import static com.x.o.Outer.*;\n\
         \n\
         public class Bare {\n\
             public void bare() { now(); }\n\
         }\n",
    ));
    let residue = residue_of(&files, CALLER);
    assert_eq!(nonzero(&residue), reasons(&[(R::TypeAmbiguous, 2)]));
}

/// A sub-lookup's miss is not the call's reason: the wildcard's own type is
/// ambiguous, but neither declaration supplies the head `Foo`, so the row reads
/// `external-type` — the wildcard's ambiguity, recorded for a non-call lookup,
/// must not become the call's `type-ambiguous`.
#[test]
fn a_wildcards_own_ambiguity_that_supplies_nothing_is_not_the_calls_reason() {
    let outer = duplicated_outer();
    let mut files: Vec<(&str, &str)> = outer.iter().map(|(p, t)| (*p, t.as_str())).collect();
    files.push((
        CALLER,
        "package com.x.app;\n\nimport com.x.o.Outer.*;\n\npublic class Caller {\n    private Foo foo;\n    public void c() { foo.bar(); }\n}\n",
    ));
    let residue = residue_of(&files, CALLER);
    assert_eq!(nonzero(&residue), reasons(&[(R::ExternalType, 1)]));
}

// ── type-in-another-member: only a workspace can tell it from external ────

fn git_init(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir member");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init {}", dir.display());
}

const SHARED_MAILER: &str = "package com.x.shared;\n\npublic class Mailer {\n    public void send() {}\n}\n";
const CONSUMER: &str = "package com.x.app;\n\
\n\
import com.x.shared.Mailer;\n\
\n\
public class Consumer {\n\
    private Mailer mailer;\n\
    public void m() { mailer.send(); }\n\
}\n";
const HEIR: &str = "package com.x.app;\n\
\n\
import com.x.shared.Mailer;\n\
\n\
public class Heir extends Mailer {\n\
    public void h() { send(); }\n\
}\n";

#[test]
fn a_type_another_member_declares_is_type_in_another_member_in_a_workspace_only() {
    let ws = TempDir::new().unwrap();
    let lib = ws.path().join("lib");
    let app = ws.path().join("app");
    git_init(&lib);
    git_init(&app);
    write(&lib, "src/main/java/com/x/shared/Mailer.java", SHARED_MAILER);
    write(&app, "src/main/java/com/x/app/Consumer.java", CONSUMER);
    write(&app, "src/main/java/com/x/app/Heir.java", HEIR);
    for member in [&lib, &app] {
        let engine = index(member);
        let _ = engine.sync(&[] as &[PathBuf]);
    }

    // Alone, `app` cannot tell another member's type from a library's.
    let alone = java_residue(&Engine::start(&app).expect("engine starts"));
    assert_eq!(alone.scope, ResidueScope::Repository);
    assert_eq!(
        nonzero(&alone),
        reasons(&[(R::ExternalType, 1), (R::SupertypeUnreached, 1)])
    );
    assert!(!alone.reasons.contains_key(&R::TypeInAnotherMember));

    write(
        ws.path(),
        "logos.workspace.toml",
        "[workspace]\nname = \"w\"\nmembers = [\"lib\", \"app\"]\n",
    );
    let federation = discover(ws.path()).expect("discovers").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let status = workspace_status(&registry);
    let app_row = status
        .members
        .iter()
        .find(|m| m.status.member == "app")
        .and_then(|m| m.status.result.as_ref())
        .expect("app is read");
    let residue = app_row
        .resolution_by_language
        .iter()
        .find(|row| row.language == "java")
        .and_then(|row| row.call_residue.clone())
        .expect("a java residue");
    assert_eq!(residue.scope, ResidueScope::Workspace);
    // The typed call names `lib`'s type; the inherited call's superclass is
    // `lib`'s too, so its walk leaves this member: still supertype-unreached.
    assert_eq!(
        nonzero(&residue),
        reasons(&[(R::SupertypeUnreached, 1), (R::TypeInAnotherMember, 1)])
    );
    assert_eq!(residue.unbound, 2);
    // `lib` itself calls nothing.
    let lib_row = status
        .members
        .iter()
        .find(|m| m.status.member == "lib")
        .and_then(|m| m.status.result.as_ref())
        .expect("lib is read");
    let lib_residue = lib_row
        .resolution_by_language
        .iter()
        .find(|row| row.language == "java")
        .and_then(|row| row.call_residue.clone())
        .expect("a java residue");
    assert_eq!(lib_residue.unbound, 0);
}

/// A simple type name in a named package never names a default-package type
/// (JLS §7.5), so another member's default-package `Mailer` is not the type a
/// `com.x.app` file wrote: the row stays `external-type` in the workspace too.
#[test]
fn another_members_default_package_type_is_not_a_named_package_files_type() {
    let ws = TempDir::new().unwrap();
    let lib = ws.path().join("lib");
    let app = ws.path().join("app");
    git_init(&lib);
    git_init(&app);
    write(
        &lib,
        "src/main/java/Mailer.java",
        "public class Mailer {\n    public void send() {}\n}\n",
    );
    write(
        &app,
        "src/main/java/com/x/app/Consumer.java",
        "package com.x.app;\n\npublic class Consumer {\n    private Mailer mailer;\n    public void m() { mailer.send(); }\n}\n",
    );
    for member in [&lib, &app] {
        let _ = index(member).sync(&[] as &[PathBuf]);
    }
    let residue = workspace_java_residue(ws.path(), "app");
    assert_eq!(residue.scope, ResidueScope::Workspace);
    assert_eq!(nonzero(&residue), reasons(&[(R::ExternalType, 1)]));
}

/// The `app` member's Java residue in `workspace status` over `root`, whose
/// manifest this writes for the members `lib` and `app`.
fn workspace_java_residue(root: &Path, member: &str) -> CallResidue {
    write(
        root,
        "logos.workspace.toml",
        "[workspace]\nname = \"w\"\nmembers = [\"lib\", \"app\"]\n",
    );
    let federation = discover(root).expect("discovers").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    workspace_status(&registry)
        .members
        .iter()
        .find(|m| m.status.member == member)
        .and_then(|m| m.status.result.as_ref())
        .and_then(|info| {
            info.resolution_by_language
                .iter()
                .find(|row| row.language == "java")
                .and_then(|row| row.call_residue.clone())
        })
        .expect("a java residue")
}

/// One two-member workspace per candidate shape: `lib` declares the types,
/// `app` holds `app_files` and calls them. Returns `app`'s Java residue read
/// alone, then in `workspace status`.
fn split_probe(app_files: &[(&str, &str)]) -> (CallResidue, CallResidue) {
    let ws = TempDir::new().unwrap();
    let lib = ws.path().join("lib");
    let app = ws.path().join("app");
    git_init(&lib);
    git_init(&app);
    write(
        &lib,
        "src/main/java/com/x/shared/Outer.java",
        "package com.x.shared;\n\npublic class Outer {\n    public static class Inner {\n        public void ping() {}\n    }\n}\n",
    );
    write(
        &lib,
        "src/main/java/com/x/shared/Helper.java",
        "package com.x.shared;\n\npublic class Helper {\n    public void go() {}\n}\n",
    );
    // A split package: `lib` declares a type in `app`'s package.
    write(
        &lib,
        "src/main/java/com/x/app/Util.java",
        "package com.x.app;\n\npublic class Util {\n    public void run() {}\n}\n",
    );
    for (rel, text) in app_files {
        write(&app, rel, text);
    }
    for member in [&lib, &app] {
        let _ = index(member).sync(&[] as &[PathBuf]);
    }
    let alone = java_residue(&Engine::start(&app).expect("engine starts"));
    (alone, workspace_java_residue(ws.path(), "app"))
}

/// A nested type another member declares matches through its outer type's
/// name, and only the rows whose type is another member's move: a JDK call in
/// the same member stays `external-type` (a partial move, not a reset).
#[test]
fn a_nested_type_of_another_member_moves_and_a_jdk_type_stays_external() {
    let (alone, ws) = split_probe(&[(
        "src/main/java/com/x/app/Nested.java",
        "package com.x.app;\n\
         \n\
         import com.x.shared.Outer.Inner;\n\
         import java.util.List;\n\
         \n\
         public class Nested {\n\
             private Inner inner;\n\
             private List<String> items;\n\
             public void n() { inner.ping(); }\n\
             public void j() { items.clear(); }\n\
         }\n",
    )]);
    assert_eq!(nonzero(&alone), reasons(&[(R::ExternalType, 2)]));
    assert_eq!(
        nonzero(&ws),
        reasons(&[(R::ExternalType, 1), (R::TypeInAnotherMember, 1)])
    );
}

/// The same-package and wildcard candidates each reach another member's type.
#[test]
fn a_same_package_or_wildcard_type_of_another_member_moves() {
    let (alone, ws) = split_probe(&[
        (
            "src/main/java/com/x/app/SamePkg.java",
            "package com.x.app;\n\npublic class SamePkg {\n    private Util util;\n    public void s() { util.run(); }\n}\n",
        ),
        (
            "src/main/java/com/x/app/Wild.java",
            "package com.x.app;\n\nimport com.x.shared.*;\n\npublic class Wild {\n    private Helper helper;\n    public void w() { helper.go(); }\n}\n",
        ),
    ]);
    assert_eq!(nonzero(&alone), reasons(&[(R::ExternalType, 2)]));
    assert_eq!(nonzero(&ws), reasons(&[(R::TypeInAnotherMember, 2)]));
}

// ── the readout: Java only, and deterministic ─────────────────────────────

#[test]
fn only_a_package_shaped_language_row_carries_a_residue_and_it_is_deterministic() {
    let mut files: Vec<(&str, &str)> = vec![
        (BASE_FILE, BASE),
        (MID_FILE, MID),
        (LEAF_FILE, LEAF),
        (USER_FILE, USER),
        (MAILER_FILE, MAILER),
        (PAGER_FILE, PAGER),
        (CLIENT_FILE, CLIENT),
    ];
    files.push(("src/lib.rs", "pub fn f() { g(); }\npub fn g() { x.h(); }\n"));
    let first = tree(&files);
    let second = tree(&files);
    let a = index(first.path());
    let b = index(second.path());

    let rows = a.status().resolution_by_language;
    let rust = rows.iter().find(|r| r.language == "rust").expect("a rust row");
    assert!(rust.call_residue.is_none(), "Rust states no Java residue");
    assert_eq!(java_residue(&a), java_residue(&b));
    assert_eq!(binding_facts(a.runtime().unwrap()), binding_facts(b.runtime().unwrap()));
    // The residue's denominator is the ledger's own unbound count.
    let java = rows.iter().find(|r| r.language == "java").expect("a java row");
    assert_eq!(java_residue(&a).unbound, java.calls.references - java.calls.bound);
}

/// The residue is an additive readout: when it cannot be decided — here the
/// configuration naming the binding policy is unreadable — `status` still
/// answers with its counts, states no residue, and says why on its warnings
/// (ADR-14), rather than one decided under a policy the pass does not use.
#[test]
fn an_unreadable_config_states_no_residue_and_says_why() {
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    write(tmp.path(), ".logos/config.toml", "this is [not toml\n");
    let status = engine.status();
    assert!(status.indexed, "the status around the residue still answers");
    let java = status
        .resolution_by_language
        .iter()
        .find(|row| row.language == "java")
        .expect("a java row");
    assert!(java.calls.references > 0, "the counts are still stated");
    assert!(java.call_residue.is_none(), "no residue under an unknown policy");
    assert!(
        status
            .warnings
            .iter()
            .any(|w| w.contains("call residue is not stated") && w.contains("configuration")),
        "{:?}",
        status.warnings
    );
}

/// The residue reaches the serialised status — what the CLI, MCP and HTTP
/// surfaces print — with its reasons as their kebab-case tokens, and its two
/// internal halves never do. A `#[serde(skip)]` on the field would pass every
/// struct-level assertion above while dropping the readout from all three.
#[test]
fn the_residue_is_on_the_serialised_status_and_its_internals_are_not() {
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    let json = serde_json::to_value(engine.status()).expect("serialises");
    let java = json["resolution_by_language"]
        .as_array()
        .expect("an array of rows")
        .iter()
        .find(|row| row["language"] == "java")
        .expect("a java row")
        .clone();
    let residue = &java["call_residue"];
    assert_eq!(residue["unbound"], 1, "{java:#}");
    assert_eq!(residue["unclassified"], 0);
    assert_eq!(residue["scope"], "repository");
    assert_eq!(
        residue["reasons"],
        serde_json::json!({
            "external-type": 0,
            "no-receiver-evidence": 1,
            "overload-ambiguous": 0,
            "supertype-unreached": 0,
            "type-ambiguous": 0,
        })
    );
    assert!(
        residue.get("declared_types").is_none() && residue.get("external_candidates").is_none(),
        "{residue:#}"
    );
}

// ── sync ≡ reindex ─────────────────────────────────────────────────────────

/// Every edge and every ledger row, capture-before-delete rows included, by
/// symbol — the store's whole binding state (CR-187).
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

/// The binding state of a cold index over the Java files `tmp` holds now.
fn cold_facts(tmp: &TempDir, files: &[&str]) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in files {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(cold.path());
    binding_facts(engine.runtime().unwrap())
}

const HIERARCHY_FILES: [&str; 4] = [BASE_FILE, MID_FILE, LEAF_FILE, USER_FILE];

#[test]
fn sync_equals_a_full_reindex_after_a_supertype_gains_the_method() {
    let tmp = hierarchy();
    write(
        tmp.path(),
        BASE_FILE,
        "package com.x.base;\n\npublic class Base {\n    public void stop() {}\n}\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        !call_edges(rt).contains(&edge(LEAF_FILE, "viaInherited", BASE_FILE, "start")),
        "precondition: `start` is declared nowhere yet"
    );
    write(tmp.path(), BASE_FILE, BASE);
    engine.sync(&[BASE_FILE.into()]);
    assert!(call_edges(rt).contains(&edge(LEAF_FILE, "viaInherited", BASE_FILE, "start")));
    assert!(call_edges(rt).contains(&edge(USER_FILE, "run", BASE_FILE, "start")));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &HIERARCHY_FILES));
}

#[test]
fn sync_equals_a_full_reindex_after_a_mid_chain_type_gains_its_superclass() {
    // `Mid` first extends nothing: `Leaf`'s inherited `start()` is unreachable.
    // Giving `Mid` its superclass changes no name `Leaf` or `User` writes — the
    // hierarchy itself moved, and that alone must re-select their rows.
    let tmp = hierarchy();
    write(
        tmp.path(),
        MID_FILE,
        "package com.x.base;\n\npublic class Mid {\n    public void stop() {}\n}\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        !call_edges(rt).contains(&edge(USER_FILE, "run", BASE_FILE, "start")),
        "precondition: the chain stops at `Mid`"
    );
    write(tmp.path(), MID_FILE, MID);
    engine.sync(&[MID_FILE.into()]);
    for expected in [
        edge(LEAF_FILE, "viaInherited", BASE_FILE, "start"),
        edge(LEAF_FILE, "viaSuper", BASE_FILE, "start"),
        edge(USER_FILE, "run", BASE_FILE, "start"),
    ] {
        assert!(call_edges(rt).contains(&expected), "{expected:?} after sync");
    }
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &HIERARCHY_FILES));
}

#[test]
fn sync_equals_a_full_reindex_after_a_supertype_loses_the_method() {
    let tmp = hierarchy();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(call_edges(rt).contains(&edge(USER_FILE, "run", BASE_FILE, "start")));
    write(
        tmp.path(),
        BASE_FILE,
        "package com.x.base;\n\npublic class Base {\n    public void stop() {}\n}\n",
    );
    engine.sync(&[BASE_FILE.into()]);
    assert!(!call_edges(rt).iter().any(|(_, t)| t.ends_with(":start")));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &HIERARCHY_FILES));
}

/// A bound row whose target type gains a same-named overload is re-selected on
/// `sync` and re-binds to nothing, and the edge it bound before goes with it
/// (S-596, FR-SY-12): the synced store equals a cold index over the same files,
/// which leaves the call unbound as `overload-ambiguous`. Until S-596 the
/// commit only flipped the row's `resolved` flag and kept the edge.
#[test]
fn sync_equals_a_full_reindex_when_a_call_target_gains_an_overload() {
    let files = [MAILER_FILE, PAGER_FILE, CLIENT_FILE];
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let bound = edge(CLIENT_FILE, "viaMailer", MAILER_FILE, "send");
    assert!(call_edges(rt).contains(&bound));

    write(
        tmp.path(),
        MAILER_FILE,
        "package com.x.mail;\n\npublic class Mailer {\n    public void send() {}\n    public void send(String to) {}\n}\n",
    );
    engine.sync(&[MAILER_FILE.into()]);

    let cold = TempDir::new().unwrap();
    for rel in files {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(cold.path());
    let cold_edges = call_edges(cold_engine.runtime().unwrap());
    assert!(
        !cold_edges.iter().any(|(s, _)| *s == bound.0),
        "a cold index leaves the overloaded call unbound: {cold_edges:?}"
    );
    assert_eq!(
        java_residue(&cold_engine).reasons[&R::OverloadAmbiguous],
        2,
        "viaMailer and viaLocal both name the overloaded `send`"
    );
    let synced = call_edges(rt);
    assert!(!synced.iter().any(|(s, _)| *s == bound.0), "{synced:?}");
    assert!(call_rows(rt, CLIENT_FILE).contains(&(
        "viaMailer".to_string(),
        "Mailer::send".to_string(),
        RefForm::Path,
        false
    )));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));
}

/// A capture-before-delete `Symbol` row ([ADR-10]) is no call site — it waits for
/// its exact target symbol to return, and its source file's own row already
/// records the call. It is no population of the residue (S-598, CR-195): the
/// reasons partition a denominator that does not hold it, so a one-file sync
/// reads the residue a cold index of the same tree reads.
///
/// Since CR-187 a capture is deleted once it is spent, so a rename that its
/// callers' own re-bound rows answer leaves none, and the residue equals a cold
/// index's. The one a sync keeps — its source not re-bound, nothing else
/// carrying its edge — is planted here, as such a sync leaves it.
///
/// [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md
#[test]
fn a_capture_before_delete_row_awaiting_its_target_is_not_in_the_residue() {
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    // `Mailer.send` becomes `Mailer.post`: the two edges into `send` are
    // captured as `Symbol` rows before the node goes, and spent with their
    // callers' rows.
    let renamed = "package com.x.mail;\n\npublic class Mailer {\n    public void post() {}\n}\n";
    write(tmp.path(), MAILER_FILE, renamed);
    engine.sync(&[MAILER_FILE.into()]);
    let cold = tree(&[(MAILER_FILE, renamed), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    assert_eq!(java_residue(&engine), java_residue(&index(cold.path())));

    // A capture from a live caller, awaiting a target nothing else names,
    // stays, unbound.
    let rt = engine.runtime().unwrap();
    let caller = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .find(|n| n.name == "viaMailer")
                .map(|n| n.symbol.as_str().to_string())
                .expect("the viaMailer node"))
        })
        .unwrap();
    rt.submit_write(move |w| {
        w.insert_unresolved_ref(&logos_core::graph_store::NewUnresolvedRef {
            file_id: w.file_id(MAILER_FILE)?,
            source_symbol: &caller,
            target: "planted vanished target",
            alias: None,
            form: RefForm::Symbol,
            kind: EdgeKind::Calls,
            line: None,
            payload: None,
            receiver: None,
            peeled: None,
        })
    })
    .expect("plant the awaiting capture");
    engine.sync(&[]);

    // The fixture now holds exactly that one awaiting capture: the two
    // spent ones were deleted (CR-187).
    let awaiting = engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .iter()
                .filter(|r| r.form == RefForm::Symbol && !r.resolved)
                .count())
        })
        .unwrap();
    assert_eq!(awaiting, 1, "the fixture really holds one unbound capture row");

    let residue = java_residue(&engine);
    // It is no population of the residue (S-598, CR-195).
    assert_eq!(residue.unclassified, 0, "{residue:?}");
    assert_eq!(
        nonzero(&residue),
        reasons(&[(R::NoReceiverEvidence, 1), (R::SupertypeUnreached, 2)])
    );
    assert_eq!(residue.unbound, residue.reasons.values().sum::<u64>() + residue.unclassified);
    let java = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .expect("a java row");
    assert_eq!(residue.unbound, java.calls.references - java.calls.bound);

    // The sync-versus-reindex acceptance: the whole Java row, residue included.
    let cold = TempDir::new().unwrap();
    for (rel, text) in [(MAILER_FILE, renamed), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)] {
        write(cold.path(), rel, text);
    }
    let cold_engine = index(cold.path());
    let java_row = |e: &Engine| {
        e.status()
            .resolution_by_language
            .into_iter()
            .find(|row| row.language == "java")
            .expect("a java row")
    };
    assert_eq!(java_row(&engine), java_row(&cold_engine));
}

/// A one-file sync of a file with inbound calls whose target survives: every
/// inbound edge is captured and rebound by symbol, and the residue — with the
/// whole Java row — reads what a cold index of the same tree reads.
#[test]
fn a_synced_file_with_inbound_calls_reports_the_residue_a_reindex_reports() {
    let tmp = tree(&[(MAILER_FILE, MAILER), (PAGER_FILE, PAGER), (CLIENT_FILE, CLIENT)]);
    let engine = index(tmp.path());
    let before = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .expect("a java row");
    // Touch `Mailer.java` so it is re-extracted; `send` keeps its symbol.
    write(tmp.path(), MAILER_FILE, &format!("// touched\n{MAILER}"));
    let result = engine.sync(&[MAILER_FILE.into()]);
    assert_eq!(result.files_modified, 1);

    let after = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "java")
        .expect("a java row");
    assert_eq!(before, after, "a sync that changes no reference moves no figure");
    assert!(after.call_residue.is_some());
}

/// A sync that moves the hierarchy re-selects every package-shaped call
/// (`is_affected` rule 5) and re-binds it. A mid-chain `Mid` gaining an
/// override of `start()` moves each inherited call to the nearest `Mid.start`,
/// and the edge into `Base.start` it bound before is retracted (S-596,
/// FR-SY-12) — a cold index has only `Mid.start`. Until S-596 the stale edge
/// stayed beside the new one.
#[test]
fn sync_equals_a_full_reindex_when_a_mid_chain_type_gains_an_override() {
    let tmp = hierarchy();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let old = edge(LEAF_FILE, "viaInherited", BASE_FILE, "start");
    assert!(call_edges(rt).contains(&old));
    write(
        tmp.path(),
        MID_FILE,
        "package com.x.base;\n\npublic class Mid extends Base {\n    public void start() {}\n    public void stop() {}\n}\n",
    );
    engine.sync(&[MID_FILE.into()]);
    let new = edge(LEAF_FILE, "viaInherited", MID_FILE, "start");
    let synced = call_edges(rt);
    assert!(synced.contains(&new) && !synced.contains(&old), "{synced:?}");
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &HIERARCHY_FILES));
}

/// A mid-chain type **drops** its superclass: the inherited calls are
/// re-selected and re-bind to nothing (`resolved` flips to false), and their
/// edges into `Base.start` are retracted (S-596, FR-SY-12) — a cold index has
/// none.
#[test]
fn sync_equals_a_full_reindex_when_a_mid_chain_type_drops_its_superclass() {
    let tmp = hierarchy();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let stale = edge(USER_FILE, "run", BASE_FILE, "start");
    assert!(call_edges(rt).contains(&stale));
    write(
        tmp.path(),
        MID_FILE,
        "package com.x.base;\n\npublic class Mid {\n    public void stop() {}\n}\n",
    );
    engine.sync(&[MID_FILE.into()]);
    assert!(
        call_rows(rt, USER_FILE).contains(&(
            "run".to_string(),
            "Leaf::start".to_string(),
            RefForm::Path,
            false
        )),
        "the row is re-selected and unbinds"
    );
    assert!(!call_edges(rt).contains(&stale));
    let cold = cold_facts(&tmp, &HIERARCHY_FILES);
    assert!(
        !cold.0.iter().any(|(_, t, k)| k == "calls" && t.contains("Base#start")),
        "a cold index binds nothing into `Base.start`: {:?}",
        cold.0
    );
    assert_eq!(binding_facts(rt), cold);
}

/// A sync of a supertype's file captures each inbound `Extends` edge as an
/// exact-symbol row filed under **that** file (capture-before-delete, ADR-10),
/// and the row outlives the subtype's own `extends` clause until the
/// supertype's file is re-extracted again. The walk's hierarchy is read from
/// the subtype's own `Path` rows only, so such a leftover never lends a type a
/// supertype it no longer declares: a call written afterwards binds exactly as
/// on a cold index (sprint-81 review).
#[test]
fn a_captured_extends_row_never_lends_the_walk_a_superclass_the_subtype_dropped() {
    const BASE: &str = "src/main/java/com/x/base/Base.java";
    const LEAF: &str = "src/main/java/com/x/app/Leaf.java";
    const USER: &str = "src/main/java/com/x/app/User.java";
    const BASE_EDITED: &str =
        "package com.x.base;\n\npublic class Base {\n    public void start() {}\n    public void stop() {}\n}\n";
    const LEAF_BARE: &str = "package com.x.app;\n\npublic class Leaf {}\n";
    const USER_HALT: &str =
        "package com.x.app;\n\npublic class User {\n    private Leaf leaf;\n    public void halt() { leaf.stop(); }\n}\n";
    let tmp = tree(&[
        (BASE, "package com.x.base;\n\npublic class Base {\n    public void start() {}\n}\n"),
        (LEAF, "package com.x.app;\n\nimport com.x.base.Base;\n\npublic class Leaf extends Base {}\n"),
        (
            USER,
            "package com.x.app;\n\npublic class User {\n    private Leaf leaf;\n    public void run() { leaf.start(); }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Editing Base.java captures `Leaf —Extends→ Base` under Base.java.
    write(tmp.path(), BASE, BASE_EDITED);
    engine.sync(&[BASE.into()]);
    write(tmp.path(), LEAF, LEAF_BARE);
    engine.sync(&[LEAF.into()]);
    write(tmp.path(), USER, USER_HALT);
    engine.sync(&[USER.into()]);
    let synced = call_edges(rt);
    assert!(
        !synced.contains(&edge(USER, "halt", BASE, "stop")),
        "a Leaf that extends nothing lent `stop` from Base: {synced:?}"
    );
    let cold = tree(&[(BASE, BASE_EDITED), (LEAF, LEAF_BARE), (USER, USER_HALT)]);
    assert_eq!(synced, call_edges(index(cold.path()).runtime().unwrap()));
}

/// A supertype **between** the caller's type and the declaring one is
/// deleted. The row names neither deleted file's type, and only the hierarchy
/// tokens (every `Extends` target, bound or not) re-select it; it re-binds to
/// nothing, and its edge into the untouched `Root.start` is retracted (S-596,
/// FR-SY-12).
#[test]
fn sync_equals_a_full_reindex_when_a_supertype_between_is_deleted() {
    const ROOT: &str = "src/main/java/com/x/h/Root.java";
    const BASE: &str = "src/main/java/com/x/h/Base.java";
    const MID: &str = "src/main/java/com/x/h/Mid.java";
    const USER: &str = "src/main/java/com/x/h/User.java";
    let tmp = tree(&[
        (ROOT, "package com.x.h;\n\npublic class Root {\n    public void start() {}\n}\n"),
        (BASE, "package com.x.h;\n\npublic class Base extends Root {}\n"),
        (MID, "package com.x.h;\n\npublic class Mid extends Base {}\n"),
        (
            USER,
            "package com.x.h;\n\npublic class User {\n    private Mid mid;\n    public void run() { mid.start(); }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let stale = edge(USER, "run", ROOT, "start");
    assert!(call_edges(rt).contains(&stale));
    fs::remove_file(tmp.path().join(BASE)).unwrap();
    engine.sync(&[BASE.into()]);
    assert!(
        call_rows(rt, USER).contains(&(
            "run".to_string(),
            "Mid::start".to_string(),
            RefForm::Path,
            false
        )),
        "the hierarchy tokens re-select the row, and it unbinds"
    );
    assert!(!call_edges(rt).contains(&stale));
    let cold = cold_facts(&tmp, &[ROOT, MID, USER]);
    assert!(!cold.0.iter().any(|(_, _, k)| k == "calls"), "{:?}", cold.0);
    assert_eq!(binding_facts(rt), cold);
}

/// **Known limitation, pinned — deferred for a decision (sprint-81 review).**
/// A member type a class inherits from an in-repository superclass does not
/// shadow a same-package type of that name (JLS §8.5, §6.4.1): the lexical rung
/// reads the enclosing classes' own member types only. S-466 recorded this for
/// type relations; receiver typing carries it into `Calls`, so `e.go()` on a
/// field of the inherited `Base.Entry` binds the same-package `Entry.go`.
/// Reading inherited member types needs the walk over `Index::supertypes` in
/// the lexical rung, a sync-selection rule for rows whose meaning depends on a
/// supertype's members, and a decision on a supertype outside the repository
/// (refusing there, as the constant fold does, gives up every simple type name
/// in a class that extends a library type). When it is fixed this fails and
/// states the new rule.
#[test]
fn known_limitation_an_inherited_member_type_does_not_shadow_a_same_package_type() {
    const BASE: &str = "src/main/java/com/x/a/Base.java";
    const ENTRY: &str = "src/main/java/com/x/b/Entry.java";
    const SVC: &str = "src/main/java/com/x/b/Svc.java";
    let tmp = tree(&[
        (
            BASE,
            "package com.x.a;\n\npublic class Base {\n    public static class Entry {\n        public void go() {}\n    }\n}\n",
        ),
        (ENTRY, "package com.x.b;\n\npublic class Entry {\n    public void go() {}\n}\n"),
        (
            SVC,
            "package com.x.b;\n\nimport com.x.a.Base;\n\npublic class Svc extends Base {\n    private Entry e;\n    public void f() { e.go(); }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(
        call_edges(engine.runtime().unwrap()),
        [edge(SVC, "f", ENTRY, "go")],
        "the limitation is closed — expect `Base.Entry.go`, or no edge"
    );
}

/// **Known limitation, pinned — deferred for a decision (S-468 review).** The
/// walk decides on the nearest level holding exactly one callable of the name,
/// as CR-150 §3.2 B specifies, and it reads no signature or visibility. So an
/// overload split across levels binds the nearer one whatever the arity —
/// `leaf.send("to")` binds `Mid.send(int)` though Java calls the inherited
/// `Base.send(String)` — and a `private` method of a supertype, which is not
/// inherited, is still a candidate. Arity selection is out of CR-150's scope
/// (§3.3) and the graph records no visibility beyond `exported`. When either is
/// narrowed this fails and states the new rule.
#[test]
fn known_limitation_a_cross_level_overload_or_a_private_supertype_method_binds_the_nearer_level() {
    for modifier in ["public", "private"] {
        let tmp = tree(&[
            (
                "src/main/java/com/x/o/Base.java",
                "package com.x.o;\n\npublic class Base {\n    public void send(String to) {}\n}\n",
            ),
            (
                "src/main/java/com/x/o/Mid.java",
                &format!("package com.x.o;\n\npublic class Mid extends Base {{\n    {modifier} void send(int n) {{}}\n}}\n"),
            ),
            (
                "src/main/java/com/x/o/Leaf.java",
                "package com.x.o;\n\npublic class Leaf extends Mid {}\n",
            ),
            (
                "src/main/java/com/x/o/Caller.java",
                "package com.x.o;\n\npublic class Caller {\n    private Leaf leaf;\n    public void m() { leaf.send(\"to\"); }\n}\n",
            ),
        ]);
        let engine = index(tmp.path());
        assert_eq!(
            call_edges(engine.runtime().unwrap()),
            [edge(
                "src/main/java/com/x/o/Caller.java",
                "m",
                "src/main/java/com/x/o/Mid.java",
                "send"
            )],
            "{modifier}: the nearer level decides"
        );
    }
}
