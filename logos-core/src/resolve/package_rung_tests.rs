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

use super::binder::{bind, Index, Outcome};
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
    // default model binds neither the same-package name nor the import.
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
    assert_eq!(
        bind_from_svc(PackageLayout::default(), &[], &import),
        Outcome::Unbound
    );
    bound(bind_from_svc(java(), &[], &import), 7, 5);
}

/// Bind `r` from the class `Svc` (node 8) under `policy`, over the fixture plus
/// `extra` nodes (each contained by the node paired with it) and `imports`.
fn bind_from_svc_under(
    policy: BindingPolicy,
    extra: &[(NodeRow, i64)],
    imports: &[UnresolvedRefRow],
    r: &UnresolvedRefRow,
) -> Outcome {
    let (mut nodes, mut edges) = fixture();
    for (n, parent) in extra {
        if *parent != 0 {
            edges.push(contains(*parent, n.id.0));
        }
        nodes.push(n.clone());
    }
    let mut refs = imports.to_vec();
    refs.push(r.clone());
    let ix = Index::build_with_layout(&nodes, &edges, &refs, java());
    bind(r, &ix, policy)
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
