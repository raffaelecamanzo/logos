//! The path model with import roots (S-519, [FR-RS-14], [NFR-RA-05]), over a
//! synthetic snapshot: relative levels, directory modules, the package an
//! `__init__.py` re-exports through, the parent-key suffix match, and the family
//! crate a fallback never leaves. End to end, against real Python, in
//! `tests/python_imports.rs`.
//!
//! The layout is told a Python-shaped declaration for `.py` (fixture data, as
//! the registry would hand it), and `.js` declares nothing:
//!
//! ```text
//! src/pkg/__init__.py           (module 100 "pkg")      ─ version (101)
//! src/pkg/routing/__init__.py   (module 110 "routing")
//! src/pkg/routing/rules.py      (module 120 "rules")    ─ class Rule (121), parse_rule (122),
//!                                                          component Rule (123) [promoted]
//! src/pkg/routing/map.py        (module 130 "map")      ─ class Map (131) ─ build (132)
//! src/pkg/_internal.py          (module 140 "_internal") ─ _wsgi_decoding_dance (141)
//! src/pkg/ns/conv.py            (module 150 "conv")     ─ convert (151)   [no ns/__init__.py]
//! tests/test_map.py             (module 160 "test_map") ─ test_it (161)
//! lib/thing.js                  (module 170 "thing")    ─ thing (171)     [JavaScript]
//! manage.py                     (module 180 "manage")   [a top-level script]
//! mylib/src/lib.rs, util.rs     (modules 190, 191)      ─ g (192)          [a Rust crate]
//! ```
//!
//! [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{HashMap, HashSet};

use super::binder::{bind, Index, Outcome};
use super::package_key::PackageLayout;
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};
use crate::plugin::PathModelDecl;

const MAP_PY: i64 = 30;
const ROUTING_INIT_PY: i64 = 31;
const TEST_MAP_PY: i64 = 32;
const TOP_PY: i64 = 34;

const POLICIES: [BindingPolicy; 3] = [
    BindingPolicy::Strict,
    BindingPolicy::Balanced,
    BindingPolicy::Aggressive,
];

fn node(id: i64, name: &str, kind: NodeKind, file: &str) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local sym{id}")).unwrap(),
        kind,
        name: name.to_string(),
        file_path: Some(file.to_string()),
        start_line: None,
        end_line: None,
    }
}

fn contains(source: i64, target: i64) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind: EdgeKind::Contains,
    }
}

fn graph() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(100, "pkg", NodeKind::Module, "src/pkg/__init__.py"),
        node(101, "version", NodeKind::Function, "src/pkg/__init__.py"),
        node(110, "routing", NodeKind::Module, "src/pkg/routing/__init__.py"),
        node(120, "rules", NodeKind::Module, "src/pkg/routing/rules.py"),
        node(121, "Rule", NodeKind::Class, "src/pkg/routing/rules.py"),
        node(122, "parse_rule", NodeKind::Function, "src/pkg/routing/rules.py"),
        node(123, "Rule", NodeKind::Component, "src/pkg/routing/rules.py"),
        node(130, "map", NodeKind::Module, "src/pkg/routing/map.py"),
        node(131, "Map", NodeKind::Class, "src/pkg/routing/map.py"),
        node(132, "build", NodeKind::Function, "src/pkg/routing/map.py"),
        node(140, "_internal", NodeKind::Module, "src/pkg/_internal.py"),
        node(141, "_wsgi_decoding_dance", NodeKind::Function, "src/pkg/_internal.py"),
        node(150, "conv", NodeKind::Module, "src/pkg/ns/conv.py"),
        node(151, "convert", NodeKind::Function, "src/pkg/ns/conv.py"),
        node(160, "test_map", NodeKind::Module, "tests/test_map.py"),
        node(161, "test_it", NodeKind::Function, "tests/test_map.py"),
        node(170, "thing", NodeKind::Module, "lib/thing.js"),
        node(171, "thing", NodeKind::Function, "lib/thing.js"),
        node(180, "manage", NodeKind::Module, "manage.py"),
        node(190, "mylib", NodeKind::Module, "mylib/src/lib.rs"),
        node(191, "util", NodeKind::Module, "mylib/src/util.rs"),
        node(192, "g", NodeKind::Function, "mylib/src/util.rs"),
    ];
    let edges = vec![
        contains(100, 101),
        contains(120, 121),
        contains(120, 122),
        contains(120, 123),
        contains(130, 131),
        contains(130, 132),
        contains(140, 141),
        contains(150, 151),
        contains(160, 161),
        contains(170, 171),
        contains(191, 192),
    ];
    (nodes, edges)
}

/// A Python-shaped import-root declaration for `.py`, beside Rust's stems for
/// `.rs` (the rust plugin's, via the test layout).
fn python_layout() -> PackageLayout {
    PackageLayout::rust_stems_for_tests().with_path_models(HashMap::from([(
        "py".to_string(),
        PathModelDecl {
            language: "python".to_string(),
            family: "python".to_string(),
            package_stems: vec!["__init__".to_string()],
            import_roots: Some(vec!["src".to_string()]),
        },
    )]))
}

fn row(id: i64, file: i64, source: i64, target: &str, kind: EdgeKind) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id,
        file_id: Some(file),
        source_symbol: format!("local sym{source}"),
        target: target.to_string(),
        alias: (kind == EdgeKind::Imports).then(|| target.rsplit("::").next().unwrap().to_string()),
        form: RefForm::Path,
        kind,
        line: Some(1),
        resolved: false,
        payload: None,
        receiver: None,
    }
}

fn import(id: i64, file: i64, source: i64, target: &str) -> UnresolvedRefRow {
    row(id, file, source, target, EdgeKind::Imports)
}

fn index(refs: &[UnresolvedRefRow]) -> Index {
    let (nodes, edges) = graph();
    Index::build_with_layout(&nodes, &edges, refs, python_layout())
}

/// Bind the last of `refs` over all of them.
fn bind_last(refs: &[UnresolvedRefRow], policy: BindingPolicy) -> Outcome {
    bind(refs.last().expect("a row"), &index(refs), policy)
}

fn bound(outcome: Outcome, source: i64, target: i64, kind: EdgeKind) {
    assert_eq!(
        outcome,
        Outcome::Bound {
            source: NodeId(source),
            target: NodeId(target),
            kind,
            payload: None,
        }
    );
}

/// FR-RS-14 AC (werkzeug `from .rules import Rule`, `from .._internal import
/// x`): `.` is the importing file's package and each `..` one package up.
#[test]
fn a_relative_import_keeps_its_level() {
    for policy in POLICIES {
        let r = import(1, MAP_PY, 130, ".::rules::Rule");
        bound(bind_last(&[r], policy), 130, 121, EdgeKind::Imports);
        let r = import(2, MAP_PY, 130, "..::_internal::_wsgi_decoding_dance");
        bound(bind_last(&[r], policy), 130, 141, EdgeKind::Imports);
        // `from . import rules` names the sibling module.
        let r = import(3, MAP_PY, 130, ".::rules");
        bound(bind_last(&[r], policy), 130, 120, EdgeKind::Imports);
        // `from .. import *` names the parent package itself.
        let glob = UnresolvedRefRow {
            form: RefForm::Glob,
            alias: None,
            ..import(4, MAP_PY, 130, "..")
        };
        bound(bind_last(&[glob], policy), 130, 100, EdgeKind::Imports);
    }
}

/// A package file is the module of its own package, so its `.` is itself, not
/// its parent — `routing/__init__.py`'s `from .rules import Rule`.
#[test]
fn a_package_files_own_level_is_its_package() {
    for policy in POLICIES {
        let r = import(1, ROUTING_INIT_PY, 110, ".::rules::Rule");
        bound(bind_last(&[r], policy), 110, 121, EdgeKind::Imports);
        let r = import(2, ROUTING_INIT_PY, 110, "..::version");
        bound(bind_last(&[r], policy), 110, 101, EdgeKind::Imports);
    }
}

/// More levels than the package is deep name nothing, and no wider rung reads a
/// relative head as a name — not the suffix match, not the unique name. Nor
/// does a level that climbs exactly to the import root: Python refuses
/// `from ... import pkg` two packages down, and `from . import x` in a
/// top-level script.
#[test]
fn a_level_above_the_import_root_names_nothing() {
    for policy in POLICIES {
        let r = import(3, MAP_PY, 130, "..::..::pkg");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = import(4, TOP_PY, 180, ".::pkg");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = import(1, MAP_PY, 130, "..::..::..::_internal");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = import(2, MAP_PY, 130, ".::nowhere::Rule");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// Every directory under an import root is a module: an absolute import
/// descends through `ns/`, which has no `__init__.py` and so no node.
#[test]
fn a_directory_without_a_package_file_still_descends() {
    for policy in POLICIES {
        let r = import(1, TEST_MAP_PY, 160, "pkg::ns::conv::convert");
        bound(bind_last(&[r], policy), 160, 151, EdgeKind::Imports);
        // …but a directory with no node is never itself a target, and has no
        // package file for a name to go through.
        let r = import(2, TEST_MAP_PY, 160, "pkg::ns::missing");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        // `from pkg import ns` still goes through `pkg/__init__.py`.
        let r = import(3, TEST_MAP_PY, 160, "pkg::ns");
        bound(bind_last(&[r], policy), 160, 100, EdgeKind::Imports);
    }
}

/// FR-RS-14 AC: an import through a package's `__init__.py` binds to that
/// package — the package itself, and a name it re-exports without declaring
/// it. A name an ordinary module does not declare stays unbound.
#[test]
fn an_import_through_a_package_file_binds_to_its_package() {
    for policy in POLICIES {
        let r = import(1, TEST_MAP_PY, 160, "pkg::routing");
        bound(bind_last(&[r], policy), 160, 110, EdgeKind::Imports);
        let r = import(2, TEST_MAP_PY, 160, "pkg::routing::Map");
        bound(bind_last(&[r], policy), 160, 110, EdgeKind::Imports);
        let r = import(3, TEST_MAP_PY, 160, "pkg::version");
        bound(bind_last(&[r], policy), 160, 101, EdgeKind::Imports);
        let r = import(4, TEST_MAP_PY, 160, "pkg::routing::rules::Missing");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        // A package never imports itself through its own `__init__.py`.
        let r = import(5, ROUTING_INIT_PY, 110, ".::Map");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// A bare call to a name a `from` import brought into view binds across files,
/// through the import's alias.
#[test]
fn a_call_through_a_from_import_binds_across_files() {
    for policy in POLICIES {
        let imported = import(1, MAP_PY, 130, ".::rules::parse_rule");
        let call = row(2, MAP_PY, 132, "parse_rule", EdgeKind::Calls);
        bound(bind_last(&[imported, call], policy), 132, 122, EdgeKind::Calls);
    }
}

/// The workspace suffix match compares a module by its **parent** key: the
/// path `routing::map` reaches the module `[pkg, routing, map]`, whose own key
/// would need to end in `routing` (S-519).
#[test]
fn the_suffix_match_compares_a_module_by_its_parent_key() {
    let r = import(1, TEST_MAP_PY, 160, "routing::map");
    bound(bind_last(std::slice::from_ref(&r), BindingPolicy::Balanced), 160, 130, EdgeKind::Imports);
    // A member still matches by the module it sits in.
    let r = import(2, TEST_MAP_PY, 160, "routing::map::Map");
    bound(bind_last(&[r], BindingPolicy::Balanced), 160, 131, EdgeKind::Imports);
    // The fallback is policy-gated, as everywhere.
    let r = import(3, TEST_MAP_PY, 160, "routing::map");
    assert_eq!(bind_last(&[r], BindingPolicy::Strict), Outcome::Unbound);
}

/// An import-root family's crate is closed: a Python import no Python module
/// answers never falls back to another language's module of the same shape
/// (the JavaScript `lib/thing.js`), under any policy.
#[test]
fn a_fallback_never_leaves_the_family_crate() {
    for policy in POLICIES {
        let r = import(1, TEST_MAP_PY, 160, "lib::thing");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let call = row(2, TEST_MAP_PY, 161, "thing", EdgeKind::Calls);
        assert_eq!(bind_last(&[call], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// Nor does a Python import name another language's crate: `import mylib`
/// beside a Rust crate `mylib` (a pyo3 layout) reaches neither its root by the
/// extern-crate rung nor its modules by a crate-name head.
#[test]
fn a_python_import_never_names_another_languages_crate() {
    for policy in POLICIES {
        let r = import(1, TEST_MAP_PY, 160, "mylib");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = import(2, TEST_MAP_PY, 160, "mylib::util::g");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// A sync that adds a package file beneath a candidate import root may move
/// the detected roots, and with them every key of the language: every row of
/// an import-root file is re-selected, under names no dirty token spells.
#[test]
fn a_package_file_beneath_a_candidate_root_reselects_every_import_root_row() {
    let r = import(1, TEST_MAP_PY, 160, "pkg::routing");
    let ix = index(std::slice::from_ref(&r));
    let file_paths: HashMap<i64, String> = [(TEST_MAP_PY, "tests/test_map.py".to_string())]
        .into_iter()
        .collect();
    let delta = super::Delta {
        changed_paths: ["src/other/__init__.py".to_string()].into_iter().collect(),
        dirty_tokens: ["src", "other", "__init__", "py"].iter().map(|t| t.to_string()).collect::<HashSet<_>>(),
        global_imports_moved: false,
    };
    assert!(ix.moves_import_roots("src/other/__init__.py"));
    assert!(!ix.moves_import_roots("src/other/module.py"));
    let moved = |import_roots| super::Moved {
        hierarchy: false,
        import_roots,
    };
    assert!(super::is_affected(&r, &delta, &file_paths, &ix, moved(true)));
    assert!(!super::is_affected(&r, &delta, &file_paths, &ix, moved(false)));
}

/// The interop family partitions the namespace index (S-519): a C# `using
/// App.Models;` beside a PHP `namespace App\Models;` reaches only the C# files
/// declaring it — never the PHP one, which no C# file can name.
#[test]
fn a_namespace_wildcard_reaches_only_its_own_familys_files() {
    let nodes = vec![
        node(200, "User", NodeKind::Module, "app/Models/User.php"),
        node(201, "User", NodeKind::Class, "app/Models/User.php"),
        node(210, "Order", NodeKind::Module, "src/Models/Order.cs"),
        node(211, "Order", NodeKind::Class, "src/Models/Order.cs"),
        node(220, "Svc", NodeKind::Module, "src/Api/Svc.cs"),
        node(221, "Svc", NodeKind::Class, "src/Api/Svc.cs"),
    ];
    let edges = vec![contains(200, 201), contains(210, 211), contains(220, 221)];
    let layout = PackageLayout::default()
        .with_namespace_extensions(["cs".to_string(), "php".to_string()])
        .with_declared_namespaces([
            ("app/Models/User.php".to_string(), "App.Models".to_string()),
            ("src/Models/Order.cs".to_string(), "App.Models".to_string()),
            ("src/Api/Svc.cs".to_string(), "App.Api".to_string()),
        ]);
    let using = UnresolvedRefRow {
        form: RefForm::Glob,
        alias: None,
        ..import(1, 40, 220, "App::Models")
    };
    let user = row(2, 40, 221, "User", EdgeKind::TypeUses);
    let ix = Index::build_with_layout(&nodes, &edges, &[using.clone(), user.clone()], layout);
    for policy in [BindingPolicy::Strict, BindingPolicy::Balanced] {
        assert_eq!(
            bind(&using, &ix, policy),
            Outcome::BoundMany {
                source: NodeId(220),
                targets: vec![NodeId(210)],
                kind: EdgeKind::Imports,
                payload: None,
            }
        );
        // The PHP class of that namespace is not in view through the wildcard.
        assert_eq!(bind(&user, &ix, policy), Outcome::Unbound);
    }
}

/// A framework-promoted `component` shares its class's name and file (a Django
/// model is both); an import names the class, so the promoted node is no
/// candidate and the import is not read as ambiguous.
#[test]
fn an_import_never_names_a_framework_promoted_node() {
    for policy in POLICIES {
        let r = import(1, TEST_MAP_PY, 160, "pkg::routing::rules::Rule");
        bound(bind_last(&[r], policy), 160, 121, EdgeKind::Imports);
    }
}
