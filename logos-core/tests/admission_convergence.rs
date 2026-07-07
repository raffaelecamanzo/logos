//! Black-box integration tests for admission unification on the incremental path
//! (S-214, [CR-054], [FR-SY-11], [FR-RC-06], [NFR-RA-06], [ADR-48]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! façade exactly as the watcher/CLI/hook surfaces do. The unit-level parity of
//! the `AdmissionAuthority` predicate itself is proven in
//! `logos_core::config::admission`; here we prove the *consumers* converge:
//! - a **partial** `sync` (the `Engine::sync` spelling the watcher/hook/CLI use)
//!   of a gitignored or nested-`.git`-boundary path creates no nodes for it
//!   ([FR-SY-11](../../docs/specs/requirements/FR-SY-11.md));
//! - a still-on-disk file that becomes gitignored is purged on the next
//!   full-walk reconcile (`scan`), with inbound cross-file edges returned to
//!   `unresolved_refs` ([FR-RC-06](../../docs/specs/requirements/FR-RC-06.md));
//! - on a tree with a gitignored subdir and a nested worktree, an
//!   index → incremental `sync`/`scan` loop converges byte-identically to a
//!   fresh `index` and `verify` reports `ok`
//!   ([NFR-RA-06](../../docs/specs/requirements/NFR-RA-06.md)).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these fixtures need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use logos_core::{Engine, Runtime};

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// The absolute path of `<root>/<rel>` — the shape the watcher/hook hand to
/// `Engine::sync` (OS events carry absolute paths).
fn abs(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

/// The canonical, rowid-independent identity of the graph: the sorted set of
/// node tuples `(symbol, kind, name, file, start, end)` and edge tuples
/// `(source_symbol, target_symbol, kind)`. Two indexes that admit the same files
/// produce the same canonical graph even though their autoincrement rowids
/// differ — the "byte-identical graph" [NFR-RA-06] names.
type CanonicalGraph = (
    Vec<(String, i32, String, Option<String>, Option<i64>, Option<i64>)>,
    Vec<(String, String, i32)>,
);

fn canonical_graph(rt: &Runtime) -> CanonicalGraph {
    rt.submit_read(|store| {
        let nodes = store.all_nodes()?;
        let edges = store.all_edges()?;
        let symbol_of: std::collections::HashMap<_, _> = nodes
            .iter()
            .map(|n| (n.id, n.symbol.as_str().to_string()))
            .collect();

        let mut node_rows: Vec<_> = nodes
            .iter()
            .map(|n| {
                (
                    n.symbol.as_str().to_string(),
                    n.kind.as_i32(),
                    n.name.clone(),
                    n.file_path.clone(),
                    n.start_line,
                    n.end_line,
                )
            })
            .collect();
        node_rows.sort();

        let mut edge_rows: Vec<_> = edges
            .iter()
            .map(|e| {
                (
                    symbol_of[&e.source].clone(),
                    symbol_of[&e.target].clone(),
                    e.kind.as_i32(),
                )
            })
            .collect();
        edge_rows.sort();

        Ok((node_rows, edge_rows))
    })
    .expect("canonical-graph read runs")
}

/// Does the graph hold any node whose defining file is `rel`?
fn has_nodes_for_file(rt: &Runtime, rel: &str) -> bool {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .iter()
            .any(|n| n.file_path.as_deref() == Some(rel.as_str())))
    })
    .expect("read runs")
}

/// Is there an unresolved (`resolved = false`) ledger row whose target text
/// contains `needle`? The honest "inbound reference returned to the ledger"
/// signal ([NFR-RA-05]).
fn has_unresolved_target(rt: &Runtime, needle: &str) -> bool {
    let needle = needle.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .unresolved_refs()?
            .iter()
            .any(|r| !r.resolved && r.target.contains(needle.as_str())))
    })
    .expect("read runs")
}

/// The `lib.rs` → `util.rs` cross-file-call fixture: `alpha` calls
/// `crate::util::run`, which the binder resolves to `run` in `util.rs`. Indexing
/// it yields a bound `Calls` edge and a fully-resolved ledger.
fn write_cross_file_fixture(root: &Path) {
    write(
        root,
        "src/lib.rs",
        "pub fn alpha() {\n    crate::util::run();\n}\n",
    );
    write(root, "src/util.rs", "pub fn run() {}\n");
}

// ── FR-SY-11: a partial sync never admits a gitignored / boundary path ────────

#[test]
fn partial_sync_of_a_gitignored_or_boundary_path_creates_no_nodes() {
    // The watcher/hook/CLI spelling is `Engine::sync` (a `SyncScope::Partial`
    // batch). Before CR-054 it applied only `admits_file` — extension/doc/config —
    // so a `.rs` file under a gitignored dir or a nested-`.git` boundary was
    // (re-)extracted, the exact leak that inflated the live `serve --ui` graph.
    let tmp = TempDir::new().expect("temp root");
    // Canonicalize so the absolute paths handed to `sync` share the prefix the
    // pipeline relativizes against (macOS `/var` → `/private/var`); otherwise a
    // path would be skipped as "outside the root" before the admission gate runs.
    let root = &tmp.path().canonicalize().expect("canonicalize root");

    // A committed source file (the control), a root `.gitignore` pruning a whole
    // dir and a single file, and a nested worktree (`nested_repo/.git` gitlink)
    // whose name is NOT an `ignored_dirs` entry — so ONLY the boundary rule can
    // exclude it, isolating the boundary gate from the name prune.
    write(root, "src/main.rs", "pub fn main_fn() {}\n");
    write(root, ".gitignore", "generated/\nsecret.rs\n");
    write(root, "generated/derived.rs", "pub fn derived() {}\n");
    write(root, "secret.rs", "pub fn secret() {}\n");
    write(root, "nested_repo/.git", "gitdir: /elsewhere\n");
    write(root, "nested_repo/copy.rs", "pub fn copied() {}\n");

    let engine = Engine::start(root).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // The index (full walk) already excluded the scratch — a precondition, not the
    // thing under test.
    assert!(has_nodes_for_file(rt, "src/main.rs"), "the control source is indexed");
    assert!(!has_nodes_for_file(rt, "generated/derived.rs"), "index excludes the gitignored dir");
    assert!(!has_nodes_for_file(rt, "secret.rs"), "index excludes the gitignored file");
    assert!(!has_nodes_for_file(rt, "nested_repo/copy.rs"), "index excludes the boundary path");

    // Now drive a PARTIAL sync naming exactly the unadmitted paths (plus a fresh
    // admitted file as a positive control) — the watcher's behaviour when those
    // files change. The unadmitted ones must create no nodes; the control must.
    write(root, "src/added.rs", "pub fn added() {}\n");
    engine.sync(&[
        abs(root, "generated/derived.rs"),
        abs(root, "secret.rs"),
        abs(root, "nested_repo/copy.rs"),
        abs(root, "src/added.rs"),
    ]);

    assert!(
        has_nodes_for_file(rt, "src/added.rs"),
        "a partial sync still indexes an admitted new file (the gate is not over-broad)"
    );
    assert!(
        !has_nodes_for_file(rt, "generated/derived.rs"),
        "a partial sync of a path under a gitignored dir creates no nodes (FR-SY-11)"
    );
    assert!(
        !has_nodes_for_file(rt, "secret.rs"),
        "a partial sync of a gitignored file creates no nodes (FR-SY-11)"
    );
    assert!(
        !has_nodes_for_file(rt, "nested_repo/copy.rs"),
        "a partial sync of a nested-`.git`-boundary path creates no nodes (FR-SY-11)"
    );
}

// ── FR-RC-06: full-walk reconcile purges a now-gitignored on-disk file ────────

#[test]
fn full_walk_reconcile_purges_a_now_gitignored_on_disk_file_and_rebinds_edges() {
    // The CR-054 drift the CR-052 sweep could not catch: a file that is STILL ON
    // DISK but has become gitignored. The old sweep escaped any on-disk file
    // (`|| …is_file()`), and a `.gitignore` edit does not move the config
    // fingerprint, so the fingerprint-gated purge never fired either — the file's
    // nodes leaked until a full reindex. Dropping the `is_file()` escape converges
    // the stored set to the freshly-discovered admitted candidate set.
    let tmp = TempDir::new().expect("temp root");
    let root = &tmp.path().canonicalize().expect("canonicalize root");
    write_cross_file_fixture(root);

    let engine = Engine::start(root).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed before gitignoring");
    assert!(
        !has_unresolved_target(rt, "run"),
        "the cross-file call is resolved before the purge"
    );

    // Gitignore the callee — but leave it ON DISK (no delete, no config change).
    write(root, ".gitignore", "src/util.rs\n");
    engine.scan(true).expect("scan reconciles (full walk)");

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "a full-walk reconcile purges the now-gitignored still-on-disk file (FR-RC-06)"
    );
    assert!(
        has_unresolved_target(rt, "run"),
        "the inbound cross-file edge returns to unresolved_refs, never fabricated (NFR-RA-05)"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller stays indexed"
    );
}

// ── FR-SY-11: a partial sync of a *deletion* still reconciles it out ──────────

#[test]
fn partial_sync_of_a_deleted_source_file_purges_it_and_rebinds_edges() {
    // The admission gate must never swallow a deletion: the `!abs.is_file()`
    // removal arm precedes the gate, so a deleted-and-then-synced admitted file is
    // reconciled out on the PARTIAL path (the watcher/hook spelling), without
    // waiting for a full reconcile. This is the integration counterpart of the
    // watcher `classify` existence guard, and guards the "a deletion must still be
    // reconcilable" half of the deletion-past-admission contract.
    let tmp = TempDir::new().expect("temp root");
    let root = &tmp.path().canonicalize().expect("canonicalize root");
    write_cross_file_fixture(root);

    let engine = Engine::start(root).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs indexed before deletion");
    assert!(!has_unresolved_target(rt, "run"), "the cross-file call is resolved before deletion");

    // Delete the callee from disk, then drive a PARTIAL sync naming exactly it.
    fs::remove_file(root.join("src/util.rs")).expect("remove util.rs");
    engine.sync(&[abs(root, "src/util.rs")]);

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "a partial sync of a deleted admitted file purges its nodes (not swallowed by the gate)"
    );
    assert!(
        has_unresolved_target(rt, "run"),
        "the inbound cross-file edge returns to unresolved_refs on the partial path too (NFR-RA-05)"
    );
    assert!(has_nodes_for_file(rt, "src/lib.rs"), "the caller stays indexed");
}

// ── NFR-RA-06: incremental sync converges byte-identically with a fresh index ─

#[test]
fn incremental_sync_converges_byte_identically_with_a_fresh_index() {
    // The CR-054 anchor: on a tree with a gitignored subdir AND a nested worktree,
    // an index → incremental `sync`/`scan` loop must end byte-identical to a fresh
    // `index`, and `verify` (shadow reindex + census diff) must report `ok`.

    // The final tree both paths converge to: an admitted library, a gitignored
    // subdir, a nested worktree (`.worktrees/wt`, a boundary AND a default-ignored
    // name post-S-213), and a nested repo under a non-ignored name (boundary only).
    fn build_tree(root: &Path) {
        write(root, "src/lib.rs", "pub fn alpha() {\n    crate::util::run();\n}\n");
        write(root, "src/util.rs", "pub fn run() {}\n");
        write(root, ".gitignore", "generated/\n");
        write(root, "generated/derived.rs", "pub fn derived() {}\n");
        write(root, ".worktrees/wt/.git", "gitdir: /elsewhere\n");
        write(root, ".worktrees/wt/copy.rs", "pub fn worktree_copy() {}\n");
        write(root, "nested_repo/.git", "gitdir: /elsewhere\n");
        write(root, "nested_repo/copy.rs", "pub fn nested_copy() {}\n");
    }

    // Path A — incremental: index an initial subset, then create the rest
    // (admitted + scratch) and drive a partial `sync` over it, then a full-walk
    // `scan` reconcile. The scratch must never enter the graph.
    let tmp_a = TempDir::new().expect("temp root A");
    let root_a = &tmp_a.path().canonicalize().expect("canonicalize root A");
    write(root_a, "src/lib.rs", "pub fn alpha() {\n    crate::util::run();\n}\n");
    write(root_a, "src/util.rs", "pub fn run() {}\n");
    let engine_a = Engine::start(root_a).expect("engine A starts");
    let rt_a = engine_a.runtime().expect("runtime A");
    engine_a.index();

    // The rest of the tree appears (as it would during live editing) and the
    // watcher-style partial sync names every new path, scratch included.
    write(root_a, ".gitignore", "generated/\n");
    write(root_a, "generated/derived.rs", "pub fn derived() {}\n");
    write(root_a, ".worktrees/wt/.git", "gitdir: /elsewhere\n");
    write(root_a, ".worktrees/wt/copy.rs", "pub fn worktree_copy() {}\n");
    write(root_a, "nested_repo/.git", "gitdir: /elsewhere\n");
    write(root_a, "nested_repo/copy.rs", "pub fn nested_copy() {}\n");
    engine_a.sync(&[
        abs(root_a, "generated/derived.rs"),
        abs(root_a, ".worktrees/wt/copy.rs"),
        abs(root_a, "nested_repo/copy.rs"),
    ]);
    // The full-walk reconcile is the load-bearing convergence step.
    engine_a.scan(true).expect("scan reconciles A");
    let graph_a = canonical_graph(rt_a);

    // Path B — fresh index of the identical final tree.
    let tmp_b = TempDir::new().expect("temp root B");
    let root_b = &tmp_b.path().canonicalize().expect("canonicalize root B");
    build_tree(root_b);
    let engine_b = Engine::start(root_b).expect("engine B starts");
    let rt_b = engine_b.runtime().expect("runtime B");
    engine_b.index();
    let graph_b = canonical_graph(rt_b);

    // No scratch leaked into the incremental graph.
    assert!(has_nodes_for_file(rt_a, "src/lib.rs"), "admitted source present");
    assert!(has_nodes_for_file(rt_a, "src/util.rs"), "admitted source present");
    assert!(!has_nodes_for_file(rt_a, "generated/derived.rs"), "gitignored dir absent");
    assert!(!has_nodes_for_file(rt_a, ".worktrees/wt/copy.rs"), "worktree scratch absent");
    assert!(!has_nodes_for_file(rt_a, "nested_repo/copy.rs"), "nested-repo boundary absent");

    // Byte-identical population, and `verify` (shadow reindex) agrees.
    assert_eq!(
        graph_a, graph_b,
        "incremental sync/scan must converge byte-identically to a fresh index (NFR-RA-06)"
    );
    let report = engine_a.verify().expect("verify runs");
    assert!(
        report.ok,
        "verify must report ok — the live graph matches a fresh reindex: {report:?}"
    );
}
