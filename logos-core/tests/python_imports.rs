//! Python files resolve as **path modules under import roots** (S-519,
//! [FR-RS-14]) — exercised end-to-end through the public [`Engine`] against real
//! temp-directory fixtures (`python_imports/fixtures.rs`).
//!
//! The python descriptor declares `[module_model] kind = "path"` with
//! `package_stems = ["__init__"]` and `import_roots = ["src"]`; the rust one
//! declares its `mod`/`lib`/`main` stems the same way. The binder rules are
//! pinned in memory by `resolve::path_module_tests`; this suite pins what
//! reaches the product: werkzeug's relative imports and `__init__` re-exports
//! bind, healthchecks' absolute imports bind from the repository root, the
//! `.logos/config.toml` override replaces the detected roots, a JavaScript
//! `main.js` is the module `main`, the interop family keeps Java↔Kotlin binding
//! and C#↛PHP apart, and a one-file sync equals a full reindex ([NFR-RA-06]).
//!
//! [FR-RS-14]: ../../docs/specs/requirements/FR-RS-14.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(all(
    feature = "lang-python",
    feature = "lang-rust",
    feature = "lang-typescript",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-c-sharp",
    feature = "lang-php"
))]

#[path = "python_imports/fixtures.rs"]
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

/// A temp tree holding `fixture` (plus `extra` files), indexed.
fn indexed_with(fixture: fixtures::Fixture, extra: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture.iter().chain(extra) {
        write(tmp.path(), rel, source);
    }
    let engine = index(&tmp);
    (tmp, engine)
}

fn indexed(fixture: fixtures::Fixture) -> (TempDir, Engine) {
    indexed_with(fixture, &[])
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

/// The name of the file module of `rel`.
fn module_name(rt: &Runtime, rel: &str) -> String {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| n.kind == NodeKind::Module && n.file_path.as_deref() == Some(rel.as_str()))
            .map(|n| n.name)
            .expect("file module"))
    })
    .expect("read runs")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ── werkzeug: a `src/` package ───────────────────────────────────────────────

/// FR-RS-14 AC: `from .rules import Rule` and `from .._internal import
/// _wsgi_decoding_dance` bind to `src/werkzeug/routing/rules.py` and
/// `src/werkzeug/_internal.py`; `from . import rules` names the module; an
/// external `import re` stays unbound.
#[test]
fn werkzeugs_relative_imports_bind_one_and_two_levels_up() {
    let (_tmp, engine) = indexed(fixtures::WERKZEUG);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/werkzeug/routing/map.py", EdgeKind::Imports),
        strings(&[
            "src/werkzeug/routing/rules.py:Rule:class",
            "src/werkzeug/routing/rules.py:rules:module",
        ])
    );
    assert_eq!(
        edges_from(rt, "src/werkzeug/routing/rules.py", EdgeKind::Imports),
        strings(&["src/werkzeug/_internal.py:_wsgi_decoding_dance:function"])
    );
    assert_eq!(unbound_imports(rt, "src/werkzeug/routing/rules.py"), strings(&["re"]));
    // The call through the imported name binds across files.
    assert_eq!(
        edges_from(rt, "src/werkzeug/routing/rules.py", EdgeKind::Calls),
        strings(&["src/werkzeug/_internal.py:_wsgi_decoding_dance:function"])
    );
}

/// FR-RS-14 AC: an import through a package's `__init__.py` binds to that
/// package — `from werkzeug import routing` is the package, and `from
/// werkzeug.routing import Map`, which `routing/__init__.py` re-exports without
/// declaring, goes through it. A test outside the `src/` root imports the
/// package by name, as an installed package is imported.
#[test]
fn an_import_through_init_binds_to_its_package() {
    let (_tmp, engine) = indexed(fixtures::WERKZEUG);
    let rt = engine.runtime().unwrap();
    // `routing` and `Map` both reach the package: one edge, two bound rows.
    assert_eq!(
        edges_from(rt, "tests/test_routing.py", EdgeKind::Imports),
        strings(&[
            "src/werkzeug/routing/__init__.py:routing:module",
            "src/werkzeug/routing/rules.py:Rule:class",
        ])
    );
    assert_eq!(unbound_imports(rt, "tests/test_routing.py"), strings(&["pytest"]));
    // A package file is named by its directory.
    assert_eq!(module_name(rt, "src/werkzeug/routing/__init__.py"), "routing");
    // The re-exports inside `__init__.py` bind to the submodules' declarations.
    assert_eq!(
        edges_from(rt, "src/werkzeug/routing/__init__.py", EdgeKind::Imports),
        strings(&[
            "src/werkzeug/routing/map.py:Map:class",
            "src/werkzeug/routing/rules.py:Rule:class",
        ])
    );
}

/// An `as` import binds the imported declaration but gives the file no alias
/// of the imported name: after `from .helpers import open as open_resource`, a
/// call to the builtin `open` is not a call into `helpers` (never fabricate).
#[test]
fn a_renamed_import_never_binds_a_call_to_the_original_name() {
    let (_tmp, engine) = indexed(&[
        ("pkg/__init__.py", ""),
        ("pkg/helpers.py", "def open(p):\n    return p\n"),
        (
            "pkg/app.py",
            "from .helpers import open as open_resource\n\n\ndef run(p):\n    return open(p)\n",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "pkg/app.py", EdgeKind::Imports), strings(&["pkg/helpers.py:open:function"]));
    assert!(edges_from(rt, "pkg/app.py", EdgeKind::Calls).is_empty());
}

// ── healthchecks: the repository root ────────────────────────────────────────

/// FR-RS-14 AC: `from hc.api.models import Check` binds — one row per imported
/// name — from the repository root, which is the import root because no
/// package sits under `src/`; a namespace package (`hc/lib/`, no
/// `__init__.py`) still descends; `django` stays unbound.
#[test]
fn healthchecks_absolute_imports_bind_from_the_repository_root() {
    let (_tmp, engine) = indexed(fixtures::HEALTHCHECKS);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "hc/front/views.py", EdgeKind::Imports),
        strings(&[
            "hc/api/models.py:Check:class",
            "hc/api/models.py:Flip:class",
            "hc/lib/date.py:format_duration:function",
        ])
    );
    assert_eq!(unbound_imports(rt, "hc/api/models.py"), strings(&["django::db::models"]));
    assert_eq!(
        edges_from(rt, "hc/front/views.py", EdgeKind::Calls),
        strings(&["hc/lib/date.py:format_duration:function"])
    );
}

// ── the config override ──────────────────────────────────────────────────────

/// The repository root replacing werkzeug's detected `src/`, under the strict
/// policy so that only the module tree — never the suffix fallback — answers.
const OVERRIDE_SRC_TO_ROOT: &str =
    "[resolution]\npolicy = \"strict\"\n\n[resolution.import_roots]\npython = [\".\"]\n";

/// FR-RS-14 AC: `.logos/config.toml` replaces the detected roots. Declaring the
/// repository root for werkzeug moves `src/werkzeug` to the module
/// `src.werkzeug`: the by-name imports of the test no longer descend to it,
/// while the relative imports inside the package, which never spell a root,
/// still bind.
#[test]
fn the_config_override_replaces_the_detected_import_roots() {
    let (_tmp, engine) =
        indexed_with(fixtures::WERKZEUG, &[(".logos/config.toml", OVERRIDE_SRC_TO_ROOT)]);
    let rt = engine.runtime().unwrap();
    assert!(
        edges_from(rt, "tests/test_routing.py", EdgeKind::Imports).is_empty(),
        "`werkzeug` is no module once the repository root replaces `src/`"
    );
    assert_eq!(
        edges_from(rt, "src/werkzeug/routing/rules.py", EdgeKind::Imports),
        strings(&["src/werkzeug/_internal.py:_wsgi_decoding_dance:function"])
    );
    // …and a root the detection would never pick binds once declared.
    let (_tmp, engine) = indexed_with(
        &[
            ("lib/pkg/__init__.py", ""),
            ("lib/pkg/core.py", "def go():\n    pass\n"),
            ("app.py", "from pkg.core import go\n"),
        ],
        &[(".logos/config.toml", "[resolution.import_roots]\npython = [\"lib\"]\n")],
    );
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["lib/pkg/core.py:go:function"]));
}

/// A malformed override is a configuration error, not a root that silently
/// matches nothing.
#[test]
fn a_malformed_import_root_override_is_refused_at_load() {
    for bad in ["\"../x\"", "\"/abs\"", "\"a//b\"", "\"\""] {
        let text = format!("[resolution.import_roots]\npython = [{bad}]\n");
        let err = logos_core::config::parse_config(&text, Path::new(".logos/config.toml"))
            .expect_err(bad)
            .to_string();
        assert!(err.contains("resolution.import_roots.python"), "{bad}: {err}");
    }
}

// ── package-file stems ───────────────────────────────────────────────────────

/// FR-RS-14 AC: a JavaScript `main.js` is the module `main`; Rust's `main.rs`
/// and `mod.rs` still name their enclosing module, as the rust plugin declares.
#[test]
fn a_main_js_is_named_main_and_rusts_stems_still_fold() {
    let (_tmp, engine) = indexed(fixtures::MAIN_JS);
    let rt = engine.runtime().unwrap();
    assert_eq!(module_name(rt, "web/src/main.js"), "main");
    assert_eq!(module_name(rt, "cli/src/main.rs"), "cli");
    assert_eq!(module_name(rt, "cli/src/cmd/mod.rs"), "cmd");
}

// ── interop families ─────────────────────────────────────────────────────────

/// The `jvm` family keeps Java↔Kotlin binding (koin's `UnitJavaTest extends
/// KoinCoreTest`), and a C# `using App.Models;` never binds a PHP file of that
/// namespace — the two are different families.
#[test]
fn a_family_keeps_jvm_binding_and_keeps_csharp_off_php() {
    let (_tmp, engine) = indexed(fixtures::FAMILIES);
    let rt = engine.runtime().unwrap();
    let java = "core/src/jvmTest/java/org/koin/java/UnitJavaTest.java";
    let kotlin = "core/src/jvmTest/kotlin/org/koin/test/KoinCoreTest.kt:KoinCoreTest:class";
    assert_eq!(edges_from(rt, java, EdgeKind::Imports), strings(&[kotlin]));
    assert_eq!(edges_from(rt, java, EdgeKind::Extends), strings(&[kotlin]));
    assert!(edges_from(rt, "src/Api/OrdersApi.cs", EdgeKind::Imports).is_empty());
    assert_eq!(unbound_imports(rt, "src/Api/OrdersApi.cs"), strings(&["App::Models"]));
}

// ── sync ≡ reindex ───────────────────────────────────────────────────────────

/// Every binding fact of the graph in an id-free form, capture-before-delete
/// rows excluded — `namespace_imports.rs`'s comparison (NFR-RA-06).
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

/// A module that loses the class a relative import names unbinds it on sync,
/// and regaining it binds it again — exactly as a cold index would.
#[test]
fn sync_equals_a_full_reindex_after_a_relative_target_changes() {
    let (tmp, engine) = indexed(fixtures::WERKZEUG);
    let rt = engine.runtime().unwrap();
    let rules = "src/werkzeug/routing/rules.py";
    write(tmp.path(), rules, "class Other:\n    pass\n");
    engine.sync(&[rules.into()]);
    assert!(
        !edges_from(rt, "src/werkzeug/routing/map.py", EdgeKind::Imports)
            .contains(&"src/werkzeug/routing/rules.py:Rule:class".to_string())
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::WERKZEUG)));
    write(tmp.path(), rules, fixtures::WERKZEUG[4].1);
    engine.sync(&[rules.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::WERKZEUG)));
}

/// A package file arriving under `src/` makes `src/` the import root, which
/// re-keys every Python file — including the ones the sync never touched, under
/// names no dirty token spells — and removing it moves the root back. The
/// package that arrives (`src/zed/`) shares no token with the untouched row
/// (`app.core.go`), so only the import-root re-selection can move it; the strict
/// policy keeps the suffix fallback from answering in either layout.
#[test]
fn sync_equals_a_full_reindex_when_a_package_file_moves_the_import_root() {
    let files: &[(&str, &str)] = &[
        (".logos/config.toml", "[resolution]\npolicy = \"strict\"\n"),
        ("src/app/core.py", "def go():\n    pass\n"),
        ("src/app/cli.py", "from app.core import go\n"),
    ];
    let (tmp, engine) = indexed(files);
    let rt = engine.runtime().unwrap();
    // No package under `src/` yet: the repository root is the import root, and
    // `app` is no module there.
    assert_eq!(unbound_imports(rt, "src/app/cli.py"), strings(&["app::core::go"]));
    let init = "src/zed/__init__.py";
    write(tmp.path(), init, "");
    engine.sync(&[init.into()]);
    // `src/` holds a package now: `app.core` is a module under it.
    assert!(unbound_imports(rt, "src/app/cli.py").is_empty());
    let mut all = paths(files);
    all.push(init);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &all));
    fs::remove_file(tmp.path().join(init)).unwrap();
    engine.sync(&[init.into()]);
    assert_eq!(unbound_imports(rt, "src/app/cli.py"), strings(&["app::core::go"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(files)));
}

/// A module and its stub (`foo.py`, `foo.pyi`) share one module key; which one
/// answers must not depend on node ids, which a re-extraction renews — the
/// path decides, so a sync of the module agrees with a cold index.
#[test]
fn sync_equals_a_full_reindex_with_a_module_beside_its_stub() {
    let files: &[(&str, &str)] = &[
        ("pkg/__init__.py", ""),
        ("pkg/foo.py", "class X:\n    pass\n"),
        ("pkg/foo.pyi", "class X: ...\n"),
        ("app.py", "from pkg.foo import X\n"),
    ];
    let (tmp, engine) = indexed(files);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["pkg/foo.py:X:class"]));
    write(tmp.path(), "pkg/foo.py", "class X:\n    y = 1\n");
    engine.sync(&["pkg/foo.py".into()]);
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["pkg/foo.py:X:class"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(files)));
}

/// `from pkg import helper` goes through `pkg/__init__.py` until a submodule
/// `pkg/helper.py` arrives and takes the name: the import's old edge to the
/// package must not outlive the binding, although neither the importing file
/// nor `__init__.py` changed.
#[test]
fn sync_equals_a_full_reindex_when_a_submodule_takes_over_a_reexported_name() {
    let files: &[(&str, &str)] = &[
        ("pkg/__init__.py", ""),
        ("pkg/core.py", "def go():\n    pass\n"),
        ("app.py", "from pkg import helper\n"),
        // Bound beforehand through the same package, and kept: the sweep
        // re-binds every row of a swept file, not only the moved one.
        ("tool.py", "from pkg import core\nfrom pkg import helper\n"),
    ];
    let (tmp, engine) = indexed(files);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["pkg/__init__.py:pkg:module"]));
    let helper = "pkg/helper.py";
    write(tmp.path(), helper, "def h():\n    pass\n");
    engine.sync(&[helper.into()]);
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["pkg/helper.py:helper:module"]));
    assert_eq!(
        edges_from(rt, "tool.py", EdgeKind::Imports),
        strings(&["pkg/core.py:core:module", "pkg/helper.py:helper:module"])
    );
    let mut all = paths(files);
    all.push(helper);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &all));
}
