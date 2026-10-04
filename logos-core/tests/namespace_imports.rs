//! A PHP, C#, Kotlin or Scala file takes its identity from the namespace or
//! package it **declares** (S-518, [FR-RS-13]), so its imports bind through a
//! fully-qualified type index — exercised end-to-end through the public
//! [`Engine`] against real temp-directory fixtures (`namespace_imports/fixtures.rs`).
//!
//! Each language's descriptor declares `[module_model] kind = "namespace"`; its
//! `symbols` query captures the declaration with `@module.namespace`. The binder
//! rules themselves are pinned in memory by `resolve::tests`; this suite pins
//! what reaches the product: a single-type import binds the type it names and an
//! external one does not, a namespace wildcard binds to the files declaring the
//! namespace, a namespace that differs from its directory still binds, Java's
//! edges are untouched by any of it, and a one-file sync equals a full reindex
//! ([NFR-RA-06]).
//!
//! [FR-RS-13]: ../../docs/specs/requirements/FR-RS-13.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(all(
    feature = "lang-php",
    feature = "lang-c-sharp",
    feature = "lang-kotlin",
    feature = "lang-scala",
    feature = "lang-java"
))]

#[path = "namespace_imports/fixtures.rs"]
mod fixtures;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A temp tree holding `fixture`, indexed.
fn indexed(fixture: fixtures::Fixture) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture {
        write(tmp.path(), rel, source);
    }
    let engine = index(&tmp);
    (tmp, engine)
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
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

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ── PHP ──────────────────────────────────────────────────────────────────────

/// FR-RS-13 AC (the monolog M1 shape): `use Monolog\Handler\HandlerInterface;`
/// binds the declared interface; `use Psr\Log\LoggerInterface;` stays unbound.
#[test]
fn a_php_use_binds_the_declared_type_and_a_psr_import_stays_unbound() {
    let (_tmp, engine) = indexed(fixtures::PHP);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/Monolog/Logger.php", EdgeKind::Imports),
        strings(&["src/Monolog/Handler/HandlerInterface.php:HandlerInterface:interface"])
    );
    assert_eq!(
        unbound_imports(rt, "src/Monolog/Logger.php"),
        strings(&["Psr::Log::LoggerInterface"])
    );
    // A namespace declared outside its PSR-4 directory still names its type.
    assert_eq!(
        edges_from(rt, "src/Monolog/Handler/StreamHandler.php", EdgeKind::Imports),
        strings(&["lib/legacy/formatters.php:LineFormatter:class"])
    );
}

// ── C# ───────────────────────────────────────────────────────────────────────

const ORDERS_API: &str = "src/Ordering.API/Api/OrdersApi.cs";
const GLOBAL_USINGS: &str = "src/Ordering.API/GlobalUsings.cs";

/// FR-RS-13 AC: a `using` is a namespace wildcard — it binds to every file
/// declaring the namespace, file-scoped and block alike, whatever the
/// directory — and an external namespace stays unbound.
#[test]
fn a_csharp_using_binds_to_every_file_declaring_the_namespace() {
    let (_tmp, engine) = indexed(fixtures::C_SHARP);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, GLOBAL_USINGS, EdgeKind::Imports),
        strings(&[
            "src/Ordering.Domain/Order.cs:Order:module",
            "src/Shared/OrderItem.cs:OrderItem:module",
        ]),
        "a file-scoped and a block `eShop.Ordering.Domain` are one namespace; \
         `src/Shared/` does not name it, its declaration does"
    );
    assert_eq!(unbound_imports(rt, GLOBAL_USINGS), strings(&["System"]));
}

/// `using static T;` binds the type whose members it brings in, and an alias
/// `using X = T;` the type it names; a framework namespace stays unbound.
#[test]
fn a_csharp_using_static_and_an_alias_bind_the_type_they_name() {
    let (_tmp, engine) = indexed(fixtures::C_SHARP);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, ORDERS_API, EdgeKind::Imports),
        strings(&[
            "src/Shared/Guard.cs:Guard:class",
            "src/Shared/OrderItem.cs:OrderItem:class",
        ])
    );
    assert_eq!(
        unbound_imports(rt, ORDERS_API),
        strings(&["Microsoft::AspNetCore::Mvc"])
    );
}

/// The ledger records each directive's shape: a wildcard row for a plain and a
/// global `using`, the global one marked so; a single-type row for an alias.
#[test]
fn each_csharp_using_form_records_its_own_ledger_shape() {
    let (_tmp, engine) = indexed(fixtures::C_SHARP);
    let rt = engine.runtime().unwrap();
    let mut rows: Vec<(String, Option<String>, RefForm)> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Imports)
                .map(|r| (r.target, r.alias, r.form))
                .collect())
        })
        .unwrap();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        rows,
        vec![
            ("Microsoft::AspNetCore::Mvc".to_string(), None, RefForm::Glob),
            ("System".to_string(), Some("global".to_string()), RefForm::Glob),
            ("eShop::Ordering::Domain".to_string(), Some("global".to_string()), RefForm::Glob),
            (
                "eShop::Ordering::Domain::OrderItem".to_string(),
                Some("OrderItem".to_string()),
                RefForm::Path
            ),
            ("eShop::Shared::Guard".to_string(), Some("*".to_string()), RefForm::Glob),
        ],
        "one row per directive — an alias no longer records its own name as an import"
    );
}

// ── Kotlin ───────────────────────────────────────────────────────────────────

/// FR-RS-13 AC: a Kotlin file outside `src/main/kotlin` (`commonMain`,
/// `jvmMain`) is keyed by its `package` header, and its imports bind.
#[test]
fn a_kotlin_multiplatform_import_binds_through_the_package_header() {
    let (_tmp, engine) = indexed(fixtures::KOTLIN);
    let rt = engine.runtime().unwrap();
    let koin = "core/src/commonMain/kotlin/org/koin/core/Koin.kt";
    assert_eq!(
        edges_from(rt, koin, EdgeKind::Imports),
        strings(&["core/src/commonMain/kotlin/org/koin/core/module/Module.kt:Module:class"])
    );
    // A top-level function and a library type stay unbound: a function is not
    // a declared type (KOTLIN-G1's own story), and the library is not here.
    assert_eq!(
        unbound_imports(rt, koin),
        strings(&["kotlinx::coroutines::CoroutineScope", "org::koin::core::module::module"])
    );
    // `import org.koin.core.module.*` names the package's files.
    assert_eq!(
        edges_from(rt, "core/src/jvmMain/kotlin/org/koin/core/KoinPlatform.kt", EdgeKind::Imports),
        strings(&["core/src/commonMain/kotlin/org/koin/core/module/Module.kt:Module:module"])
    );
}

// ── Scala ────────────────────────────────────────────────────────────────────

/// Scala's imports — which no single node spans — bind through the package
/// each file declares, chained clauses composed (`package cats` then `package
/// data` is `cats.data`).
#[test]
fn scala_imports_bind_through_the_declared_package() {
    let (_tmp, engine) = indexed(fixtures::SCALA);
    let rt = engine.runtime().unwrap();
    let main = "app/src/main/scala/app/Main.scala";
    let chain = "core/src/main/scala/cats/data/Chain.scala";
    assert_eq!(
        edges_from(rt, main, EdgeKind::Imports),
        strings(&[
            // `import cats.Show` of the comma-separated pair.
            "core/src/main/scala/cats/Show.scala:Show:trait",
            // `import cats.data._` names the package's one file.
            &format!("{chain}:Chain:module"),
            // `import cats.data.Chain`: the class and its companion object
            // share the name — ambiguous, so neither; `cats.data.Chain.empty`
            // walks into that ambiguity too. `Validated` from the group binds.
            &format!("{chain}:Validated:trait"),
        ])
    );
    assert_eq!(
        unbound_imports(rt, main),
        strings(&[
            "cats::data::Chain",
            "cats::data::Chain::empty",
            "cats::data::Missing",
            "scala::util::Try",
        ])
    );
}

// ── Java: unchanged ──────────────────────────────────────────────────────────

/// Every edge of the graph whose source file is under `prefix`, id-free.
fn edges_under(rt: &Runtime, prefix: &str) -> Vec<String> {
    let prefix = prefix.to_string();
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}:{}", n.file_path.unwrap_or_default(), n.name, n.kind.as_str())))
            .collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| label[&e.source].starts_with(&prefix))
            .map(|e| format!("{} -{}-> {}", label[&e.source], e.kind.as_str(), label[&e.target]))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// FR-RS-13 AC: Java's package model and every Java bound edge are unchanged.
/// The Java fixture's edges are pinned, and identical with every
/// declared-namespace language's fixture indexed beside it — including a Kotlin
/// type of the very package the Java file imports on demand, whose own key now
/// comes from its header.
#[test]
fn java_edges_are_identical_with_the_namespace_languages_beside_them() {
    let (_tmp, alone) = indexed(fixtures::JAVA);
    let java_alone = edges_under(alone.runtime().unwrap(), "svc/");
    let web_alone = edges_under(alone.runtime().unwrap(), "web/");
    let svc = "svc/src/main/java/com/x/svc";
    let ctl = "web/src/main/java/com/x/web/Ctl.java";
    let pinned: Vec<String> = [
        format!("{ctl}:Ctl:class -contains-> {ctl}:a:method"),
        format!("{ctl}:Ctl:class -contains-> {ctl}:b:method"),
        format!("{ctl}:Ctl:class -contains-> {ctl}:svc:field"),
        format!("{ctl}:Ctl:class -extends-> {svc}/Base.java:Base:class"),
        format!("{ctl}:Ctl:module -contains-> {ctl}:Ctl:class"),
        format!("{ctl}:Ctl:module -imports-> {svc}/Svc.java:Svc:class"),
        format!("{ctl}:Ctl:module -imports-> {svc}/Svc.java:helper:method"),
        format!("{ctl}:a:method -calls-> {svc}/Svc.java:helper:method"),
        format!("{ctl}:b:method -calls-> {svc}/Svc.java:util:method"),
        format!("{ctl}:svc:field -type_uses-> {svc}/Svc.java:Svc:class"),
    ]
    .into_iter()
    .collect();
    assert_eq!(web_alone, pinned, "the Java fixture's edges, pinned");

    let mut mixed: Vec<(&str, &str)> = fixtures::JAVA.to_vec();
    for fixture in [fixtures::PHP, fixtures::C_SHARP, fixtures::KOTLIN, fixtures::SCALA] {
        mixed.extend_from_slice(fixture);
    }
    mixed.push((
        "svc/src/main/kotlin/com/x/svc/Extra.kt",
        "package com.x.svc\n\nclass Extra\n",
    ));
    let mixed: &'static [(&'static str, &'static str)] = Box::leak(mixed.into_boxed_slice());
    let (_tmp, together) = indexed(mixed);
    let rt = together.runtime().unwrap();
    assert_eq!(edges_under(rt, "web/"), web_alone, "Java's edges are byte-identical");
    assert_eq!(
        edges_under(rt, "svc/src/main/java/"),
        java_alone,
        "Java's edges are byte-identical"
    );
}

// ── sync ≡ reindex ───────────────────────────────────────────────────────────

/// Every binding fact of the graph in an id-free form, capture-before-delete
/// rows excluded — `java_imports.rs`'s comparison (NFR-RA-06).
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

/// Index a fresh copy of `tmp`'s `files` and return its binding facts.
fn cold_facts(tmp: &TempDir, files: &[&str]) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in files {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(&cold);
    binding_facts(engine.runtime().unwrap())
}

fn paths(fixture: fixtures::Fixture) -> Vec<&'static str> {
    fixture.iter().map(|(rel, _)| *rel).collect()
}

/// A file that changes the namespace it declares moves every import of its
/// types — no node is renamed, so only the recorded namespace can carry it.
#[test]
fn sync_equals_a_full_reindex_after_a_file_changes_its_namespace() {
    let (tmp, engine) = indexed(fixtures::PHP);
    let rt = engine.runtime().unwrap();
    let formatter = "lib/legacy/formatters.php";
    write(
        tmp.path(),
        formatter,
        "<?php\n\nnamespace Monolog\\Legacy;\n\nclass LineFormatter\n{\n}\n",
    );
    engine.sync(&[formatter.into()]);
    assert!(
        edges_from(rt, "src/Monolog/Handler/StreamHandler.php", EdgeKind::Imports).is_empty(),
        "the import names a namespace the type left"
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::PHP)));
}

/// A `using` wildcard binds to the files declaring its namespace, so a file
/// leaving that namespace leaves its targets and one joining it joins them —
/// under a directory that names neither namespace, only the namespace's own
/// segments select the row. (Leaving alone would pass without them: the
/// re-extract deletes the file's module node, and the edge with it. Joining
/// would not: nothing else re-binds the wildcard.)
#[test]
fn sync_equals_a_full_reindex_after_a_file_leaves_and_joins_a_wildcards_namespace() {
    let (tmp, engine) = indexed(fixtures::C_SHARP);
    let rt = engine.runtime().unwrap();
    let item = "src/Shared/OrderItem.cs";
    write(
        tmp.path(),
        item,
        "namespace eShop.Catalog\n{\n    public class OrderItem\n    {\n        public int Units;\n    }\n}\n",
    );
    engine.sync(&[item.into()]);
    assert_eq!(
        edges_from(rt, GLOBAL_USINGS, EdgeKind::Imports),
        strings(&["src/Ordering.Domain/Order.cs:Order:module"])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::C_SHARP)));

    let guard = "src/Shared/Guard.cs";
    write(
        tmp.path(),
        guard,
        "namespace eShop.Ordering.Domain\n{\n    public static class Guard\n    {\n        public static void NotNull(object o) { }\n    }\n}\n",
    );
    engine.sync(&[guard.into()]);
    assert_eq!(
        edges_from(rt, GLOBAL_USINGS, EdgeKind::Imports),
        strings(&[
            "src/Ordering.Domain/Order.cs:Order:module",
            "src/Shared/Guard.cs:Guard:module",
        ])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::C_SHARP)));
}

/// A `global using` added or removed moves names in files the change never
/// touches, under names no changed node spells.
#[test]
fn sync_equals_a_full_reindex_after_a_global_using_changes() {
    let (tmp, engine) = indexed(fixtures::C_SHARP);
    let rt = engine.runtime().unwrap();
    write(tmp.path(), GLOBAL_USINGS, "global using System;\n");
    engine.sync(&[GLOBAL_USINGS.into()]);
    assert!(edges_from(rt, GLOBAL_USINGS, EdgeKind::Imports).is_empty());
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::C_SHARP)));

    write(tmp.path(), GLOBAL_USINGS, "global using eShop.Shared;\n");
    engine.sync(&[GLOBAL_USINGS.into()]);
    assert_eq!(
        edges_from(rt, GLOBAL_USINGS, EdgeKind::Imports),
        strings(&["src/Shared/Guard.cs:Guard:module"])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::C_SHARP)));
}

/// A file arriving on sync binds the imports already waiting for it, and its
/// deletion unbinds them, exactly as a cold index would.
#[test]
fn sync_equals_a_full_reindex_after_adding_and_deleting_a_namespaced_file() {
    let koin = paths(fixtures::KOTLIN);
    let module = koin[0];
    let tmp = TempDir::new().unwrap();
    for (rel, source) in &fixtures::KOTLIN[1..] {
        write(tmp.path(), rel, source);
    }
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    write(tmp.path(), module, fixtures::KOTLIN[0].1);
    engine.sync(&[module.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &koin));
    fs::remove_file(tmp.path().join(module)).unwrap();
    engine.sync(&[module.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &koin));
}

/// A config narrowing that purges the only file declaring a namespace unbinds
/// the `using` wildcards that bound to it, as a cold index under the narrowed
/// config would (FR-SY-07, NFR-RA-06): their rows spell the namespace, never a
/// node name of the purged file.
#[test]
fn a_purge_unbinds_a_wildcard_whose_namespace_file_left() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/Other/Impl.cs", "namespace Lib.Core\n{\n    public class Thing { }\n}\n");
    let user = "src/App/U.cs";
    write(tmp.path(), user, "using Lib.Core;\n\nnamespace App\n{\n    public class U { }\n}\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, user, EdgeKind::Imports), strings(&["src/Other/Impl.cs:Impl:module"]));

    engine
        .config_write(logos_core::config::PolicyFile::Config, "exclude = [\"src/Other/**\"]\n")
        .expect("a valid config write succeeds");
    engine
        .config_apply(logos_core::config::PolicyFile::Config)
        .expect("config apply runs");
    assert!(edges_from(rt, user, EdgeKind::Imports).is_empty());
    assert_eq!(
        unbound_imports(rt, user),
        strings(&["Lib::Core"]),
        "the wildcard returns to the ledger unresolved, as a cold index leaves it"
    );
}
