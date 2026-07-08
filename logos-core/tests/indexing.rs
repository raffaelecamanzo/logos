//! Black-box integration tests for the indexing + incremental-sync pipeline
//! (S-010, [pipeline-orchestrator], [ADR-10]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! façade exactly as the CLI/MCP surfaces will:
//! - a full index populates the graph with nodes and edges
//!   ([FR-IX-01](../../docs/specs/requirements/FR-IX-01.md),
//!   [UAT-IX-01](../../docs/specs/requirements/UAT-IX-01.md));
//! - sync re-extracts only the files whose blake3 hash changed
//!   ([FR-SY-03](../../docs/specs/requirements/FR-SY-03.md),
//!   [UAT-SY-01](../../docs/specs/requirements/UAT-SY-01.md));
//! - a cross-file caller→callee edge survives a sync of the callee's file via
//!   capture-before-delete
//!   ([FR-SY-02](../../docs/specs/requirements/FR-SY-02.md),
//!   [NFR-RA-04](../../docs/specs/requirements/NFR-RA-04.md),
//!   [UAT-SY-02](../../docs/specs/requirements/UAT-SY-02.md));
//! - the first evaluation against an un-indexed project auto-indexes
//!   ([FR-IX-07](../../docs/specs/requirements/FR-IX-07.md)).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these tests need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use logos_core::model::{EdgeKind, NodeId, NodeKind, RefForm};
use logos_core::{Engine, Runtime};

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// The id of the (single) node named `name`, via the FTS read seam.
fn node_id_of(rt: &Runtime, name: &str) -> NodeId {
    // Filter to Function: since S-011 every file also carries a Module node
    // named after its stem (`caller.rs` → module "caller"), so a bare-name
    // search would be ambiguous for these single-fn-per-file fixtures.
    rt.submit_read(|store| {
        let rows = store.search(name, Some(NodeKind::Function), 16)?;
        Ok(rows.into_iter().find(|r| r.name == name).map(|r| r.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no function node named {name}"))
}

/// Recursively copy every `.rs` file under `dir` into `dst_root`, preserving the
/// path relative to `base`.
fn copy_rs_tree(base: &Path, dir: &Path, dst_root: &Path) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            copy_rs_tree(base, &path, dst_root);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path.strip_prefix(base).expect("under base");
            let dst = dst_root.join(rel);
            fs::create_dir_all(dst.parent().expect("has parent")).expect("mkdir");
            fs::copy(&path, &dst).expect("copy source file");
        }
    }
}

#[test]
fn index_populates_the_graph_on_logos_own_source() {
    // The first dogfood milestone (UAT-IX-01): index logos-core's own source.
    // The tree is copied into a throwaway root so we never write a `.logos/`
    // store into the repository itself (NFR-SE-04: writes confined to `.logos/`).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = TempDir::new().expect("temp root");
    copy_rs_tree(&manifest, &manifest.join("src"), tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();

    assert!(
        result.files_indexed >= 10,
        "expected to index logos-core's source files, got {}",
        result.files_indexed
    );
    assert!(
        result.nodes_created > 100,
        "the source tree should yield many nodes, got {}",
        result.nodes_created
    );
    assert!(
        result.edges_created > 0,
        "nested declarations should yield Contains edges, got {}",
        result.edges_created
    );
    assert!(
        !result.warnings.iter().any(|w| w.contains("failed")),
        "a clean index reports no failure warnings: {:?}",
        result.warnings
    );

    // The files table reflects exactly what the result reported.
    let rt = engine.runtime().expect("runtime present");
    let stored = rt
        .submit_read(|s| Ok(s.indexed_files()?.len()))
        .expect("read runs");
    assert_eq!(stored as u64, result.files_indexed);
}

#[test]
fn index_result_carries_a_reconciling_per_phase_breakdown() {
    // FR-OB-06 / CR-057: a full index surfaces a per-phase duration breakdown
    // (discover/load/extract/persist/resolve/framework/dispatch/annotate). It is
    // sourced from the single `tracing` seam (FR-OB-01), so this asserts the
    // *shape and reconciliation* contract, not wall-clock magnitudes (which are
    // host-dependent and belong to the perf_envelope cold-index benchmark).
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = TempDir::new().expect("temp root");
    copy_rs_tree(&manifest, &manifest.join("src"), tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    let p = &result.phases;

    // A non-trivial index takes measurable wall-clock overall.
    assert!(
        result.duration_ms > 0,
        "indexing logos-core's own source takes measurable time: {result:?}"
    );

    // Reconciliation (FR-OB-06): each phase is timed exactly once and none is
    // double-counted, so the eight disjoint phase intervals sum to no more than
    // the whole-index wall-clock. Millisecond flooring only widens the margin.
    let phases_sum = p.discover_ms
        + p.load_ms
        + p.extract_ms
        + p.persist_ms
        + p.resolve_ms
        + p.framework_ms
        + p.dispatch_ms
        + p.annotate_ms;
    assert!(
        phases_sum <= result.duration_ms,
        "the per-phase breakdown must not exceed the total (no double-counting): \
         sum {phases_sum}ms > total {}ms — {p:?}",
        result.duration_ms
    );

    // The breakdown genuinely attributes the cold index: the four phases that
    // any non-trivial index must spend measurable time in — parse, write,
    // resolve, annotate — are each individually non-zero, proving the seam is
    // wired for the phases that matter (a dropped or un-instrumented one would
    // read 0). Asserted per-phase rather than as a fraction of the total so the
    // guard stays independent of host timing and of the untimed inter-pass
    // bookkeeping (purge/fingerprint/revision-advance, and the debug-build
    // structural-integrity assertion) that sits between the timed phases.
    // discover/load/framework/dispatch are intentionally excluded — they can
    // legitimately round to sub-millisecond on this fixture.
    assert!(
        p.extract_ms > 0 && p.persist_ms > 0 && p.resolve_ms > 0 && p.annotate_ms > 0,
        "the heavy timed phases must each attribute measurable time: {p:?}"
    );

    // The `--json index` surface (FR-OB-06 AC): `phases` serialises as an object
    // carrying every one of the eight phase keys. This is the machine contract
    // the CLI's `serde_json::to_string(&IndexResult)` emits.
    let json = serde_json::to_value(&result).expect("IndexResult serialises");
    let phases = json
        .get("phases")
        .and_then(|v| v.as_object())
        .expect("phases object present on the index result");
    for key in [
        "discover_ms",
        "load_ms",
        "extract_ms",
        "persist_ms",
        "resolve_ms",
        "framework_ms",
        "dispatch_ms",
        "annotate_ms",
    ] {
        assert!(
            phases.get(key).and_then(serde_json::Value::as_u64).is_some(),
            "the per-phase breakdown exposes `{key}` as a duration: {phases:?}"
        );
    }
}

#[test]
fn sync_re_extracts_only_changed_files() {
    // FR-SY-03 / UAT-SY-01: blake3 dirty detection — only the modified file is
    // re-extracted; unchanged files are skipped.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");
    write(tmp.path(), "beta.rs", "fn beta() {}\n");
    write(tmp.path(), "gamma.rs", "fn gamma() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    let idx = engine.index();
    assert_eq!(idx.files_indexed, 3, "all three files indexed");

    // beta and gamma are unchanged across the sync; record their node ids to
    // prove they are not re-extracted (a re-extract deletes and re-inserts,
    // assigning a new rowid).
    let beta_before = node_id_of(rt, "beta");
    let gamma_before = node_id_of(rt, "gamma");

    // Change only alpha's content (its hash changes).
    write(tmp.path(), "alpha.rs", "fn alpha() { let _x = 1; }\n");

    let result = engine.sync(&[
        PathBuf::from("alpha.rs"),
        PathBuf::from("beta.rs"),
        PathBuf::from("gamma.rs"),
    ]);
    assert_eq!(result.files_modified, 1, "only alpha changed");
    assert_eq!(result.files_added, 0);
    assert_eq!(result.files_removed, 0);

    let beta_after = node_id_of(rt, "beta");
    let gamma_after = node_id_of(rt, "gamma");
    assert_eq!(
        gamma_before, gamma_after,
        "the other unchanged file must not be re-extracted either (blake3 dirty check)"
    );
    assert_eq!(
        beta_before, beta_after,
        "an unchanged file must not be re-extracted (blake3 dirty check)"
    );
}

#[test]
fn capture_before_delete_preserves_a_cross_file_edge() {
    // ADR-10 / FR-SY-02 / UAT-SY-02: a caller→callee edge crossing a file
    // boundary must survive a sync of the callee's file.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "caller.rs", "fn caller() {}\n");
    write(tmp.path(), "callee.rs", "fn callee() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    let caller_id = node_id_of(rt, "caller");
    let callee_id = node_id_of(rt, "callee");

    // Plant a synthetic cross-file Calls edge (the resolution engine emits these
    // for real in S-011; here one is planted to exercise capture-before-delete).
    rt.submit_write(move |w| w.insert_edge(caller_id, callee_id, EdgeKind::Calls))
        .expect("plant edge");
    let callees = rt
        .submit_read(move |s| s.callees(caller_id))
        .expect("read callees");
    assert!(
        callees.iter().any(|n| n.id == callee_id),
        "the planted caller→callee edge is present before the sync"
    );

    // Edit the callee's file: its hash changes, but `fn callee` keeps its symbol.
    write(tmp.path(), "callee.rs", "// touched\nfn callee() {}\n");
    let result = engine.sync(&[PathBuf::from("callee.rs")]);
    assert_eq!(
        result.files_modified, 1,
        "the callee's file was re-extracted"
    );

    // The callee node was deleted and re-created (new rowid) — yet the inbound
    // cross-file edge was captured before the delete and rebound by symbol.
    let new_callee_id = node_id_of(rt, "callee");
    assert_ne!(
        new_callee_id, callee_id,
        "the callee node is re-created on a sync of its file"
    );
    let callers = rt
        .submit_read(move |s| s.callers(new_callee_id))
        .expect("read callers");
    assert!(
        callers.iter().any(|n| n.id == caller_id),
        "the cross-file caller→callee edge must survive the sync \
         (ADR-10 capture-before-delete)"
    );
}

#[test]
fn auto_index_triggers_on_first_evaluation() {
    // FR-IX-07: the first evaluation against an un-indexed project auto-indexes
    // before serving; a subsequent call is a no-op.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "solo.rs", "fn solo() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    let before = rt
        .submit_read(|s| Ok(s.indexed_files()?.len()))
        .expect("read runs");
    assert_eq!(before, 0, "a freshly started engine has an empty graph");

    let first = engine.ensure_indexed();
    assert!(
        first.files_indexed >= 1,
        "the first evaluation auto-indexes"
    );
    let after = rt
        .submit_read(|s| Ok(s.indexed_files()?.len()))
        .expect("read runs");
    assert_eq!(after, 1, "the graph is populated after auto-index");

    let second = engine.ensure_indexed();
    assert_eq!(
        second.files_indexed, 0,
        "an already-indexed graph is not re-indexed"
    );
}

#[test]
fn sync_adds_new_and_removes_deleted_files() {
    // FR-SY-01: an explicit changed-file set drives additions and removals.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // A brand-new file is an addition.
    write(tmp.path(), "beta.rs", "fn beta() {}\n");
    let added = engine.sync(&[PathBuf::from("beta.rs")]);
    assert_eq!(added.files_added, 1);
    assert_eq!(added.files_modified, 0);

    // A path gone from disk is a removal.
    fs::remove_file(tmp.path().join("alpha.rs")).expect("delete alpha");
    let removed = engine.sync(&[PathBuf::from("alpha.rs")]);
    assert_eq!(removed.files_removed, 1);

    // The graph now holds beta only.
    let mut paths: Vec<String> = rt
        .submit_read(|s| s.indexed_files())
        .expect("read runs")
        .into_iter()
        .map(|f| f.path)
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["beta.rs".to_string()]);
}

#[test]
fn sync_drops_a_captured_edge_when_the_target_symbol_disappears() {
    // ADR-10 / rebind drop path: if the synced file's edit removes the callee
    // declaration, the captured inbound edge cannot rebind (no target) and is
    // dropped — the sync must still succeed without error or panic.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "caller.rs", "fn caller() {}\n");
    write(tmp.path(), "callee.rs", "fn callee() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    let caller_id = node_id_of(rt, "caller");
    let callee_id = node_id_of(rt, "callee");
    rt.submit_write(move |w| w.insert_edge(caller_id, callee_id, EdgeKind::Calls))
        .expect("plant edge");

    // The edit renames the callee, so its old symbol no longer exists after
    // re-extraction — the captured edge has no target to rebind to.
    write(tmp.path(), "callee.rs", "fn callee_renamed() {}\n");
    let result = engine.sync(&[PathBuf::from("callee.rs")]);
    assert_eq!(
        result.files_modified, 1,
        "the callee's file was re-extracted"
    );
    assert!(
        result.warnings.is_empty(),
        "dropping an unrebindable edge is not a warning-worthy fault: {:?}",
        result.warnings
    );

    // The renamed declaration exists; the old callee is gone; the dangling edge
    // was dropped (the caller now calls nothing).
    let renamed = node_id_of(rt, "callee_renamed");
    let callees = rt
        .submit_read(move |s| s.callees(caller_id))
        .expect("read callees");
    assert!(
        !callees.iter().any(|n| n.id == renamed),
        "the edge to the renamed callee must not be fabricated (NFR-RA-05)"
    );
    assert!(
        callees.is_empty(),
        "the unrebindable edge was dropped, leaving the caller with no callees"
    );
}

#[test]
fn sync_skips_unsupported_extensions_and_rejects_outside_root_paths() {
    // A changed-file set may name non-source or out-of-tree paths; sync must
    // skip the former silently and reject the latter with a warning (NFR-SE-04),
    // never touching the graph in either case.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");
    write(tmp.path(), "Cargo.toml", "[package]\nname = \"x\"\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // An unsupported extension is skipped: no grammar claims `.toml`.
    let unsupported = engine.sync(&[PathBuf::from("Cargo.toml")]);
    assert_eq!(unsupported.files_added, 0);
    assert_eq!(unsupported.files_modified, 0);
    assert_eq!(unsupported.files_removed, 0);
    assert!(unsupported.warnings.is_empty());

    // An escaping path is rejected with a warning and no graph mutation.
    let escaping = engine.sync(&[PathBuf::from("../escape.rs")]);
    assert_eq!(escaping.files_added, 0);
    assert_eq!(escaping.files_modified, 0);
    assert_eq!(escaping.files_removed, 0);
    assert!(
        escaping.warnings.iter().any(|w| w.contains("outside")),
        "an out-of-root path is reported, got {:?}",
        escaping.warnings
    );
}

// ── CR-052 / FR-SY-10: Channel-B orphan-file removal on a full-walk sync ──────
//
// A file deleted on disk must leave zero resident nodes after one *full-walk*
// sync whose path-set no longer names it (it is gone, so discovery cannot), and
// a *partial* watcher sync must never purge a file outside its own path-set.
// [`SyncScope`] gates the sweep between these two contracts ([ADR-46]).

/// Does a Function node named `name` currently exist in the graph?
fn function_node_exists(rt: &Runtime, name: &str) -> bool {
    rt.submit_read(|store| {
        let rows = store.search(name, Some(NodeKind::Function), 16)?;
        Ok(rows.into_iter().any(|r| r.name == name))
    })
    .expect("read runs")
}

#[test]
fn full_walk_sync_purges_a_file_deleted_off_the_path_set() {
    // FR-SY-10 AC: after indexing A+B and deleting B on disk, one full-walk sync
    // whose path-set EXCLUDES B (B is gone, so discovery no longer yields it)
    // leaves zero resident nodes for B — the Channel-B disk-deletion sweep, run
    // over the full stored set independent of the admission fingerprint.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");
    write(tmp.path(), "beta.rs", "fn beta() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(function_node_exists(rt, "beta"), "beta indexed to start");

    // Delete beta.rs on disk, then run a FULL-WALK sync whose path-set is only the
    // surviving candidate (alpha.rs) — exactly what `reconcile` passes once
    // discovery drops the deleted file. beta.rs is neither in the path-set nor on
    // disk, so the full-walk sweep reconciles it out.
    fs::remove_file(tmp.path().join("beta.rs")).expect("delete beta");
    let registry =
        logos_core::plugin::LanguageRegistry::load(tmp.path()).expect("registry loads");
    let config = logos_core::config::load_config_from_root(tmp.path()).expect("config loads");
    let result = logos_core::pipeline::sync(
        rt,
        &registry,
        tmp.path(),
        &config,
        &[PathBuf::from("alpha.rs")],
        logos_core::pipeline::SyncScope::FullWalk,
    )
    .expect("full-walk sync runs");

    assert_eq!(result.files_removed, 1, "the deleted file was swept out");
    assert!(
        !function_node_exists(rt, "beta"),
        "zero resident nodes remain for the deleted file (FR-SY-10)"
    );
    // The still-present, unchanged file is untouched (its hash matched — skipped).
    assert!(function_node_exists(rt, "alpha"), "the surviving file stays");
    let stored: Vec<String> = rt
        .submit_read(|s| s.indexed_files())
        .expect("read runs")
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(stored, vec!["alpha.rs".to_string()], "only alpha remains stored");
}

#[test]
fn partial_sync_does_not_purge_files_outside_its_path_set() {
    // FR-SY-10 AC / ADR-46 open question: a partial watcher `sync` carries no
    // evidence about files outside its set. Deleting beta.rs on disk and then
    // syncing ONLY alpha.rs (a partial batch that never names beta) must leave
    // beta's nodes resident — a single-file watcher event must not purge the rest
    // of the graph. Reconciling the deletion is the full-walk backstop's job.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "alpha.rs", "fn alpha() {}\n");
    write(tmp.path(), "beta.rs", "fn beta() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // Delete beta.rs off disk, then edit + partial-sync ONLY alpha.rs.
    fs::remove_file(tmp.path().join("beta.rs")).expect("delete beta");
    write(tmp.path(), "alpha.rs", "fn alpha() { let _ = 1; }\n");
    let partial = engine.sync(&[PathBuf::from("alpha.rs")]);
    assert_eq!(partial.files_modified, 1, "alpha re-extracted");
    assert_eq!(
        partial.files_removed, 0,
        "a partial sync purges nothing outside its path-set"
    );
    assert!(
        function_node_exists(rt, "beta"),
        "beta stays resident — the partial sync did not sweep it (ADR-46 path-set contract)"
    );

    // The full-walk backstop (a reconciling scan) then does remove it.
    engine.scan(true).expect("reconcile scan runs");
    assert!(
        !function_node_exists(rt, "beta"),
        "the full-walk reconcile sweeps the disk-deleted file out"
    );
}

#[test]
fn full_walk_sweep_returns_inbound_refs_to_unresolved() {
    // NFR-RA-05: when the full-walk sweep removes a disk-deleted file, the
    // cross-file edges that pointed *into* it must be dropped, never fabricated —
    // the swept file's nodes cascade their incident edges away and the producing
    // ledger row is left unresolved, exactly as a fresh index would leave it.
    // Mirrors `sync_drops_a_captured_edge_when_the_target_symbol_disappears`: a
    // bare cross-file call needs a `use`/module path to auto-bind, so we plant the
    // caller -> callee edge directly, then remove callee.rs via the full-walk
    // sweep (its path-set excludes the deleted file).
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "caller.rs", "fn caller() {}\n");
    write(tmp.path(), "callee.rs", "pub fn target() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    let caller_id = node_id_of(rt, "caller");
    let target_id = node_id_of(rt, "target");
    rt.submit_write(move |w| w.insert_edge(caller_id, target_id, EdgeKind::Calls))
        .expect("plant cross-file edge");
    let bound = rt
        .submit_read(move |s| s.callees(caller_id))
        .expect("read callees");
    assert!(
        bound.iter().any(|n| n.id == target_id),
        "the cross-file caller -> target edge is bound before deletion"
    );

    // Delete callee.rs and full-walk sync with a path-set that excludes it.
    fs::remove_file(tmp.path().join("callee.rs")).expect("delete callee");
    let registry =
        logos_core::plugin::LanguageRegistry::load(tmp.path()).expect("registry loads");
    let config = logos_core::config::load_config_from_root(tmp.path()).expect("config loads");
    let result = logos_core::pipeline::sync(
        rt,
        &registry,
        tmp.path(),
        &config,
        &[PathBuf::from("caller.rs")],
        logos_core::pipeline::SyncScope::FullWalk,
    )
    .expect("full-walk sync runs");
    assert_eq!(result.files_removed, 1, "callee.rs was swept out");

    assert!(
        !function_node_exists(rt, "target"),
        "the swept file's nodes are gone"
    );
    let callees = rt
        .submit_read(move |s| s.callees(caller_id))
        .expect("read callees");
    assert!(
        callees.is_empty(),
        "the inbound edge returned to unresolved, never fabricated (NFR-RA-05): {callees:?}"
    );
}

// ── CR-015: incremental-sync ≡ reindex equivalence safety net ────────────────
//
// The invariant the incremental-resolution rework (CR-015) MUST preserve:
// syncing a tree into a given state produces a graph byte-identical to a full
// index of that state. We compare a *rowid-independent* fingerprint — node ids
// differ between two independently built stores, so every id (edge endpoints) is
// mapped to its canonical symbol before comparison — of nodes, edges, the
// reference ledger, and annotation verdicts.
//
// These pin the cases where a naive "only re-bind the changed file's refs" would
// diverge from a from-scratch index: a deferred ref binding when its target is
// added in another file, an edge retracting when its target is renamed/removed,
// and an edge retracting when a second same-named definition makes the call
// ambiguous (never-fabricate). All three change a binding in an UNCHANGED file
// because of a change elsewhere — exactly what an incremental resolver must not
// miss. If incremental resolution ever binds a different edge set than a
// reindex, these fail loudly.

/// A rowid-independent fingerprint of the whole graph (nodes, edges, the
/// reference ledger, annotation verdicts), each section a sorted multiset of
/// lines. `clone_group` is deliberately excluded: its representative is the
/// component's minimum rowid, which is insertion-order-sensitive and so differs
/// between two independently built stores even for identical clusters — and
/// near-clone clustering is orthogonal to (and unchanged by) CR-015.
fn graph_fingerprint(rt: &Runtime) -> String {
    rt.submit_read(|store| {
        let nodes = store.all_nodes()?;
        let edges = store.all_edges()?;
        let refs = store.unresolved_refs()?;
        let anns = store.annotation_nodes()?;

        // rowid -> canonical symbol, so edge endpoints compare by identity, not
        // by store-local rowid.
        let sym_of: std::collections::BTreeMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id.0, n.symbol.as_str().to_string()))
            .collect();
        let key = |id: i64| -> String {
            sym_of
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{id}>"))
        };

        let mut node_lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "N {}|{:?}|{}|{}|{:?}|{:?}",
                    n.symbol.as_str(),
                    n.kind,
                    n.name,
                    n.file_path.as_deref().unwrap_or(""),
                    n.start_line,
                    n.end_line,
                )
            })
            .collect();
        node_lines.sort();

        let mut edge_lines: Vec<String> = edges
            .iter()
            .map(|e| format!("E {} -> {} [{:?}]", key(e.source.0), key(e.target.0), e.kind))
            .collect();
        edge_lines.sort();

        let mut ref_lines: Vec<String> = refs
            .iter()
            // Exclude capture-before-delete rows (`RefForm::Symbol`, ADR-10): they
            // are a sync-only internal bookkeeping artifact a from-scratch index
            // never produces (capture runs only on re-extraction), so counting them
            // would make sync ≠ reindex for reasons orthogonal to CR-015. The edges
            // they preserve ARE compared above, so a mis-bound capture still fails
            // the edge section — only the redundant ledger row itself is ignored.
            .filter(|r| r.form != RefForm::Symbol)
            .map(|r| {
                format!(
                    "R {}|{}|{:?}|{:?}|{}|{:?}",
                    r.source_symbol, r.target, r.form, r.kind, r.resolved, r.payload,
                )
            })
            .collect();
        ref_lines.sort();

        let mut ann_lines: Vec<String> = anns
            .iter()
            .map(|a| {
                format!(
                    "A {}|dead={:?}|dup={:?}|test={}|layer={:?}|exp={}|der={}|fp={:?}",
                    key(a.id.0),
                    a.is_dead,
                    a.is_duplicate,
                    a.is_test,
                    a.layer_membership,
                    a.exported,
                    a.derived,
                    a.fingerprint,
                )
            })
            .collect();
        ann_lines.sort();

        let mut out = String::new();
        for section in [node_lines, edge_lines, ref_lines, ann_lines] {
            for line in section {
                out.push_str(&line);
                out.push('\n');
            }
            out.push_str("----\n");
        }
        Ok(out)
    })
    .expect("fingerprint read runs")
}

/// One edit applied to a tree before a sync.
enum Edit<'a> {
    /// Create or overwrite a file with new contents.
    Put(&'a str, &'a str),
    /// Delete a file.
    Del(&'a str),
}

/// The CR-015 invariant: applying `edits` to `initial` via `sync` yields a graph
/// identical to a full `index` of the post-edit tree.
fn assert_sync_matches_reindex(initial: &[(&str, &str)], edits: &[Edit]) {
    // Arm A — index the initial tree, then sync exactly the edited paths.
    let tmp_a = TempDir::new().expect("temp a");
    for (rel, body) in initial {
        write(tmp_a.path(), rel, body);
    }
    let engine_a = Engine::start(tmp_a.path()).expect("engine a starts");
    engine_a.index();
    // The real watcher canonicalizes the root and `notify` reports *real* paths,
    // so the changed set `sync` receives is canonical-root-relative. Mirror that:
    // on macOS a TempDir under `/var` symlinks to `/private/var`, and `sync`
    // canonicalizes the root, so an un-canonicalized changed path would
    // `strip_prefix`-fail and be skipped as "outside the project root" — turning
    // every sync into a silent no-op (the bug the watcher avoids at
    // `watch/mod.rs` for this exact reason).
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root a");
    let mut changed: Vec<PathBuf> = Vec::new();
    for edit in edits {
        match edit {
            Edit::Put(rel, body) => {
                write(tmp_a.path(), rel, body);
                changed.push(root_a.join(rel));
            }
            Edit::Del(rel) => {
                fs::remove_file(tmp_a.path().join(rel)).expect("remove fixture file");
                changed.push(root_a.join(rel));
            }
        }
    }
    engine_a.sync(&changed);
    let fp_sync = graph_fingerprint(engine_a.runtime().expect("runtime a"));

    // Arm B — the post-edit tree indexed from scratch: the oracle.
    let mut final_state: std::collections::BTreeMap<&str, &str> =
        initial.iter().copied().collect();
    for edit in edits {
        match edit {
            Edit::Put(rel, body) => {
                final_state.insert(rel, body);
            }
            Edit::Del(rel) => {
                final_state.remove(rel);
            }
        }
    }
    let tmp_b = TempDir::new().expect("temp b");
    for (rel, body) in &final_state {
        write(tmp_b.path(), rel, body);
    }
    let engine_b = Engine::start(tmp_b.path()).expect("engine b starts");
    engine_b.index();
    let fp_reindex = graph_fingerprint(engine_b.runtime().expect("runtime b"));

    assert_eq!(
        fp_sync, fp_reindex,
        "sync-to-state must equal index-of-state (CR-015 equivalence invariant)"
    );
}

#[test]
fn sync_equiv_modifying_a_body() {
    // A pure body edit (no signature change) must leave the graph as a reindex
    // would — the baseline equivalence.
    assert_sync_matches_reindex(
        &[("a.rs", "fn a() { b(); }\nfn b() {}\n")],
        &[Edit::Put("a.rs", "fn a() { b(); b(); }\nfn b() {}\n")],
    );
}

#[test]
fn sync_equiv_added_file_satisfies_a_deferred_ref() {
    // a.rs calls `target`, unresolved while nothing defines it. Adding b.rs (and
    // syncing ONLY b.rs) must bind the deferred ref in the UNCHANGED a.rs — the
    // case incremental resolution most easily misses.
    assert_sync_matches_reindex(
        &[("a.rs", "fn a() { target(); }\n")],
        &[Edit::Put("b.rs", "pub fn target() {}\n")],
    );
}

#[test]
fn sync_equiv_renaming_a_target_unbinds_the_edge() {
    // The reverse: renaming b.rs's `target` away must retract the a.rs -> target
    // edge, exactly as a reindex would leave it unresolved.
    assert_sync_matches_reindex(
        &[
            ("a.rs", "fn a() { target(); }\n"),
            ("b.rs", "pub fn target() {}\n"),
        ],
        &[Edit::Put("b.rs", "pub fn renamed() {}\n")],
    );
}

#[test]
fn sync_equiv_second_definition_introduces_ambiguity() {
    // a.rs -> target is uniquely bound while only b.rs defines `target`. Adding a
    // second definition (c.rs) makes the call ambiguous, so never-fabricate
    // retracts the edge. Syncing ONLY c.rs must match a reindex (no edge) — a
    // retraction in the UNCHANGED a.rs driven by a change elsewhere.
    assert_sync_matches_reindex(
        &[
            ("a.rs", "fn a() { target(); }\n"),
            ("b.rs", "pub fn target() {}\n"),
        ],
        &[Edit::Put("c.rs", "pub fn target() {}\n")],
    );
}

#[test]
fn sync_equiv_deleting_a_file() {
    // Deleting the callee's file removes its nodes and returns the cross-file
    // ref to unresolved — identical to never having indexed it.
    assert_sync_matches_reindex(
        &[
            ("a.rs", "fn a() { target(); }\n"),
            ("b.rs", "pub fn target() {}\n"),
        ],
        &[Edit::Del("b.rs")],
    );
}

#[test]
fn sync_equiv_renaming_through_an_alias_unbinds() {
    // a.rs reaches b.rs's `orig` through an `as`-rename (`use crate::b::orig as
    // ali; ali();`). Renaming `orig` away in b.rs must retract that edge AND flip
    // a.rs's ledger row to unresolved — even though a.rs's row spells its target
    // "ali", a name b.rs never defined. This is the case bare target-token matching
    // misses: the incremental resolver only catches it by expanding a.rs's alias
    // chain (ali -> crate::b::orig) against the dirty name `orig`. Guards
    // `Index::ref_affected`'s alias chase.
    assert_sync_matches_reindex(
        &[
            ("a.rs", "use crate::b::orig as ali;\nfn a() { ali(); }\n"),
            ("b.rs", "pub fn orig() {}\n"),
        ],
        &[Edit::Put("b.rs", "pub fn renamed() {}\n")],
    );
}

#[test]
fn sync_equiv_glob_import_satisfies_a_deferred_call() {
    // a.rs calls `helper()` under a glob import of b.rs; while b.rs defines no
    // `helper` the call is deferred. Adding `helper` to b.rs (syncing ONLY b.rs)
    // must bind the call in the UNCHANGED a.rs. Globs need no alias expansion — the
    // call resolves under its own bare name `helper`, which this sync marks dirty —
    // so this pins that the bare-token path covers glob satisfaction.
    assert_sync_matches_reindex(
        &[
            ("a.rs", "use crate::b::*;\nfn a() { helper(); }\n"),
            ("b.rs", "pub fn other() {}\n"),
        ],
        &[Edit::Put("b.rs", "pub fn other() {}\npub fn helper() {}\n")],
    );
}

/// Bench (run with `--ignored --nocapture`): the CR-015 win in isolation. Times
/// the resolve pass over the WHOLE ledger (`None` — what every sync re-bound
/// before) vs over only a one-file change-set (`Some(delta)`) on the same large
/// synthetic graph. `resolve::run` calls the binder on every selected row, so
/// the gap is the bind-attempt count: ~all-N vs |change-affected|. Not an
/// assertion (timings are machine-dependent); it prints the ratio so the
/// per-sync cost reduction is visible rather than merely argued.
#[test]
#[ignore = "perf bench — run explicitly with --ignored --nocapture"]
fn bench_incremental_vs_full_resolve() {
    use std::time::{Duration, Instant};

    // The real-graph cost driver is receiver-method calls to a COMMON name
    // (`x.fmt()`, `x.new()` — names shared by hundreds of nodes): each one scans
    // every same-named candidate through the policy fallback (`unique_by_name`).
    // Bare single-ident calls fail fast and DON'T reproduce it (they showed ~no
    // win). HUBS files each define `fn hot`, so by_name["hot"] is large; each
    // caller makes CALLS `x.hot()` receiver-calls that each scan all of them.
    const HUBS: usize = 200; // each defines hot0..hot{M-1}; by_name["hotJ"] == HUBS
    const CALLERS: usize = 400;
    const M: usize = 40; // distinct common method names (one ledger row each)

    let tmp = TempDir::new().expect("temp");
    for h in 0..HUBS {
        let mut s = String::new();
        for j in 0..M {
            s.push_str(&format!("pub fn hot{j}() {{}}\n"));
        }
        write(tmp.path(), &format!("hub{h}.rs"), &s);
    }
    for i in 0..CALLERS {
        let mut body = String::new();
        for j in 0..M {
            body.push_str(&format!("    x.hot{j}();\n")); // distinct -> distinct refs
        }
        write(
            tmp.path(),
            &format!("f{i}.rs"),
            &format!("pub fn func_{i}() {{\n{body}}}\n"),
        );
    }

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime");
    let policy = logos_core::config::BindingPolicy::Balanced;

    // Best of 3 to damp noise; returns (fastest run, ledger size).
    let best = |delta: Option<&logos_core::resolve::Delta>| -> (Duration, u64) {
        let mut fastest = Duration::MAX;
        let mut refs_total = 0u64;
        for _ in 0..3 {
            let t = Instant::now();
            let s = logos_core::resolve::run(rt, policy, delta).expect("resolve runs");
            fastest = fastest.min(t.elapsed());
            refs_total = s.refs_total;
        }
        (fastest, refs_total)
    };

    // Floor = read snapshot + build index + commit, with ZERO binds (empty delta):
    // the single-threaded per-sync cost incremental resolution does NOT remove.
    let (t_floor, refs_total) = best(Some(&logos_core::resolve::Delta::default()));
    // Full rebind binds the whole ledger — the all-core par_iter storm that was
    // the melt; each `x.hotJ` row scans all HUBS same-named candidates.
    let (t_full, _) = best(None);

    // A one-file body edit of caller f200.rs: its own M rows (A); it defines no
    // `hot`, so the other (CALLERS-1)*M rows stay clean and are skipped. Built the
    // way pipeline::sync would, by hand (tokens() is crate-private).
    let dirty: std::collections::HashSet<String> = ["func_200", "f200", "rs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let changed: std::collections::HashSet<String> =
        ["f200.rs"].iter().map(|s| s.to_string()).collect();
    let delta = logos_core::resolve::Delta {
        changed_paths: changed,
        dirty_tokens: dirty,
    };
    let (t_incr, _) = best(Some(&delta));

    let bind_full = t_full.saturating_sub(t_floor);
    let bind_incr = t_incr.saturating_sub(t_floor);
    eprintln!(
        "\nCR-015 resolve bench — {} files, ledger {refs_total} rows\n  \
         floor (read+build+commit, 0 binds):   {t_floor:?}\n  \
         full rebind total {t_full:?}  -> bind storm (all cores) {bind_full:?}\n  \
         incremental total {t_incr:?}  -> bind {bind_incr:?}\n  \
         bind-work speedup {:.0}x ; wall-clock speedup {:.1}x\n",
        HUBS + CALLERS,
        bind_full.as_secs_f64() / bind_incr.as_secs_f64().max(1e-9),
        t_full.as_secs_f64() / t_incr.as_secs_f64().max(1e-9),
    );
}

// ── CR-071 / S-279 / FR-IX-11: index + doctor surface the unindexed warning ───

/// End-to-end through the public `Engine`: a git-ignored documentation
/// directory-symlink with no sanctioned bypass (no `.swe-skills`) is surfaced as
/// a warning by BOTH `index` (via `IndexResult.warnings`) and `doctor` (via the
/// diagnostic-only `DoctorReport.doc_symlink_warnings`), naming the path and the
/// reason — while `doctor` stays `ok` (the unindexed symlink is not indexed, so it
/// is no admission drift). This pins the user-facing surface of the FR-IX-11
/// warning that the discovery-level unit tests do not exercise.
#[cfg(unix)]
#[test]
fn index_and_doctor_surface_the_unindexed_doc_symlink_warning() {
    use std::os::unix::fs::symlink;

    let base = TempDir::new().expect("temp root");
    let base = base.path().canonicalize().expect("canonicalize base");
    // A sibling docs tree the symlink points into (escapes the project root; with
    // no `.swe-skills` it resolves no sanctioned root and is refused).
    let external = base.join("external-docs");
    write(&external, "specs/ADR-46.md", "# ADR-46\n");

    let proj = base.join("project");
    write(&proj, "src/main.rs", "fn main() {}\n");
    fs::create_dir_all(proj.join("docs")).expect("mkdir docs");
    symlink(external.join("specs"), proj.join("docs/specs")).expect("symlink docs/specs");
    write(&proj, ".gitignore", "/docs/specs\n"); // git-ignored, and NO `.swe-skills`.

    let engine = Engine::start(&proj).expect("engine starts");
    let result = engine.index();

    // `index` surfaces the warning naming the dropped path and the reason.
    let index_warning = result
        .warnings
        .iter()
        .find(|w| w.contains("docs/specs") && w.contains("unindexed"));
    assert!(
        index_warning.is_some(),
        "index warns about the unindexed doc symlink: {:?}",
        result.warnings
    );
    assert!(
        index_warning.unwrap().contains("escapes"),
        "the index warning names the reason: {index_warning:?}"
    );

    // `doctor` surfaces the same warning and stays ok (no admission drift, since
    // the unindexed symlink was never indexed).
    let report = engine.doctor().expect("doctor runs");
    assert!(
        report.ok,
        "doctor stays ok — the diagnostic warning does not flip the verdict: {}",
        report.message
    );
    assert!(
        report
            .doc_symlink_warnings
            .iter()
            .any(|w| w.contains("docs/specs")),
        "doctor surfaces the unindexed-doc-symlink warning: {:?}",
        report.doc_symlink_warnings
    );
}
