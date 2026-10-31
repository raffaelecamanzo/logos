//! Python files resolve as **path modules under import roots** (S-519,
//! [FR-RS-14]) — exercised end-to-end through the public [`Engine`] against real
//! temp-directory fixtures (`python_imports/fixtures.rs`).
//!
//! The python descriptor declares `[module_model] kind = "path"` with
//! `package_stems = ["__init__"]` and `import_roots = ["src"]`; the rust one
//! declares its `mod`/`lib`/`main` stems the same way. The binder rules are
//! pinned in memory by `resolve::path_module_tests`; this suite pins what
//! reaches the product: werkzeug's relative imports and `__init__` re-exports
//! bind, an import repeated in every method binds as one import does,
//! healthchecks' absolute imports bind from the repository root, the
//! `.logos/config.toml` override replaces the detected roots, a JavaScript
//! `main.js` is the module `main`, the interop family keeps Java↔Kotlin binding
//! and C#↛PHP apart, two local names of one import target are two ledger rows
//! that both bind ([FR-DB-07]), a call through an imported module or class
//! binds that module's or class's member and an external one stays unbound
//! (S-544, [FR-RS-23]), Django routes to a shared handler name bind their own
//! integration's handler, and a one-file sync equals a full reindex
//! ([NFR-RA-06]).
//!
//! [FR-RS-14]: ../../docs/specs/requirements/FR-RS-14.md
//! [FR-RS-23]: ../../docs/specs/requirements/FR-RS-23.md
//! [FR-DB-07]: ../../docs/specs/requirements/FR-DB-07.md
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

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

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
    coordinate::pin(tmp.path());
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

/// S-520: an `as` import is aliased by the name it binds, so a call through the
/// alias resolves as the unaliased form does — `open_resource(p)` reaches
/// `helpers.open`. (The original name binding nothing is
/// `a_renamed_import_never_binds_a_call_to_the_original_name`.)
#[test]
fn a_call_through_an_import_alias_binds_the_imported_declaration() {
    let (_tmp, engine) = indexed(&[
        ("pkg/__init__.py", ""),
        ("pkg/helpers.py", "def open(p):\n    return p\n"),
        (
            "pkg/app.py",
            "from .helpers import open as open_resource\n\n\ndef run(p):\n    return open_resource(p)\n",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "pkg/app.py", EdgeKind::Calls),
        strings(&["pkg/helpers.py:open:function"])
    );
}

/// S-520: `import typing as t` binds the module under the name `t`; S-544
/// ([FR-RS-23]): `t.cast(...)` is then a call through that import, and it goes
/// exactly where the import goes. Here the workspace holds the `typing` module
/// the import binds, so the call reaches its `cast`. Without that module the
/// import binds nothing, and neither does the call
/// (`a_call_through_an_external_module_stays_unbound`).
///
/// [FR-RS-23]: ../../docs/specs/requirements/FR-RS-23.md
#[test]
fn a_call_through_a_module_alias_goes_where_its_import_goes() {
    let (_tmp, engine) = indexed(&[
        ("typing.py", "def cast(tp, value):\n    return value\n"),
        ("pkg/__init__.py", ""),
        (
            "pkg/app.py",
            "import typing as t\n\n\ndef run(v):\n    return t.cast(int, v)\n",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges_from(rt, "pkg/app.py", EdgeKind::Imports), strings(&["typing.py:typing:module"]));
    assert_eq!(edges_from(rt, "pkg/app.py", EdgeKind::Calls), strings(&["typing.py:cast:function"]));
}

// ── S-544: a call through an imported module or class ───────────────────────

/// A healthchecks-shaped tree (S-544, [FR-RS-23]): `hc/lib/curl.py`'s
/// functions, a model class with a class method beside a module function of
/// the same name, and two integrations whose URLconfs both route to a handler
/// named `add` — one through `from . import views`, one through an absolute
/// import.
///
/// [FR-RS-23]: ../../docs/specs/requirements/FR-RS-23.md
const QUALIFIED_CALLS: fixtures::Fixture = &[
    ("hc/__init__.py", ""),
    ("hc/lib/__init__.py", ""),
    ("hc/lib/curl.py", "def post(url):\n    return url\n\n\ndef get(url):\n    return url\n"),
    ("hc/api/__init__.py", ""),
    (
        "hc/api/models.py",
        "class Check:\n    @classmethod\n    def for_project(cls, p):\n        return p\n\n\ndef for_project(p):\n    return p\n",
    ),
    ("hc/integrations/__init__.py", ""),
    ("hc/integrations/pd/__init__.py", ""),
    ("hc/integrations/pd/views.py", "def add(request):\n    return request\n"),
    (
        "hc/integrations/pd/urls.py",
        "from django.urls import path\n\nfrom . import views\n\nurlpatterns = [path(\"add_pd/\", views.add)]\n",
    ),
    ("hc/integrations/sms/__init__.py", ""),
    ("hc/integrations/sms/views.py", "def add(request):\n    return request\n"),
    (
        "hc/integrations/sms/urls.py",
        "from django.urls import path\n\nfrom hc.integrations.sms import views\n\nurlpatterns = [path(\"add_sms/\", views.add)]\n",
    ),
    (
        "hc/integrations/pd/transport.py",
        "import os\n\nfrom hc.lib import curl\nfrom hc.api import models\nfrom hc.api.models import Check\n\n\n\
def notify(p):\n    curl.post(p)\n    models.Check()\n    Check.for_project(p)\n    os.getcwd()\n    return os.path.join(p)\n",
    ),
];

/// Every edge of `kind` out of the file `rel`, as `target file:line name`, sorted
/// — the line tells a method from a module function of its name.
fn edges_with_lines(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, (String, String)> = store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                let at = format!("{file}:{} {}", n.start_line.unwrap_or_default(), n.name);
                (n.id, (file, at))
            })
            .collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind && label[&e.source].0 == rel)
            .map(|e| label[&e.target].1.clone())
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// FR-RS-23 AC: `curl.post()` through `from hc.lib import curl` binds the
/// module's function, `models.Check()` constructs the module's class, and
/// `Check.for_project()` through a named-imported class binds among that class's
/// methods — never the module function of the same name beside it.
#[test]
fn a_call_through_an_imported_module_or_class_binds_its_member() {
    let (_tmp, engine) = indexed(QUALIFIED_CALLS);
    let rt = engine.runtime().unwrap();
    let transport = "hc/integrations/pd/transport.py";
    assert_eq!(
        edges_with_lines(rt, transport, EdgeKind::Calls),
        strings(&["hc/api/models.py:3 for_project", "hc/lib/curl.py:1 post"])
    );
    assert_eq!(edges_from(rt, transport, EdgeKind::Instantiates), strings(&["hc/api/models.py:Check:class"]));
}

/// FR-RS-23 AC: an external module call stays unbound — `os.getcwd()` through
/// an import no workspace module answers, and `os.path.join()`, whose receiver
/// is no imported name at all — under every policy, even beside a workspace
/// module that declares both names. The call reads its import's path as Python
/// does, so neither the balanced suffix match nor the aggressive unique-name
/// guess (which binds the import `os` itself to that module) reaches it.
#[test]
fn a_call_through_an_external_module_stays_unbound() {
    for policy in ["strict", "balanced", "aggressive"] {
        let config = format!("[resolution]\npolicy = \"{policy}\"\n");
        let (_tmp, engine) = indexed_with(
            QUALIFIED_CALLS,
            &[
                ("hc/compat/os.py", "def getcwd():\n    return ''\n\n\ndef join(p):\n    return p\n"),
                (".logos/config.toml", &config),
            ],
        );
        let rt = engine.runtime().unwrap();
        let transport = "hc/integrations/pd/transport.py";
        let calls = edges_from(rt, transport, EdgeKind::Calls);
        assert!(!calls.iter().any(|c| c.starts_with("hc/compat/os.py")), "{policy}: {calls:?}");
    }
}

/// S-544 review: `import json` beside `import myproj.json` binds the name `json`
/// to the standard library's module, so `json.dumps()` binds nothing — never
/// `myproj/json.py`'s `dumps`, which the file's alias table also keys `json`.
#[test]
fn a_call_through_a_stdlib_import_never_reaches_a_workspace_module_of_its_name() {
    let (_tmp, engine) = indexed(&[
        ("myproj/__init__.py", ""),
        ("myproj/json.py", "def dumps(x):\n    return x\n"),
        (
            "myproj/app.py",
            "import json\nimport myproj.json\n\n\ndef f(x):\n    return json.dumps(x)\n",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, "myproj/app.py", EdgeKind::Calls).is_empty());
}

/// FR-RS-23 AC: Django routes to a handler name two integrations share
/// (`views.add`) each bind to their own integration's handler, through a
/// relative and an absolute `views` import alike.
#[test]
fn a_django_route_binds_to_its_own_integrations_handler() {
    let (_tmp, engine) = indexed(QUALIFIED_CALLS);
    let rt = engine.runtime().unwrap();
    for integration in ["pd", "sms"] {
        assert_eq!(
            edges_from(rt, &format!("hc/integrations/{integration}/urls.py"), EdgeKind::RoutesTo),
            strings(&[&format!("hc/integrations/{integration}/views.py:add:function")])
        );
    }
}

/// S-519 T2: the same import repeated in every method (werkzeug's
/// `wrappers/request.py`) binds as one import does — each copy reaches the
/// package and the class, the external `warnings` stays unbound, and the calls
/// through the alias bind. Two methods import one class as `_Fallback` and two
/// import another under the same name: the file's scope holds two rivals
/// however often each is repeated, so each import binds its own class and a
/// call through the name binds neither. The work bound is
/// `resolve::path_module_tests`'s; this pins what the real extractor records.
#[test]
fn an_import_repeated_in_every_method_binds_as_one_import() {
    let (_tmp, engine) = indexed(fixtures::REPEATED_IMPORTS);
    let rt = engine.runtime().unwrap();
    let request = "src/werkzeug/wrappers/request.py";
    let copies = |n, edge: &str| vec![edge.to_string(); n];
    let mut imports = copies(4, "src/werkzeug/__init__.py:werkzeug:module");
    imports.extend(copies(2, "src/werkzeug/compat.py:BadRequest:class"));
    imports.extend(copies(4, "src/werkzeug/exceptions.py:BadRequest:class"));
    imports.extend(copies(2, "src/werkzeug/legacy.py:BadRequest:class"));
    assert_eq!(edges_from(rt, request, EdgeKind::Imports), imports);
    assert_eq!(
        edges_from(rt, request, EdgeKind::Instantiates),
        copies(4, "src/werkzeug/exceptions.py:BadRequest:class")
    );
    assert!(edges_from(rt, request, EdgeKind::Calls).is_empty());
    assert_eq!(unbound_imports(rt, request), copies(4, "warnings"));
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

/// The framework pass binds a Django route's handler through the same layout
/// as the resolution pass — the override included: under a declared `lib`
/// root, `views.index` in `lib/app/urls.py` reaches `lib/app/views.py`.
#[test]
fn the_framework_pass_binds_handlers_under_the_configured_import_roots() {
    let (_tmp, engine) = indexed_with(
        &[
            ("lib/app/__init__.py", ""),
            ("lib/app/views.py", "def index(request):\n    return None\n"),
            (
                "lib/app/urls.py",
                "from django.urls import path\n\nfrom app import views\n\nurlpatterns = [path(\"users/\", views.index)]\n",
            ),
        ],
        &[(".logos/config.toml", "[resolution]\npolicy = \"strict\"\n\n[resolution.import_roots]\npython = [\"lib\"]\n")],
    );
    let rt = engine.runtime().unwrap();
    let handlers: Vec<String> = rt
        .submit_read(|store| {
            let label: HashMap<NodeId, String> = store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
                .collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::RoutesTo)
                .map(|e| label[&e.target].clone())
                .collect())
        })
        .expect("read runs");
    assert_eq!(handlers, strings(&["lib/app/views.py:index"]));
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
/// rows included — `namespace_imports.rs`'s comparison (NFR-RA-06), so a
/// capture a sync leaves behind fails it (CR-187).
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
            .map(|r| {
                format!(
                    "{} {} {:?} {:?} {:?} {}",
                    r.source_symbol, r.target, r.alias, r.form, r.kind, r.resolved
                )
            })
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
        // Another language's edges are no import-root edges: the retraction
        // the moved root triggers must leave them alone.
        ("tool/src/lib.rs", "pub fn run() {\n    helper();\n}\n\nfn helper() {}\n"),
    ];
    let (tmp, engine) = indexed(files);
    let rt = engine.runtime().unwrap();
    let rust_call = strings(&["tool/src/lib.rs:helper:function"]);
    assert_eq!(edges_from(rt, "tool/src/lib.rs", EdgeKind::Calls), rust_call);
    // No package under `src/` yet: the repository root is the import root, and
    // `app` is no module there.
    assert_eq!(unbound_imports(rt, "src/app/cli.py"), strings(&["app::core::go"]));
    let init = "src/zed/__init__.py";
    write(tmp.path(), init, "");
    engine.sync(&[init.into()]);
    // `src/` holds a package now: `app.core` is a module under it.
    assert!(unbound_imports(rt, "src/app/cli.py").is_empty());
    assert_eq!(edges_from(rt, "tool/src/lib.rs", EdgeKind::Calls), rust_call);
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
        // re-binds every row of the moved row's source (here the module),
        // not only the moved one.
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

// ── S-597 / FR-DB-07: the ledger identity includes the alias ─────────────────

/// The `Imports` ledger rows out of `rel` as `(target, alias, resolved)`, sorted.
fn import_rows(rt: &Runtime, rel: &str) -> Vec<(String, Option<String>, bool)> {
    let rel = rel.to_string();
    let mut rows: Vec<(String, Option<String>, bool)> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| {
                    r.kind == EdgeKind::Imports && r.file_id.and_then(|id| files.get(&id)) == Some(&rel)
                })
                .map(|r| (r.target, r.alias, r.resolved))
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows
}

const ALIAS_TWINS: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    ("pkg/m.py", "class X:\n    pass\n"),
    (
        "app.py",
        "from pkg.m import X as A\nfrom pkg.m import X as B\nimport numpy\nimport numpy as np\n\n\n\
         def run():\n    return A(), B()\n",
    ),
];

/// `from pkg.m import X as A` and `… as B` in one scope are two ledger rows and
/// both bind; `import numpy` beside `import numpy as np` keeps both rows (the
/// external module binds nothing, so both stay unresolved). Before migration 31
/// the second name of each pair was dropped at insert.
#[test]
fn two_local_names_of_one_import_target_both_persist_and_both_bind() {
    let (_tmp, engine) = indexed(ALIAS_TWINS);
    let rt = engine.runtime().unwrap();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        import_rows(rt, "app.py"),
        vec![
            ("numpy".to_string(), some("np"), false),
            ("numpy".to_string(), some("numpy"), false),
            ("pkg::m::X".to_string(), some("A"), true),
            ("pkg::m::X".to_string(), some("B"), true),
        ]
    );
    // Two bound rows, one edge: an edge is `(source, target, kind)`, so the twins
    // share it — the second name is a binding in the ledger, not a second edge.
    assert_eq!(edges_from(rt, "app.py", EdgeKind::Imports), strings(&["pkg/m.py:X:class"]));
}

/// A one-file sync of the importer equals a cold index: the alias twins that the
/// old identity dropped are re-created identically on the incremental path.
#[test]
fn sync_equals_a_full_reindex_over_alias_twins() {
    let (tmp, engine) = indexed(ALIAS_TWINS);
    let rt = engine.runtime().unwrap();
    write(
        tmp.path(),
        "app.py",
        "from pkg.m import X as A\nfrom pkg.m import X as B\nfrom pkg.m import X as C\n",
    );
    engine.sync(&["app.py".into()]);
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        import_rows(rt, "app.py"),
        vec![
            ("pkg::m::X".to_string(), some("A"), true),
            ("pkg::m::X".to_string(), some("B"), true),
            ("pkg::m::X".to_string(), some("C"), true),
        ]
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(ALIAS_TWINS)));
}

/// S-544: a call through an imported module or class re-binds on a sync of the
/// module it names — a renamed function, a renamed method, a method moved out
/// of its class to the module beside it, a route handler renamed — exactly as a
/// cold index binds it, and binds again when the module is restored.
#[test]
fn sync_equals_a_full_reindex_when_a_qualified_calls_target_changes() {
    let (tmp, engine) = indexed(QUALIFIED_CALLS);
    let rt = engine.runtime().unwrap();
    let transport = "hc/integrations/pd/transport.py";
    let files = paths(QUALIFIED_CALLS);
    let original = |rel: &str| QUALIFIED_CALLS.iter().find(|(path, _)| *path == rel).expect("fixture file").1;
    for (rel, text) in [
        ("hc/lib/curl.py", "def post2(url):\n    return url\n"),
        ("hc/api/models.py", "class Check:\n    pass\n\n\ndef for_project(p):\n    return p\n"),
    ] {
        write(tmp.path(), rel, text);
        engine.sync(&[rel.into()]);
        assert_eq!(binding_facts(rt), cold_facts(&tmp, &files), "{rel}");
    }
    assert!(edges_from(rt, transport, EdgeKind::Calls).is_empty());
    for rel in ["hc/lib/curl.py", "hc/api/models.py"] {
        write(tmp.path(), rel, original(rel));
        engine.sync(&[rel.into()]);
        assert_eq!(binding_facts(rt), cold_facts(&tmp, &files), "{rel} restored");
    }
    assert_eq!(edges_from(rt, transport, EdgeKind::Calls).len(), 2);
    // A route handler renamed and restored: the edges agree with a cold index.
    // (The ledger does not yet: the framework pass keeps the deleted handler's
    // capture-before-delete `RoutesTo` row, as it does before S-544.)
    let (views, urls) = ("hc/integrations/pd/views.py", "hc/integrations/pd/urls.py");
    write(tmp.path(), views, "def add2(request):\n    return request\n");
    engine.sync(&[views.into()]);
    assert_eq!(binding_facts(rt).0, cold_facts(&tmp, &files).0, "{views}");
    assert!(edges_from(rt, urls, EdgeKind::RoutesTo).is_empty());
    write(tmp.path(), views, original(views));
    engine.sync(&[views.into()]);
    assert_eq!(binding_facts(rt).0, cold_facts(&tmp, &files).0, "{views} restored");
    assert_eq!(edges_from(rt, urls, EdgeKind::RoutesTo), strings(&["hc/integrations/pd/views.py:add:function"]));
}

// ── S-545: a bare decorator is a reference ──────────────────────────────────

/// A healthchecks-shaped tree (S-545, [FR-RS-23]): `hc/api/decorators.py`'s
/// `authorize` is applied bare, by a named import and through the module, beside
/// the builtin decorators, a decorator no import names and an attribute the
/// module does not declare.
///
/// [FR-RS-23]: ../../docs/specs/requirements/FR-RS-23.md
const DECORATORS: fixtures::Fixture = &[
    ("hc/__init__.py", ""),
    ("hc/api/__init__.py", ""),
    (
        "hc/api/decorators.py",
        "def authorize(f):\n    return f\n\n\ndef audited(f):\n    return f\n\n\ndef property(f):\n    return f\n",
    ),
    (
        "hc/api/views.py",
        "from hc.api import decorators\nfrom hc.api.decorators import authorize\n\n\n@authorize\ndef ping(r):\n    return r\n\n\n\
@decorators.audited\ndef status(r):\n    return r\n\n\nclass Check:\n    @property\n    def name(self):\n        return 1\n\n    \
@staticmethod\n    def make():\n        return 2\n\n    @undeclared\n    def other(self):\n        return 3\n\n    \
@decorators.missing\n    def last(self):\n        return 4\n",
    ),
];

/// FR-RS-23 AC (S-545): `@authorize` through a named import and `@mod.name`
/// through an imported module bind the decorator's declaration. `@property` and
/// `@staticmethod` stay unbound — the workspace's own `property` is not the
/// builtin, and the file never imports it — as does a name no import binds and
/// an attribute the module does not declare.
#[test]
fn a_bare_decorator_binds_the_declaration_it_names_and_a_builtin_stays_unbound() {
    let (_tmp, engine) = indexed(DECORATORS);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_with_lines(rt, "hc/api/views.py", EdgeKind::Calls),
        strings(&["hc/api/decorators.py:1 authorize", "hc/api/decorators.py:5 audited"])
    );
}

/// S-545: under every policy `@authorize` binds and `@staticmethod` stays
/// unbound. `@property` stays unbound under strict and balanced even beside a
/// workspace function of that name, which `views.py` never imports; only the
/// aggressive unique-name guess binds it, as it binds a `property(x)` call.
#[test]
fn a_bare_decorator_binds_the_same_under_every_policy() {
    for policy in ["strict", "balanced", "aggressive"] {
        let config = format!("[resolution]\npolicy = \"{policy}\"\n");
        let (_tmp, engine) = indexed_with(DECORATORS, &[(".logos/config.toml", &config)]);
        let rt = engine.runtime().unwrap();
        let calls = edges_with_lines(rt, "hc/api/views.py", EdgeKind::Calls);
        assert!(calls.contains(&"hc/api/decorators.py:1 authorize".to_string()), "{policy}: {calls:?}");
        assert!(!calls.iter().any(|c| c.ends_with(" staticmethod")), "{policy}: {calls:?}");
        let property = calls.contains(&"hc/api/decorators.py:9 property".to_string());
        assert_eq!(property, policy == "aggressive", "{policy}: {calls:?}");
    }
}

/// S-545: a decorator re-binds on a sync of the module it names exactly as a
/// cold index binds it, and binds again when the module is restored.
#[test]
fn sync_equals_a_full_reindex_when_a_decorators_target_changes() {
    let (tmp, engine) = indexed(DECORATORS);
    let rt = engine.runtime().unwrap();
    let files = paths(DECORATORS);
    let rel = "hc/api/decorators.py";
    let original = DECORATORS.iter().find(|(path, _)| *path == rel).expect("fixture file").1;
    write(tmp.path(), rel, "def authorize2(f):\n    return f\n");
    engine.sync(&[rel.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files), "renamed");
    assert!(edges_from(rt, "hc/api/views.py", EdgeKind::Calls).is_empty());
    write(tmp.path(), rel, original);
    engine.sync(&[rel.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files), "restored");
    assert_eq!(edges_from(rt, "hc/api/views.py", EdgeKind::Calls).len(), 2);
}
