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
//! [CR-195]: ../../docs/requests/CR-195-status-resolution-figures-exclude-capture-before-delete-rows.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [FR-RS-04]: ../../docs/specs/requirements/FR-RS-04.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md

#![cfg(feature = "lang-go")]

use std::fs;
use std::path::Path;

use logos_core::graph_store::NewUnresolvedRef;
use logos_core::model::{EdgeKind, RefForm};
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
    let cold = TempDir::new().unwrap();
    for rel in ["go.mod", A_FILE, B_FILE] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = index(cold.path());
    (cold, engine)
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

    // Touch `a/a.go`: `b`'s inbound call and import are captured and rebound.
    write(tmp.path(), A_FILE, &format!("// touched\n{A_GO}"));
    let result = engine.sync(&[A_FILE.into()]);
    assert_eq!(result.files_modified, 1);
    assert!(
        capture_rows(&engine).0 > 0,
        "the sync wrote capture rows, or this test would pin nothing"
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
#[test]
fn an_unbound_capture_row_awaiting_a_renamed_target_moves_no_figure() {
    let tmp = module();
    let engine = index(tmp.path());
    write(tmp.path(), A_FILE, "package a\n\nfunc Renamed() {}\n");
    engine.sync(&[A_FILE.into()]);
    assert!(
        capture_rows(&engine).1 > 0,
        "an inbound edge awaits a symbol that is gone"
    );

    let (_cold_dir, cold) = cold_index(&tmp);
    assert_eq!(go_row(&engine), go_row(&cold));
    assert_eq!(global(&engine), global(&cold));
}

/// The per-relation-class coverage `index` and `sync` return reads the same
/// population: a capture row keeps its relation `payload`, so an unbound one
/// planted beside a real row of the class must not move the class's counts.
/// (No default-feature plugin binds an artifact relation across files, so the
/// rows are planted: the pass reads every ledger row for its `by_relation`,
/// selected or not.)
#[test]
fn a_syncs_per_relation_coverage_leaves_out_capture_rows() {
    let tmp = module();
    let engine = index(tmp.path());
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
                    source_symbol: "cfg b",
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

    write(tmp.path(), A_FILE, &format!("// touched\n{A_GO}"));
    let result = engine.sync(&[A_FILE.into()]);
    let class = &result.resolution.by_relation["proto-import"];
    assert_eq!(
        (class.bound, class.unresolved),
        (0, 1),
        "only the Path-form row is a reference of the class: {class:?}"
    );
}
