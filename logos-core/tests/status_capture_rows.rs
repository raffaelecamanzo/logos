//! `status` resolution figures exclude capture-before-delete rows (S-598,
//! [CR-195], [FR-RS-09], [FR-RS-04], [NFR-RA-06]) — a synced store and a cold
//! reindex of the same tree report the same figures.
//!
//! A capture-before-delete row ([ADR-10], `RefForm::Symbol`) is written under the
//! *target* file when that file is synced, and duplicates a reference its
//! source's own row already records. Counted, it made every inbound bound edge
//! of a synced file count twice: the 1.10.0 reproduction — a two-file Go module
//! reading calls and imports 1/1 → 2/2 after `sync a/a.go`, and 1/1 again after a
//! cold reindex. These tests pin that reproduction end to end through the
//! public [`Engine`] façade, with the figures the surfaces render.
//!
//! Since CR-187 a sync deletes each capture it spends — bound, or its source's
//! own rows re-bound — so the reproduction's own captures no longer outlive the
//! sync that wrote them. The rows this rule still governs are the ones a sync
//! keeps: unbound, from a live source no row of which was re-bound. Each test
//! below leaves such a row in the ledger, and asserts it is there, or it would
//! pin nothing: the renamed-away test through a sync alone (an edge no ledger
//! row produces), the others planted through the store as that sync leaves it.
//!
//! [CR-195]: ../../docs/requests/CR-195-status-resolution-figures-exclude-capture-before-delete-rows.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [FR-RS-04]: ../../docs/specs/requirements/FR-RS-04.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md

#![cfg(feature = "lang-go")]

use std::fs;
use std::path::Path;

use logos_core::graph_store::NewUnresolvedRef;
use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::models::LanguageResolution;
use logos_core::Engine;
use tempfile::TempDir;

const GO_MOD: &str = "module example.com/m\n\ngo 1.22\n";
const A_FILE: &str = "a/a.go";
const B_FILE: &str = "b/b.go";
const A_GO: &str = "package a\n\nfunc F() {}\n";
const B_GO: &str = "package b\n\nimport \"example.com/m/a\"\n\nfunc G() {\n\ta.F()\n}\n";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The two-file Go module: `b` imports `a` and calls `a.F()`.
fn module() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", GO_MOD);
    write(tmp.path(), A_FILE, A_GO);
    write(tmp.path(), B_FILE, B_GO);
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine
}

/// A cold index of `tmp`'s tree as it stands: its files copied into a fresh
/// directory, so the index shares no `.logos` store with the synced engine.
fn cold_index(tmp: &TempDir) -> (TempDir, Engine) {
    cold_index_of(tmp, &["go.mod", A_FILE, B_FILE])
}

/// [`cold_index`] over the named files of a tree that holds more than the module.
fn cold_index_of(tmp: &TempDir, files: &[&str]) -> (TempDir, Engine) {
    let cold = TempDir::new().unwrap();
    for rel in files {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = index(cold.path());
    (cold, engine)
}

/// The node named `name`: its rowid and symbol.
fn node(engine: &Engine, name: &'static str) -> (NodeId, String) {
    engine
        .runtime()
        .unwrap()
        .submit_read(move |store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .find(|n| n.name == name)
                .map(|n| (n.id, n.symbol.as_str().to_string()))
                .unwrap_or_else(|| panic!("the {name} node")))
        })
        .expect("read runs")
}

/// Plant the capture a sync keeps (CR-187): an unbound `Symbol` row under the
/// target's file `a/a.go`, from the live caller `G`, awaiting a target nothing
/// else in the ledger names. `G`'s own rows are not re-bound by a sync that
/// re-reads no file, so the row outlives one — exactly as such a sync leaves a
/// capture whose target was renamed away.
fn plant_awaiting_capture(engine: &Engine) {
    let (_, caller) = node(engine, "G");
    engine
        .runtime()
        .unwrap()
        .submit_write(move |w| {
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: w.file_id(A_FILE)?,
                source_symbol: &caller,
                target: "planted vanished target",
                alias: None,
                form: RefForm::Symbol,
                kind: EdgeKind::Calls,
                line: None,
                payload: None,
                receiver: None,
            })
        })
        .expect("plant the awaiting capture");
}

fn go_row(engine: &Engine) -> LanguageResolution {
    engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "go")
        .expect("a go row")
}

/// The `Symbol`-form rows the ledger holds: `(total, unresolved)`.
fn capture_rows(engine: &Engine) -> (usize, usize) {
    engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            let rows: Vec<_> = store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.form == RefForm::Symbol)
                .collect();
            Ok((rows.len(), rows.iter().filter(|r| !r.resolved).count()))
        })
        .expect("read runs")
}

/// The global ratio's figures as `status` reports them.
fn global(engine: &Engine) -> (u64, u64) {
    let status = engine.status();
    (status.refs_total, status.refs_resolved)
}

#[test]
fn the_two_file_go_module_reads_the_same_before_and_after_a_sync_and_after_a_reindex() {
    let tmp = module();
    let engine = index(tmp.path());
    let before = go_row(&engine);
    let before_global = global(&engine);
    assert!(
        before.calls.bound >= 1 && before.imports.bound >= 1,
        "the fixture binds a cross-file call and import: {before:?}"
    );
    assert_eq!(capture_rows(&engine), (0, 0), "a fresh index holds no capture row");

    // Touch `a/a.go`: `b`'s inbound call and import are captured and rebound —
    // and, spent, deleted by the same sync (CR-187).
    write(tmp.path(), A_FILE, &format!("// touched\n{A_GO}"));
    let touched = engine.sync(&[A_FILE.into()]);
    assert_eq!(touched.files_modified, 1);
    assert_eq!(capture_rows(&engine), (0, 0), "the sync spent the captures it wrote");
    assert_eq!(global(&engine), before_global);

    // The capture a sync keeps, then a sync over it: the row stays, unbound.
    plant_awaiting_capture(&engine);
    let result = engine.sync(&[]);
    assert_eq!(
        capture_rows(&engine),
        (1, 1),
        "an awaiting capture outlives the sync, or this test would pin nothing"
    );

    let after = go_row(&engine);
    assert_eq!(
        (&before.calls, &before.imports),
        (&after.calls, &after.imports),
        "the synced store reads what it read before the sync"
    );
    assert_eq!(before_global, global(&engine));
    assert_eq!(
        (result.resolution.refs_total, result.resolution.refs_resolved),
        before_global,
        "the sync's own global ratio is the one `status` reports"
    );

    let (_cold_dir, cold) = cold_index(&tmp);
    assert_eq!(capture_rows(&cold), (0, 0));
    assert_eq!(after, go_row(&cold), "and equals a cold reindex");
    assert_eq!(global(&engine), global(&cold));
}

/// A fresh index writes no capture row, so every figure is what the ledger
/// holds, row for row — the change moves nothing on a freshly indexed store.
#[test]
fn a_freshly_indexed_store_reports_every_ledger_row() {
    let tmp = module();
    let engine = index(tmp.path());
    assert_eq!(capture_rows(&engine), (0, 0));
    let (total, resolved) = engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            let rows = store.unresolved_refs()?;
            Ok((rows.len() as u64, rows.iter().filter(|r| r.resolved).count() as u64))
        })
        .unwrap();
    assert_eq!(global(&engine), (total, resolved));
    let go = go_row(&engine);
    let (calls, imports) = engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            let rows = store.unresolved_refs()?;
            let tally = |kind: EdgeKind| {
                let of_kind: Vec<_> = rows.iter().filter(|r| r.kind == kind).collect();
                (of_kind.len() as u64, of_kind.iter().filter(|r| r.resolved).count() as u64)
            };
            Ok((tally(EdgeKind::Calls), tally(EdgeKind::Imports)))
        })
        .unwrap();
    // Every ledger row of the module is a Go file's, so the per-language figures
    // are the ledger's tallies.
    assert_eq!((go.calls.references, go.calls.bound), calls);
    assert_eq!((go.imports.references, go.imports.bound), imports);
}

/// A capture row that stays unbound — its target renamed away — is the case a
/// bound-row cleanup cannot reach, and it must not move a figure either.
///
/// `G`'s capture of its call into the renamed `F` is spent with `G`'s own
/// re-bound rows (CR-187). The capture a sync keeps carries an edge no ledger
/// row produces: here a call from `H` (a third file, with no rows of its own)
/// into `F`, as `sync_drops_a_captured_edge_when_the_target_symbol_disappears`
/// in `tests/indexing.rs` plants it. Renaming `F` away leaves that capture in
/// the ledger, unbound, through the sync alone.
#[test]
fn an_unbound_capture_row_awaiting_a_renamed_target_moves_no_figure() {
    const C_FILE: &str = "c/c.go";
    let tmp = module();
    write(tmp.path(), C_FILE, "package c\n\nfunc H() {}\n");
    let engine = index(tmp.path());
    let (h, _) = node(&engine, "H");
    let (f, _) = node(&engine, "F");
    engine
        .runtime()
        .unwrap()
        .submit_write(move |w| w.insert_edge(h, f, EdgeKind::Calls))
        .expect("plant the edge no ledger row produces");

    write(tmp.path(), A_FILE, "package a\n\nfunc Renamed() {}\n");
    let result = engine.sync(&[A_FILE.into()]);
    assert_eq!(
        capture_rows(&engine),
        (1, 1),
        "the planted edge's capture awaits a symbol that is gone, or this test would pin nothing"
    );

    let (_cold_dir, cold) = cold_index_of(&tmp, &["go.mod", A_FILE, B_FILE, C_FILE]);
    assert_eq!(capture_rows(&cold), (0, 0));
    assert_eq!(go_row(&engine), go_row(&cold));
    assert_eq!(global(&engine), global(&cold));
    assert_eq!(
        (result.resolution.refs_total, result.resolution.refs_resolved),
        global(&cold),
        "the sync's own global ratio is a cold reindex's"
    );
}

/// The per-relation-class coverage `index` and `sync` return reads the same
/// population: a capture row keeps its relation `payload`, so an unbound one
/// planted beside a real row of the class must not move the class's counts.
/// (No default-feature plugin binds an artifact relation across files, so the
/// rows are planted: the pass reads every ledger row for its `by_relation`,
/// selected or not.)
///
/// The rows' source is the live `G`, and the sync re-reads no file, so no row
/// of `G`'s is re-bound and the capture is not spent (CR-187): it stays in the
/// ledger, and only the Symbol-form exclusion keeps it out of the class.
#[test]
fn a_syncs_per_relation_coverage_leaves_out_capture_rows() {
    let tmp = module();
    let engine = index(tmp.path());
    let (_, source) = node(&engine, "G");
    let rt = engine.runtime().unwrap();
    let b_id = rt
        .submit_read(|store| {
            Ok(store
                .indexed_files()?
                .into_iter()
                .find(|f| f.path == B_FILE)
                .expect("b/b.go is indexed")
                .id)
        })
        .expect("read runs");
    rt.submit_write(move |w| {
            for (form, target) in [(RefForm::Path, "real.proto"), (RefForm::Symbol, "captured")] {
                w.insert_unresolved_ref(&NewUnresolvedRef {
                    file_id: Some(b_id),
                    source_symbol: &source,
                    target,
                    alias: None,
                    form,
                    kind: EdgeKind::ArtifactRef,
                    line: Some(1),
                    payload: Some("proto-import"),
                    receiver: None,
                })?;
            }
            Ok(())
        })
        .expect("rows planted");

    let result = engine.sync(&[]);
    assert_eq!(
        capture_rows(&engine),
        (1, 1),
        "the capture outlives the sync, or this test would pin nothing"
    );
    let class = &result.resolution.by_relation["proto-import"];
    assert_eq!(
        (class.bound, class.unresolved),
        (0, 1),
        "only the Path-form row is a reference of the class: {class:?}"
    );
}
