//! Black-box integration tests for the fingerprint-gated navigation-prologue
//! purge (S-041, [CR-004], [FR-SY-08], [ADR-20]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! navigation surface (`search`/`node`) — the same seam the CLI/MCP tools read
//! through — exercising the one-time prologue that auto-indexes an empty graph
//! and now also runs the config-change purge before serving:
//! - after narrowing the admission-relevant config, a navigation read with no
//!   governance command in between surfaces no now-unadmitted symbols
//!   ([FR-SY-08](../../docs/specs/requirements/FR-SY-08.md));
//! - the purge runs at most once per config change — across a process restart
//!   the durable fingerprint disarms it, with no further purge work or node
//!   churn ([NFR-PE-01](../../docs/specs/requirements/NFR-PE-01.md));
//! - with the configuration unchanged, the prologue issues no write and no
//!   discovery walk (observed as zero node churn);
//! - a fresh (never-indexed) tree auto-indexes on the prologue and the purge is
//!   a no-op (the just-recorded fingerprint matches) — no double work.
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these fixtures need.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use logos_core::model::{NodeId, NodeKind};
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

/// The `lib.rs` → `util.rs` cross-file-call fixture: `alpha` calls
/// `crate::util::run`, which the binder resolves to `run` in `util.rs`.
fn write_cross_file_fixture(root: &Path) {
    write(
        root,
        "src/lib.rs",
        "pub fn alpha() {\n    crate::util::run();\n}\n",
    );
    write(root, "src/util.rs", "pub fn run() {}\n");
}

/// The canonical, rowid-independent identity of the graph: the sorted set of
/// node tuples `(symbol, kind, name, file, start, end)` and edge tuples
/// `(source_symbol, target_symbol, kind)`.
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

/// Does the graph hold any node whose defining file is `rel`? A raw store read —
/// deliberately does **not** go through the navigation prologue.
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

/// The stable rowid of the `alpha` function node — a raw store read (no
/// prologue), used to prove the absence of node churn (a re-extract would assign
/// a fresh rowid).
fn alpha_rowid(rt: &Runtime) -> NodeId {
    rt.submit_read(|s| {
        Ok(s.search("alpha", Some(NodeKind::Function), 8)?
            .into_iter()
            .find(|r| r.name == "alpha")
            .map(|r| r.id))
    })
    .expect("read runs")
    .expect("alpha exists")
}

// ── FR-SY-08: a navigation read after a config change purges on the prologue ───

#[test]
fn narrowing_config_then_a_navigation_read_purges_now_unadmitted_symbols() {
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    // Index under the default (wide) config: util.rs is admitted and indexed,
    // and the index records the admission fingerprint.
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed");

    // Narrow the config to exclude util.rs — but run NO governance command.
    write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");

    // The very first navigation read runs the one-time prologue, which detects
    // the config change and purges the now-unadmitted nodes *before* serving.
    let hits = engine.search("run", None, None);

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the now-excluded file's nodes are purged on the navigation prologue (FR-SY-08)"
    );
    assert!(
        hits.hits
            .iter()
            .all(|h| h.file.as_deref() != Some("src/util.rs") && h.name != "run"),
        "the navigation read surfaces no now-unadmitted symbol (FR-SY-08)"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller is untouched"
    );
}

#[test]
fn the_prologue_purge_runs_at_most_once_per_config_change() {
    // FR-SY-08 / NFR-PE-01: once the prologue purge records the new fingerprint,
    // a later process serving the same (narrowed) config must do no further purge
    // work and no node churn — the durable fingerprint disarms it. A process
    // restart is the meaningful boundary: within one process the prologue is a
    // one-shot, so this proves the fingerprint is actually persisted.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    // Process A: index wide, narrow, then a navigation read fires the purge once.
    {
        let engine_a = Engine::start(tmp.path()).expect("engine A starts");
        let rt_a = engine_a.runtime().expect("runtime A");
        engine_a.index();
        write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");
        let _ = engine_a.search("run", None, None);
        assert!(
            !has_nodes_for_file(rt_a, "src/util.rs"),
            "util.rs purged on the first (narrowing) prologue"
        );
    }

    // Process B: a fresh engine over the same DB and the same narrowed config.
    let engine_b = Engine::start(tmp.path()).expect("engine B starts");
    let rt_b = engine_b.runtime().expect("runtime B");

    // The post-purge graph, read raw (no prologue) before B's first navigation.
    let before = canonical_graph(rt_b);
    let alpha_before = alpha_rowid(rt_b);

    // B's first navigation read runs its prologue: the stored fingerprint now
    // matches the (unchanged) config, so the gate short-circuits — no purge, no
    // discovery walk, no write.
    let _ = engine_b.search("alpha", None, None);

    assert_eq!(
        before,
        canonical_graph(rt_b),
        "a second process under the same narrowed config must not churn the graph"
    );
    assert_eq!(
        alpha_before,
        alpha_rowid(rt_b),
        "no rowid churn — the purge ran at most once per config change (FR-SY-08, ADR-20)"
    );
}

#[test]
fn unchanged_config_prologue_does_no_write_and_no_node_churn() {
    // FR-SY-08: with the configuration unchanged, the prologue issues no write
    // and no discovery walk. The observable proxy is zero node churn: the gate
    // returns early (a single fingerprint comparison) before any discovery or
    // removal, so the node a re-extract would re-row keeps its rowid.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    let before = canonical_graph(rt);
    let alpha_before = alpha_rowid(rt);

    // The first navigation read runs the prologue under the unchanged config.
    let _ = engine.search("alpha", None, None);

    assert_eq!(
        before,
        canonical_graph(rt),
        "an unchanged config must not churn the graph on the prologue"
    );
    assert_eq!(
        alpha_before,
        alpha_rowid(rt),
        "no rowid churn — the prologue issued no write under the unchanged config"
    );
}

#[test]
fn a_fresh_index_prologue_does_not_double_purge() {
    // The prologue auto-indexes a never-indexed tree ([FR-IX-07]); the index
    // records the admission fingerprint, so the purge that follows in the same
    // prologue gates to a no-op — the freshly-built graph is served whole, no
    // file is wrongly removed, and no second index runs.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    // No explicit index: the first navigation read both auto-indexes and runs
    // the (no-op) config-change purge.
    let hits = engine.search("alpha", None, None);

    assert!(
        has_nodes_for_file(rt, "src/lib.rs") && has_nodes_for_file(rt, "src/util.rs"),
        "the prologue auto-indexed both files and the no-op purge removed nothing"
    );
    assert!(
        hits.hits.iter().any(|h| h.name == "alpha"),
        "the freshly-indexed graph is served on the same prologue read"
    );
}

#[test]
fn the_prologue_purge_fires_from_a_non_search_navigation_read() {
    // FR-SY-08 is a property of the shared one-time prologue (`nav_runtime`), not
    // of `search` specifically: every navigation entry point runs the same
    // prologue, so any of them must trigger the purge. Drive it through `node`
    // (which the spec header also names) to prove the purge is not search-specific
    // — a regression that routed `node` through `nav_runtime_no_prologue` would be
    // caught here.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed");

    write_config(tmp.path(), "exclude = [\"src/util.rs\"]\n");

    // A `node` read is the first navigation call on this engine → runs the
    // prologue, which purges before serving.
    let _ = engine.node("crate::util::run", false);

    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the prologue purge fires from a `node` read too — FR-SY-08 is nav_runtime-wide"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller is untouched"
    );
}
