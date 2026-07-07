//! Black-box integration tests for config-change reconciliation on the write
//! path (S-040, [CR-004], [FR-SY-07], [ADR-20]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! façade exactly as the CLI/MCP surfaces do:
//! - narrowing the admission-relevant config (a code `exclude`, or disabling
//!   documentation) then running a reconciling command (`scan`) over an existing
//!   DB purges exactly the now-unadmitted nodes/edges
//!   ([FR-SY-07](../../docs/specs/requirements/FR-SY-07.md));
//! - the reconciled graph is byte-identical to a fresh index under the new
//!   config ([NFR-RA-06](../../docs/specs/requirements/NFR-RA-06.md));
//! - inbound cross-file edges into a purged file return to `unresolved_refs`,
//!   never fabricated ([NFR-RA-05](../../docs/specs/requirements/NFR-RA-05.md));
//! - documentation purge stays signal-neutral
//!   ([FR-DG-06](../../docs/specs/requirements/FR-DG-06.md));
//! - with config unchanged, no purge work runs and no nodes churn.
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these fixtures need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use logos_core::model::{EdgeKind, NodeKind};
use logos_core::{Engine, Runtime};

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// Write a `.logos/config.toml` carrying `body` (the `.logos/` dir is pruned by
/// discovery, so the config file itself is never indexed).
fn write_config(root: &Path, body: &str) {
    write(root, ".logos/config.toml", body);
}

/// The canonical, rowid-independent identity of the graph: the sorted set of
/// node tuples `(symbol, kind, name, file, start, end)` and edge tuples
/// `(source_symbol, target_symbol, kind)`. Two indexes that admit the same files
/// produce the same canonical graph even though their autoincrement rowids
/// differ — this is the "byte-identical graph" [NFR-RA-06] names.
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

// ── FR-SY-07: narrowing config purges now-unadmitted nodes on reconcile ───────

#[test]
fn narrowing_a_code_exclude_purges_now_unadmitted_nodes_on_reconcile() {
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    // Index under the default (wide) config: util.rs is admitted and the
    // cross-file call binds.
    let idx = engine.index();
    assert_eq!(idx.files_indexed, 2, "both source files indexed");
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed");
    assert_eq!(
        idx.resolution.refs_unresolved, 0,
        "the cross-file call binds — full coverage before the narrowing"
    );

    // Narrow the config: exclude util.rs, then run a reconciling command.
    write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");
    engine.scan(true).expect("scan reconciles");

    // util.rs's nodes/edges are gone…
    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the now-excluded file's nodes are purged (FR-SY-07)"
    );
    let calls_remain = rt
        .submit_read(|s| {
            Ok(s.all_edges()?
                .into_iter()
                .any(|e| e.kind == EdgeKind::Calls))
        })
        .expect("read runs");
    assert!(!calls_remain, "the cross-file Calls edge is purged with its target");

    // …and lib.rs's inbound reference to `crate::util::run` returns to the ledger
    // as unresolved — never fabricated (NFR-RA-05).
    assert!(
        has_unresolved_target(rt, "run"),
        "the inbound reference re-enters unresolved_refs after the purge"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller is untouched"
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn a_languages_allowlist_gates_code_admission_and_narrowing_purges_a_dropped_language() {
    // CR-017 / S-081 / FR-CF-01: the `languages` field gates which code grammars
    // are admitted. Omitted/empty = all compiled-in (the twelve-out-of-the-box
    // default); a non-empty list restricts to those, and dropping a grammar from a
    // broader effective set purges its now-unadmitted nodes on reconcile (FR-SY-07),
    // reusing the same purge+demote path as a code `exclude`.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "src/lib.rs", "pub fn alpha() {}\n");
    write(tmp.path(), "app.py", "def beta():\n    pass\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    // Default config (no `languages` key): both code files are admitted — an empty
    // allowlist means "every compiled-in code grammar".
    engine.index();
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "rust is admitted under the empty (all) default"
    );
    assert!(
        has_nodes_for_file(rt, "app.py"),
        "python is admitted under the empty (all) default — the field is no longer inert"
    );

    // Narrow to a rust-only allowlist, then reconcile: python is no longer admitted,
    // so its nodes are purged.
    write_config(tmp.path(), "languages = [\"rust\"]\n");
    engine.scan(true).expect("scan reconciles");
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "rust stays admitted by the allowlist"
    );
    assert!(
        !has_nodes_for_file(rt, "app.py"),
        "python is purged when dropped from the languages allowlist (FR-CF-01 + FR-SY-07)"
    );
}

#[test]
fn reconciled_graph_is_byte_identical_to_a_fresh_index_under_the_new_config() {
    // NFR-RA-06: reconcile-after-narrowing must converge to exactly what a
    // from-scratch index under the new config would build.
    let narrow = "exclude = [\"src/util.rs\"]\n";

    // Path A: index wide, then narrow + reconcile.
    let tmp_a = TempDir::new().expect("temp root A");
    write_cross_file_fixture(tmp_a.path());
    let engine_a = Engine::start(tmp_a.path()).expect("engine A starts");
    let rt_a = engine_a.runtime().expect("runtime A");
    engine_a.index();
    write_config(tmp_a.path(), narrow);
    engine_a.scan(true).expect("scan reconciles A");
    let graph_a = canonical_graph(rt_a);

    // Path B: fresh index under the narrow config from the start.
    let tmp_b = TempDir::new().expect("temp root B");
    write_cross_file_fixture(tmp_b.path());
    write_config(tmp_b.path(), narrow);
    let engine_b = Engine::start(tmp_b.path()).expect("engine B starts");
    let rt_b = engine_b.runtime().expect("runtime B");
    engine_b.index();
    let graph_b = canonical_graph(rt_b);

    assert_eq!(
        graph_a, graph_b,
        "the reconciled graph must be byte-identical to a fresh index under the new config"
    );
}

#[test]
fn re_index_over_an_existing_db_purges_now_unadmitted_nodes() {
    // FR-IX-01 (refined): index gains a symmetric purge — a re-index over a
    // populated DB shrinks to the fresh-index set when the config narrowed.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"));

    // Narrow, then re-index (not sync/reconcile) over the existing DB.
    write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");
    engine.index();

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "a re-index purges the now-excluded file's nodes (index symmetric purge)"
    );
    assert!(
        has_unresolved_target(rt, "run"),
        "the inbound reference re-enters unresolved_refs on the re-index too"
    );
}

#[test]
fn re_index_reconciles_a_file_deleted_from_disk_without_a_config_change() {
    // Issue 2.2-A: a full `index` reconciles the stored set to the working tree.
    // Previously the symmetric purge was gated on a config-fingerprint change, so a
    // file deleted from disk (no config edit) left ghost nodes in the graph until a
    // `sync`/`reconcile` — exactly how a removed git worktree's files survived a
    // "fresh" index. The purge now runs unconditionally, so the deleted file's
    // nodes are gone after a plain re-index.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "the file is indexed first");

    // Delete the file on disk (no config change), then re-index (not sync/reconcile).
    fs::remove_file(tmp.path().join("src/util.rs")).expect("remove file");
    engine.index();

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "a plain re-index purges nodes for a file deleted from disk (unconditional purge)"
    );
}

#[test]
fn unchanged_config_does_no_purge_and_no_node_churn() {
    // FR-SY-07: with the config unchanged, the reconciliation is a pure no-op —
    // no node is deleted and re-inserted (a re-extract would assign new rowids).
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    let before = canonical_graph(rt);
    let alpha_id = rt
        .submit_read(|s| {
            Ok(s.search("alpha", Some(NodeKind::Function), 8)?
                .into_iter()
                .find(|r| r.name == "alpha")
                .map(|r| r.id))
        })
        .expect("read runs")
        .expect("alpha exists");

    // Reconcile twice with no config change.
    engine.scan(true).expect("first scan");
    engine.scan(true).expect("second scan");

    let after = canonical_graph(rt);
    assert_eq!(before, after, "an unchanged config must not churn the graph");

    let alpha_id_after = rt
        .submit_read(|s| {
            Ok(s.search("alpha", Some(NodeKind::Function), 8)?
                .into_iter()
                .find(|r| r.name == "alpha")
                .map(|r| r.id))
        })
        .expect("read runs")
        .expect("alpha still exists");
    assert_eq!(
        alpha_id, alpha_id_after,
        "no rowid churn — the node was never re-extracted (no spurious re-write)"
    );
}

#[test]
fn the_purge_runs_at_most_once_per_config_change() {
    // FR-SY-07 / ADR-20: after a narrowing reconcile records the new fingerprint,
    // a second reconciling command under the *same* (narrowed) config must do no
    // further purge work and no node churn — the durable fingerprint disarms it.
    // This is the end-to-end counterpart of the kv-store unit test: it proves the
    // fingerprint is actually persisted across the reconcile boundary.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // Narrow + reconcile: the purge fires once.
    write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");
    engine.scan(true).expect("first (narrowing) scan");
    assert!(!has_nodes_for_file(rt, "src/util.rs"), "util purged on the first narrowing scan");

    // The surviving caller's identity, and the whole graph, after the purge.
    let graph_after_purge = canonical_graph(rt);
    let alpha_id = rt
        .submit_read(|s| {
            Ok(s.search("alpha", Some(NodeKind::Function), 8)?
                .into_iter()
                .find(|r| r.name == "alpha")
                .map(|r| r.id))
        })
        .expect("read runs")
        .expect("alpha exists");

    // A second scan under the unchanged (still-narrowed) config: fingerprint now
    // matches, so no purge re-runs and nothing churns.
    engine.scan(true).expect("second scan, config unchanged");

    assert_eq!(
        graph_after_purge,
        canonical_graph(rt),
        "a second scan under the same narrowed config must not churn the graph"
    );
    let alpha_id_after = rt
        .submit_read(|s| {
            Ok(s.search("alpha", Some(NodeKind::Function), 8)?
                .into_iter()
                .find(|r| r.name == "alpha")
                .map(|r| r.id))
        })
        .expect("read runs")
        .expect("alpha still exists");
    assert_eq!(
        alpha_id, alpha_id_after,
        "no rowid churn on the second scan — the purge ran at most once per config change"
    );
}

#[test]
fn direct_sync_purges_a_now_unadmitted_stored_file() {
    // FR-SY-07 on the bare sync path: a file the current config no longer admits
    // (here, the whole config-artifact layer is disabled) is routed to the
    // removal path on a direct sync, not skipped — best-effort coverage for the
    // watcher/hook seam. We model "no longer admitted" with the config-artifact
    // toggle so `admits_file` (which sees layer toggles) detects it directly.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "src/lib.rs", "pub fn alpha() {}\n");
    // A YAML artifact admitted by the default config-artifact layer.
    write(tmp.path(), "deploy.yaml", "name: demo\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // Only assert the purge behaviour if the artifact layer actually indexed the
    // YAML (depends on which artifact grammars this build embeds); otherwise the
    // toggle case is vacuous here and covered by the exclude-based tests above.
    if has_nodes_for_file(rt, "deploy.yaml") {
        write_config(tmp.path(), "[config_artifacts]\nenabled = false\n");
        let result = engine.sync(&[PathBuf::from("deploy.yaml")]);
        assert_eq!(
            result.files_removed, 1,
            "a stored, on-disk, now-unadmitted file is removed on a direct sync"
        );
        assert!(
            !has_nodes_for_file(rt, "deploy.yaml"),
            "its nodes are purged"
        );
    }
}

// ── FR-DG-06: documentation purge is signal-neutral ───────────────────────────

#[cfg(feature = "lang-markdown")]
#[test]
fn disabling_documentation_purges_doc_nodes_but_keeps_the_signal_byte_identical() {
    // FR-DG-06: documentation is metric-neutral, so purging the doc layer on a
    // config change must leave the aggregate quality signal byte-identical while
    // removing every doc node.
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "src/lib.rs", "pub fn alpha() -> i32 {\n    1\n}\n");
    write(
        tmp.path(),
        "README.md",
        "# Demo\n\nSome prose describing the project.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(
        has_nodes_for_file(rt, "README.md"),
        "the doc file is indexed under the default (docs-on) config"
    );
    let signal_with_docs = engine.scan(true).expect("scan with docs").signal;

    // Disable documentation, then reconcile.
    write_config(tmp.path(), "[documentation]\nenabled = false\n");
    let signal_without_docs = engine.scan(true).expect("scan after disabling docs").signal;

    assert!(
        !has_nodes_for_file(rt, "README.md"),
        "the doc nodes are purged when documentation is disabled"
    );
    assert_eq!(
        signal_with_docs, signal_without_docs,
        "purging documentation is signal-neutral (FR-DG-06)"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn direct_sync_removes_a_now_unadmitted_doc_file() {
    // FR-SY-07 on the bare `sync` path, deterministically: disabling
    // documentation makes a stored, on-disk `.md` no longer admitted, and a
    // direct sync of that path routes it to the removal path (not a skip) — the
    // self-limiting admission-gate branch in `sync`. Uses the doc toggle because
    // `admits_file` sees layer toggles directly (a code `exclude` would not flip
    // it — that narrowing is covered through `reconcile`/`index`).
    let tmp = TempDir::new().expect("temp root");
    write(tmp.path(), "src/lib.rs", "pub fn alpha() {}\n");
    write(tmp.path(), "README.md", "# Demo\n\nProse.\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "README.md"), "the doc file is indexed");

    write_config(tmp.path(), "[documentation]\nenabled = false\n");
    let result = engine.sync(&[PathBuf::from("README.md")]);
    assert_eq!(
        result.files_removed, 1,
        "a stored, on-disk, now-unadmitted doc is removed on a direct sync (not skipped)"
    );
    assert!(
        !has_nodes_for_file(rt, "README.md"),
        "its doc nodes are purged"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted code file is untouched"
    );
}
