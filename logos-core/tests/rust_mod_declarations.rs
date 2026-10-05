//! A bodyless Rust `mod x;` binds as the file module it declares (S-585,
//! [FR-RS-41], [CR-186]) — exercised end-to-end through the public [`Engine`]
//! against real temp-directory crates (`rust_mod_declarations/fixtures.rs`).
//!
//! The extractor keeps a `mod x;` line as a `Module` node of the declaring
//! file, keyed exactly where `x.rs` / `x/mod.rs` is keyed. The binder rule —
//! the key answers with the one file it declares — is pinned in memory by
//! `resolve::mod_declaration_tests`; this suite pins what reaches the product:
//! the minimal crate binds its import and its call, chained declarations bind
//! in both layouts, an undeclarable target stays in the ledger unbound, and a
//! sync equals a full reindex ([NFR-RA-06]).
//!
//! [FR-RS-41]: ../../docs/specs/requirements/FR-RS-41.md
//! [CR-186]: ../../docs/requests/CR-186-a-rust-mod-declaration-never-blocks-binding.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(all(feature = "lang-rust", feature = "lang-typescript"))]

#[path = "rust_mod_declarations/fixtures.rs"]
mod fixtures;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn indexed(fixture: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture {
        write(tmp.path(), rel, source);
    }
    let engine = index(&tmp);
    (tmp, engine)
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// Every edge of `kind` out of the file `rel`, as `source name -> target
/// file:name`, sorted.
fn edges_from(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, (String, String)> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, (n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind && label[&e.source].0 == rel)
            .map(|e| {
                let (target_file, target) = &label[&e.target];
                format!("{} -> {target_file}:{target}", label[&e.source].1)
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's unresolved targets of `kind` out of the file `rel`, sorted.
fn unbound(rt: &Runtime, rel: &str, kind: EdgeKind) -> Vec<String> {
    let rel = rel.to_string();
    let mut rows: Vec<String> = rt
        .submit_read(move |store| {
            let files: HashMap<i64, String> =
                store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| {
                    r.kind == kind && !r.resolved && r.file_id.and_then(|id| files.get(&id)) == Some(&rel)
                })
                .map(|r| r.target)
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// Every edge (by endpoint symbols) and every non-`Symbol` ledger row with its
/// resolved flag — what a sync must leave exactly as a cold index does.
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

/// Index a fresh copy of `tmp`'s `files` (those still on disk) and return its
/// binding facts.
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

/// CR-186 §3.1's reproduction, FR-RS-41 AC: `pub mod util;`, `use
/// crate::util::run;` and `run()` bind 1 cross-file `Imports` and 1 cross-file
/// `Calls` edge, and no ledger row stays unbound for them.
#[test]
fn the_minimal_crate_binds_its_import_and_its_call_across_files() {
    let (_tmp, engine) = indexed(fixtures::MINIMAL);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Imports),
        strings(&["crate -> src/util.rs:run"])
    );
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Calls),
        strings(&["alpha -> src/util.rs:run"])
    );
    assert!(unbound(rt, "src/lib.rs", EdgeKind::Imports).is_empty());
    assert!(unbound(rt, "src/lib.rs", EdgeKind::Calls).is_empty());
}

/// FR-RS-41 AC: `use crate::a::b::c` through `mod a;` → `a/mod.rs` (and
/// `a.rs`) → `mod b;` → `b.rs` binds, and so do `self::b::c()` from `a` and
/// `super::helper()` from `b` — also with names that number every declaration
/// before the file it declares (`x`/`y`).
#[test]
fn chained_declarations_bind_in_both_layouts() {
    for (fixture, outer_file, leaf_file) in [
        (fixtures::CHAIN_MOD_RS, "src/a/mod.rs", "src/a/b.rs"),
        (fixtures::CHAIN_A_RS, "src/a.rs", "src/a/b.rs"),
        (fixtures::CHAIN_LATE_MOD_RS, "src/x/mod.rs", "src/x/y.rs"),
        (fixtures::CHAIN_LATE_X_RS, "src/x.rs", "src/x/y.rs"),
    ] {
        let (_tmp, engine) = indexed(fixture);
        let rt = engine.runtime().unwrap();
        assert_eq!(
            edges_from(rt, "src/lib.rs", EdgeKind::Imports),
            vec![format!("crate -> {leaf_file}:c")],
            "{outer_file}"
        );
        assert_eq!(
            edges_from(rt, "src/lib.rs", EdgeKind::Calls),
            vec![format!("alpha -> {leaf_file}:c")],
            "{outer_file}"
        );
        assert_eq!(
            edges_from(rt, outer_file, EdgeKind::Calls),
            vec![format!("helper -> {leaf_file}:c")],
            "{outer_file}"
        );
        assert_eq!(
            edges_from(rt, leaf_file, EdgeKind::Calls),
            vec![format!("up -> {outer_file}:helper")],
            "{outer_file}"
        );
    }
}

/// FR-RS-41 AC: `#[path = "x_impl.rs"] mod x;` and a `mod missing;` with no
/// file stay unresolved — their rows stay in the ledger unbound, and nothing is
/// guessed for them (`x_impl.rs`'s `go` is never reached by the path) — while
/// the inline `mod inner { … }` binds as before.
#[test]
fn an_undeclarable_module_stays_unresolved_and_an_inline_one_binds() {
    let (_tmp, engine) = indexed(fixtures::UNDECLARABLE);
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, "src/lib.rs", EdgeKind::Imports).is_empty());
    assert_eq!(
        unbound(rt, "src/lib.rs", EdgeKind::Imports),
        strings(&["crate::missing::gone", "crate::x::go"])
    );
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Calls),
        strings(&["alpha -> src/lib.rs:deep"])
    );
    assert_eq!(unbound(rt, "src/lib.rs", EdgeKind::Calls), strings(&["go", "gone"]));
}

/// A declaration names a file of its own language (review fix): with only
/// `src/x.js` at the path the Rust import and call stay unbound, and beside the
/// `src/x.rs` it does name, that file is the one candidate — never a pair.
#[test]
fn a_declaration_never_binds_into_a_file_of_another_language() {
    let (_tmp, engine) = indexed(fixtures::FOREIGN_FILE);
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, "src/lib.rs", EdgeKind::Imports).is_empty());
    assert!(edges_from(rt, "src/lib.rs", EdgeKind::Calls).is_empty());
    assert_eq!(unbound(rt, "src/lib.rs", EdgeKind::Imports), strings(&["crate::x::run"]));
    let mut beside = fixtures::FOREIGN_FILE.to_vec();
    beside.push(("src/x.rs", "pub fn run() {}\n"));
    let (_tmp, engine) = indexed(&beside);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Calls),
        strings(&["alpha -> src/x.rs:run"])
    );
}

/// A declaration arriving in a re-extracted file binds as it does on a cold
/// index, and so does the declared file arriving and leaving ([NFR-RA-06]).
#[test]
fn sync_equals_a_full_reindex_as_a_declaration_and_its_file_come_and_go() {
    let undeclared: &[(&str, &str)] = &[
        fixtures::MINIMAL[0],
        ("src/lib.rs", "use crate::util::run;\n\npub fn alpha() {\n    run();\n}\n"),
        fixtures::MINIMAL[2],
    ];
    let (tmp, engine) = indexed(undeclared);
    let rt = engine.runtime().unwrap();
    let all = paths(fixtures::MINIMAL);
    // The `mod` line arrives.
    write(tmp.path(), "src/lib.rs", fixtures::MINIMAL[1].1);
    engine.sync(&["src/lib.rs".into()]);
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Calls),
        strings(&["alpha -> src/util.rs:run"])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &all));
    // The declared file leaves: the declaration holds the key, and nothing binds.
    fs::remove_file(tmp.path().join("src/util.rs")).unwrap();
    engine.sync(&["src/util.rs".into()]);
    assert_eq!(unbound(rt, "src/lib.rs", EdgeKind::Calls), strings(&["run"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &all));
    // It returns.
    write(tmp.path(), "src/util.rs", fixtures::MINIMAL[2].1);
    engine.sync(&["src/util.rs".into()]);
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Calls),
        strings(&["alpha -> src/util.rs:run"])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &all));
}

/// A second file at the declaration's path (`a.rs` beside `a/mod.rs`, which
/// rustc refuses as E0761) makes two candidates: a path through `a` binds
/// neither ([NFR-RA-05]) — `crate::a::b::c` still binds, as `b.rs` is the one
/// file at `a::b` in either layout — and once the rival leaves, a sync binds
/// exactly what a cold index does ([NFR-RA-06]).
///
/// The rival *arriving* by sync is not pinned here: the row it makes ambiguous
/// flips unresolved but keeps its edge, because a re-bound row that turns
/// unbound retracts its edge only in an import-root file (S-519). That gap
/// predates this story and reaches any binding a new rival makes ambiguous (a
/// second glob supplying a called name, on the base binary too); it is recorded
/// in the implementation notes.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn sync_equals_a_full_reindex_when_a_second_candidate_file_leaves() {
    let rival = "src/a.rs";
    let mut with_rival = fixtures::CHAIN_MOD_RS.to_vec();
    with_rival.push((rival, "pub fn helper() {}\n"));
    let (tmp, engine) = indexed(&with_rival);
    let rt = engine.runtime().unwrap();
    assert!(edges_from(rt, "src/a/b.rs", EdgeKind::Calls).is_empty());
    assert_eq!(unbound(rt, "src/a/b.rs", EdgeKind::Calls), strings(&["super::helper"]));
    assert_eq!(
        edges_from(rt, "src/lib.rs", EdgeKind::Imports),
        strings(&["crate -> src/a/b.rs:c"])
    );
    fs::remove_file(tmp.path().join(rival)).unwrap();
    engine.sync(&[rival.into()]);
    assert_eq!(
        edges_from(rt, "src/a/b.rs", EdgeKind::Calls),
        strings(&["up -> src/a/mod.rs:helper"])
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &paths(fixtures::CHAIN_MOD_RS)));
}
