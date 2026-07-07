//! Black-box integration tests for the persisted monotonic graph revision
//! (S-102, [CR-027], [ADR-32], [FR-SY-09]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! façade exactly as the CLI/MCP surfaces will:
//! - the revision strictly increases across an `index` and across a `sync` that
//!   changes ≥1 file, and is unchanged by a no-op `sync` or a read-only
//!   navigation
//!   ([FR-SY-09](../../docs/specs/requirements/FR-SY-09.md));
//! - the current value is exposed on the status read-model
//!   ([RP-02](../../docs/specs/requirements/RP-02.md)) and read identically by a
//!   second process opening the same `logos.db`.
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these tests need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use logos_core::Engine;

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

#[test]
fn revision_is_zero_before_indexing_and_status_reports_it() {
    // FR-SY-09: `0` before the first index — "no graph yet". status() reads the
    // persisted revision without advancing it (a read-only report).
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert_eq!(
        engine.status().graph_revision,
        0,
        "a never-indexed store reports revision 0 on status"
    );
    assert_eq!(
        engine.status().graph_revision,
        0,
        "merely reading status never advances the revision"
    );
}

#[test]
fn revision_advances_on_index_and_a_file_changing_sync_only() {
    // FR-SY-09 AC: strictly increases across an index, then a sync that changes
    // ≥1 file; a no-op sync and a read-only navigation leave it unchanged.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");
    write(tmp.path(), "beta.rs", "fn beta() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");

    // A completed index advances the revision from 0 to 1.
    engine.index();
    let after_index = engine.status().graph_revision;
    assert_eq!(after_index, 1, "a completed index advances the revision to 1");

    // A sync that genuinely changes a file advances it again.
    write(tmp.path(), "alpha.rs", "fn alpha() { let _x = 1; }\n");
    let changed = engine.sync(&[PathBuf::from("alpha.rs")]);
    assert_eq!(changed.files_modified, 1, "the sync re-extracted alpha");
    assert_eq!(
        engine.status().graph_revision,
        2,
        "a file-changing sync strictly increases the revision"
    );

    // A no-op sync (alpha unchanged since the last sync) must NOT advance it.
    let noop = engine.sync(&[PathBuf::from("alpha.rs"), PathBuf::from("beta.rs")]);
    assert_eq!(
        noop.files_added + noop.files_modified + noop.files_removed,
        0,
        "nothing changed on disk, so the sync is a no-op"
    );
    assert_eq!(
        engine.status().graph_revision,
        2,
        "a no-op sync leaves the revision unchanged"
    );

    // A read-only navigation query never advances the revision.
    let _ = engine.search("alpha", None, None);
    let _ = engine.callers("alpha", None);
    assert_eq!(
        engine.status().graph_revision,
        2,
        "read-only navigation never advances the revision"
    );
}

#[test]
fn revision_advances_on_a_sync_that_adds_or_removes_a_file() {
    // The mutation predicate covers added and removed files, not just modified.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    assert_eq!(engine.status().graph_revision, 1);

    // Adding a file advances it.
    write(tmp.path(), "beta.rs", "fn beta() {}\n");
    let added = engine.sync(&[PathBuf::from("beta.rs")]);
    assert_eq!(added.files_added, 1);
    assert_eq!(
        engine.status().graph_revision,
        2,
        "a sync that adds a file advances the revision"
    );

    // Removing a file advances it.
    fs::remove_file(tmp.path().join("beta.rs")).expect("remove fixture");
    let removed = engine.sync(&[PathBuf::from("beta.rs")]);
    assert_eq!(removed.files_removed, 1);
    assert_eq!(
        engine.status().graph_revision,
        3,
        "a sync that removes a file advances the revision"
    );
}

#[test]
fn re_indexing_advances_the_revision_again() {
    // index always rebuilds the graph, so a second index advances the revision
    // again (1 -> 2) — distinct from sync, which is gated on a dirty set.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    assert_eq!(engine.status().graph_revision, 1, "first index -> 1");
    engine.index();
    assert_eq!(
        engine.status().graph_revision,
        2,
        "a second index advances the revision again, strictly monotonic"
    );
}

#[test]
fn a_second_process_reads_the_same_revision() {
    // FR-SY-09 AC: a second process opening the same logos.db reads the identical
    // current revision. A fresh `Engine::open` over the same root is that reader.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");

    let writer = Engine::start(tmp.path()).expect("writer engine starts");
    writer.index();
    write(tmp.path(), "alpha.rs", "fn alpha() { let _x = 1; }\n");
    writer.sync(&[PathBuf::from("alpha.rs")]);
    let writer_revision = writer.status().graph_revision;
    assert_eq!(writer_revision, 2, "index then a changing sync → revision 2");
    drop(writer);

    // A second engine over the same persisted store reads the identical value.
    let reader = Engine::start(tmp.path()).expect("reader engine starts");
    assert_eq!(
        reader.status().graph_revision,
        writer_revision,
        "a second process reads the identical persisted revision (FR-SY-09)"
    );
}
