//! A Java or Kotlin call reaches the body of an implemented interface's member
//! (S-609, [CR-202] Part 2, [FR-RS-48], [NFR-RA-05]) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! each language's SHIPPED queries and `plugin.toml` (no override).
//!
//! One fixture shape per language, one caller per case:
//!
//! - `m(1)`, `this.m(1)` and a proven `c.m(1)` reach the interface's `default`
//!   body (Kotlin: a bodied interface `fun`); before S-609 the walk stopped at
//!   the class's `Extends` chain and left them unbound;
//! - the class's own `check` takes five arguments and the interface's default
//!   takes three: a three-argument call passes the class's over and reaches the
//!   default — the pec-services `verifyUserRetailOrPix` shape;
//! - `sup(1)` binds the superclass's `sup`, never the default of that name:
//!   class before interface;
//! - `up(1)` reaches a default of the super-interface, one level further up,
//!   and `hidden(1)` reaches none: the interface re-declares it abstract;
//! - an abstract, a `static` and a `private` interface member are never bound
//!   through an implementing class, and a `private` one still binds from its
//!   own interface's default;
//! - two unrelated interfaces each supplying `both` bind nothing
//!   (`overload-ambiguous`), and a call no default admits binds nothing;
//! - a class whose superclass the graph does not hold reaches no default: the
//!   unseen superclass may declare the method, and a class's beats an
//!   interface's.
//!
//! Sync ≡ reindex holds when an interface gains or loses a default or a
//! super-interface and when a class gains or loses an `implements` clause (the
//! interface names join the hierarchy tokens). Java's `implements` rows are the
//! ones that newly add tokens; Kotlin's supertype list is captured as `Extends`
//! rows, whose targets were hierarchy tokens already.
//!
//! [CR-202]: ../../docs/requests/CR-202-one-rust-associated-item-lookup.md
//! [FR-RS-48]: ../../docs/specs/requirements/FR-RS-48.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `Container.name`, its `Contains` parent's name first.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        let nodes: HashMap<NodeId, String> = store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect();
        let parent: HashMap<NodeId, NodeId> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .map(|e| (e.target, e.source))
            .collect();
        Ok(nodes
            .iter()
            .map(|(id, name)| {
                let owner = parent.get(id).and_then(|p| nodes.get(p)).map_or("", String::as_str);
                (*id, format!("{owner}.{name}"))
            })
            .collect())
    })
    .expect("read runs")
}

/// The `Calls` edges leaving the callable labelled `caller`, as target labels,
/// sorted.
fn calls_from(rt: &Runtime, caller: &str) -> Vec<String> {
    let label = labels(rt);
    let mut out: Vec<String> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls && label[&e.source] == caller)
        .map(|e| label[&e.target].clone())
        .collect();
    out.sort();
    out
}

/// The `language` row's call residue: its non-zero reasons.
fn residue(engine: &Engine, language: &str) -> BTreeMap<R, u64> {
    let residue: CallResidue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == language)
        .unwrap_or_else(|| panic!("a {language} row"))
        .call_residue
        .unwrap_or_else(|| panic!("the {language} row states its call residue"));
    residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect()
}

/// The callers each case of a fixture is written in, and what each binds.
fn assert_cases(rt: &Runtime, cases: &[(&str, &[&str])]) {
    for (caller, want) in cases {
        let want: Vec<String> = want.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(calls_from(rt, caller), want, "{caller}");
    }
}

/// A fresh index of `tmp`'s current files, fingerprinted.
fn cold(files: &[&str], tmp: &TempDir) -> String {
    let fresh = TempDir::new().unwrap();
    for rel in files {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    graph_fingerprint(index(fresh.path()).runtime().unwrap())
}

// ── Java ──────────────────────────────────────────────────────────────────

const JAVA_I: &str = "src/main/java/com/x/I.java";
const JAVA_J: &str = "src/main/java/com/x/J.java";
const JAVA_K: &str = "src/main/java/com/x/K.java";
const JAVA_BASE: &str = "src/main/java/com/x/Base.java";
const JAVA_C: &str = "src/main/java/com/x/C.java";
const JAVA_USER: &str = "src/main/java/com/x/User.java";
const JAVA_OUT: &str = "src/main/java/com/x/Out.java";

const JAVA_FILES: [(&str, &str); 7] = [
    (
        JAVA_I,
        "package com.x;

public interface I extends J {
    default void m(int a) {}
    default void check(String a, String b, int c) {}
    default void sup(int a) {}
    default void both(int a) {}
    void abs(int a);
    void hidden(int a);
    static void stat(int a) {}
    private void priv(int a) {}
    default void viaPriv(int a) { this.priv(a); }
}
",
    ),
    (
        JAVA_J,
        "package com.x;

public interface J {
    default void up(int a) {}
    default void hidden(int a) {}
}
",
    ),
    (
        JAVA_K,
        "package com.x;

public interface K {
    default void both(int a) {}
}
",
    ),
    (
        JAVA_BASE,
        "package com.x;

public class Base {
    public void sup(int a) {}
}
",
    ),
    (
        JAVA_C,
        "package com.x;

public abstract class C extends Base implements I, K {
    void callBare() { m(1); }
    void callThis() { this.m(1); }
    void callCheck() { check(\"a\", \"b\", 1); }
    void callSup() { sup(1); }
    void callUp() { up(1); }
    void callAbs() { abs(1); }
    void callHidden() { hidden(1); }
    void callStat() { stat(1); }
    void callPriv() { priv(1); }
    void callBoth() { both(1); }
    void callNoArity() { m(1, 2); }

    private void check(String a, String b, int c, int d, String e) {}
}
",
    ),
    (
        JAVA_USER,
        "package com.x;

public class User {
    void callProven(C c) { c.m(1); }
}
",
    ),
    (
        JAVA_OUT,
        "package com.x;

import org.lib.External;

public abstract class Out extends External implements I {
    void callUnseen() { m(1); }
}
",
    ),
];

#[cfg(feature = "lang-java")]
#[test]
fn a_java_call_reaches_an_implemented_interfaces_default_body() {
    let tmp = tree(&JAVA_FILES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_cases(
        rt,
        &[
            ("C.callBare", &["I.m"]),
            ("C.callThis", &["I.m"]),
            ("User.callProven", &["I.m"]),
            ("C.callCheck", &["I.check"]),
            // Class before interface.
            ("C.callSup", &["Base.sup"]),
            // The super-interface's default, one level up.
            ("C.callUp", &["J.up"]),
            // Never inherited: abstract, `static`, `private`.
            ("C.callAbs", &[]),
            // `I` re-declares `J`'s default abstract: nothing is inherited.
            ("C.callHidden", &[]),
            ("C.callStat", &[]),
            ("C.callPriv", &[]),
            // Two unrelated defaults, and a call no default admits.
            ("C.callBoth", &[]),
            ("C.callNoArity", &[]),
            // The marker never hides a member from its own interface: a
            // default's `this.priv(a)` binds it, as the estate's
            // `this.buildUnauthorizedException(…)` does.
            ("I.viaPriv", &["I.priv"]),
            // The superclass leaves the graph: it may declare `m`.
            ("Out.callUnseen", &[]),
        ],
    );
    let reasons = residue(&engine, "java");
    assert_eq!(reasons.get(&R::OverloadAmbiguous), Some(&1), "`both`: {reasons:?}");
}

/// Sync ≡ reindex when an interface gains or loses a default, when it gains or
/// loses a super-interface, and when a class gains or loses an `implements`
/// clause — each edit moves calls written in a file the sync did not touch.
/// The super-interface edit is the one only the interface names in the
/// hierarchy tokens re-select: `C.callUp`'s row spells `up`, which `I.java`
/// never declares, and `I` is a supertype of `C` through `implements` alone.
#[cfg(feature = "lang-java")]
#[test]
fn sync_equals_a_full_reindex_when_a_java_default_or_implements_clause_changes() {
    let tmp = tree(&JAVA_FILES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let files: Vec<&str> = JAVA_FILES.iter().map(|(rel, _)| *rel).collect();
    let i = JAVA_FILES[0].1;
    let c = JAVA_FILES[4].1;
    let edits = [
        // The default loses its body, then gets it back.
        (JAVA_I, i.replace("default void m(int a) {}", "void m(int a);")),
        (JAVA_I, i.to_string()),
        // The interface drops its super-interface, then extends it again.
        (JAVA_I, i.replace(" extends J", "")),
        (JAVA_I, i.to_string()),
        // The class drops `implements`, then declares it again.
        (JAVA_C, c.replace(" implements I, K", "")),
        (JAVA_C, c.to_string()),
    ];
    for (file, text) in edits {
        write(tmp.path(), file, &text);
        engine.sync(&[file.into()]);
        assert_eq!(graph_fingerprint(rt), cold(&files, &tmp), "after writing {file}:\n{text}");
    }
    // The edits moved the calls, and moved them back.
    write(tmp.path(), JAVA_I, &i.replace("default void m(int a) {}", "void m(int a);"));
    engine.sync(&[JAVA_I.into()]);
    assert_eq!(calls_from(rt, "User.callProven"), Vec::<String>::new());
    write(tmp.path(), JAVA_I, i);
    engine.sync(&[JAVA_I.into()]);
    assert_eq!(calls_from(rt, "User.callProven"), ["I.m"]);
}

// ── Kotlin ────────────────────────────────────────────────────────────────

const KOTLIN_FILE: &str = "src/main/kotlin/com/x/C.kt";

const KOTLIN_SOURCE: &str = "package com.x

interface J {
    fun up(a: Int) {}
}

interface I : J {
    fun m(a: Int) {}
    fun check(a: String, b: String, c: Int) {}
    fun sup(a: Int) {}
    fun both(a: Int) {}
    fun abs(a: Int)
    private fun priv(a: Int) {}
    fun viaPriv(a: Int) { this.priv(a) }
}

interface K {
    fun both(a: Int) {}
}

open class Base {
    fun sup(a: Int) {}
}

abstract class C : Base(), I, K {
    fun callBare() { m(1) }
    fun callThis() { this.m(1) }
    fun callCheck() { check(\"a\", \"b\", 1) }
    fun callSup() { sup(1) }
    fun callUp() { up(1) }
    fun callAbs() { abs(1) }
    fun callPriv() { priv(1) }
    fun callBoth() { both(1) }
    fun callNoArity() { m(1, 2) }

    private fun check(a: String, b: String, c: Int, d: Int, e: String) {}
}

abstract class Out : External(), I {
    fun callUnseen() { m(1) }
}
";

#[cfg(feature = "lang-kotlin")]
#[test]
fn a_kotlin_call_reaches_an_implemented_interfaces_bodied_member() {
    let tmp = tree(&[(KOTLIN_FILE, KOTLIN_SOURCE)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_cases(
        rt,
        &[
            ("C.callBare", &["I.m"]),
            ("C.callThis", &["I.m"]),
            ("C.callCheck", &["I.check"]),
            ("C.callSup", &["Base.sup"]),
            ("C.callUp", &["J.up"]),
            ("C.callAbs", &[]),
            ("C.callPriv", &[]),
            ("C.callBoth", &[]),
            ("C.callNoArity", &[]),
            ("Out.callUnseen", &[]),
            ("I.viaPriv", &["I.priv"]),
        ],
    );
}
