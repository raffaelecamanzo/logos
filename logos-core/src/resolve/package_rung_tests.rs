//! The package-shaped scope rungs (S-465, CR-149, FR-RS-03, NFR-RA-05), over a
//! synthetic snapshot: a bare or type-qualified **type** name is a reference
//! S-466 (type relations) and S-467 (a typed receiver's `T::m`) capture, so the
//! same-package and package-wildcard rungs are pinned here at the binder level,
//! against the ledger shapes those stories emit. The import-then-same-package
//! order for a `T::m` call is also pinned end to end, in
//! `tests/java_receiver_typing.rs`.
//!
//! ```text
//! com.x.web  src/main/java/com/x/web/Ctl.java     (module 1) ─ class Ctl (2)
//!            src/main/java/com/x/web/Helper.java  (module 4) ─ class Helper (5) ─ util (6)
//! com.x.svc  src/main/java/com/x/svc/Svc.java     (module 7) ─ class Svc (8)
//! com.y      src/main/java/com/y/Helper.java      (module 10) ─ class Helper (11) ─ util (12)
//!            src/main/java/com/y/Only.java        (module 13) ─ class Only (14) ─ go (15)
//! ```

use super::binder::{bind, bind_counting_path_visits, residue, Index, Outcome, Residue};
use super::package_key::PackageLayout;
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};
use crate::plugin::LanguageRegistry;

const SVC_FILE: i64 = 70;

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

fn fixture() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let web = "src/main/java/com/x/web";
    let nodes = vec![
        node(1, "Ctl", NodeKind::Module, &format!("{web}/Ctl.java")),
        node(2, "Ctl", NodeKind::Class, &format!("{web}/Ctl.java")),
        node(4, "Helper", NodeKind::Module, &format!("{web}/Helper.java")),
        node(5, "Helper", NodeKind::Class, &format!("{web}/Helper.java")),
        node(6, "util", NodeKind::Method, &format!("{web}/Helper.java")),
        node(
            7,
            "Svc",
            NodeKind::Module,
            "src/main/java/com/x/svc/Svc.java",
        ),
        node(
            8,
            "Svc",
            NodeKind::Class,
            "src/main/java/com/x/svc/Svc.java",
        ),
        node(
            10,
            "Helper",
            NodeKind::Module,
            "src/main/java/com/y/Helper.java",
        ),
        node(
            11,
            "Helper",
            NodeKind::Class,
            "src/main/java/com/y/Helper.java",
        ),
        node(
            12,
            "util",
            NodeKind::Method,
            "src/main/java/com/y/Helper.java",
        ),
        node(
            13,
            "Only",
            NodeKind::Module,
            "src/main/java/com/y/Only.java",
        ),
        node(14, "Only", NodeKind::Class, "src/main/java/com/y/Only.java"),
        node(15, "go", NodeKind::Method, "src/main/java/com/y/Only.java"),
    ];
    let edges = vec![
        contains(1, 2),
        contains(4, 5),
        contains(5, 6),
        contains(7, 8),
        contains(10, 11),
        contains(11, 12),
        contains(13, 14),
        contains(14, 15),
    ];
    (nodes, edges)
}

/// The layout the loaded Java descriptor declares — read from the registry, so
/// this directory spells no language id (the `jvm_parity` guard).
fn java() -> PackageLayout {
    let tmp = tempfile::tempdir().expect("tempdir");
    PackageLayout::from_registry(&LanguageRegistry::load(tmp.path()).expect("registry loads"))
}

/// A ledger row of `file_id`, sourced from node `source`. A single-type import
/// carries its last segment as alias, as extraction records it; a glob none.
fn row(
    id: i64,
    file_id: i64,
    source: i64,
    target: &str,
    form: RefForm,
    kind: EdgeKind,
) -> UnresolvedRefRow {
    let single_import = kind == EdgeKind::Imports && form == RefForm::Path;
    UnresolvedRefRow {
        id,
        file_id: Some(file_id),
        source_symbol: format!("local sym{source}"),
        target: target.to_string(),
        alias: single_import.then(|| target.rsplit("::").next().unwrap_or(target).to_string()),
        form,
        kind,
        line: Some(1),
        resolved: false,
        payload: None,
        receiver: None,
    }
}

/// Bind `r` from the class `Svc` (node 8, file `SVC_FILE`) under `layout`, with
/// `imports` as the file's import rows.
fn bind_from_svc(
    layout: PackageLayout,
    imports: &[(&str, RefForm)],
    r: &UnresolvedRefRow,
) -> Outcome {
    let (nodes, edges) = fixture();
    let mut refs: Vec<UnresolvedRefRow> = imports
        .iter()
        .enumerate()
        .map(|(i, (t, form))| row(100 + i as i64, SVC_FILE, 7, t, *form, EdgeKind::Imports))
        .collect();
    refs.push(r.clone());
    let ix = Index::build_with_layout(&nodes, &edges, &refs, layout);
    bind(r, &ix, BindingPolicy::Balanced)
}

/// Bind `r` from the class `Ctl` (node 2) — a file of package `com.x.web`.
fn bind_from_ctl(layout: PackageLayout, r: &UnresolvedRefRow) -> Outcome {
    let (nodes, edges) = fixture();
    let ix = Index::build_with_layout(&nodes, &edges, std::slice::from_ref(r), layout);
    bind(r, &ix, BindingPolicy::Balanced)
}

fn bound(outcome: Outcome, source: i64, target: i64) {
    match outcome {
        Outcome::Bound {
            source: s,
            target: t,
            ..
        } => {
            assert_eq!((s, t), (NodeId(source), NodeId(target)));
        }
        other => panic!("expected {source} → {target}, got {other:?}"),
    }
}

#[test]
fn a_type_of_the_sources_own_package_binds_without_an_import() {
    // `Helper` and `Helper::util` from `com.x.web.Ctl`: the same package's
    // `Helper`, never `com.y.Helper` of the same name.
    let bare = row(1, 10, 2, "Helper", RefForm::Path, EdgeKind::TypeUses);
    bound(bind_from_ctl(java(), &bare), 2, 5);
    let qualified = row(2, 10, 2, "Helper::util", RefForm::Path, EdgeKind::Calls);
    bound(bind_from_ctl(java(), &qualified), 2, 6);
}

#[test]
fn another_packages_type_is_not_in_scope_without_an_import() {
    // From `com.x.svc.Svc`, `Only` names nothing: the one type of that name is in
    // `com.y`, which the file neither is nor imports. The package rung decides
    // the path, so the workspace suffix match — which would find exactly one
    // `go` under a module ending in `Only`, and bind it — is never consulted.
    let r = row(1, SVC_FILE, 8, "Only::go", RefForm::Path, EdgeKind::Calls);
    assert_eq!(bind_from_svc(java(), &[], &r), Outcome::Unbound);
    let bare = row(2, SVC_FILE, 8, "Only", RefForm::Path, EdgeKind::TypeUses);
    assert_eq!(bind_from_svc(java(), &[], &bare), Outcome::Unbound);
    // …and it is in scope the moment the file imports it.
    bound(
        bind_from_svc(java(), &[("com::y::Only", RefForm::Path)], &r),
        8,
        15,
    );
}

#[test]
fn a_package_wildcard_brings_that_packages_types_into_scope() {
    let r = row(
        1,
        SVC_FILE,
        8,
        "Helper::util",
        RefForm::Path,
        EdgeKind::Calls,
    );
    bound(
        bind_from_svc(java(), &[("com::y", RefForm::Glob)], &r),
        8,
        12,
    );
    bound(
        bind_from_svc(java(), &[("com::x::web", RefForm::Glob)], &r),
        8,
        6,
    );
    // Two wildcards that both bring a `Helper` into view: an ambiguity.
    assert_eq!(
        bind_from_svc(
            java(),
            &[("com::y", RefForm::Glob), ("com::x::web", RefForm::Glob)],
            &r
        ),
        Outcome::Unbound
    );
}

#[test]
fn a_single_type_import_shadows_a_wildcard_and_the_package_rung() {
    // `import com.y.Helper; import com.x.web.*;` — the single-type import wins.
    let r = row(
        1,
        SVC_FILE,
        8,
        "Helper::util",
        RefForm::Path,
        EdgeKind::Calls,
    );
    bound(
        bind_from_svc(
            java(),
            &[
                ("com::y::Helper", RefForm::Path),
                ("com::x::web", RefForm::Glob),
            ],
            &r,
        ),
        8,
        12,
    );
}

#[test]
fn the_sources_own_package_shadows_a_wildcard() {
    // `com.x.web.Ctl` with `import com.y.*;`: `Helper` is its own package's,
    // never the wildcard's `com.y.Helper` (the language's shadowing order).
    let (nodes, edges) = fixture();
    let glob = row(100, 10, 1, "com::y", RefForm::Glob, EdgeKind::Imports);
    let r = row(1, 10, 2, "Helper::util", RefForm::Path, EdgeKind::Calls);
    let refs = [glob, r.clone()];
    let ix = Index::build_with_layout(&nodes, &edges, &refs, java());
    bound(bind(&r, &ix, BindingPolicy::Balanced), 2, 6);
}

#[test]
fn a_package_wildcards_own_row_binds_nothing_and_a_type_wildcards_binds_the_type() {
    let pkg = row(
        1,
        SVC_FILE,
        7,
        "com::x::web",
        RefForm::Glob,
        EdgeKind::Imports,
    );
    assert_eq!(bind_from_svc(java(), &[], &pkg), Outcome::Unbound);
    let ty = row(
        2,
        SVC_FILE,
        7,
        "com::x::web::Helper",
        RefForm::Glob,
        EdgeKind::Imports,
    );
    bound(bind_from_svc(java(), &[], &ty), 7, 5);
}

#[test]
fn without_the_layout_no_package_rung_exists() {
    // The descriptor data is the switch (NFR-MA-01): the same snapshot under the
    // default model binds neither the same-package name nor the import to the
    // type it names.
    let bare = row(1, 10, 2, "Helper", RefForm::Path, EdgeKind::TypeUses);
    assert_eq!(
        bind_from_ctl(PackageLayout::default(), &bare),
        Outcome::Unbound
    );
    let import = row(
        2,
        SVC_FILE,
        7,
        "com::x::web::Helper",
        RefForm::Path,
        EdgeKind::Imports,
    );
    // The path model's workspace fallback reads the import as a module path:
    // it reaches the file module `Helper` whose parent key ends in
    // `com::x::web` (S-519's parent-key match), never the class — naming a
    // type is the package rung's alone.
    bound(bind_from_svc(PackageLayout::default(), &[], &import), 7, 4);
    bound(bind_from_svc(java(), &[], &import), 7, 5);
}

/// The index of the fixture plus `extra` nodes (each contained by the node
/// paired with it) and the ledger rows `imports` and `r`.
fn svc_index(extra: &[(NodeRow, i64)], imports: &[UnresolvedRefRow], r: &UnresolvedRefRow) -> Index {
    let (mut nodes, mut edges) = fixture();
    for (n, parent) in extra {
        if *parent != 0 {
            edges.push(contains(*parent, n.id.0));
        }
        nodes.push(n.clone());
    }
    let mut refs = imports.to_vec();
    refs.push(r.clone());
    Index::build_with_layout(&nodes, &edges, &refs, java())
}

/// Bind `r` from the class `Svc` (node 8) under `policy`, over the fixture plus
/// `extra` nodes and `imports`.
fn bind_from_svc_under(
    policy: BindingPolicy,
    extra: &[(NodeRow, i64)],
    imports: &[UnresolvedRefRow],
    r: &UnresolvedRefRow,
) -> Outcome {
    bind(r, &svc_index(extra, imports, r), policy)
}

#[test]
fn the_package_rungs_workspace_fallback_is_aggressive_only_and_never_for_a_receiver_call() {
    // `go` is declared once in the workspace (`com.y.Only.go`) and nowhere in
    // `com.x.svc`'s scope. A bare `go()` from `Svc` binds to it only under the
    // aggressive policy's name fallback — never under balanced (FR-RS-03), and
    // never for a receiver call `x.go()` at any policy (CR-066, FR-RS-06).
    let bare = row(1, SVC_FILE, 8, "go", RefForm::Path, EdgeKind::Calls);
    assert_eq!(
        bind_from_svc_under(BindingPolicy::Balanced, &[], &[], &bare),
        Outcome::Unbound
    );
    bound(
        bind_from_svc_under(BindingPolicy::Aggressive, &[], &[], &bare),
        8,
        15,
    );
    let receiver = row(2, SVC_FILE, 8, "go", RefForm::Method, EdgeKind::Calls);
    assert_eq!(
        bind_from_svc_under(BindingPolicy::Aggressive, &[], &[], &receiver),
        Outcome::Unbound
    );
}

#[test]
fn an_ambiguous_static_wildcard_that_could_supply_the_name_stops_the_aggressive_fallback() {
    // `import static com.y.Only.*` where `com.y.Only` is declared twice (a
    // `src/test` copy without `go`): the main declaration could supply `go`, so
    // the wildcard's ambiguity is final — the aggressive name fallback, which
    // would find the one `go` in the workspace, must not overrule it
    // (NFR-RA-05).
    let dup_file = "src/test/java/com/y/Only.java";
    let extra = [
        (node(30, "Only", NodeKind::Module, dup_file), 0),
        (node(31, "Only", NodeKind::Class, dup_file), 30),
    ];
    let mut glob = row(
        100,
        SVC_FILE,
        7,
        "com::y::Only",
        RefForm::Glob,
        EdgeKind::Imports,
    );
    glob.alias = Some(super::STATIC_WILDCARD_ALIAS.to_string());
    let bare = row(1, SVC_FILE, 8, "go", RefForm::Path, EdgeKind::Calls);
    assert_eq!(
        bind_from_svc_under(BindingPolicy::Aggressive, &extra, &[glob], &bare),
        Outcome::Unbound
    );
    // Without the duplicate, the same wildcard supplies `go` outright.
    let mut glob = row(
        100,
        SVC_FILE,
        7,
        "com::y::Only",
        RefForm::Glob,
        EdgeKind::Imports,
    );
    glob.alias = Some(super::STATIC_WILDCARD_ALIAS.to_string());
    bound(
        bind_from_svc_under(BindingPolicy::Balanced, &[], &[glob], &bare),
        8,
        15,
    );
}

/// A capture-before-delete `Extends` row targets a whole symbol, whose path
/// segments (`src`, `main`, `java`, the package) every Java file shares. It
/// adds no hierarchy token, so a sync of an unrelated Java file does not
/// re-select every package-shaped call; the source's own `Path` row still
/// names its supertype's token (sprint-81 review).
#[test]
fn a_captured_extends_row_adds_no_hierarchy_token() {
    let (nodes, edges) = fixture();
    let dirty: std::collections::HashSet<String> = ["src/main/java/com/y/Only.java", "Only"]
        .iter()
        .flat_map(|s| super::tokens(s))
        .collect();
    let path_row = row(1, SVC_FILE, 8, "Base", RefForm::Path, EdgeKind::Extends);
    let captured = row(
        2,
        SVC_FILE,
        8,
        "logos . . . src/main/java/com/x/base/`Base.java`/Base#",
        RefForm::Symbol,
        EdgeKind::Extends,
    );
    let ix = Index::build_with_layout(&nodes, &edges, &[path_row.clone(), captured], java());
    assert!(!ix.hierarchy_touched(&dirty), "an unrelated Java file moved the hierarchy");
    let base: std::collections::HashSet<String> = super::tokens("Base").into_iter().collect();
    let ix = Index::build_with_layout(&nodes, &edges, &[path_row], java());
    assert!(ix.hierarchy_touched(&base), "the Path row's own target still counts");
}

/// A single-type import row of `Svc`'s file.
fn import_of(id: i64, target: &str) -> UnresolvedRefRow {
    row(id, SVC_FILE, 7, target, RefForm::Path, EdgeKind::Imports)
}

/// A nested `Inner` class under each `Helper` (nodes 40 and 41) and a lexical
/// member type `Helper` of `Svc` itself, with its own `util` (nodes 50, 51).
fn nested_and_lexical() -> Vec<(NodeRow, i64)> {
    let web = "src/main/java/com/x/web/Helper.java";
    let y = "src/main/java/com/y/Helper.java";
    let svc = "src/main/java/com/x/svc/Svc.java";
    vec![
        (node(40, "Inner", NodeKind::Class, web), 5),
        (node(41, "Inner", NodeKind::Class, y), 11),
        (node(50, "Helper", NodeKind::Class, svc), 8),
        (node(51, "util", NodeKind::Method, svc), 50),
    ]
}

/// The two rival imports of one simple name (`import com.x.web.Helper; import
/// com.y.Helper;`) and a call through the head.
fn rival_imports() -> [UnresolvedRefRow; 2] {
    [import_of(100, "com::x::web::Helper"), import_of(101, "com::y::Helper")]
}

/// The reason the call `r` from `Svc` stays unbound, under the fixture and
/// `extra` nodes.
fn residue_from_svc(extra: &[(NodeRow, i64)], imports: &[UnresolvedRefRow], r: &UnresolvedRefRow) -> Option<Residue> {
    residue(r, &svc_index(extra, imports, r), BindingPolicy::Balanced)
}

/// Two imports of one simple name name two declarations (S-599): the head
/// `Helper` is read under both, never the first import's alone. `Helper.util()`
/// binds nothing and records why.
#[test]
fn a_qualified_call_through_two_rival_imports_binds_nothing_and_is_type_ambiguous() {
    let call = row(1, SVC_FILE, 8, "Helper::util", RefForm::Path, EdgeKind::Calls);
    for policy in [BindingPolicy::Strict, BindingPolicy::Balanced, BindingPolicy::Aggressive] {
        assert_eq!(
            bind_from_svc_under(policy, &[], &rival_imports(), &call),
            Outcome::Unbound,
            "{policy:?}"
        );
    }
    assert_eq!(residue_from_svc(&[], &rival_imports(), &call), Some(Residue::TypeAmbiguous));
    // The order the imports are written in never decides.
    let mut reversed = rival_imports();
    reversed.reverse();
    assert_eq!(
        bind_from_svc_under(BindingPolicy::Balanced, &[], &reversed, &call),
        Outcome::Unbound
    );
}

/// The same head in an `extends` clause: `Helper.Inner` names two nested types.
#[test]
fn a_qualified_supertype_through_two_rival_imports_binds_nothing() {
    let extra = &nested_and_lexical()[..2];
    let r = row(1, SVC_FILE, 8, "Helper::Inner", RefForm::Path, EdgeKind::Extends);
    assert_eq!(
        bind_from_svc_under(BindingPolicy::Balanced, extra, &rival_imports(), &r),
        Outcome::Unbound
    );
    // A type relation is no call: it records no reason, ambiguous or not.
    assert_eq!(residue_from_svc(extra, &rival_imports(), &r), None);
    // One import of either alone binds its own `Inner`.
    let [web, y] = rival_imports();
    bound(bind_from_svc_under(BindingPolicy::Balanced, extra, &[web], &r), 8, 40);
    bound(bind_from_svc_under(BindingPolicy::Balanced, extra, &[y], &r), 8, 41);
}

/// Imports that reach one declaration are not rivals: a single import and the
/// same import repeated verbatim bind the one declaration, as before.
#[test]
fn imports_that_reach_one_declaration_bind_as_before() {
    let call = row(1, SVC_FILE, 8, "Helper::util", RefForm::Path, EdgeKind::Calls);
    let y = || import_of(100, "com::y::Helper");
    bound(bind_from_svc_under(BindingPolicy::Balanced, &[], &[y()], &call), 8, 12);
    let repeated = [y(), import_of(101, "com::y::Helper"), import_of(102, "com::y::Helper")];
    bound(bind_from_svc_under(BindingPolicy::Balanced, &[], &repeated, &call), 8, 12);
}

/// A rival that names no in-repository type (a library `Helper`) is no second
/// declaration: the rungs skip it as S-519's rival rule does, so the one
/// in-repository `Helper` binds in either import order — the first-wins map
/// bound it only when its import came first.
#[test]
fn a_rival_import_naming_no_repository_type_never_hides_the_one_that_does() {
    let call = row(1, SVC_FILE, 8, "Helper::util", RefForm::Path, EdgeKind::Calls);
    let library = import_of(100, "org::lib::Helper");
    let y = import_of(101, "com::y::Helper");
    for imports in [[library.clone(), y.clone()], [y, library]] {
        bound(bind_from_svc_under(BindingPolicy::Balanced, &[], &imports, &call), 8, 12);
    }
}

/// A member type in lexical scope is read before any import (JLS §6.5.5), so
/// rival imports of its name never reach the rung.
#[test]
fn a_lexical_member_type_still_wins_over_rival_imports() {
    let extra = nested_and_lexical();
    let call = row(1, SVC_FILE, 8, "Helper::util", RefForm::Path, EdgeKind::Calls);
    bound(bind_from_svc_under(BindingPolicy::Balanced, &extra, &rival_imports(), &call), 8, 51);
}

/// Repeating the rival pair costs what one pair costs: every distinct import is
/// read once, a copy never again, wherever it sits in the file.
#[test]
fn rival_imports_repeated_cost_what_one_pair_costs() {
    let call = row(1, SVC_FILE, 8, "Helper::util", RefForm::Path, EdgeKind::Calls);
    let with = |pairs: i64| {
        let refs: Vec<UnresolvedRefRow> = (0..pairs)
            .flat_map(|n| {
                [
                    import_of(100 + 2 * n, "com::x::web::Helper"),
                    import_of(101 + 2 * n, "com::y::Helper"),
                ]
            })
            .collect();
        let ix = svc_index(&[], &refs, &call);
        bind_counting_path_visits(&call, &ix, BindingPolicy::Balanced)
    };
    let (once, once_visits) = with(1);
    let (repeated, repeated_visits) = with(8);
    assert_eq!(once, Outcome::Unbound);
    assert_eq!(repeated, once);
    assert_eq!(repeated_visits, once_visits, "repeated rival imports did more work");
    // …and it is the rivals that cost: a pair visits more than a lone import.
    let lone = {
        let ix = svc_index(&[], &[import_of(100, "com::y::Helper")], &call);
        bind_counting_path_visits(&call, &ix, BindingPolicy::Balanced).1
    };
    assert!(once_visits > lone, "a pair ({once_visits}) must read both imports, a lone import ({lone}) one");
}

/// A rival whose own walk is ambiguous is no reason to bind the other: web's
/// `Helper` declares two `Inner` types (nodes 40, 42) and y's one (41), so the
/// head's rivals are not "one declaration and nothing" — the sticky ambiguity
/// of the first stays, whichever import is written first.
#[test]
fn an_ambiguous_walk_under_one_rival_import_is_not_hidden_by_the_other() {
    let web = "src/main/java/com/x/web/Helper.java";
    let mut extra = nested_and_lexical()[..2].to_vec();
    extra.push((node(42, "Inner", NodeKind::Class, web), 5));
    let r = row(1, SVC_FILE, 8, "Helper::Inner", RefForm::Path, EdgeKind::Extends);
    let [web_import, y_import] = rival_imports();
    for imports in [[web_import.clone(), y_import.clone()], [y_import, web_import]] {
        assert_eq!(
            bind_from_svc_under(BindingPolicy::Balanced, &extra, &imports, &r),
            Outcome::Unbound
        );
    }
}

/// Rival imports of names that are themselves heads of each other's paths
/// (`import a.a; import b.a; import a.b; import b.b;` and `a.X.m()`) must not
/// fan out per rival at every alias level: the work stays flat as the imports
/// multiply, and the call binds nothing.
#[test]
fn rival_heads_met_inside_rival_expansions_cost_no_fan_out() {
    let call = row(1, SVC_FILE, 8, "a::X::m", RefForm::Path, EdgeKind::Calls);
    let with = |names: &[&str], rivals: &[&str]| {
        let refs: Vec<UnresolvedRefRow> = names
            .iter()
            .flat_map(|n| rivals.iter().map(move |r| format!("{r}::{n}")))
            .enumerate()
            .map(|(i, target)| import_of(100 + i as i64, &target))
            .collect();
        let ix = svc_index(&[], &refs, &call);
        bind_counting_path_visits(&call, &ix, BindingPolicy::Balanced)
    };
    let (small, small_visits) = with(&["a", "b"], &["a", "b"]);
    let (large, large_visits) = with(&["a", "b", "c", "d"], &["a", "b", "c", "d"]);
    assert_eq!((small, large), (Outcome::Unbound, Outcome::Unbound));
    // Unguarded the 16 imports cost 87,381 visits and the 4 cost 511 (N^depth).
    assert!(small_visits < 50, "4 rival imports took {small_visits} visits");
    assert!(large_visits < 50, "16 rival imports took {large_visits} visits");
}
