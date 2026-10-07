//! A call binds only a callable whose arity admits it (S-592, CR-190,
//! FR-RS-43, NFR-RA-05) — exercised end to end through the public [`Engine`]
//! façade against temp-directory fixtures, with each language's SHIPPED
//! queries and `plugin.toml` (no override).
//!
//! Every language is checked on one fixture shape, one caller making each call:
//!
//! - `inh` — the caller's own class declares an `inh` the call's argument count
//!   cannot fit, and its base declares one that it can: the call binds the
//!   inherited overload — where the language records its classes' bases
//!   (Java, Kotlin, C#, Python, PHP; S-522). Scala, C++ and TypeScript record
//!   none, so there the call stays unbound: the own `inh` it bound by name
//!   before is never bound, and the base it reaches is not in the graph;
//! - `top` — an unqualified call whose same-named member cannot take it, beside
//!   a free function (or C#'s `using static` import) that can: Kotlin, whose
//!   resolution tries each scope level for an applicable candidate, goes on to
//!   the free function; a language whose bare call never reaches a member
//!   (TypeScript, Python, PHP; S-590) binds the free function anyway; in Java,
//!   C#, Scala and C++ the member hides every outer name even when no overload
//!   applies (JLS 15.12.1, C# §12.8.4), so the call stays unbound;
//! - `none` — nothing of the name admits the call: unbound, and a
//!   package-shaped language's `status` counts it `no-applicable-overload`;
//! - `same` — two overloads of one arity (the overloading languages only):
//!   still `overload-ambiguous`, no argument type is read;
//! - `vari` / `dflt` — a variadic and a defaulted parameter widen the range,
//!   so the call is admitted;
//! - `unk` — a spread argument makes the count unknown, which filters nothing
//!   (the languages that spell one).
//!
//! JavaScript enforces no arity, so a `.js`/`.jsx` call is never filtered
//! (Sprint 91 Autonomous Decision 3), while `.ts` is.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}@{}", n.name, n.start_line.unwrap_or(0))))
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
    assert_eq!(
        residue.unbound,
        residue.reasons.values().sum::<u64>() + residue.unclassified,
        "the reasons partition the unbound rows"
    );
    residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect()
}

fn reasons(pairs: &[(R, u64)]) -> BTreeMap<R, u64> {
    pairs.iter().copied().collect()
}

fn targets(labels: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = labels.iter().map(|l| (*l).to_string()).collect();
    out.sort();
    out
}

// ── Java ──────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-java")]
#[test]
fn java_binds_only_an_overload_whose_arity_admits_the_call() {
    let file = "src/main/java/com/x/C.java";
    let tmp = tree(&[(
        file,
        "package com.x;

import static com.x.Util.top;

class Base {
    public void inh(int a, int b) {}
}

class Util {
    public static void top(int a) {}
}

public class C extends Base {
    public void inh(int a) {}
    public void top(int a, int b) {}
    public void none(int a) {}
    public void same(int a) {}
    public void same(String a) {}
    public void vari(int... xs) {}
    public void caller() {
        inh(1, 2);
        this.inh(1, 2);
        top(1);
        none();
        same(1);
        vari(1, 2, 3);
        vari();
    }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "caller@20"),
        targets(&["inh@6", "vari@19"]),
        "both `inh(1, 2)` forms bind the inherited overload; `vari` takes 3 or 0"
    );
    // `top(1)`: the class's own `top` shadows the static import (JLS 15.12.1),
    // so no overload applies — as `none()`. `same(1)`: two of one arity.
    assert_eq!(
        residue(&engine, "java"),
        reasons(&[(R::NoApplicableOverload, 2), (R::OverloadAmbiguous, 1)])
    );
}

// ── Kotlin ────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_binds_only_an_overload_whose_arity_admits_the_call() {
    let file = "src/main/kotlin/app/C.kt";
    let tmp = tree(&[(
        file,
        "package app

open class Base {
    open fun inh(a: Int, b: Int) {}
}

fun top(a: Int) {}

class C : Base() {
    fun inh(a: Int) {}
    fun top(a: Int, b: Int) {}
    fun none(a: Int) {}
    fun same(a: Int) {}
    fun same(a: String) {}
    fun vari(vararg xs: Int) {}
    fun dflt(a: Int, b: Int = 0) {}
    fun unk(a: Int, b: Int) {}
    fun caller(xs: IntArray) {
        inh(1, 2)
        this.inh(1, 2)
        top(1)
        none()
        same(1)
        vari(1, 2, 3)
        dflt(1)
        unk(*xs)
    }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "caller@18"),
        targets(&["dflt@16", "inh@4", "top@7", "unk@17", "vari@15"]),
        "the inherited `inh`, the top-level `top`, and every admitted call"
    );
    assert_eq!(
        residue(&engine, "kotlin"),
        reasons(&[(R::NoApplicableOverload, 1), (R::OverloadAmbiguous, 1)])
    );
}

// ── Scala ─────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-scala")]
#[test]
fn scala_binds_only_an_overload_whose_arity_admits_the_call() {
    let file = "src/main/scala/app/C.scala";
    let tmp = tree(&[(
        file,
        "package app

class Base {
  def inh(a: Int, b: Int): Unit = ()
}

def top(a: Int): Unit = ()

class C extends Base {
  def inh(a: Int): Unit = ()
  def top(a: Int, b: Int): Unit = ()
  def none(a: Int): Unit = ()
  def same(a: Int): Unit = ()
  def same(a: String): Unit = ()
  def vari(xs: Int*): Unit = ()
  def dflt(a: Int, b: Int = 0): Unit = ()
  def unk(a: Int, b: Int): Unit = ()
  def caller(xs: Seq[Int]): Unit = {
    inh(1, 2)
    this.inh(1, 2)
    top(1)
    none()
    same(1)
    vari(1, 2, 3)
    dflt(1)
    unk(xs: _*)
  }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Scala records no class's bases, so neither `inh(1, 2)` reaches `Base`
    // nor `top(1)` falls through to the top-level `top`: an unseen base may
    // hold the overload either call reaches.
    assert_eq!(
        calls_from(rt, "caller@18"),
        targets(&["dflt@16", "unk@17", "vari@15"]),
        "every admitted call, and never the own `inh` or `top` the call cannot take"
    );
    assert_eq!(
        residue(&engine, "scala"),
        reasons(&[(R::NoApplicableOverload, 3), (R::OverloadAmbiguous, 1)])
    );
}

// ── C# ────────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-c-sharp")]
#[test]
fn c_sharp_binds_only_an_overload_whose_arity_admits_the_call() {
    let file = "src/App/C.cs";
    let tmp = tree(&[(
        file,
        "using static App.Util;

namespace App;

public class Base {
    public virtual void Inh(int a, int b) {}
}

public static class Util {
    public static void Top(int a) {}
}

public class C : Base {
    public void Inh(int a) {}
    public void Top(int a, int b) {}
    public void None(int a) {}
    public void Same(int a) {}
    public void Same(string a) {}
    public void Vari(params int[] xs) {}
    public void Dflt(int a, int b = 0) {}
    public void Caller() {
        Inh(1, 2);
        this.Inh(1, 2);
        Top(1);
        None();
        Same(1);
        Vari(1, 2, 3);
        Dflt(1);
    }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `Top(1)`: the class's own `Top` hides the `using static` one (C#
    // §12.8.4), so no overload applies — as `None()`.
    assert_eq!(
        calls_from(rt, "Caller@21"),
        targets(&["Dflt@20", "Inh@6", "Vari@19"]),
        "the inherited `Inh`, and every admitted call"
    );
    assert_eq!(
        residue(&engine, "c-sharp"),
        reasons(&[(R::NoApplicableOverload, 2), (R::OverloadAmbiguous, 1)])
    );
}

// ── C++ ───────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-cpp")]
#[test]
fn cpp_binds_only_an_overload_whose_arity_admits_the_call() {
    let file = "src/c.cpp";
    let tmp = tree(&[(
        file,
        "struct Base {
    void inh(int a, int b) {}
};

void top(int a) {}

void ov(int a) {}
void ov(int a, int b) {}
void solo(int a, int b) {}
void outside() { ov(1, 2); ov(1, 2, 3); solo(1); }

struct C : Base {
    using Base::inh;
    void inh(int a) {}
    void top(int a, int b) {}
    void none(int a) {}
    void same(int a) {}
    void same(double a) {}
    void vari(int a, ...) {}
    void dflt(int a, int b = 0) {}
    void caller() {
        inh(1, 2);
        this->inh(1, 2);
        top(1);
        none();
        same(1);
        vari(1, 2, 3);
        dflt(1);
    }
};
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // C++ records no class's bases, so neither `inh(1, 2)` reaches `Base` nor
    // `top(1)` falls through to the free `top`; `none()` and the two
    // `same(int)`/`same(double)` bind nothing either.
    assert_eq!(
        calls_from(rt, "caller@21"),
        targets(&["dflt@20", "vari@19"]),
        "every admitted call, and never the own `inh` or `top` the call cannot take"
    );
    // A free function's definition records no range — its defaults may sit on
    // a prototype elsewhere (S-591) — and an unknown range never filters: both
    // `ov` stay candidates for `ov(1, 2)` and `ov(1, 2, 3)`, which bind
    // neither, and the lone `solo` binds `solo(1)`.
    assert_eq!(calls_from(rt, "outside@10"), targets(&["solo@9"]));
}

// ── TypeScript, and JavaScript, which is never filtered ──────────────────

#[cfg(feature = "lang-typescript")]
const TS: &str = "class Base {
    inh(a: number, b: number) {}
}

function top(a: number) {}

class C extends Base {
    inh(a: number) {}
    top(a: number, b: number) {}
    none(a: number) {}
    vari(...xs: number[]) {}
    dflt(a: number, b = 0) {}
    opt(a: number, b?: number) {}
    unk(a: number, b: number) {}
    caller(xs: number[]) {
        this.inh(1, 2);
        top(1);
        this.none();
        this.vari(1, 2, 3);
        this.dflt(1);
        this.opt(1);
        this.unk(...xs);
    }
}
";

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_binds_only_a_callable_whose_arity_admits_the_call() {
    let tmp = tree(&[("src/c.ts", TS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // TypeScript records no class's bases, so `this.inh(1, 2)` cannot reach
    // `Base.inh`, and the own `inh(a)` it bound by name before is never bound.
    assert_eq!(
        calls_from(rt, "caller@15"),
        targets(&["dflt@12", "opt@13", "top@5", "unk@14", "vari@11"]),
        "the free `top`, and every admitted call; `inh(1, 2)` and `none()` bind nothing"
    );
}

/// JavaScript enforces no arity (Sprint 91 Autonomous Decision 3): the same
/// calls in a `.js` file are never filtered, so `this.inh(1, 2)` binds the own
/// class's `inh(a)` and `this.none()` its `none(a)` — what binding by name did
/// before — and a free `two(1)` binds `function two(a, b)`.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_javascript_call_is_never_filtered_by_arity() {
    for file in ["src/c.js", "src/c.mjs", "src/c.cjs", "src/c.jsx"] {
        let src = "class Base {\n    inh(a, b) {}\n}\n\nfunction two(a, b) {}\n\n\
            class C extends Base {\n    inh(a) {}\n    none(a) {}\n    \
            caller() {\n        this.inh(1, 2);\n        this.none();\n        two(1);\n    }\n}\n";
        let tmp = tree(&[(file, src)]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(
            calls_from(rt, "caller@10"),
            targets(&["inh@8", "none@9", "two@5"]),
            "{file}: no candidate is dropped for its range"
        );
    }
}

// ── Python ────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-python")]
#[test]
fn python_binds_only_a_callable_whose_arity_admits_the_call() {
    let tmp = tree(&[(
        "pkg/c.py",
        "class Base:
    def inh(self, a, b):
        pass


def top(a):
    pass


class C(Base):
    def inh(self, a):
        pass

    def top(self, a, b):
        pass

    def none(self, a):
        pass

    def vari(self, *xs):
        pass

    def dflt(self, a, b=0):
        pass

    def unk(self, a, b):
        pass

    def caller(self, xs):
        self.inh(1, 2)
        top(1)
        self.none()
        self.vari(1, 2, 3)
        self.dflt(1)
        self.unk(*xs)
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "caller@29"),
        targets(&["dflt@23", "inh@2", "top@6", "unk@26", "vari@20"]),
        "the inherited `inh`, the free `top`, and every admitted call; `none()` binds nothing"
    );
}

// ── PHP ───────────────────────────────────────────────────────────────────

/// PHP accepts surplus arguments to a user function (`func_get_args()`), so a
/// range's maximum is unbounded there (S-591) and only a call short of the
/// required parameters is ruled out: the own `inh($a, $b)` cannot take
/// `inh(1)`, the inherited `inh($a)` can.
#[cfg(feature = "lang-php")]
#[test]
fn php_binds_only_a_callable_whose_arity_admits_the_call() {
    let tmp = tree(&[(
        "src/C.php",
        "<?php
namespace App;

class Base {
    public function inh($a) {}
}

function top($a) {}

class C extends Base {
    public function inh($a, $b) {}
    public function top($a, $b) {}
    public function none($a) {}
    public function vari(...$xs) {}
    public function dflt($a, $b = 0) {}
    public function unk($a, $b) {}
    public function caller($xs) {
        $this->inh(1);
        top(1);
        $this->none();
        $this->vari(1, 2, 3);
        $this->dflt(1);
        $this->unk(...$xs);
    }
}
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "caller@17"),
        targets(&["dflt@15", "inh@5", "top@8", "unk@16", "vari@14"]),
        "the inherited `inh`, the free `top`, and every admitted call"
    );
    assert_eq!(residue(&engine, "php"), reasons(&[(R::NoApplicableOverload, 1)]));
}



// ── A class whose bases the walk cannot see never falls through ──────────

/// A Kotlin unqualified in-class call falls through to a free function only
/// when the class walk saw every supertype (S-592): a class with an external
/// base — the eShop `ViewModelBase : ObservableObject` shape — or one
/// implementing an interface, whose default body the walk does not climb, may
/// inherit the overload the call reaches. Neither binds the free function.
#[cfg(feature = "lang-kotlin")]
#[test]
fn a_call_whose_class_has_an_unseen_supertype_never_falls_through() {
    for (supertype, preamble) in [
        ("External()", ""),
        ("I", "interface I {\n    fun top(a: Int) {}\n}\n"),
    ] {
        let src = format!(
            "package app\n\n{preamble}fun top(a: Int) {{}}\n\nclass C : {supertype} {{\n    \
             fun top(a: Int, b: Int) {{}}\n    fun caller() {{ top(1) }}\n}}\n"
        );
        let tmp = tree(&[("src/main/kotlin/app/C.kt", src.as_str())]);
        let engine = index(tmp.path());
        let caller = if preamble.is_empty() { "caller@7" } else { "caller@10" };
        assert_eq!(calls_from(engine.runtime().unwrap(), caller), targets(&[]), "{supertype}");
        assert_eq!(residue(&engine, "kotlin"), reasons(&[(R::NoApplicableOverload, 1)]), "{supertype}");
    }
}

/// A Kotlin class inherits `Any`'s `equals`, `hashCode` and `toString`, which
/// the graph never holds (S-592, the descriptor's `implicit_root_members`):
/// `hashCode()` that the class's own `hashCode(seed)` cannot take is
/// `Any.hashCode()`, which a member beats the top-level `hashCode` to — so the
/// call never falls through, while one of another name still does.
#[cfg(feature = "lang-kotlin")]
#[test]
fn a_kotlin_call_of_a_root_member_never_falls_through() {
    let tmp = tree(&[(
        "src/main/kotlin/app/A.kt",
        "package app

fun hashCode(): Int = 0
fun size(): Int = 0

class A {
    fun hashCode(seed: Int): Int = seed
    fun size(seed: Int): Int = seed
    fun caller(): Int { return hashCode() + size() }
}
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(calls_from(engine.runtime().unwrap(), "caller@9"), targets(&["size@4"]));
    assert_eq!(residue(&engine, "kotlin"), reasons(&[(R::NoApplicableOverload, 1)]));
}

/// The fall-through is by scope and imports only (S-592): under the
/// `aggressive` policy the workspace name match would otherwise bind
/// `this.foo(1)` — whose own `foo(a, b)` cannot take it — to the one `foo` of
/// an unrelated class in another package. A receiver call never reaches that
/// guess.
#[cfg(feature = "lang-kotlin")]
#[test]
fn a_fall_through_never_reaches_the_workspace_name_match() {
    let tmp = tree(&[
        (".logos/config.toml", "[resolution]\npolicy = \"aggressive\"\n"),
        (
            "src/main/kotlin/app/A.kt",
            "package app\n\nclass A {\n    fun foo(a: Int, b: Int) {}\n    fun caller() { this.foo(1) }\n}\n",
        ),
        (
            "src/main/kotlin/other/B.kt",
            "package other\n\nclass B {\n    fun pad() {}\n    fun pad2() {}\n    fun pad3() {}\n    fun foo(a: Int) {}\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(calls_from(engine.runtime().unwrap(), "caller@5"), targets(&[]));
    assert_eq!(residue(&engine, "kotlin"), reasons(&[(R::NoApplicableOverload, 1)]));
}

/// A static wildcard brings each imported type's members into view, and the
/// count chooses among them (S-592): `top(1)` binds `U1.top(a)` beside
/// `U2.top(a, b)`, and `two(1)`, which only `U2.two(a, b)` names, binds
/// nothing — `no-applicable-overload`.
#[cfg(feature = "lang-java")]
#[test]
fn a_static_wildcards_members_are_filtered_by_the_count() {
    let tmp = tree(&[
        (
            "src/main/java/com/u/U1.java",
            "package com.u;\n\npublic class U1 {\n    public static void top(int a) {}\n}\n",
        ),
        (
            "src/main/java/com/u/U2.java",
            "package com.u;\n\npublic class U2 {\n    public static void top(int a, int b) {}\n    \
             public static void two(int a, int b) {}\n}\n",
        ),
        (
            "src/main/java/com/x/C.java",
            "package com.x;\n\nimport static com.u.U1.*;\nimport static com.u.U2.*;\n\n\
             public class C {\n    public void caller() { top(1); two(1); }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    assert_eq!(calls_from(engine.runtime().unwrap(), "caller@7"), targets(&["top@4"]));
    assert_eq!(residue(&engine, "java"), reasons(&[(R::NoApplicableOverload, 1)]));
}

/// The fall-through's guard reads every level the walk crosses (S-592): a
/// base two levels up that the graph cannot see still keeps a Kotlin call from
/// falling through, as one at the first level does.
#[cfg(feature = "lang-kotlin")]
#[test]
fn an_unseen_supertype_above_the_first_level_keeps_the_call_from_falling_through() {
    let tmp = tree(&[(
        "src/main/kotlin/app/C.kt",
        "package app

fun top(a: Int) {}

open class B : External()

class C : B() {
    fun top(a: Int, b: Int) {}
    fun caller() { top(1) }
}
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(calls_from(engine.runtime().unwrap(), "caller@9"), targets(&[]));
    assert_eq!(residue(&engine, "kotlin"), reasons(&[(R::NoApplicableOverload, 1)]));
}

// ── Sync ≡ reindex ────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced graph's
/// `Calls` targets from `caller` are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)], caller: &str) -> Vec<String> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        fs::write(tmp_a.path().join(rel), text).unwrap();
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
    calls_from(rt_a, caller)
}

#[cfg(feature = "lang-java")]
const JAVA_BASE: &str = "src/main/java/com/x/Base.java";
#[cfg(feature = "lang-java")]
const JAVA_KID: &str = "src/main/java/com/x/Kid.java";
#[cfg(feature = "lang-java")]
const KID: &str = "package com.x;

public class Kid extends Base {
    public void inh(int a) {}
    public void caller() { inh(1, 2); }
}
";

/// A change to a callable's parameters in one file moves a call written in
/// another, which spells only the callable's name (S-592): the base's `inh`
/// stops admitting `inh(1, 2)` and the call is left unbound — and the reverse
/// binds it — exactly as a cold index of the edited tree does.
#[cfg(feature = "lang-java")]
#[test]
fn a_range_changed_in_another_file_rebinds_the_call_on_sync() {
    let two = "package com.x;\n\npublic class Base {\n    public void inh(int a, int b) {}\n}\n";
    let three = "package com.x;\n\npublic class Base {\n    public void inh(int a, int b, int c) {}\n}\n";
    let edges = synced_equals_reindexed(&[(JAVA_BASE, two), (JAVA_KID, KID)], &[(JAVA_BASE, three)], "caller@5");
    assert_eq!(edges, targets(&[]), "no `inh` takes two arguments now");
    let edges = synced_equals_reindexed(&[(JAVA_BASE, three), (JAVA_KID, KID)], &[(JAVA_BASE, two)], "caller@5");
    assert_eq!(edges, targets(&["inh@4"]), "the base's `inh` takes two again");
}
