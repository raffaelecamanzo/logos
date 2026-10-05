//! A call binds what its plugin declares callable (S-521, [FR-RS-16]) —
//! exercised end to end through the public [`Engine`] against real temp-directory
//! fixtures (`call_targets/fixtures.rs`).
//!
//! The python, kotlin and scala descriptors declare `class_call_instantiates`,
//! the c descriptor `macros_callable`, and every other descriptor neither. The
//! binder rules are pinned in memory by `resolve::call_target_tests`; this suite
//! pins what reaches the product: a constructed class records `Instantiates`, a
//! C macro call binds `Calls`, two candidates or none stay in the ledger
//! ([NFR-RA-05]), Rust binds exactly as before, and the result is the same on a
//! second index and after a sync ([NFR-RA-06]).
//!
//! [FR-RS-16]: ../../docs/specs/requirements/FR-RS-16.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(all(
    feature = "lang-python",
    feature = "lang-kotlin",
    feature = "lang-c",
    feature = "lang-rust"
))]

#[path = "call_targets/fixtures.rs"]
mod fixtures;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn indexed(fixture: fixtures::Fixture) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture {
        write(tmp.path(), rel, source);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

/// Every edge of `kind` out of the file `rel`, as `source name -> target
/// file:name:kind`, sorted.
fn edges_from(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind && nodes[&e.source].file_path.as_deref() == Some(rel.as_str()))
            .map(|e| {
                let (s, t) = (&nodes[&e.source], &nodes[&e.target]);
                let file = t.file_path.as_deref().unwrap_or_default();
                format!("{} -> {file}:{}:{}", s.name, t.name, t.kind.as_str())
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's unresolved `Calls` targets out of the file `rel`, sorted.
fn unbound_calls(rt: &Runtime, rel: &str) -> Vec<String> {
    let rel = rel.to_string();
    let mut rows: Vec<String> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| {
                    r.kind == EdgeKind::Calls
                        && !r.resolved
                        && r.file_id.and_then(|id| files.get(&id)) == Some(&rel)
                })
                .map(|r| r.target)
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows
}

/// Every edge as `(source symbol, target symbol, kind)` and every textual ledger
/// row, sorted — what a cold index and a sync must agree on.
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
            .map(|e| (sym[&e.source].clone(), sym[&e.target].clone(), e.kind.as_str().to_string()))
            .collect();
        edges.sort();
        let mut refs: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .filter(|r| r.form != RefForm::Symbol)
            .map(|r| format!("{} {} {:?} {:?} {}", r.source_symbol, r.target, r.form, r.kind, r.resolved))
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// FR-RS-16 AC (healthchecks H1): `Check(project=p)` after `from
/// hc.api.models import Check` records `Instantiates` to the one class `Check`,
/// and `Project()` likewise. `prepare(check)` through the same import is still a
/// `Calls`, and a library class (`HttpResponse`) stays unbound.
#[test]
fn a_python_call_to_one_imported_class_records_instantiates() {
    let (_tmp, engine) = indexed(fixtures::PYTHON);
    let rt = engine.runtime().unwrap();
    let views = "hc/front/views.py";
    assert_eq!(
        edges_from(rt, views, EdgeKind::Instantiates),
        strings(&[
            "add_check -> hc/api/models.py:Check:class",
            "add_check -> hc/api/models.py:Project:class",
        ])
    );
    assert_eq!(
        edges_from(rt, views, EdgeKind::Calls),
        strings(&["add_check -> hc/api/models.py:prepare:function"])
    );
    assert_eq!(unbound_calls(rt, views), strings(&["HttpResponse"]));
}

/// FR-RS-16 AC: a class declared twice (two `Clock`s, one per branch of a
/// version check) is two candidates, and a name nothing declares is none — both
/// stay in `unresolved_refs`, never guessed ([NFR-RA-05]).
#[test]
fn two_python_classes_or_none_stay_unbound() {
    let (_tmp, engine) = indexed(fixtures::PYTHON);
    let rt = engine.runtime().unwrap();
    let compat = "hc/front/compat.py";
    assert!(edges_from(rt, compat, EdgeKind::Instantiates).is_empty());
    assert_eq!(unbound_calls(rt, compat), strings(&["Clock", "Missing"]));
}

/// FR-RS-16 AC: a Kotlin `Foo()` with one in-repository class `Foo` records
/// `Instantiates`, through a single-type import and from the class's own
/// package.
#[test]
fn a_kotlin_call_to_one_class_records_instantiates() {
    let (_tmp, engine) = indexed(fixtures::KOTLIN);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/main/kotlin/com/y/Use.kt", EdgeKind::Instantiates),
        strings(&["build -> src/main/kotlin/com/x/Foo.kt:Foo:class"])
    );
    assert_eq!(
        edges_from(rt, "src/main/kotlin/com/x/Make.kt", EdgeKind::Instantiates),
        strings(&["make -> src/main/kotlin/com/x/Foo.kt:Foo:class"])
    );
}

/// A Kotlin `Bar()` naming the two `com.z.Bar` classes of two source sets, and
/// a `Missing()` nothing declares, stay unbound. A bare `Foo()` inside a class
/// body is a call on the current instance (`implicit_receiver = "self"`): it
/// binds among the class's own members only, so it never reaches `Foo`.
#[test]
fn two_kotlin_classes_none_or_a_call_inside_a_class_stay_unbound() {
    let (_tmp, engine) = indexed(fixtures::KOTLIN);
    let rt = engine.runtime().unwrap();
    let mk = "src/main/kotlin/com/z/Mk.kt";
    assert!(edges_from(rt, mk, EdgeKind::Instantiates).is_empty());
    assert_eq!(unbound_calls(rt, mk), strings(&["Bar"]));
    let use_kt = "src/main/kotlin/com/y/Use.kt";
    assert_eq!(unbound_calls(rt, use_kt), strings(&["Foo", "Missing"]));
}

/// A Kotlin factory function beside its class (`fun Job(s: String): Job`) is a
/// second declaration the call names, from the package and through an import:
/// the package rungs read types only, and the call must not be read as
/// constructing the class ([NFR-RA-05]).
#[test]
fn a_kotlin_class_beside_its_factory_function_stays_unbound() {
    let (_tmp, engine) = indexed(fixtures::KOTLIN);
    let rt = engine.runtime().unwrap();
    for file in ["src/main/kotlin/com/f/Start.kt", "src/main/kotlin/com/g/Run.kt"] {
        assert!(edges_from(rt, file, EdgeKind::Instantiates).is_empty(), "{file}");
        assert!(edges_from(rt, file, EdgeKind::Calls).is_empty(), "{file}");
        assert_eq!(unbound_calls(rt, file), strings(&["Job"]), "{file}");
    }
}

/// An aliased import names only its alias ([FR-EX-14], [NFR-RA-05]): beside
/// `import other.Base as OtherBase`, a Kotlin `Base()` and `: Base()` reach the
/// file's own-package `Base`, and `OtherBase()` / `: OtherBase()` the imported
/// one — the import row is aliased `OtherBase`, so it never answers for `Base`.
/// Scala's `import other.{Base => OtherBase}` reads the same way.
///
/// [FR-EX-14]: ../../docs/specs/requirements/FR-EX-14.md
#[test]
fn an_aliased_import_never_answers_for_the_name_it_renames() {
    let (_tmp, engine) = indexed(fixtures::ALIASED_IMPORT);
    let rt = engine.runtime().unwrap();
    let child = "src/main/kotlin/app/models/Child.kt";
    assert_eq!(
        edges_from(rt, child, EdgeKind::Instantiates),
        strings(&[
            "make -> src/main/kotlin/app/models/Base.kt:Base:class",
            "makeOther -> src/main/kotlin/other/Base.kt:Base:class",
        ])
    );
    assert_eq!(
        edges_from(rt, child, EdgeKind::Extends),
        strings(&[
            "Child -> src/main/kotlin/app/models/Base.kt:Base:class",
            "Stranger -> src/main/kotlin/other/Base.kt:Base:class",
        ])
    );
    #[cfg(feature = "lang-scala")]
    assert_eq!(
        edges_from(rt, "src/main/scala/sapp/models/Make.scala", EdgeKind::Instantiates),
        strings(&[
            "make -> src/main/scala/sapp/models/Base.scala:Base:class",
            "makeOther -> src/main/scala/sother/Base.scala:Base:class",
        ])
    );
}

/// FR-RS-16 AC (libuv `src/fs-poll.c`): a call to the macro its file defines
/// binds `Calls` to the Macro node, while `src/unix/core.c`'s call binds its own
/// function of the same name. A macro and a function of one name in one file
/// are two candidates.
#[test]
fn a_c_call_to_a_macro_binds_to_the_macro_node() {
    let (_tmp, engine) = indexed(fixtures::C);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/fs-poll.c", EdgeKind::Calls),
        strings(&["poll_cb -> src/fs-poll.c:uv__make_close_pending:macro"])
    );
    assert_eq!(
        edges_from(rt, "src/unix/core.c", EdgeKind::Calls),
        strings(&["uv__close -> src/unix/core.c:uv__make_close_pending:function"])
    );
    assert!(edges_from(rt, "src/dual.c", EdgeKind::Calls).is_empty());
    assert_eq!(unbound_calls(rt, "src/dual.c"), strings(&["dual"]));
}

/// A plugin that declares neither key binds as before: Rust's call to a
/// function binds, while its calls to a `macro_rules!` name and to a struct do
/// not, and no Rust call records `Instantiates`.
#[test]
fn a_language_without_either_key_binds_a_callable_only() {
    let (_tmp, engine) = indexed(fixtures::RUST);
    let rt = engine.runtime().unwrap();
    let lib = "tool/src/lib.rs";
    assert_eq!(
        edges_from(rt, lib, EdgeKind::Calls),
        strings(&["run -> tool/src/lib.rs:helper:function"])
    );
    assert!(edges_from(rt, lib, EdgeKind::Instantiates).is_empty());
    assert_eq!(unbound_calls(rt, lib), strings(&["Point", "twice"]));
}

/// Resolution is deterministic ([NFR-RA-06]): two indexes of one tree hold the
/// same edges and ledger, symbol for symbol.
#[test]
fn indexing_twice_gives_identical_edges() {
    let all: Vec<(&str, &str)> = [fixtures::PYTHON, fixtures::KOTLIN, fixtures::C, fixtures::RUST]
        .concat();
    let fixture: fixtures::Fixture = Box::leak(all.into_boxed_slice());
    let (_a, first) = indexed(fixture);
    let (_b, second) = indexed(fixture);
    let facts = binding_facts(first.runtime().unwrap());
    assert!(facts.0.iter().any(|(_, _, kind)| kind == "instantiates"));
    assert_eq!(facts, binding_facts(second.runtime().unwrap()));
}

/// A sync equals a full reindex ([NFR-RA-06]) when the constructed class's file
/// changes: re-extracting `models.py` captures the inbound `Instantiates` edge
/// and re-binds it by symbol; removing the class unbinds it; restoring it binds
/// it again.
#[test]
fn sync_equals_a_full_reindex_when_the_constructed_class_changes() {
    let (tmp, engine) = indexed(fixtures::PYTHON);
    let rt = engine.runtime().unwrap();
    let models = "hc/api/models.py";
    let cold = |tmp: &TempDir| {
        let fresh = TempDir::new().unwrap();
        for (rel, _) in fixtures::PYTHON {
            write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
        }
        let engine = Engine::start(fresh.path()).expect("engine starts");
        engine.index();
        binding_facts(engine.runtime().unwrap())
    };
    let original = fixtures::PYTHON[2].1;
    for edit in [
        format!("{original}\n\ndef extra():\n    pass\n"),
        "class Project:\n    pass\n\n\ndef prepare(check):\n    return check\n".to_string(),
        original.to_string(),
    ] {
        write(tmp.path(), models, &edit);
        engine.sync(&[models.into()]);
        assert_eq!(binding_facts(rt), cold(&tmp), "after writing:\n{edit}");
    }
}
