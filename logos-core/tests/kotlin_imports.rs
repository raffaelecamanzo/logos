//! A Kotlin file's module identity follows its package, as a Java file's does
//! (S-472, CR-152; the "data change later" CR-149 named for Kotlin), so its
//! imports of in-repository types bind — exercised end-to-end through the public
//! [`Engine`] against real temp-directory fixtures.
//!
//! The Kotlin descriptor now declares `[package_modules]` under
//! `src/{main,test}/kotlin`, which is what lets a declared type be named by the
//! one derivation (`resolve::package_key::PackageLayout`). The Java shapes are
//! pinned in `java_imports.rs`; this file pins the Kotlin consequences the
//! change has: an import reaches the class it names, a type declared in both
//! trees binds neither, an external import stays unbound, and a call inside a
//! class still binds on its lexical scope.
#![cfg(feature = "lang-kotlin")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Every edge of `kind` out of the file `rel`, as `target file:name:kind`, sorted.
fn edges_from(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let prefix = format!("{rel}:");
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, (String, NodeKind)> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, (format!("{}:{}", n.file_path.unwrap_or_default(), n.name), n.kind)))
            .collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind && label[&e.source].0.starts_with(&prefix))
            .map(|e| {
                let (target, target_kind) = &label[&e.target];
                format!("{target}:{}", target_kind.as_str())
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's unresolved `Imports` targets out of the file `rel`, sorted.
fn unbound_imports(rt: &Runtime, rel: &str) -> Vec<String> {
    let rel = rel.to_string();
    let mut rows: Vec<String> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| {
                    r.kind == EdgeKind::Imports
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

const SVC: &str = "package com.x.svc\n\nclass Svc {\n    fun run() = helper()\n    fun helper() = 1\n}\n";
const APP: &str = "package com.x.app\n\n\
                   import com.x.svc.Svc\n\
                   import org.springframework.stereotype.Component\n\n\
                   class App(val svc: Svc)\n";
const APP_PATH: &str = "app/src/main/kotlin/com/x/app/App.kt";

#[test]
fn a_kotlin_import_of_an_in_repository_type_binds_to_the_class_and_an_external_one_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "svc/src/main/kotlin/com/x/svc/Svc.kt", SVC);
    write(tmp.path(), APP_PATH, APP);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    assert_eq!(
        edges_from(rt, APP_PATH, EdgeKind::Imports),
        vec!["svc/src/main/kotlin/com/x/svc/Svc.kt:Svc:class".to_string()],
        "the import reaches the class it names, never its file module"
    );
    assert_eq!(
        unbound_imports(rt, APP_PATH),
        vec!["org::springframework::stereotype::Component".to_string()],
        "a library import stays unbound"
    );
    // INTERIM(S-516): the bare in-class call `helper()` is a method-form row
    // with no receiver shape until the Kotlin plugin declares its implicit
    // receiver — unbound, never bound through the lexical scope (S-514,
    // FR-RS-12). With S-516's marker and `implicit_receiver = "self"` it binds
    // `Svc.helper` again: restore the one-edge expectation.
    assert_eq!(
        edges_from(rt, "svc/src/main/kotlin/com/x/svc/Svc.kt", EdgeKind::Calls),
        Vec::<String>::new()
    );
}

#[test]
fn a_kotlin_type_declared_in_both_trees_binds_neither() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "svc/src/main/kotlin/com/x/svc/Svc.kt", SVC);
    write(tmp.path(), "svc/src/test/kotlin/com/x/svc/Svc.kt", "package com.x.svc\n\nclass Svc\n");
    write(tmp.path(), APP_PATH, APP);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    assert!(
        edges_from(rt, APP_PATH, EdgeKind::Imports).is_empty(),
        "one fully-qualified name in both trees is ambiguous and binds neither"
    );
    assert!(unbound_imports(rt, APP_PATH).contains(&"com::x::svc::Svc".to_string()));
}
