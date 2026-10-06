//! Type relations beyond package-shaped sources (S-522, [FR-RS-15],
//! [NFR-RA-05]), over a synthetic snapshot: a type's `Extends` and
//! `Implements` bind through whichever module model its file declares, an
//! `Implements` binds an interface or a trait, a supertype whose syntax was
//! silent takes its edge kind from the target, and the workspace fallbacks
//! never guess one. Rust's impl-method `Implements` keeps the S-281 trait bind.
//! End to end, against real Python, PHP, C# and Kotlin, in
//! `tests/type_relations.rs`.
//!
//! The layout is told what the registry would hand it: `.py` declares the
//! Python path model, `.cs` the declared-namespace model and
//! `supertype_kind_follows_target`, and `.rs` the Rust path model:
//!
//! ```text
//! src/app/__init__.py   (module 100 "app")
//! src/app/base.py       (module 110 "base")   ─ class Base (111), class Trait (112) [trait],
//!                                                class Port (113) [interface]
//! src/app/views.py      (module 120 "views")  ─ class View (121), class Plain (122)
//! src/app/models/__init__.py (module 130 "models")
//! src/elsewhere/lib.py  (module 140 "lib")    ─ class Hidden (141)
//! Src/Reader.cs         (module 200 "Reader") ─ class Reader (201), interface ILine (202),
//!                                                class Text (203), interface IPos (204),
//!                                                class Exception (205), trait Mixin (206) [trait]
//! src/lib.rs            (module 300 "lib")    ─ struct Sq (302), fn run (303) [impl Port for Sq]
//! src/shapes.rs         (module 310 "shapes") ─ trait Port (301)
//! ```
//!
//! The `[trait]` and `[interface]` nodes are synthetic: no Python file yields
//! them. They are here so that each admission is seen to take its own kinds.
//!
//! [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::HashMap;

use super::binder::{bind, Index, Outcome};
use super::package_key::PackageLayout;
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};
use crate::plugin::PathModelDecl;

const VIEWS_PY: i64 = 40;
const READER_CS: i64 = 41;
const LIB_RS: i64 = 42;

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
        node(100, "app", NodeKind::Module, "src/app/__init__.py"),
        node(110, "base", NodeKind::Module, "src/app/base.py"),
        node(111, "Base", NodeKind::Class, "src/app/base.py"),
        node(112, "Trait", NodeKind::Trait, "src/app/base.py"),
        node(113, "Port", NodeKind::Interface, "src/app/base.py"),
        node(120, "views", NodeKind::Module, "src/app/views.py"),
        node(121, "View", NodeKind::Class, "src/app/views.py"),
        node(122, "Plain", NodeKind::Class, "src/app/views.py"),
        node(130, "models", NodeKind::Module, "src/app/models/__init__.py"),
        node(140, "lib", NodeKind::Module, "src/elsewhere/lib.py"),
        node(141, "Hidden", NodeKind::Class, "src/elsewhere/lib.py"),
        node(200, "Reader", NodeKind::Module, "Src/Reader.cs"),
        node(201, "Reader", NodeKind::Class, "Src/Reader.cs"),
        node(202, "ILine", NodeKind::Interface, "Src/Reader.cs"),
        node(203, "Text", NodeKind::Class, "Src/Reader.cs"),
        node(204, "IPos", NodeKind::Interface, "Src/Reader.cs"),
        node(205, "Exception", NodeKind::Class, "Src/Reader.cs"),
        node(206, "Mixin", NodeKind::Trait, "Src/Reader.cs"),
        node(300, "lib", NodeKind::Module, "src/lib.rs"),
        node(301, "Port", NodeKind::Trait, "src/shapes.rs"),
        node(310, "shapes", NodeKind::Module, "src/shapes.rs"),
        node(302, "Sq", NodeKind::Struct, "src/lib.rs"),
        node(303, "run", NodeKind::Method, "src/lib.rs"),
    ];
    let edges = vec![
        contains(110, 111),
        contains(110, 112),
        contains(110, 113),
        contains(120, 121),
        contains(120, 122),
        contains(140, 141),
        contains(200, 201),
        contains(200, 202),
        contains(200, 203),
        contains(200, 204),
        contains(200, 205),
        contains(200, 206),
        contains(310, 301),
        contains(300, 302),
        contains(300, 303),
    ];
    (nodes, edges)
}

/// What the registry hands the layout: the Python path model, C#'s declared
/// namespace and its supertype key, and Rust's stems.
fn layout() -> PackageLayout {
    PackageLayout::rust_stems_for_tests()
        .with_path_models(HashMap::from([(
            "py".to_string(),
            PathModelDecl {
                language: "python".to_string(),
                family: "python".to_string(),
                package_stems: vec!["__init__".to_string()],
                import_roots: Some(vec!["src".to_string()]),
            },
        )]))
        .with_namespace_extensions(["cs".to_string()])
        .with_declared_namespaces([("Src/Reader.cs".to_string(), "Json".to_string())])
        .with_kind_following_supertypes(["cs".to_string()])
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
        peeled: None,
    }
}

fn index(refs: &[UnresolvedRefRow]) -> Index {
    let (nodes, edges) = graph();
    Index::build_with_layout(&nodes, &edges, refs, layout())
}

/// Bind the last of `refs` over all of them.
fn bind_last(refs: &[UnresolvedRefRow], policy: BindingPolicy) -> Outcome {
    bind(refs.last().expect("a row"), &index(refs), policy)
}

fn bound(source: i64, target: i64, kind: EdgeKind) -> Outcome {
    Outcome::Bound {
        source: NodeId(source),
        target: NodeId(target),
        kind,
        payload: None,
    }
}

/// A Python class's base, imported by name, binds through the path modules —
/// the source is no package-shaped file, which before S-522 bound nothing.
#[test]
fn a_path_model_class_extends_the_base_its_import_names() {
    for policy in POLICIES {
        let imported = row(1, VIEWS_PY, 120, "app::base::Base", EdgeKind::Imports);
        let base = row(2, VIEWS_PY, 121, "Base", EdgeKind::Extends);
        assert_eq!(
            bind_last(&[imported, base], policy),
            bound(121, 111, EdgeKind::Extends),
            "{policy:?}"
        );
    }
}

/// An `Implements` from a type binds an interface or a trait, and never a
/// class; an `Extends` from a class binds a class, never an interface.
#[test]
fn implements_binds_an_interface_or_a_trait_and_extends_a_class() {
    for (target, kind, expected) in [
        ("app::base::Trait", EdgeKind::Implements, Some(112)),
        ("app::base::Port", EdgeKind::Implements, Some(113)),
        ("app::base::Base", EdgeKind::Implements, None),
        ("app::base::Port", EdgeKind::Extends, None),
        ("app::base::Trait", EdgeKind::Extends, None),
    ] {
        let r = row(1, VIEWS_PY, 121, target, kind);
        let want = expected.map_or(Outcome::Unbound, |t| bound(121, t, kind));
        assert_eq!(bind_last(&[r], BindingPolicy::Strict), want, "{kind:?} {target}");
    }
}

/// A type relation never takes a workspace guess: under the balanced policy a
/// path the module tree cannot reach is no suffix match, and under the
/// aggressive one a name the file never imports is no unique-name match —
/// though either fallback would find the one `Hidden` class.
#[test]
fn a_type_relation_takes_no_workspace_fallback_under_any_policy() {
    for policy in POLICIES {
        let by_name = row(1, VIEWS_PY, 121, "Hidden", EdgeKind::Extends);
        assert_eq!(bind_last(&[by_name], policy), Outcome::Unbound, "{policy:?} name");
        let by_suffix = row(2, VIEWS_PY, 121, "lib::Hidden", EdgeKind::Extends);
        assert_eq!(bind_last(&[by_suffix], policy), Outcome::Unbound, "{policy:?} suffix");
    }
}

/// A module of the base's name is no base: `class View(app.models)` names the
/// package `models`, and `class View(app)` the package `app` — a path's last
/// segment and a bare name each reach a module first for an import, never for
/// a type relation.
#[test]
fn a_type_relation_never_binds_a_module() {
    for policy in POLICIES {
        let r = row(1, VIEWS_PY, 121, "app::models", EdgeKind::Extends);
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?} path");
        let r = row(2, VIEWS_PY, 121, "app", EdgeKind::Extends);
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?} name");
    }
}

/// Where the language leaves a supertype's kind unsaid (C#), a class's entry
/// binds the class, interface or trait it names, and its edge kind follows the
/// target — `Implements` to an interface or a trait. An interface's entry binds an interface, as `Extends`, and never a
/// class.
#[test]
fn a_silent_supertype_takes_the_kind_of_what_it_binds() {
    for (source, target, expected) in [
        (203, "Reader", Some((201, EdgeKind::Extends))),
        (203, "ILine", Some((202, EdgeKind::Implements))),
        (203, "Mixin", Some((206, EdgeKind::Implements))),
        (204, "ILine", Some((202, EdgeKind::Extends))),
        (204, "Reader", None),
    ] {
        let r = row(1, READER_CS, source, target, EdgeKind::Extends);
        let want = expected.map_or(Outcome::Unbound, |(t, kind)| bound(source, t, kind));
        assert_eq!(bind_last(&[r], BindingPolicy::Strict), want, "{source} {target}");
    }
}

/// Only an `Extends`-bound supertype is a level of the hierarchy a `base.`
/// call climbs: a silent supertype bound as `Implements` is not.
#[test]
fn only_an_extends_bound_supertype_enters_the_hierarchy() {
    let to_class = row(1, READER_CS, 203, "Reader", EdgeKind::Extends);
    let to_interface = row(2, READER_CS, 203, "ILine", EdgeKind::Extends);
    let ix = index(&[to_class, to_interface]);
    assert!(ix.walks_hierarchy("Src/Reader.cs"));
    assert_eq!(ix.supertypes_for_tests(NodeId(203)), [NodeId(201)]);
}

/// A class with two rows that may each name a base class — two bound bases,
/// or a bound and an unbound one — has no supertypes the walk may climb: its
/// method order is its language's (Python's MRO), not one level after another.
/// Where the kind is unsaid (C#) an unbound entry beside the bound base class is
/// an interface, so the base class is still climbed.
#[test]
fn two_bases_leave_a_class_without_supertypes_unless_the_kind_is_unsaid() {
    let two = [
        row(1, VIEWS_PY, 122, "app::base::Base", EdgeKind::Extends),
        row(2, VIEWS_PY, 122, "External", EdgeKind::Extends),
    ];
    let ix = index(&two);
    assert!(ix.supertypes_for_tests(NodeId(122)).is_empty());
    let one = [row(1, VIEWS_PY, 122, "app::base::Base", EdgeKind::Extends)];
    assert_eq!(index(&one).supertypes_for_tests(NodeId(122)), [NodeId(111)]);
    let silent = [
        row(1, READER_CS, 203, "Reader", EdgeKind::Extends),
        row(2, READER_CS, 203, "IDisposable", EdgeKind::Extends),
    ];
    assert_eq!(index(&silent).supertypes_for_tests(NodeId(203)), [NodeId(201)]);
}

/// A fully-qualified name (`\Exception`, `global::Exception`) is read from the
/// global namespace alone: never the same-namespace class of that name, which
/// would be the class extending it.
#[test]
fn a_fully_qualified_supertype_never_names_a_same_namespace_type() {
    let rooted = format!("{}::Exception", super::FULLY_QUALIFIED_HEAD);
    let r = row(1, READER_CS, 205, &rooted, EdgeKind::Extends);
    assert_eq!(bind_last(&[r], BindingPolicy::Strict), Outcome::Unbound);
    let r = row(2, READER_CS, 203, "Exception", EdgeKind::Extends);
    assert_eq!(bind_last(&[r], BindingPolicy::Strict), bound(203, 205, EdgeKind::Extends));
}

/// A type's `Implements` never enters the `dyn T` fan-out universe (S-281),
/// though it names the one workspace trait: a `dyn Port` call to `View` fans out
/// to no class.
#[test]
fn a_types_implements_never_enters_the_dyn_fan_out() {
    let implements = row(1, VIEWS_PY, 121, "Port", EdgeKind::Implements);
    let dyn_call = UnresolvedRefRow {
        form: RefForm::Method,
        ..row(2, LIB_RS, 303, "Port::View", EdgeKind::Calls)
    };
    assert_eq!(bind_last(&[implements, dyn_call], BindingPolicy::Strict), Outcome::Unbound);
}

/// Rust's `Implements` is sourced at an impl **method**, so it keeps the S-281
/// rule — the one workspace trait of its last segment, here in a file the
/// impl's never imports, which no scope-only type relation would reach — beside
/// an interface of the same name.
#[test]
fn a_rust_impl_method_implements_its_trait_as_before() {
    for policy in POLICIES {
        let r = row(1, LIB_RS, 303, "Port", EdgeKind::Implements);
        assert_eq!(bind_last(&[r], policy), bound(303, 301, EdgeKind::Implements), "{policy:?}");
    }
}
