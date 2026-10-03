//! A file that cannot be persisted fails alone and is reported (S-513,
//! [FR-EH-05](../../docs/specs/requirements/FR-EH-05.md),
//! [CR-168](../../docs/requests/CR-168-an-index-never-silently-empties.md)),
//! end to end through the public `Engine` façade.
//!
//! Every real trigger of a per-file persistence failure is a defect, so these
//! fail a named file on purpose through the runtime's test-only fault seam
//! (`Runtime::inject_persist_fault`, debug builds only). The seam fails the file
//! **after** its facts were written, so the rollback is exercised.
//!
//! - `index` with one failing file stores every other file, lists the file in
//!   `files_failed` with its reason, warns naming it, and is not `failed`;
//!   `status` and `scan` read one file failed to persist;
//! - `sync` of a previously indexed file that fails keeps its last good facts,
//!   reports it, and `status` reads it stale; once the fault is removed the next
//!   reconcile stores the new facts and clears the mark;
//! - a run in which every file reaching persistence fails is `failed` and says
//!   nothing was persisted, on `index` and `sync`; a zero-admission index is not
//!   ([FR-IX-13](../../docs/specs/requirements/FR-IX-13.md)).
//!
//! The watcher's share lives in `tests/watcher.rs`; sync ≡ reindex after the
//! heal in `tests/indexing.rs`, beside the CR-015 fingerprint it reuses; the
//! CLI exit codes in `cli/tests/persist_failure.rs`; the MCP payloads in
//! `mcp/tests/persist_failure_payloads.rs`.
#![cfg(all(feature = "lang-rust", debug_assertions))]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use logos_core::Engine;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A three-file Rust project: `src/a.rs` calls into `src/b.rs`, `src/c.rs`
/// stands alone. The canonical root is returned for `sync` paths (macOS
/// tempdirs live behind `/var → /private/var`).
fn project() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    write(&root, "src/a.rs", "pub fn alpha_entry() { crate::b::beta_old(); }\n");
    write(&root, "src/b.rs", "pub fn beta_old() {}\n");
    write(&root, "src/c.rs", "pub fn gamma_alone() {}\n");
    (tmp, root)
}

/// `true` when a node named exactly `name` is in the graph.
fn has(engine: &Engine, name: &str) -> bool {
    engine
        .search(name, None, Some(10))
        .hits
        .iter()
        .any(|h| h.name == name)
}

fn runtime(engine: &Engine) -> &logos_core::Runtime {
    engine.runtime().expect("a started engine has a runtime")
}

#[test]
fn index_stores_every_other_file_and_reports_the_failing_one() {
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    runtime(&engine).inject_persist_fault("src/b.rs");

    let result = engine.index();
    assert_eq!(result.files_indexed, 2, "every other file persists: {result:?}");
    assert!(!result.failed, "one failed file is degraded, not a failed run");
    assert_eq!(result.files_failed, ["src/b.rs"]);
    assert_eq!(result.persist_failures.len(), 1);
    assert_eq!(result.persist_failures[0].path, "src/b.rs");
    assert!(
        result.persist_failures[0].reason.contains("injected persistence fault"),
        "the reason is the error that rolled the file back: {:?}",
        result.persist_failures[0]
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.starts_with("src/b.rs: could not be persisted") && w.contains("absent")),
        "a warning names the file: {:?}",
        result.warnings
    );
    assert!(has(&engine, "alpha_entry") && has(&engine, "gamma_alone"));
    assert!(!has(&engine, "beta_old"), "the failed file is absent from the graph");

    let status = engine.status();
    assert_eq!(status.persistence.failed_to_persist, 1, "{:?}", status.persistence);
    assert!(status.persistence.stale_files.is_empty(), "absent, not stale");
    assert!(
        status.warnings.iter().any(|w| w.contains("1 file(s) failed to persist")),
        "{:?}",
        status.warnings
    );
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["persistence"]["failed_to_persist"], 1, "{json}");

    // `scan` reconciles first: the file is retried, fails again under the
    // still-injected fault, and the readout survives the reconcile.
    let scan = engine.scan(true).expect("scan runs");
    assert_eq!(scan.persistence.failed_to_persist, 1, "{:?}", scan.persistence);
    assert!(scan.freshness.starts_with("INCOMPLETE"), "{}", scan.freshness);
    assert!(!has(&engine, "beta_old"));
}

#[test]
fn sync_keeps_the_last_good_facts_marks_them_stale_and_the_next_reconcile_clears_it() {
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    let clean = engine.index();
    assert_eq!((clean.files_indexed, clean.files_failed.len()), (3, 0));
    let status = engine.status();
    assert!(status.persistence.failed_to_persist == 0, "nothing failed yet");
    assert!(
        serde_json::to_value(&status).unwrap().get("persistence").is_none(),
        "a clean status elides the readout"
    );

    // Edit b.rs and c.rs and sync both with b.rs's persistence failing.
    write(&root, "src/b.rs", "pub fn beta_new() {}\n");
    write(&root, "src/c.rs", "pub fn gamma_edited() {}\n");
    runtime(&engine).inject_persist_fault("src/b.rs");
    let synced = engine.sync(&[root.join("src/b.rs"), root.join("src/c.rs")]);
    assert_eq!(synced.files_failed, ["src/b.rs"]);
    assert_eq!(synced.files_modified, 1, "c.rs persists; the failed b.rs is not counted");
    assert!(!synced.failed, "one of two files failed: degraded, not failed");
    assert!(has(&engine, "gamma_edited"), "every other file's change persists");
    assert!(
        synced
            .warnings
            .iter()
            .any(|w| w.starts_with("src/b.rs: could not be persisted") && w.contains("stale")),
        "{:?}",
        synced.warnings
    );
    assert!(has(&engine, "beta_old"), "the previous nodes remain");
    assert!(!has(&engine, "beta_new"), "the new facts did not land");
    let edges_kept = engine.callers("beta_old", Some(10));
    assert!(
        edges_kept.callers.iter().any(|c| c.name == "alpha_entry"),
        "the previous edges remain: {edges_kept:?}"
    );
    let status = engine.status();
    assert_eq!(status.persistence.failed_to_persist, 1);
    assert_eq!(status.persistence.stale_files, ["src/b.rs"], "status reports it stale");

    // Remove the fault: the next reconcile (the full-walk one `scan` runs)
    // retries the file — its hash still differs — stores the new facts and
    // clears the mark.
    runtime(&engine).clear_persist_faults();
    let scan = engine.scan(true).expect("scan runs");
    assert!(scan.persistence.is_clean(), "{:?}", scan.persistence);
    assert!(has(&engine, "beta_new"), "the new facts are stored");
    assert!(!has(&engine, "beta_old"), "the old facts are replaced");
    assert!(engine.status().persistence.is_clean(), "the stale mark is cleared");
}

#[test]
fn a_stale_file_reverted_to_its_indexed_content_is_no_longer_stale() {
    // The stale mark means "the graph does not hold this file as it stands".
    // Reverting the edit makes the stored facts exact again, and the next
    // reconcile clears the mark without re-persisting anything.
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    engine.index();
    write(&root, "src/b.rs", "pub fn beta_new() {}\n");
    runtime(&engine).inject_persist_fault("src/b.rs");
    engine.sync(&[root.join("src/b.rs")]);
    assert_eq!(engine.status().persistence.stale_files, ["src/b.rs"]);

    write(&root, "src/b.rs", "pub fn beta_old() {}\n");
    let synced = engine.sync(&[root.join("src/b.rs")]);
    assert!(synced.files_failed.is_empty(), "unchanged: nothing to persist");
    assert!(engine.status().persistence.is_clean(), "the mark is obsolete and cleared");
}

#[test]
fn a_stale_file_that_then_fails_to_load_keeps_its_mark_on_index_and_sync() {
    // The graph still holds the stale file's last good facts, and neither a
    // full index nor a sync that cannot read the file replaces them, so the
    // mark must survive both — `status` must never present it as indexed.
    for heal_with_index in [true, false] {
        let (_tmp, root) = project();
        let engine = Engine::start(&root).unwrap();
        engine.index();
        write(&root, "src/b.rs", "pub fn beta_new() {}\n");
        runtime(&engine).inject_persist_fault("src/b.rs");
        engine.sync(&[root.join("src/b.rs")]);
        runtime(&engine).clear_persist_faults();
        assert_eq!(engine.status().persistence.stale_files, ["src/b.rs"]);

        fs::write(root.join("src/b.rs"), b"pub fn beta_new() {}\n\xff\xfe\n").unwrap();
        let unread = if heal_with_index {
            engine.index().files_failed
        } else {
            engine.sync(&[root.join("src/b.rs")]).files_failed
        };
        assert_eq!(unread, ["src/b.rs"], "the file failed to load");
        assert_eq!(
            engine.status().persistence.stale_files,
            ["src/b.rs"],
            "index = {heal_with_index}: the unreadable file keeps its stale mark"
        );
        assert!(has(&engine, "beta_old"), "its last good facts are still in the graph");

        write(&root, "src/b.rs", "pub fn beta_new() {}\n");
        engine.index();
        assert!(engine.status().persistence.is_clean(), "readable again: persisted, cleared");
    }
}

#[test]
fn a_partial_sync_never_clears_the_mark_of_a_file_it_did_not_handle() {
    // A watcher or hook sync names only the files it saw change; another
    // file's stale mark is not its to clear — clearing it would present that
    // file as indexed (NFR-CC-04).
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    engine.index();
    write(&root, "src/b.rs", "pub fn beta_new() {}\n");
    runtime(&engine).inject_persist_fault("src/b.rs");
    engine.sync(&[root.join("src/b.rs")]);
    runtime(&engine).clear_persist_faults();

    write(&root, "src/c.rs", "pub fn gamma_edited() {}\n");
    let synced = engine.sync(&[root.join("src/c.rs")]);
    assert_eq!(synced.files_modified, 1);
    assert_eq!(
        engine.status().persistence.stale_files,
        ["src/b.rs"],
        "an unrelated sync leaves b.rs's mark alone"
    );
}

#[test]
fn a_new_file_failing_on_sync_is_absent_not_stale_and_its_reason_is_reported() {
    // A file the graph never held has no last good facts: it is recorded
    // absent, never stale, and its warning says so. The sync result carries
    // the file with its reason.
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    engine.index();
    write(&root, "src/d.rs", "pub fn delta_new() {}\n");
    write(&root, "src/c.rs", "pub fn gamma_edited() {}\n");
    runtime(&engine).inject_persist_fault("src/d.rs");
    let synced = engine.sync(&[root.join("src/d.rs"), root.join("src/c.rs")]);
    assert_eq!(synced.persist_failures.len(), 1, "{synced:?}");
    assert_eq!(synced.persist_failures[0].path, "src/d.rs");
    assert!(synced.persist_failures[0].reason.contains("injected persistence fault"));
    let warning = synced
        .warnings
        .iter()
        .find(|w| w.starts_with("src/d.rs: could not be persisted"))
        .expect("a warning names the file");
    assert!(warning.contains("absent from the graph") && !warning.contains("stale"), "{warning}");
    let p = engine.status().persistence;
    assert_eq!(p.failed_to_persist, 1);
    assert!(p.stale_files.is_empty(), "a never-held file is absent, not stale: {p:?}");
    assert!(!has(&engine, "delta_new"));
}

#[test]
fn a_full_walk_reconcile_clears_the_mark_of_a_file_it_no_longer_finds() {
    // A file that failed on index has no `files` row, so no removal can clear
    // it; a full-walk reconcile that no longer finds it must, even though no
    // path names it.
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    runtime(&engine).inject_persist_fault("src/b.rs");
    engine.index();
    runtime(&engine).clear_persist_faults();
    assert_eq!(engine.status().persistence.failed_to_persist, 1);

    fs::remove_file(root.join("src/b.rs")).unwrap();
    engine.scan(true).expect("a full-walk reconcile runs");
    assert!(engine.status().persistence.is_clean(), "{:?}", engine.status().persistence);
}

#[test]
fn a_failed_file_deleted_from_disk_leaves_no_mark() {
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    runtime(&engine).inject_persist_fault("src/b.rs");
    engine.index();
    assert_eq!(engine.status().persistence.failed_to_persist, 1);

    fs::remove_file(root.join("src/b.rs")).unwrap();
    engine.sync(&[root.join("src/b.rs")]);
    assert!(engine.status().persistence.is_clean(), "a deleted file is neither failed nor stale");
}

#[test]
fn a_stale_file_purged_by_a_config_narrowing_leaves_no_mark() {
    // The removal path itself clears the mark: a config narrowing purges the
    // stale file through the navigation prologue — no sync names it, so only
    // `remove_file` can say it is no longer stale.
    let (_tmp, root) = project();
    {
        let engine = Engine::start(&root).unwrap();
        engine.index();
        write(&root, "src/b.rs", "pub fn beta_new() {}\n");
        runtime(&engine).inject_persist_fault("src/b.rs");
        engine.sync(&[root.join("src/b.rs")]);
        assert_eq!(engine.status().persistence.stale_files, ["src/b.rs"]);
    }
    write(&root, ".logos/config.toml", "exclude = [\"src/b.rs\"]\n");
    let engine = Engine::start(&root).unwrap();
    assert!(has(&engine, "alpha_entry"), "the read runs the prologue purge");
    assert!(!has(&engine, "beta_old"), "the narrowing purged the stale file");
    assert!(engine.status().persistence.is_clean(), "a purged file is no longer stale");
}

#[test]
fn a_run_that_persists_nothing_it_reached_is_failed_on_index_and_sync() {
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    runtime(&engine).inject_persist_fault("*");

    let result = engine.index();
    assert_eq!(result.files_indexed, 0);
    assert!(result.failed, "admitted files and persisted none: {result:?}");
    assert!(
        result.warnings.iter().any(|w| w.starts_with("nothing was persisted: all 3 file(s)")),
        "{:?}",
        result.warnings
    );
    assert_eq!(result.files_failed.len(), 3);

    write(&root, "src/c.rs", "pub fn gamma_edited() {}\n");
    let synced = engine.sync(&[root.join("src/c.rs")]);
    assert!(synced.failed, "{synced:?}");
    assert!(synced.warnings.iter().any(|w| w.starts_with("nothing was persisted")));

    // A sync whose one edit fails but whose removal commits persisted
    // something: degraded, and it never claims "nothing was persisted".
    runtime(&engine).clear_persist_faults();
    assert_eq!(engine.index().files_indexed, 3, "a clean re-index");
    write(&root, "src/b.rs", "pub fn beta_failing_edit() {}\n");
    fs::remove_file(root.join("src/c.rs")).unwrap();
    runtime(&engine).inject_persist_fault("src/b.rs");
    let mixed = engine.sync(&[root.join("src/b.rs"), root.join("src/c.rs")]);
    assert_eq!((mixed.files_removed, mixed.files_failed.len()), (1, 1), "{mixed:?}");
    assert!(!mixed.failed, "a committed removal is something persisted: {mixed:?}");
    assert!(
        !mixed.warnings.iter().any(|w| w.starts_with("nothing was persisted")),
        "{:?}",
        mixed.warnings
    );

    // Some persisted, some failed: degraded, never failed.
    runtime(&engine).clear_persist_faults();
    runtime(&engine).inject_persist_fault("src/a.rs");
    let partial = engine.index();
    assert_eq!(partial.files_indexed, 1, "b.rs persists; c.rs was deleted above");
    assert!(!partial.failed, "a partial failure is degraded");
}

#[test]
fn an_index_admitting_no_file_is_not_failed() {
    // FR-IX-13: zero admission is not a persistence failure, even with every
    // file faulted — nothing reached persistence.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "README.txt", "no source here\n");
    let engine = Engine::start(tmp.path()).unwrap();
    runtime(&engine).inject_persist_fault("*");
    let result = engine.index();
    assert_eq!(result.files_indexed, 0);
    assert!(!result.failed, "{result:?}");
    assert!(result.persist_failures.is_empty());
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("failed").is_none() && json.get("persist_failures").is_none(), "{json}");
}

#[test]
fn a_full_index_rewrites_the_record_to_its_own_failures() {
    // A file that failed on a sync and then persists on a later full index
    // leaves the record; a file failing on that index enters it, absent.
    let (_tmp, root) = project();
    let engine = Engine::start(&root).unwrap();
    engine.index();
    write(&root, "src/b.rs", "pub fn beta_new() {}\n");
    runtime(&engine).inject_persist_fault("src/b.rs");
    engine.sync(&[root.join("src/b.rs")]);
    assert_eq!(engine.status().persistence.stale_files, ["src/b.rs"]);

    runtime(&engine).clear_persist_faults();
    runtime(&engine).inject_persist_fault("src/c.rs");
    engine.index();
    let p = engine.status().persistence;
    assert_eq!(p.failed_to_persist, 1, "{p:?}");
    assert!(p.stale_files.is_empty(), "src/b.rs persisted; src/c.rs is absent, not stale");
    assert!(has(&engine, "beta_new") && !has(&engine, "gamma_alone"));
}
