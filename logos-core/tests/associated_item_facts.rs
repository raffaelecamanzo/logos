//! The facts one Rust associated-item lookup reads are persisted (S-606,
//! CR-202 F1–F3, FR-EX-34, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures.
//!
//! Migration 34 records, beside the graph: every impl block's header
//! (`impl_blocks`), each callable's receiver mode, each enum's variant names
//! and the required-signature marker (`nodes`), and whether an import is a
//! re-export (`unresolved_refs.exported`). A trait's required signature is a
//! new bodyless `Method` node; `self.m()` records its `self` shape on the
//! `Self::m` row, and `<T as Tr>::m` its type and trait.
//!
//! None of it binds yet: no `Calls` edge reaches a signature, a `self.m()` in
//! a trait's default body stays unbound as before, and a signature is never
//! reported dead. A one-file edit re-derives every fact through sync to
//! exactly what a fresh index of the edited tree records, and the sync ≡
//! reindex fingerprint sees each fact.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! suite here: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-rust")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind, ReceiverMode};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

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
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}@{}", n.name, n.start_line.unwrap_or(0))))
            .collect())
    })
    .expect("read runs")
}

/// Every edge of `kind` as `(source label, target label)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    let label = labels(rt);
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

const LIB: &str = "src/lib.rs";
const LIB_SRC: &str = "\
pub mod a;
pub struct X;
pub struct Inner;
pub trait Greet {
    fn hello(&self);
    fn meta(&self) -> u8;
    fn twice(&self) { self.hello(); self.meta(); }
}
impl Greet for X {}
impl X {
    pub fn go(&mut self) { self.go2(); Self::go2(self); }
    fn go2(&self) {}
    pub fn new() -> Self { X }
}
impl std::ops::Deref for X {
    type Target = Inner;
    fn deref(&self) -> &Inner { &Inner }
}
pub enum E { A, B(u8), C { x: u8 } }
pub use a::Y;
use std::fmt;
pub fn q(x: X) { <X as Greet>::hello(&x); }
";
const A: &str = "src/a.rs";
const A_SRC: &str = "\
pub struct Y;
impl crate::Greet for Y {
    fn hello(&self) {}
    fn meta(&self) -> u8 { 0 }
}
";

/// One recorded impl block as `(file, start, end, self type, ref, trait,
/// target)`.
type Block = (String, u32, u32, String, bool, Option<String>, Option<String>);

/// Every recorded impl block.
fn impl_blocks(rt: &Runtime) -> Vec<Block> {
    rt.submit_read(|store| store.impl_blocks())
        .expect("read runs")
        .into_iter()
        .map(|b| (b.file_path, b.start_line, b.end_line, b.self_type, b.self_ref, b.trait_path, b.deref_target))
        .collect()
}

/// S-606 / FR-EX-34: every fact is persisted — the impl blocks, an empty one
/// included; the receiver modes, variant names and signature marker; the
/// export mark on each import; the `self` shape on `self.m()` beside a written
/// `Self::m()`; and `<X as Greet>::hello` with its type and trait.
#[test]
fn every_associated_item_fact_is_persisted() {
    let tmp = tree(&[(LIB, LIB_SRC), (A, A_SRC)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let s = |t: &str| t.to_string();
    assert_eq!(
        impl_blocks(rt),
        vec![
            (s(A), 2, 5, s("Y"), false, Some(s("crate::Greet")), None),
            (s(LIB), 9, 9, s("X"), false, Some(s("Greet")), None),
            (s(LIB), 10, 14, s("X"), false, None, None),
            (s(LIB), 15, 18, s("X"), false, Some(s("std::ops::Deref")), Some(s("Inner"))),
        ]
    );

    let label = labels(rt);
    let facts: BTreeMap<String, (Option<ReceiverMode>, Option<String>, bool)> = rt
        .submit_read(|store| store.node_item_facts())
        .expect("read runs")
        .into_iter()
        .map(|f| (label[&f.id].clone(), (f.receiver_mode, f.variants, f.signature)))
        .collect();
    use ReceiverMode::{None as NoReceiver, Ref, RefMut};
    let want: BTreeMap<String, (Option<ReceiverMode>, Option<String>, bool)> = [
        ("hello@5", (Some(Ref), None, true)),
        ("meta@6", (Some(Ref), None, true)),
        ("go@11", (Some(RefMut), None, false)),
        ("go2@12", (Some(Ref), None, false)),
        ("new@13", (Some(NoReceiver), None, false)),
        ("deref@17", (Some(Ref), None, false)),
        ("E@19", (None, Some(s("A B C")), false)),
        ("hello@3", (Some(Ref), None, false)),
        ("meta@4", (Some(Ref), None, false)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(facts, want);

    let refs = rt.submit_read(|store| store.unresolved_refs()).expect("read runs");
    let mut imports: Vec<(String, Option<bool>)> = refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Imports)
        .map(|r| (r.target.clone(), r.exported))
        .collect();
    imports.sort();
    assert_eq!(imports, vec![(s("a::Y"), Some(true)), (s("std::fmt"), Some(false))]);
    let mut self_rows: Vec<String> = refs
        .iter()
        .filter(|r| r.target == "Self::go2")
        .map(|r| format!("{:?}", r.receiver))
        .collect();
    self_rows.sort();
    assert_eq!(self_rows, ["None", "Some(SelfInstance)"], "`self.go2()` and `Self::go2(self)` are two rows");
    assert!(refs.iter().any(|r| r.target == "<X as Greet>::hello"), "the qualified call keeps its type and trait");
}

/// S-606 / CR-202: a required signature is a bodyless `Method` contained by
/// its trait, and nothing else touches it — no call binds it: the `self.m()`
/// calls in the trait's default body fan out to the impls of `m` alone (S-608,
/// `Y`'s; `X`'s empty impl lends no body), and the qualified call, which `X`'s
/// impl supplies no body for, binds nothing — and it is never reported dead.
#[test]
fn a_required_signature_binds_no_call_and_is_never_dead() {
    let tmp = tree(&[(LIB, LIB_SRC), (A, A_SRC)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let calls = edges(rt, EdgeKind::Calls);
    assert_eq!(
        calls,
        vec![
            ("go@11".to_string(), "go2@12".to_string()),
            ("twice@7".to_string(), "hello@3".to_string()),
            ("twice@7".to_string(), "meta@4".to_string()),
        ],
        "`go` binds `go2`, and the default body's calls reach `Y`'s impls, never a signature"
    );
    let signatures = ["hello@5", "meta@6"];
    let label = labels(rt);
    for e in rt.submit_read(|store| store.all_edges()).expect("read runs") {
        let (source, target) = (&label[&e.source], &label[&e.target]);
        if signatures.contains(&target.as_str()) || signatures.contains(&source.as_str()) {
            assert_eq!((source.as_str(), e.kind), ("Greet@4", EdgeKind::Contains), "a signature's one edge is its trait's");
        }
    }
    let unbound: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && !r.resolved)
        .map(|r| r.target)
        .collect();
    assert!(
        unbound.contains(&"<X as Greet>::hello".to_string()),
        "the qualified call stays unbound: {unbound:?}"
    );

    let verdicts: HashMap<String, (NodeKind, Option<bool>, Option<bool>)> = rt
        .submit_read(|store| store.annotation_nodes())
        .expect("read runs")
        .into_iter()
        .map(|n| (label[&n.id].clone(), (n.kind, n.has_body, n.is_dead)))
        .collect();
    for signature in signatures {
        assert_eq!(verdicts[signature], (NodeKind::Method, Some(false), Some(false)), "{signature}");
    }
    assert_eq!(verdicts["go2@12"].2, Some(false), "a called private method is live");
}

// ── Sync ≡ reindex ────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced graph's
/// impl blocks are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<Block> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        fs::write(tmp_a.path().join(rel), text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    impl_blocks(rt_a)
}

const B: &str = "src/b.rs";
const B_SRC: &str = "\
use crate::Greet;
pub struct Z;
impl Z { pub fn f(&self) {} }
";

/// S-606 / NFR-RA-06: an impl block added, emptied or re-headed in one file is
/// recorded by sync exactly as a fresh index of the edited tree records it.
#[test]
fn an_impl_block_added_emptied_or_re_headed_syncs_as_a_fresh_index() {
    let base = [(LIB, LIB_SRC), (A, A_SRC), (B, B_SRC)];
    let s = |t: &str| t.to_string();

    let added = format!("{B_SRC}impl Greet for Z {{}}\n");
    let blocks = synced_equals_reindexed(&base, &[(B, &added)]);
    assert!(blocks.contains(&(s(B), 4, 4, s("Z"), false, Some(s("Greet")), None)), "added: {blocks:?}");

    let emptied = B_SRC.replace("impl Z { pub fn f(&self) {} }", "impl Z {}");
    let blocks = synced_equals_reindexed(&base, &[(B, &emptied)]);
    assert!(blocks.contains(&(s(B), 3, 3, s("Z"), false, None, None)), "emptied: {blocks:?}");

    let re_headed = B_SRC.replace("impl Z {", "impl<'a> Greet for &'a Z {");
    let blocks = synced_equals_reindexed(&base, &[(B, &re_headed)]);
    assert!(blocks.contains(&(s(B), 3, 3, s("Z"), true, Some(s("Greet")), None)), "re-headed: {blocks:?}");
    assert!(!blocks.contains(&(s(B), 3, 3, s("Z"), false, None, None)), "the old header is gone");

    let removed = B_SRC.replace("impl Z { pub fn f(&self) {} }\n", "");
    let blocks = synced_equals_reindexed(&base, &[(B, &removed)]);
    assert!(blocks.iter().all(|b| b.0 != B), "removed: {blocks:?}");
}

/// S-606: the sync ≡ reindex fingerprint sees every new fact — a store that
/// differs from a fresh index in any one of them fingerprints differently, so
/// a sync that lost one could never pass as equal. Each fact is changed in
/// place in an indexed store, and the fingerprint read again.
#[test]
fn the_graph_fingerprint_sees_every_associated_item_fact() {
    let mutations = [
        ("receiver mode", "UPDATE nodes SET receiver_mode = NULL WHERE receiver_mode = 3"),
        ("variant names", "UPDATE nodes SET variants = 'A' WHERE variants IS NOT NULL"),
        ("signature marker", "UPDATE nodes SET signature = NULL WHERE signature = 1 AND name = 'hello'"),
        ("export mark", "UPDATE unresolved_refs SET exported = 0 WHERE exported = 1"),
        ("impl self type", "UPDATE impl_blocks SET self_type = 'W' WHERE self_type = 'Y'"),
        ("impl reference flag", "UPDATE impl_blocks SET self_ref = 1 WHERE trait_path = 'Greet'"),
        ("impl trait", "UPDATE impl_blocks SET trait_path = 'Other' WHERE trait_path = 'Greet'"),
        ("Deref target", "UPDATE impl_blocks SET deref_target = 'Outer' WHERE deref_target IS NOT NULL"),
        ("impl block", "DELETE FROM impl_blocks WHERE trait_path = 'Greet'"),
    ];
    for (fact, sql) in mutations {
        let tmp = tree(&[(LIB, LIB_SRC), (A, A_SRC)]);
        let engine = index(tmp.path());
        let before = graph_fingerprint(engine.runtime().unwrap());
        drop(engine);
        let conn = rusqlite::Connection::open(tmp.path().join(".logos").join("logos.db")).unwrap();
        assert_eq!(conn.execute(sql, []).unwrap(), 1, "{fact}: the fixture holds exactly one such fact");
        drop(conn);
        let engine = Engine::start(tmp.path()).expect("engine restarts");
        let after = graph_fingerprint(engine.runtime().unwrap());
        assert_ne!(before, after, "the fingerprint must see a changed {fact}");
    }
}

/// A Rust crate using axum whose handler `index` is handed to a route — and
/// whose trait declares a required signature of that name.
fn axum_tree(lib: &str) -> TempDir {
    tree(&[
        ("Cargo.toml", "[package]\nname = \"p\"\nversion = \"0.1.0\"\n[dependencies]\naxum = \"0.7\"\n"),
        (LIB, lib),
    ])
}

const AXUM_SIGNATURE_ONLY: &str = "\
use axum::{routing::get, Router};
pub trait Api {
    fn index(&self);
}
pub fn app() -> Router {
    Router::new().route(\"/\", get(index))
}
";

/// S-606: the framework pass binds a route's handler among the same
/// signature-free graph as the binder — a route naming only a trait's required
/// signature reaches no node, as before the signature was one.
#[test]
fn a_route_never_reaches_a_required_signature() {
    let tmp = axum_tree(AXUM_SIGNATURE_ONLY);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let routes: Vec<(String, String)> = edges(rt, EdgeKind::RoutesTo)
        .into_iter()
        .filter(|(source, target)| source != target)
        .collect();
    assert!(routes.iter().all(|(_, target)| target != "index@3"), "no route reaches the signature: {routes:?}");
}

const AXUM_SIGNATURE_AND_HANDLER: &str = "\
use axum::{routing::get, Router};
pub trait Api {
    fn index(&self);
}
pub async fn index() {}
pub fn app() -> Router {
    Router::new().route(\"/\", get(index))
}
";

/// S-606: the dispatch pass's same-file handoff resolves a handler name among
/// bodied callables only — a trait's same-named required signature neither
/// makes the free handler ambiguous nor takes its live-root marker — on a cold
/// index and through sync (the change-proportional read) alike.
#[test]
fn a_handoff_beside_a_same_named_signature_roots_the_handler() {
    let markers = |rt: &Runtime| -> Vec<String> {
        edges(rt, EdgeKind::RoutesTo).into_iter().filter(|(s, t)| s == t).map(|(s, _)| s).collect()
    };
    let tmp = axum_tree(AXUM_SIGNATURE_AND_HANDLER);
    let engine = index(tmp.path());
    assert_eq!(markers(engine.runtime().unwrap()), ["index@5"], "the handler, never the signature, is rooted");

    let root = tmp.path().canonicalize().unwrap();
    let edited = format!("{AXUM_SIGNATURE_AND_HANDLER}pub fn other() {{}}\n");
    fs::write(tmp.path().join(LIB), &edited).unwrap();
    engine.sync(&[root.join(LIB)]);
    assert_eq!(markers(engine.runtime().unwrap()), ["index@5"], "a sync roots the same handler");
}

/// S-606: `status` reads the call residue over the same signature-free graph
/// the binder binds against, so it states what binding produced. The default
/// body's `self.hello()` and `self.meta()` stay unbound — no impl supplies a
/// body (S-608) — and each reads `supertype-unreached`; `x.twice()` binds the
/// default `X`'s empty impl lends. Over a graph that held the signatures they
/// would read as bindable, and land in `unclassified`.
#[test]
fn the_call_residue_states_what_binding_left_beside_required_signatures() {
    let src = "\
pub struct X;
pub trait Greet {
    fn hello(&self);
    fn meta(&self) -> u8;
    fn twice(&self) { self.hello(); self.meta(); }
}
impl Greet for X {}
pub fn q(x: &X) { x.twice(); }
";
    let tmp = tree(&[(LIB, src)]);
    let engine = index(tmp.path());
    let residue: CallResidue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "rust")
        .and_then(|row| row.call_residue)
        .expect("the rust row states its call residue");
    let reasons: BTreeMap<R, u64> = residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect();
    assert_eq!((residue.unbound, residue.unclassified), (2, 0), "{reasons:?}");
    assert_eq!(reasons, [(R::SupertypeUnreached, 2)].into_iter().collect());
}
