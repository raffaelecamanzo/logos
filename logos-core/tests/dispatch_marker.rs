//! Trait fan-out edges to implementations are marked dispatch (S-712, CR-226
//! BR-76, FR-RS-08, FR-RS-47 rule 5) — end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with the SHIPPED Rust,
//! Go and Terraform plugins.
//!
//! - A call through a trait binds every implementation of the method plus the
//!   trait's own default, as before; each edge to an implementation carries
//!   `dispatch:<trait method symbol>`, the trait method's edge carries none.
//! - A direct call from the same caller to an implementation unmarks its edge,
//!   in a sync and in a fresh index alike, and its removal marks it again.
//! - A store bound before the marker is re-bound by migration 43 and its first
//!   reconcile; every re-index is byte-identical.
//! - `callers`, `callees` and `impact` label a row reached only through
//!   dispatch `via: "<Trait>::<method>"`; every other row is unchanged.
//! - Go package-path and Terraform module fan-outs carry no marker.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::graph_store::AnnotationNodeRow;
use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

#[path = "support/dispatch_crates.rs"]
mod dispatch_crates;
use dispatch_crates::{two_crates, A_LIB, A_LIB_DIRECT, B_LIB};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    tmp
}

fn index(root: &Path) -> Engine {
    coordinate::pin(root);
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`, by id and by symbol.
fn labels(rt: &Runtime) -> (HashMap<NodeId, String>, HashMap<String, String>) {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let label = |n: &logos_core::graph_store::NodeRow| {
        format!("{}:{}@{}", n.file_path.as_deref().unwrap_or_default(), n.name, n.start_line.unwrap_or(0))
    };
    (
        nodes.iter().map(|n| (n.id, label(n))).collect(),
        nodes.iter().map(|n| (n.symbol.as_str().to_string(), label(n))).collect(),
    )
}

/// Every `Calls` edge out of `caller`, by target label, with the label of the
/// trait method its dispatch marker names — `None` for an unmarked edge.
fn calls_from(rt: &Runtime, caller: &str) -> BTreeMap<String, Option<String>> {
    let (by_id, by_symbol) = labels(rt);
    let marked: HashMap<(NodeId, NodeId), String> = rt
        .submit_read(|store| store.dispatch_links())
        .expect("read runs")
        .into_iter()
        .map(|(source, target, method)| ((source, target), method))
        .collect();
    rt.submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls && by_id[&e.source] == caller)
        .map(|e| {
            let via = marked.get(&(e.source, e.target)).map(|m| by_symbol.get(m).cloned().unwrap_or_else(|| m.clone()));
            (by_id[&e.target].clone(), via)
        })
        .collect()
}

fn expect(pairs: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    pairs.iter().map(|(t, via)| ((*t).to_string(), via.map(str::to_string))).collect()
}

const BY_DYN: &str = "a/src/lib.rs:by_dyn@5";
const BY_IMPL: &str = "a/src/lib.rs:by_impl@6";
const TR_M: &str = "a/src/lib.rs:m@2";
const TR_D: &str = "a/src/lib.rs:d@3";
const B_M: &str = "b/src/lib.rs:m@4";
const B_D: &str = "b/src/lib.rs:d@5";

/// The fan-outs of the unedited fixture: every edge into `b` is marked, by the
/// trait method it went through — the required signature `m` included, which
/// is never itself a target — and the edge to the trait's default `d` is not.
fn marked_fan_outs() -> [(&'static str, BTreeMap<String, Option<String>>); 2] {
    [
        (BY_DYN, expect(&[(B_M, Some(TR_M)), (B_D, Some(TR_D)), (TR_D, None)])),
        (BY_IMPL, expect(&[(B_M, Some(TR_M))])),
    ]
}

#[test]
fn every_implementor_edge_of_a_two_crate_fan_out_is_marked_and_the_trait_methods_is_not() {
    let tmp = two_crates(A_LIB);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    for (caller, want) in marked_fan_outs() {
        assert_eq!(calls_from(rt, caller), want, "{caller}");
    }
}

#[test]
fn a_direct_call_unmarks_its_edge_in_a_sync_and_a_fresh_index_and_its_removal_marks_it_again() {
    let tmp = two_crates(A_LIB);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    write(tmp.path(), "a/src/lib.rs", A_LIB_DIRECT);
    let synced = engine.sync(&[PathBuf::from("a/src/lib.rs")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    // The direct `b::B::m(..)` binds the edge the fan-out marked: it is a
    // dependency now, and the target set is the one it was.
    assert_eq!(calls_from(rt, BY_IMPL), expect(&[(B_M, None)]));
    assert_eq!(calls_from(rt, BY_DYN), marked_fan_outs()[0].1, "another caller keeps its marks");

    let fresh = two_crates(A_LIB_DIRECT);
    let cold = index(fresh.path());
    assert_eq!(calls_from(cold.runtime().unwrap(), BY_IMPL), expect(&[(B_M, None)]), "a fresh index agrees");
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(cold.runtime().unwrap()), "sync ≡ reindex");

    write(tmp.path(), "a/src/lib.rs", A_LIB);
    let reverted = engine.sync(&[PathBuf::from("a/src/lib.rs")]);
    assert!(reverted.warnings.is_empty(), "{:?}", reverted.warnings);
    assert_eq!(calls_from(rt, BY_IMPL), marked_fan_outs()[1].1, "the direct call gone, the edge is dispatch again");
}

/// An edit inside the implementation's body re-extracts `b` alone. Every name
/// `b` declares is dirty, so each fan-out row spelling `m` or `d` is re-bound
/// and marks its edge again; the capture-before-delete rows of those edges
/// restore nothing over it, carrying no marker of their own.
#[test]
fn an_edit_to_the_implementation_keeps_its_inbound_edges_marked() {
    let tmp = two_crates(A_LIB);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let edited = B_LIB.replace("fn m(&self) {}", "fn m(&self) { let _ = 1; }");
    write(tmp.path(), "b/src/lib.rs", &edited);
    let synced = engine.sync(&[PathBuf::from("b/src/lib.rs")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    for (caller, want) in marked_fan_outs() {
        assert_eq!(calls_from(rt, caller), want, "{caller} after the sync");
    }

    let fresh = two_crates(A_LIB);
    write(fresh.path(), "b/src/lib.rs", &edited);
    let cold = index(fresh.path());
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(cold.runtime().unwrap()), "sync ≡ reindex");
}

/// Both files changed in one sync, the direct call removed: the capture of the
/// edge it left unmarked must not unmark the fan-out the fresh rows bind. The
/// implementation's file is synced first, so that capture exists — the
/// caller's file first would cascade the edge away before it is captured.
#[test]
fn a_capture_of_an_unmarked_edge_never_outlives_the_direct_call_that_left_it() {
    let tmp = two_crates(A_LIB_DIRECT);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(calls_from(rt, BY_IMPL), expect(&[(B_M, None)]), "precondition: the direct call unmarks");
    let edited = B_LIB.replace("fn m(&self) {}", "fn m(&self) { let _ = 1; }");
    write(tmp.path(), "a/src/lib.rs", A_LIB);
    write(tmp.path(), "b/src/lib.rs", &edited);
    let synced = engine.sync(&[PathBuf::from("b/src/lib.rs"), PathBuf::from("a/src/lib.rs")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert_eq!(calls_from(rt, BY_IMPL), marked_fan_outs()[1].1);

    let fresh = two_crates(A_LIB);
    write(fresh.path(), "b/src/lib.rs", &edited);
    let cold = index(fresh.path());
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(cold.runtime().unwrap()), "sync ≡ reindex");
}

#[test]
fn a_re_index_is_byte_identical_across_runs() {
    let one = two_crates(A_LIB);
    let two = two_crates(A_LIB);
    let (first, second) = (index(one.path()), index(two.path()));
    let fingerprint = graph_fingerprint(first.runtime().unwrap());
    assert!(fingerprint.contains("\nD "), "the fingerprint holds the markers: {fingerprint}");
    assert_eq!(fingerprint, graph_fingerprint(second.runtime().unwrap()));
    // And re-indexing the same store over itself moves nothing.
    let again = first.index();
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(graph_fingerprint(first.runtime().unwrap()), fingerprint);
}

fn dead(rt: &Runtime) -> BTreeMap<String, Option<bool>> {
    rt.submit_read(|store| store.annotation_nodes())
        .expect("read runs")
        .into_iter()
        .filter(|n: &AnnotationNodeRow| !n.derived)
        .map(|n| (format!("{}@{:?}", n.name, n.kind), n.is_dead))
        .collect()
}

/// A store bound before the marker holds every fan-out edge unmarked. Its
/// upgrade — migration 43, then the first reconcile — leaves none unmarked,
/// and moves no edge and no dead-code verdict.
#[test]
fn a_pre_change_store_carries_no_unmarked_implementor_edge_after_its_migration_and_first_reconcile() {
    let tmp = two_crates(A_LIB);
    let engine = index(tmp.path());
    let marked = graph_fingerprint(engine.runtime().unwrap());
    let verdicts = dead(engine.runtime().unwrap());
    drop(engine);

    // Back to what the release before S-712 left on disk: the same edges,
    // none of them marked, migrations 43 and 44 (S-571, the
    // `files.parse_damaged` column and the snapshot entries' `partial_parse`
    // flags) unrecorded, `user_version` 42.
    let conn = rusqlite::Connection::open(tmp.path().join(".logos").join("logos.db")).unwrap();
    let stripped = conn.execute("UPDATE edges SET payload = NULL WHERE payload GLOB 'dispatch:*'", []).unwrap();
    assert_eq!(stripped, 3, "precondition: the fixture's three implementor edges were marked");
    conn.execute_batch(
        "ALTER TABLE files DROP COLUMN parse_damaged; ALTER TABLE metric_snapshot_offenders DROP COLUMN partial_parse; \
         ALTER TABLE metric_snapshot_offenders_uncapped DROP COLUMN partial_parse; \
         ALTER TABLE metric_snapshot_fix_units DROP COLUMN partial_parse; DELETE FROM schema_versions WHERE version = 44; \
         DELETE FROM schema_versions WHERE version = 43; PRAGMA user_version = 42;",
    )
        .expect("downgrade the store to v42");
    drop(conn);

    let engine = Engine::start(tmp.path()).expect("engine reopens, applying migration 43");
    let rt = engine.runtime().unwrap();
    assert!(rt.submit_read(|s| s.dispatch_links()).unwrap().is_empty(), "the upgrade alone marks nothing");
    let unhashed = rt.submit_read(|s| s.indexed_files()).unwrap().iter().filter(|f| f.content_hash.is_none()).count();
    assert!(unhashed >= 2, "every file awaits re-extraction: {unhashed}");

    engine.health(true).expect("a full-walk reconcile runs");
    for (caller, want) in marked_fan_outs() {
        assert_eq!(calls_from(rt, caller), want, "{caller} after the first reconcile");
    }
    assert_eq!(graph_fingerprint(rt), marked, "the upgraded store is a fresh index's");
    assert_eq!(dead(rt), verdicts, "no dead-code verdict moves");
}

#[test]
fn callers_callees_and_impact_label_a_dispatch_row_via_its_trait_method() {
    let tmp = two_crates(A_LIB_DIRECT);
    let engine = index(tmp.path());
    let (_, by_symbol) = labels(engine.runtime().unwrap());
    let symbol_of = |label: &str| by_symbol.iter().find(|(_, l)| *l == label).map(|(s, _)| s.clone()).unwrap();
    let (b_m, by_dyn) = (symbol_of(B_M), symbol_of(BY_DYN));
    let via = |rows: &[logos_core::models::SymbolRef]| -> BTreeMap<String, Option<String>> {
        rows.iter().map(|r| (r.name.clone(), r.via.clone())).collect()
    };

    // `by_dyn` reaches `B::m` only by dispatch; `by_impl` calls it directly too.
    let callers = engine.callers(&b_m, None);
    assert!(callers.resolved.is_some(), "{callers:?}");
    let want: BTreeMap<String, Option<String>> =
        [("by_dyn".to_string(), Some("Tr::m".to_string())), ("by_impl".to_string(), None)].into();
    assert_eq!(via(&callers.callers), want);

    let callees = engine.callees(&by_dyn, None);
    let mut got: Vec<(String, Option<String>)> =
        callees.callees.iter().map(|r| (r.file.clone().unwrap_or_default(), r.via.clone())).collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("a/src/lib.rs".to_string(), None),
            ("b/src/lib.rs".to_string(), Some("Tr::d".to_string())),
            ("b/src/lib.rs".to_string(), Some("Tr::m".to_string())),
        ],
        "the trait's own default is a dependency, the impls are dispatch"
    );

    let impact = engine.impact(&b_m, None);
    let upstream: BTreeMap<String, (u32, Option<String>)> =
        impact.upstream.iter().map(|e| (e.symbol.name.clone(), (e.distance, e.symbol.via.clone()))).collect();
    assert_eq!(upstream.get("by_dyn"), Some(&(1, Some("Tr::m".to_string()))), "{upstream:?}");
    assert_eq!(upstream.get("by_impl"), Some(&(1, None)), "{upstream:?}");

    // Downstream, the callee rows of the same walk: the impls by dispatch, the
    // trait's own default as a dependency.
    let impact = engine.impact(&by_dyn, Some(1));
    let mut downstream: Vec<(String, u32, Option<String>)> = impact
        .downstream
        .iter()
        .map(|e| (e.symbol.file.clone().unwrap_or_default(), e.distance, e.symbol.via.clone()))
        .collect();
    downstream.sort();
    assert_eq!(
        downstream,
        [
            ("a/src/lib.rs".to_string(), 1, None),
            ("b/src/lib.rs".to_string(), 1, Some("Tr::d".to_string())),
            ("b/src/lib.rs".to_string(), 1, Some("Tr::m".to_string())),
        ]
    );

    // On the wire the label is additive: a row reached otherwise carries no key.
    let wire = serde_json::to_value(&callers).unwrap();
    let rows = wire["callers"].as_array().unwrap();
    let by_name = |name: &str| rows.iter().find(|r| r["name"] == name).unwrap();
    assert_eq!(by_name("by_dyn")["via"], "Tr::m");
    assert!(by_name("by_impl").get("via").is_none(), "{wire}");
}

/// The edges out of sources labelled `from…`, of `kind`, in the store of `rt`
/// at `root`, and how many edges of any kind carry a dispatch marker — read
/// off the table itself, since `dispatch_links` reads `Calls` edges alone.
fn fanned(root: &Path, rt: &Runtime, kind: EdgeKind, from: &str) -> (usize, i64) {
    let (by_id, _) = labels(rt);
    let edges = rt.submit_read(|s| s.all_edges()).unwrap();
    let count = edges.iter().filter(|e| e.kind == kind && by_id[&e.source].starts_with(from)).count();
    let conn = rusqlite::Connection::open(root.join(".logos").join("logos.db")).unwrap();
    let marked = conn
        .query_row("SELECT count(*) FROM edges WHERE payload GLOB 'dispatch:*'", [], |r| r.get(0))
        .unwrap();
    (count, marked)
}

#[test]
fn a_go_package_path_fan_out_carries_no_marker() {
    let tmp = tree(&[
        ("go.mod", "module example.com/m\n\ngo 1.21\n"),
        ("util/a.go", "package util\n\nfunc A() {}\n"),
        ("util/b.go", "package util\n\nfunc B() {}\n"),
        ("main.go", "package main\n\nimport \"example.com/m/util\"\n\nfunc main() { util.A() }\n"),
    ]);
    let engine = index(tmp.path());
    // The import names a package and binds each of its files — and none of
    // those edges is dispatch.
    assert_eq!(fanned(tmp.path(), engine.runtime().unwrap(), EdgeKind::Imports, "main.go:"), (2, 0));
}

/// Terraform is a feature-gated plugin: compiled only where it is on, so a
/// build without it runs no vacuous half of this test.
#[cfg(feature = "lang-terraform")]
#[test]
fn a_terraform_module_fan_out_carries_no_marker() {
    let tmp = tree(&[
        ("main.tf", "module \"net\" {\n  source = \"./modules/net\"\n}\n"),
        ("modules/net/main.tf", "variable \"a\" {}\n"),
        ("modules/net/outputs.tf", "output \"b\" { value = 1 }\n"),
    ]);
    let engine = index(tmp.path());
    assert_eq!(fanned(tmp.path(), engine.runtime().unwrap(), EdgeKind::ArtifactRef, "main.tf:"), (2, 0));
}

// ── An edge that survives a sync carries the marker a fresh index gives it ───
//
// A sync re-binds every row of a source whose binding may move ("swept"), but
// re-extracts only the changed files: the edges out of a swept source in an
// unchanged file survive. Each must leave the sync marked as its fresh rows
// mark it — not as an earlier run left it (S-712 review).

/// The two-crate workspace manifests, beside `files`.
fn workspace(files: &[(&str, &str)]) -> TempDir {
    let mut all: Vec<(&str, &str)> = vec![
        ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
        ("a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        (
            "b/Cargo.toml",
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\na = { path = \"../a\" }\n",
        ),
    ];
    all.extend_from_slice(files);
    tree(&all)
}

/// Index `before`, write `after` over it and sync those files; then index the
/// edited tree cold. The synced store must be the fresh one, markers included.
fn sync_is_reindex(before: &[(&str, &str)], after: &[(&str, &str)]) {
    let tmp = workspace(before);
    let engine = index(tmp.path());
    for (rel, text) in after {
        write(tmp.path(), rel, text);
    }
    let paths: Vec<PathBuf> = after.iter().map(|(rel, _)| PathBuf::from(rel)).collect();
    let synced = engine.sync(&paths);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    let mut edited: BTreeMap<&str, &str> = before.iter().copied().collect();
    edited.extend(after.iter().copied());
    let fresh = workspace(&edited.into_iter().collect::<Vec<_>>());
    let cold = index(fresh.path());
    assert_eq!(graph_fingerprint(engine.runtime().unwrap()), graph_fingerprint(cold.runtime().unwrap()));
}

const B_IMP: &str = "use a::Tr;\npub struct B;\nimpl Tr for B {\n    fn m(&self) {}\n}\n";

/// The direct call stops binding through an edit in a third file — the
/// re-export it named is removed — so only the fan-out produces the edge,
/// and it is dispatch again.
#[test]
fn an_edge_a_direct_call_stops_producing_from_another_file_is_marked_again() {
    sync_is_reindex(
        &[
            ("a/src/lib.rs", "pub trait Tr {\n    fn m(&self);\n}\npub fn by_impl(t: &impl Tr) { t.m(); b::X::m(&b::X); }\n"),
            ("b/src/lib.rs", "pub mod imp;\npub use imp::B as X;\n"),
            ("b/src/imp.rs", B_IMP),
        ],
        &[("b/src/lib.rs", "pub mod imp;\n")],
    );
}

/// The trait moves into an inline module of its own file: the marker names
/// the trait method's new symbol, never the one no node holds any more.
#[test]
fn a_marker_follows_its_trait_method_to_its_new_symbol() {
    sync_is_reindex(
        &[
            ("a/src/lib.rs", "mod tr;\npub use tr::Tr;\npub fn by_dyn(t: &dyn Tr) { t.m(); }\n"),
            ("a/src/tr.rs", "pub trait Tr {\n    fn m(&self);\n}\n"),
            ("b/src/lib.rs", B_IMP),
        ],
        &[("a/src/tr.rs", "pub mod z {\n    pub trait Tr {\n        fn m(&self);\n    }\n}\npub use z::Tr;\n")],
    );
}

/// A new inherent `m` in another file takes the direct call over: the edge to
/// the trait impl is produced by the fan-out alone, and marked.
#[test]
fn an_edge_a_direct_call_moves_away_from_is_marked_again() {
    sync_is_reindex(
        &[
            ("a/src/lib.rs", "pub trait Tr {\n    fn m(&self);\n}\n"),
            ("b/src/lib.rs", "pub mod imp;\npub mod extra;\npub mod caller;\n"),
            ("b/src/imp.rs", "use a::Tr;\npub struct B;\nimpl Tr for B {\n    fn m(&self) {}\n}\n"),
            ("b/src/extra.rs", "\n"),
            ("b/src/caller.rs", "use a::Tr;\nuse crate::imp::B;\npub fn by_dyn(t: &dyn Tr) { t.m(); B::m(&B); }\n"),
        ],
        &[("b/src/extra.rs", "impl crate::imp::B {\n    pub fn m(&self) {}\n}\n")],
    );
}

/// A call through the trait from inside the trait — `self.m()` in a default
/// body — and a written `Tr::m(x)` fan out by another route than a typed
/// receiver, and mark the same way.
#[test]
fn a_default_bodys_self_call_and_a_written_trait_path_are_marked() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Tr {
    fn m(&self);
    fn d(&self) { self.m(); }
}
pub struct B;
impl Tr for B {
    fn m(&self) {}
}
pub fn ufcs(b: &B) { Tr::m(b); }
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let marked = expect(&[("src/lib.rs:m@7", Some("src/lib.rs:m@2"))]);
    assert_eq!(calls_from(rt, "src/lib.rs:d@3"), marked, "self.m() in a default body");
    assert_eq!(calls_from(rt, "src/lib.rs:ufcs@9"), marked, "Tr::m(b)");
}

/// An impact entry two hops out is `via` a trait only when every edge into it
/// from the hop before is dispatch: `c` reaches `s` through `T1::m` and through
/// a direct `X::n`, so it carries none; `e` reaches it through two traits and
/// carries the smaller label.
#[test]
fn an_impact_entry_is_via_only_when_every_nearest_edge_is_dispatch() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait T1 { fn m(&self); }
pub trait T2 { fn n(&self); }
pub fn s() {}
pub struct X;
impl T1 for X { fn m(&self) { s(); } }
impl T2 for X { fn n(&self) { s(); } }
pub fn c(a: &dyn T1) { a.m(); X::n(&X); }
pub fn e(a: &dyn T1, b: &dyn T2) { a.m(); b.n(); }
",
    )]);
    let engine = index(tmp.path());
    let (_, by_symbol) = labels(engine.runtime().unwrap());
    let s = by_symbol.iter().find(|(_, l)| *l == "src/lib.rs:s@3").map(|(s, _)| s.clone()).unwrap();
    let upstream: BTreeMap<String, (u32, Option<String>)> = engine
        .impact(&s, Some(2))
        .upstream
        .iter()
        .map(|e| (e.symbol.name.clone(), (e.distance, e.symbol.via.clone())))
        .collect();
    assert_eq!(upstream.get("c"), Some(&(2, None)), "one nearest edge is direct: {upstream:?}");
    assert_eq!(upstream.get("e"), Some(&(2, Some("T1::m".to_string()))), "the smaller of two: {upstream:?}");
}
