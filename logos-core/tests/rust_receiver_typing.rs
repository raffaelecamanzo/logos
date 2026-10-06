//! A Rust receiver's type is proven from its declaration (S-587, CR-188,
//! FR-RS-42, NFR-RA-05, NFR-RA-06) — exercised end-to-end through the public
//! [`Engine`] façade against real temp-directory fixtures.
//!
//! Each proof form records the call `x.f()` as the Path-form `T::f` of shape
//! `other` in the reference ledger, with the wrappers peeled to reach `T` in the
//! ledger's `peeled` column (migration 32). The binder does not bind such a row
//! yet — that is S-588 — so the graph's `Calls` edges are exactly what they were:
//! a written `A::f(&x)` binds, and every receiver call stays unbound.
//!
//! [`Engine`]: logos_core::Engine

#![cfg(feature = "lang-rust")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

const MANIFEST: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
const TYPES_FILE: &str = "src/types.rs";
const CALLER_FILE: &str = "src/caller.rs";

/// Two types that both define `f`, and the constructors and fields the proof
/// forms read — all in the caller's file where a form needs its declarations
/// there, and in `types.rs` where it does not.
const TYPES: &str = "\
pub struct A;
pub struct B;
impl A { pub fn f(&self) {} }
impl B { pub fn f(&self) {} }
";

const CALLER: &str = "\
use crate::types::{A, B};
use std::sync::Arc;

pub struct Holder { inner: B, shared: Arc<A> }
impl Holder {
    pub fn new() -> Self { Holder { inner: B, shared: Arc::new(A) } }
    pub fn via_field(&self) { self.inner.f(); self.shared.f(); }
}

pub fn via_param(x: &A) { x.f(); }
pub fn via_let() { let x: Arc<B> = make(); x.f(); }
pub fn via_constructor() { let h = Holder::new(); h.via_field(); }
pub fn via_literal() { let x = A {}; x.f(); }
pub fn via_written(x: A) { A::f(&x); }
pub fn unproven(xs: Vec<A>) { for x in xs { x.f(); } }
";

fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "Cargo.toml", MANIFEST);
    write(tmp.path(), "src/lib.rs", "pub mod types;\npub mod caller;\n");
    write(tmp.path(), TYPES_FILE, TYPES);
    write(tmp.path(), CALLER_FILE, CALLER);
    tmp
}

/// One `Calls` ledger row: `(source name, target, form, receiver, peeled)`.
type CallRow = (String, String, RefForm, Option<ReceiverShape>, Option<String>);

/// The `peeled` column of every ledger row, by id — read from the store file,
/// as nothing above the store reads the column yet (S-588 will).
fn peeled_by_id(tmp: &TempDir) -> HashMap<i64, Option<String>> {
    let conn = rusqlite::Connection::open(tmp.path().join(".logos").join("logos.db")).unwrap();
    let mut stmt = conn.prepare("SELECT id, peeled FROM unresolved_refs").unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The `Calls` rows whose source declaration lies in `file`, sorted.
fn calls_from(tmp: &TempDir, rt: &Runtime, file: &str) -> Vec<CallRow> {
    let peeled = peeled_by_id(tmp);
    let needle = file.to_string();
    let mut rows: Vec<CallRow> = rt
        .submit_read(move |store| {
            let sources: HashMap<String, (String, String)> = store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), (n.file_path.unwrap_or_default(), n.name)))
                .collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
                .filter_map(|r| {
                    let (f, name) = sources.get(&r.source_symbol)?;
                    (*f == needle).then(|| (name.clone(), r.target, r.form, r.receiver, peeled[&r.id].clone()))
                })
                .collect())
        })
        .expect("read runs");
    rows.sort_by(|a, b| (&a.0, &a.1, a.2.as_i32()).cmp(&(&b.0, &b.1, b.2.as_i32())));
    rows
}

/// Every bound `Calls` edge as `(source file:name, target file:name)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

fn typed(source: &str, target: &str, peeled: Option<&str>) -> CallRow {
    (
        source.to_string(),
        target.to_string(),
        RefForm::Path,
        Some(ReceiverShape::Other),
        peeled.map(str::to_string),
    )
}

#[test]
fn every_proof_form_records_a_type_qualified_path_row_of_shape_other() {
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let rows = calls_from(&tmp, rt, CALLER_FILE);
    for expected in [
        typed("via_param", "A::f", Some("&")),
        typed("via_let", "B::f", Some("Arc")),
        typed("via_constructor", "Holder::via_field", None),
        typed("via_literal", "A::f", None),
        typed("via_field", "B::f", None),
        typed("via_field", "A::f", Some("Arc")),
    ] {
        assert!(rows.contains(&expected), "{expected:?} not in {rows:#?}");
    }
    // A written path call keeps its shapeless row; an unproven receiver keeps
    // the `other` Method row it had.
    assert!(rows.contains(&("via_written".to_string(), "A::f".to_string(), RefForm::Path, None, None)));
    assert!(rows.contains(&(
        "unproven".to_string(),
        "f".to_string(),
        RefForm::Method,
        Some(ReceiverShape::Other),
        None
    )));
}

#[test]
fn a_retyped_row_binds_nothing_and_the_call_edges_are_what_they_were() {
    // The caller file's one `Calls` edge is the written `Holder::new()`: every
    // receiver call — proven or not — stays unbound until S-588. This is the
    // edge set the 1.11.1 binary binds on this fixture, before any receiver
    // was typed (checked 2026-10-06).
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let from_caller: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(s, _)| s.starts_with(CALLER_FILE))
        .collect();
    assert_eq!(
        from_caller,
        vec![(format!("{CALLER_FILE}:via_constructor"), format!("{CALLER_FILE}:new"))]
    );
}

/// Every edge and every non-Symbol ledger row, by symbol — receiver shape and
/// peeled wrappers included — the store's whole binding state.
fn binding_facts(tmp: &TempDir, rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    let peeled = peeled_by_id(tmp);
    rt.submit_read(move |store| {
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
            .map(|r| {
                format!(
                    "{} {} {:?} {:?} {:?} {:?} {}",
                    r.source_symbol, r.target, r.form, r.kind, r.receiver, peeled[&r.id], r.resolved
                )
            })
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// The binding state of a cold index over the files `tmp` holds now.
fn cold_facts(tmp: &TempDir) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in ["Cargo.toml", "src/lib.rs", TYPES_FILE, CALLER_FILE] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = index(&cold);
    let facts = binding_facts(&cold, engine.runtime().unwrap());
    drop(engine);
    facts
}

#[test]
fn a_one_file_edit_re_derives_that_files_rows_and_sync_equals_a_full_reindex() {
    let tmp = fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let types_rows_before = calls_from(&tmp, rt, TYPES_FILE);

    // The parameter changes type and wrapper; the `let` loses its proof (an
    // untyped re-binding now shadows it).
    write(
        tmp.path(),
        CALLER_FILE,
        &CALLER
            .replace("pub fn via_param(x: &A)", "pub fn via_param(x: Box<B>)")
            .replace("let x: Arc<B> = make(); x.f();", "let x: Arc<B> = make(); let x = x; x.f();"),
    );
    engine.sync(&[CALLER_FILE.into()]);

    let rows = calls_from(&tmp, rt, CALLER_FILE);
    assert!(rows.contains(&typed("via_param", "B::f", Some("Box"))), "{rows:#?}");
    assert!(!rows.iter().any(|r| r.0 == "via_param" && r.1 == "A::f"), "{rows:#?}");
    assert!(rows.contains(&(
        "via_let".to_string(),
        "f".to_string(),
        RefForm::Method,
        Some(ReceiverShape::Other),
        None
    )));
    assert_eq!(calls_from(&tmp, rt, TYPES_FILE), types_rows_before, "the untouched file's rows are its own");
    assert_eq!(binding_facts(&tmp, rt), cold_facts(&tmp));
}
