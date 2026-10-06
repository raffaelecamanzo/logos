//! Call targets beyond a callable (S-521, [FR-RS-16], [NFR-RA-05]), over a
//! synthetic snapshot: a class a Python call constructs records `Instantiates`,
//! a macro a C call expands binds `Calls`, and a language whose plugin declares
//! neither key binds exactly as before. End to end, against real Python, Kotlin
//! and C, in `tests/call_targets.rs`.
//!
//! The layout is told what the registry would hand it: `.py` declares the
//! Python path model and `class_call_instantiates`, `.c` declares
//! `macros_callable`, and `.ts` and `.rs` declare neither:
//!
//! ```text
//! src/app/__init__.py   (module 100 "app")
//! src/app/models.py     (module 110 "models") ─ class Check (111), check_all (112)
//! src/app/views.py      (module 120 "views")  ─ index (121)
//! src/app/dup_a.py      (module 130 "dup_a")  ─ class Dup (131)
//! src/app/dup_b.py      (module 140 "dup_b")  ─ class Dup (141)
//! src/app/twin.py       (module 150 "twin")   ─ class Twin (151), Twin (152),
//!                                                make (153), class Solo (154),
//!                                                macro Expand (155) [synthetic]
//! src/fs-poll.c         (module 200 "fs-poll") ─ macro uv__make_close_pending (201),
//!                                                poll_cb (202), macro dual (203), dual (204),
//!                                                class Box (205) [synthetic], variable hook (206),
//!                                                struct Stat (207) [synthetic]
//! src/core.c            (module 210 "core")   ─ uv__make_close_pending (211), uv__close (212)
//! lib/thing.ts          (module 300 "thing")  ─ class Thing (301), use_thing (302)
//! mylib/src/lib.rs      (module 310 "mylib")  ─ macro my_macro (311), f (312), g (313)
//! ```
//!
//! The two `[synthetic]` nodes no real Python or C file yields; they are here so
//! that each key is seen to admit its own kind only. A Kotlin-shaped section
//! further down keys `.pkt` files by their declared package (S-518) — a
//! synthetic extension, since no JVM language id may be spelt under
//! `src/resolve/` (the `jvm_parity` guard).
//!
//! [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::HashMap;

use super::binder::{bind, Index, Outcome};
use super::package_key::PackageLayout;
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};
use crate::plugin::{CallTargets, PathModelDecl};

const VIEWS_PY: i64 = 40;
const TWIN_PY: i64 = 41;
const FS_POLL_C: i64 = 42;
const CORE_C: i64 = 43;
const THING_TS: i64 = 44;
const MYLIB_RS: i64 = 45;

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
        node(110, "models", NodeKind::Module, "src/app/models.py"),
        node(111, "Check", NodeKind::Class, "src/app/models.py"),
        node(112, "check_all", NodeKind::Function, "src/app/models.py"),
        node(120, "views", NodeKind::Module, "src/app/views.py"),
        node(121, "index", NodeKind::Function, "src/app/views.py"),
        node(130, "dup_a", NodeKind::Module, "src/app/dup_a.py"),
        node(131, "Dup", NodeKind::Class, "src/app/dup_a.py"),
        node(140, "dup_b", NodeKind::Module, "src/app/dup_b.py"),
        node(141, "Dup", NodeKind::Class, "src/app/dup_b.py"),
        node(150, "twin", NodeKind::Module, "src/app/twin.py"),
        node(151, "Twin", NodeKind::Class, "src/app/twin.py"),
        node(152, "Twin", NodeKind::Function, "src/app/twin.py"),
        node(153, "make", NodeKind::Function, "src/app/twin.py"),
        node(154, "Solo", NodeKind::Class, "src/app/twin.py"),
        node(155, "Expand", NodeKind::Macro, "src/app/twin.py"),
        node(200, "fs-poll", NodeKind::Module, "src/fs-poll.c"),
        node(201, "uv__make_close_pending", NodeKind::Macro, "src/fs-poll.c"),
        node(202, "poll_cb", NodeKind::Function, "src/fs-poll.c"),
        node(203, "dual", NodeKind::Macro, "src/fs-poll.c"),
        node(204, "dual", NodeKind::Function, "src/fs-poll.c"),
        node(205, "Box", NodeKind::Class, "src/fs-poll.c"),
        node(206, "hook", NodeKind::Variable, "src/fs-poll.c"),
        node(207, "Stat", NodeKind::Struct, "src/fs-poll.c"),
        node(210, "core", NodeKind::Module, "src/core.c"),
        node(211, "uv__make_close_pending", NodeKind::Function, "src/core.c"),
        node(212, "uv__close", NodeKind::Function, "src/core.c"),
        node(300, "thing", NodeKind::Module, "lib/thing.ts"),
        node(301, "Thing", NodeKind::Class, "lib/thing.ts"),
        node(302, "use_thing", NodeKind::Function, "lib/thing.ts"),
        node(310, "mylib", NodeKind::Module, "mylib/src/lib.rs"),
        node(311, "my_macro", NodeKind::Macro, "mylib/src/lib.rs"),
        node(312, "f", NodeKind::Function, "mylib/src/lib.rs"),
        node(313, "g", NodeKind::Function, "mylib/src/lib.rs"),
    ];
    let edges = vec![
        contains(110, 111),
        contains(110, 112),
        contains(120, 121),
        contains(130, 131),
        contains(140, 141),
        contains(150, 151),
        contains(150, 152),
        contains(150, 153),
        contains(150, 154),
        contains(150, 155),
        contains(200, 201),
        contains(200, 202),
        contains(200, 203),
        contains(200, 204),
        contains(200, 205),
        contains(200, 206),
        contains(200, 207),
        contains(210, 211),
        contains(210, 212),
        contains(300, 301),
        contains(300, 302),
        contains(310, 311),
        contains(310, 312),
        contains(310, 313),
    ];
    (nodes, edges)
}

/// What the registry hands the layout for the shipped Python and C plugins;
/// TypeScript and Rust declare no call target, so they are absent.
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
        .with_call_targets(HashMap::from([
            (
                "py".to_string(),
                CallTargets {
                    classes: true,
                    macros: false,
                },
            ),
            (
                "c".to_string(),
                CallTargets {
                    classes: false,
                    macros: true,
                },
            ),
        ]))
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
        arg_count: None,
    }
}

fn call(id: i64, file: i64, source: i64, target: &str) -> UnresolvedRefRow {
    row(id, file, source, target, EdgeKind::Calls)
}

fn import(id: i64, file: i64, source: i64, target: &str) -> UnresolvedRefRow {
    row(id, file, source, target, EdgeKind::Imports)
}

/// Bind the last of `refs` over all of them.
fn bind_last(refs: &[UnresolvedRefRow], policy: BindingPolicy) -> Outcome {
    let (nodes, edges) = graph();
    let ix = Index::build_with_layout(&nodes, &edges, refs, layout());
    bind(refs.last().expect("a row"), &ix, policy)
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

/// FR-RS-16 AC (healthchecks `Check(project=p)`): a call to the one class a
/// `from` import brought into view records `Instantiates` to it, not `Calls`.
/// A function through the same kind of import is still a `Calls`.
#[test]
fn a_python_call_to_an_imported_class_records_instantiates() {
    for policy in POLICIES {
        let imported = import(1, VIEWS_PY, 120, "app::models::Check");
        let constructed = call(2, VIEWS_PY, 121, "Check");
        bound(
            bind_last(&[imported.clone(), constructed], policy),
            121,
            111,
            EdgeKind::Instantiates,
        );
        let function = import(3, VIEWS_PY, 120, "app::models::check_all");
        let called = call(4, VIEWS_PY, 121, "check_all");
        bound(bind_last(&[imported, function, called], policy), 121, 112, EdgeKind::Calls);
    }
}

/// A class of the caller's own file, reached on the lexical rung, is
/// constructed the same way — and so is one named by a module-qualified path.
#[test]
fn a_python_call_to_a_same_file_or_qualified_class_records_instantiates() {
    for policy in POLICIES {
        let r = call(1, TWIN_PY, 153, "Solo");
        bound(bind_last(&[r], policy), 153, 154, EdgeKind::Instantiates);
        let r = call(2, VIEWS_PY, 121, "app::models::Check");
        bound(bind_last(&[r], policy), 121, 111, EdgeKind::Instantiates);
    }
}

/// FR-RS-16 AC: a name that two classes carry stays unbound — imported twice,
/// or found twice by the workspace name fallback — and so does a name that no
/// declaration carries ([NFR-RA-05]).
#[test]
fn two_classes_or_none_leave_a_python_call_unbound() {
    for policy in POLICIES {
        let a = import(1, VIEWS_PY, 120, "app::dup_a::Dup");
        let b = UnresolvedRefRow {
            alias: Some("Dup".to_string()),
            ..import(2, VIEWS_PY, 120, "app::dup_b::Dup")
        };
        let r = call(3, VIEWS_PY, 121, "Dup");
        assert_eq!(bind_last(&[a, b, r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(4, VIEWS_PY, 121, "Dup");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(5, VIEWS_PY, 121, "Missing");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// A function and a class of one name in one scope are two candidates: the
/// CR-068 tie-break drops only methods, so it never picks the function over the
/// class (or the class over the function).
#[test]
fn a_function_and_a_class_of_one_name_are_two_candidates() {
    for policy in POLICIES {
        let r = call(1, TWIN_PY, 153, "Twin");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// FR-RS-16 AC (libuv `src/fs-poll.c`): a C call to the one macro its file
/// defines binds `Calls` to the Macro node — not to the same-named function
/// another translation unit defines, which that file binds to itself.
#[test]
fn a_c_call_to_a_macro_binds_calls_to_it() {
    for policy in POLICIES {
        let r = call(1, FS_POLL_C, 202, "uv__make_close_pending");
        bound(bind_last(&[r], policy), 202, 201, EdgeKind::Calls);
        let r = call(2, CORE_C, 212, "uv__make_close_pending");
        bound(bind_last(&[r], policy), 212, 211, EdgeKind::Calls);
    }
}

/// A macro and a function of one name in one C file are two candidates.
#[test]
fn a_macro_and_a_function_of_one_name_are_two_candidates() {
    for policy in POLICIES {
        let r = call(1, FS_POLL_C, 202, "dual");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

/// A language whose plugin declares neither key resolves exactly as before: a
/// TypeScript call never reaches a class, a Rust call never reaches a
/// `macro_rules!` macro, and a Rust call to a function still binds `Calls`.
#[test]
fn a_language_without_either_key_binds_a_callable_only() {
    for policy in POLICIES {
        let r = call(1, THING_TS, 302, "Thing");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(2, MYLIB_RS, 312, "my_macro");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(3, MYLIB_RS, 312, "g");
        bound(bind_last(&[r], policy), 312, 313, EdgeKind::Calls);
    }
}

/// `class_call_instantiates` admits no macro and `macros_callable` no class:
/// each key widens the call by its own kind only, even where the other kind
/// sits in the caller's own file. Nor does either admit any other kind: a C
/// call through a function-pointer variable, or to a struct, and a call naming
/// an interface or an enum, stay unbound.
#[test]
fn each_key_admits_its_own_kind_only() {
    for policy in POLICIES {
        let r = call(1, TWIN_PY, 153, "Expand");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        for name in ["Box", "hook", "Stat"] {
            let r = call(2, FS_POLL_C, 202, name);
            assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{name}, {policy:?}");
        }
        for name in ["Only", "Mode"] {
            let r = call(3, RUN_KT, 581, name);
            assert_eq!(bind_kotlin(&[r], policy), Outcome::Unbound, "{name}, {policy:?}");
        }
    }
}

/// A module is never a call's target, under a declared call as under a plain
/// one: neither a bare name that names a package (`app()`) nor a path whose last
/// segment names a module (`app.models()`) binds.
#[test]
fn a_declared_call_never_binds_a_module() {
    for policy in POLICIES {
        let r = call(1, VIEWS_PY, 121, "app");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(2, VIEWS_PY, 121, "app::models");
        assert_eq!(bind_last(&[r], policy), Outcome::Unbound, "{policy:?}");
    }
}

// ── A declared-package language (Kotlin-shaped `.pkt`, S-518) ─────────────
//
// ```text
// src/main/kotlin/com/x/Foo.pkt   com.x   class Foo (501)
// src/main/kotlin/com/x/Make.pkt  com.x   make (511)
// src/main/kotlin/com/y/Use.pkt   com.y   build (521)
// src/main/kotlin/com/z/Bar.pkt   com.z   class Bar (531)
// src/test/kotlin/com/z/Bar.pkt   com.z   class Bar (541)
// src/main/kotlin/com/z/Mk.pkt    com.z   mk (551)
// src/main/kotlin/com/w/Base.pkt  com.w   class Base (561) ─ m (562)
// src/main/kotlin/com/w/Sub.pkt   com.w   class Sub (571), extends Base
// src/main/kotlin/com/w/Run.pkt   com.w   run (581)
// src/main/kotlin/com/w/Only.pkt  com.w   interface Only (591)
// src/main/kotlin/com/w/Mode.pkt  com.w   enum Mode (601)
// src/main/kotlin/com/v/Job.pkt   com.v   class Job (611)
// src/main/kotlin/com/v/Jobs.pkt  com.v   Job (621)       ← a factory function
// src/main/kotlin/com/v/Start.pkt com.v   start (631)
// src/main/kotlin/com/v/Pipe.pkt  com.v   class Pipe (641)
// src/main/kotlin/com/u/Pipes.pkt com.u   Pipe (651)      ← another package's
// ```

const USE_KT: i64 = 50;
const MAKE_KT: i64 = 51;
const MK_KT: i64 = 52;
const SUB_KT: i64 = 53;
const RUN_KT: i64 = 54;
const START_KT: i64 = 55;

/// Bind the last of `refs` over all of them, against the Kotlin-shaped graph.
fn bind_kotlin(refs: &[UnresolvedRefRow], policy: BindingPolicy) -> Outcome {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut declared = Vec::new();
    for (module, file, package, member, name, kind) in [
        (500, "src/main/kotlin/com/x/Foo.pkt", "com.x", 501, "Foo", NodeKind::Class),
        (510, "src/main/kotlin/com/x/Make.pkt", "com.x", 511, "make", NodeKind::Function),
        (520, "src/main/kotlin/com/y/Use.pkt", "com.y", 521, "build", NodeKind::Function),
        (530, "src/main/kotlin/com/z/Bar.pkt", "com.z", 531, "Bar", NodeKind::Class),
        (540, "src/test/kotlin/com/z/Bar.pkt", "com.z", 541, "Bar", NodeKind::Class),
        (550, "src/main/kotlin/com/z/Mk.pkt", "com.z", 551, "mk", NodeKind::Function),
        (560, "src/main/kotlin/com/w/Base.pkt", "com.w", 561, "Base", NodeKind::Class),
        (570, "src/main/kotlin/com/w/Sub.pkt", "com.w", 571, "Sub", NodeKind::Class),
        (580, "src/main/kotlin/com/w/Run.pkt", "com.w", 581, "run", NodeKind::Function),
        (590, "src/main/kotlin/com/w/Only.pkt", "com.w", 591, "Only", NodeKind::Interface),
        (600, "src/main/kotlin/com/w/Mode.pkt", "com.w", 601, "Mode", NodeKind::Enum),
        (610, "src/main/kotlin/com/v/Job.pkt", "com.v", 611, "Job", NodeKind::Class),
        (620, "src/main/kotlin/com/v/Jobs.pkt", "com.v", 621, "Job", NodeKind::Function),
        (630, "src/main/kotlin/com/v/Start.pkt", "com.v", 631, "start", NodeKind::Function),
        (640, "src/main/kotlin/com/v/Pipe.pkt", "com.v", 641, "Pipe", NodeKind::Class),
        (650, "src/main/kotlin/com/u/Pipes.pkt", "com.u", 651, "Pipe", NodeKind::Function),
    ] {
        let stem = file.rsplit('/').next().unwrap().trim_end_matches(".pkt");
        nodes.push(node(module, stem, NodeKind::Module, file));
        nodes.push(node(member, name, kind, file));
        edges.push(contains(module, member));
        declared.push((file.to_string(), package.to_string()));
    }
    nodes.push(node(562, "m", NodeKind::Method, "src/main/kotlin/com/w/Base.pkt"));
    edges.push(contains(561, 562));
    let layout = PackageLayout::default()
        .with_namespace_extensions(["pkt".to_string()])
        .with_declared_namespaces(declared)
        .with_call_targets(HashMap::from([(
            "pkt".to_string(),
            CallTargets {
                classes: true,
                macros: false,
            },
        )]));
    let ix = Index::build_with_layout(&nodes, &edges, refs, layout);
    bind(refs.last().expect("a row"), &ix, policy)
}

/// FR-RS-16 AC (a Kotlin `Foo()`): the one class of the caller's own package,
/// or the one a single-type import names, is constructed. Two classes of one
/// fully-qualified name (two source sets) stay unbound under every policy.
#[test]
fn a_kotlin_call_to_one_class_of_its_package_or_import_records_instantiates() {
    for policy in POLICIES {
        let r = call(1, MAKE_KT, 511, "Foo");
        bound(bind_kotlin(&[r], policy), 511, 501, EdgeKind::Instantiates);
        let imported = import(2, USE_KT, 520, "com::x::Foo");
        let r = call(3, USE_KT, 521, "Foo");
        bound(bind_kotlin(&[imported, r], policy), 521, 501, EdgeKind::Instantiates);
        let r = call(4, MK_KT, 551, "Bar");
        assert_eq!(bind_kotlin(&[r], policy), Outcome::Unbound, "{policy:?}");
        // Unimported and of another package, `Foo` is out of view — until the
        // aggressive policy's workspace name fallback, which finds the one class.
        let r = call(5, USE_KT, 521, "Foo");
        if policy == BindingPolicy::Aggressive {
            bound(bind_kotlin(&[r], policy), 521, 501, EdgeKind::Instantiates);
        } else {
            assert_eq!(bind_kotlin(&[r], policy), Outcome::Unbound, "unimported, {policy:?}");
        }
    }
}

/// A top-level factory function of a class's name in the class's package —
/// `fun Job(s: String): Job` beside `class Job` — rivals the class: the package
/// rungs read types only, so the class alone would answer, but the call names
/// two declarations and stays unbound, from the package and through an import
/// alike ([NFR-RA-05]). A function of that name in another package is no rival.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_factory_function_of_the_package_rivals_its_class() {
    for policy in POLICIES {
        let r = call(1, START_KT, 631, "Job");
        assert_eq!(bind_kotlin(&[r], policy), Outcome::Unbound, "{policy:?}");
        let imported = import(2, USE_KT, 520, "com::v::Job");
        let r = call(3, USE_KT, 521, "Job");
        assert_eq!(bind_kotlin(&[imported, r], policy), Outcome::Unbound, "{policy:?}");
        let r = call(4, START_KT, 631, "Pipe");
        bound(bind_kotlin(&[r], policy), 631, 641, EdgeKind::Instantiates);
    }
}

/// A declared call through a type takes a call's walk: its last segment is the
/// type's callable, found up the type's in-repository supertypes (S-468) — an
/// inherited method binds `Calls`, as it would for a plain call.
#[test]
fn a_declared_call_through_a_type_reaches_an_inherited_method() {
    for policy in POLICIES {
        let extends = row(1, SUB_KT, 571, "Base", EdgeKind::Extends);
        let r = call(2, RUN_KT, 581, "Sub::m");
        bound(bind_kotlin(&[extends, r], policy), 581, 562, EdgeKind::Calls);
    }
}

/// A declared call is a call to the residue readout ([FR-RS-09]): an unbound
/// type-qualified call from a declared-package file names the fully-qualified
/// names its type could be, as a plain call's does.
///
/// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
#[test]
fn an_unbound_declared_call_records_its_residue() {
    use super::binder::{residue, Residue};
    let r = call(1, USE_KT, 521, "Ext::m");
    let refs = [r];
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (id, name, kind) in [(520, "Use", NodeKind::Module), (521, "build", NodeKind::Function)] {
        nodes.push(node(id, name, kind, "src/main/kotlin/com/y/Use.pkt"));
    }
    edges.push(contains(520, 521));
    let layout = PackageLayout::default()
        .with_namespace_extensions(["pkt".to_string()])
        .with_declared_namespaces([(
            "src/main/kotlin/com/y/Use.pkt".to_string(),
            "com.y".to_string(),
        )])
        .with_call_targets(HashMap::from([(
            "pkt".to_string(),
            CallTargets {
                classes: true,
                macros: false,
            },
        )]));
    let ix = Index::build_with_layout(&nodes, &edges, &refs, layout);
    let segs = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        residue(&refs[0], &ix, BindingPolicy::Aggressive),
        Some(Residue::ExternalType {
            candidates: vec![segs(&["com", "y", "Ext"])],
        })
    );
}
