//! A Kotlin file's module identity follows its package (S-472, CR-152; the "data
//! change later" CR-149 named for Kotlin), so its imports of in-repository types
//! bind — exercised end-to-end through the public [`Engine`] against real
//! temp-directory fixtures.
//!
//! The Kotlin descriptor declares the declared-namespace module model (S-518,
//! FR-RS-13; it replaced S-472's `[package_modules]` source roots): a file is
//! named by its `package` header, which is what lets a declared type be named
//! by the one derivation (`resolve::package_key::PackageLayout`) wherever the
//! file sits — `namespace_imports.rs` pins a Multiplatform `commonMain` tree.
//! The Java shapes are
//! pinned in `java_imports.rs`; this file pins the Kotlin consequences the
//! change has: an import reaches the class it names, a type declared in both
//! trees binds neither, an external import stays unbound, and a bare call
//! inside a class binds the class's own member (the `self` shape, S-516).
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
    engine.pipeline().index();
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
    // A bare call inside the class is a call on the current instance (the
    // `self` shape, S-516): it binds the class's own `helper`.
    assert_eq!(
        edges_from(rt, "svc/src/main/kotlin/com/x/svc/Svc.kt", EdgeKind::Calls),
        vec!["svc/src/main/kotlin/com/x/svc/Svc.kt:helper:method".to_string()]
    );
}

#[test]
fn a_kotlin_type_declared_in_both_trees_binds_neither() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "svc/src/main/kotlin/com/x/svc/Svc.kt", SVC);
    write(tmp.path(), "svc/src/test/kotlin/com/x/svc/Svc.kt", "package com.x.svc\n\nclass Svc\n");
    write(tmp.path(), APP_PATH, APP);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let rt = engine.runtime().unwrap();

    assert!(
        edges_from(rt, APP_PATH, EdgeKind::Imports).is_empty(),
        "one fully-qualified name in both trees is ambiguous and binds neither"
    );
    assert!(unbound_imports(rt, APP_PATH).contains(&"com::x::svc::Svc".to_string()));
}

const UTIL: &str = "package app.util\n\nval DEFAULT_NAME = \"x\"\nconst val LIMIT = 10\n\nfun slug() = 1\n";
const UTIL_PATH: &str = "src/main/kotlin/app/util/Util.kt";
const USER_PATH: &str = "src/main/kotlin/app/web/User.kt";

/// An import of a top-level property binds to its Variable or Constant (S-831,
/// FR-RS-31); an import of one no file declares, and one of another package's
/// same-named property, stay unbound.
#[test]
fn a_kotlin_import_of_a_top_level_property_binds_to_its_variable_or_constant() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), UTIL_PATH, UTIL);
    write(tmp.path(), "src/main/kotlin/app/other/Other.kt", "package app.other\n\nval ELSEWHERE = 1\n");
    write(
        tmp.path(),
        USER_PATH,
        "package app.web\n\n\
         import app.util.DEFAULT_NAME\n\
         import app.util.LIMIT\n\
         import app.util.MISSING\n\
         import app.util.ELSEWHERE\n\n\
         class User\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let rt = engine.runtime().unwrap();

    assert_eq!(
        edges_from(rt, USER_PATH, EdgeKind::Imports),
        vec![format!("{UTIL_PATH}:DEFAULT_NAME:variable"), format!("{UTIL_PATH}:LIMIT:constant")],
        "an import of a top-level property reaches the node it names"
    );
    assert_eq!(
        unbound_imports(rt, USER_PATH),
        vec!["app::util::ELSEWHERE".to_string(), "app::util::MISSING".to_string()],
        "a name the package does not declare stays unbound"
    );
}

/// A top-level property is no callable and types no receiver: a call of a
/// same-named function binds the function, and a call on a property typed by a
/// class binds nothing through it (S-563). The two receiver calls sit in files
/// of their own: edges are unique per source and target, so a binding of
/// `repo.find()` in the file that also holds the control would not show.
/// Receiver typing is file-local, so the receiver half is owned by
/// `kotlin_typed_receivers` (`a_top_level_property_never_types_a_call_…`); this
/// pins that an import of the property adds no typing across files.
#[test]
fn a_top_level_property_is_no_call_target_and_types_no_receiver() {
    const LONE_PATH: &str = "src/main/kotlin/app/web/Lone.kt";
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        UTIL_PATH,
        "package app.util\n\nclass Repo {\n    fun find() = 1\n}\n\nval repo: Repo = Repo()\nval slug = 1\n\nfun slug() = 1\n",
    );
    write(
        tmp.path(),
        USER_PATH,
        "package app.web\n\nimport app.util.slug\nimport app.util.Repo\n\nclass User(private val typed: Repo) {\n    fun run() = slug() + typed.find()\n}\n",
    );
    write(
        tmp.path(),
        LONE_PATH,
        "package app.web\n\nimport app.util.repo\n\nclass Lone {\n    fun go() = repo.find()\n}\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let rt = engine.runtime().unwrap();

    // An import of a name a function and a property share has always reached
    // the function, which a property of the name does not take from it.
    assert_eq!(
        edges_from(rt, USER_PATH, EdgeKind::Imports),
        vec![format!("{UTIL_PATH}:Repo:class"), format!("{UTIL_PATH}:slug:function")]
    );
    assert_eq!(edges_from(rt, LONE_PATH, EdgeKind::Imports), vec![format!("{UTIL_PATH}:repo:variable")]);
    // The control: a constructor property typed `Repo` proves its receiver.
    assert_eq!(
        edges_from(rt, USER_PATH, EdgeKind::Calls),
        vec![format!("{UTIL_PATH}:find:method"), format!("{UTIL_PATH}:slug:function")]
    );
    assert!(
        edges_from(rt, LONE_PATH, EdgeKind::Calls).is_empty(),
        "`repo.find()` is typed by no top-level property: {:?}",
        edges_from(rt, LONE_PATH, EdgeKind::Calls)
    );
}

/// A property that is the only candidate of its name is no call target: `handler()`
/// through an import, a wildcard and the same package binds nothing, while the
/// import row itself binds the property (S-831). Each caller sits in a file of
/// its own, so a stray binding in one cannot hide behind another.
#[test]
fn a_top_level_property_alone_in_its_name_is_never_called() {
    const BY_IMPORT: &str = "src/main/kotlin/app/web/ByImport.kt";
    const BY_GLOB: &str = "src/main/kotlin/app/web/ByGlob.kt";
    const BESIDE: &str = "src/main/kotlin/app/util/Beside.kt";
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), UTIL_PATH, "package app.util\n\nval handler = 1\n");
    write(tmp.path(), BY_IMPORT, "package app.web\n\nimport app.util.handler\n\nclass A {\n    fun go() = handler()\n}\n");
    write(tmp.path(), BY_GLOB, "package app.web\n\nimport app.util.*\n\nclass B {\n    fun go() = handler()\n}\n");
    write(tmp.path(), BESIDE, "package app.util\n\nclass C {\n    fun go() = handler()\n}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let rt = engine.runtime().unwrap();

    assert_eq!(edges_from(rt, BY_IMPORT, EdgeKind::Imports), vec![format!("{UTIL_PATH}:handler:variable")]);
    for path in [BY_IMPORT, BY_GLOB, BESIDE] {
        assert!(
            edges_from(rt, path, EdgeKind::Calls).is_empty(),
            "{path}: a property is no call target: {:?}",
            edges_from(rt, path, EdgeKind::Calls)
        );
    }
}

/// A top-level property is not a namespace: `val util` of package `app` does not
/// take `import app.util.*` from the package `app.util` (S-831). The wildcard
/// still binds to the package's files and brings its members into view.
#[test]
fn a_top_level_property_named_like_a_package_does_not_shadow_its_wildcard() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/main/kotlin/app/util/Helper.kt", "package app.util\n\nclass Helper\n");
    write(tmp.path(), "src/main/kotlin/app/util/F.kt", "package app.util\n\nfun f() = 1\n");
    write(tmp.path(), "src/main/kotlin/app/P.kt", "package app\n\nval util = 1\n");
    write(
        tmp.path(),
        "src/main/kotlin/other/D.kt",
        "package other\n\nimport app.util.*\n\nclass D(val h: Helper) {\n    fun go() = f()\n}\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.pipeline().index();
    let rt = engine.runtime().unwrap();
    let d = "src/main/kotlin/other/D.kt";

    assert_eq!(
        edges_from(rt, d, EdgeKind::Imports),
        vec![
            "src/main/kotlin/app/util/F.kt:F:module".to_string(),
            "src/main/kotlin/app/util/Helper.kt:Helper:module".to_string()
        ],
        "the wildcard still binds the package's files"
    );
    assert!(unbound_imports(rt, d).is_empty(), "{:?}", unbound_imports(rt, d));
    assert_eq!(
        edges_from(rt, d, EdgeKind::Calls),
        vec!["src/main/kotlin/app/util/F.kt:f:function".to_string()],
        "and brings its function into view"
    );
}
