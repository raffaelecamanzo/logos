//! Black-box integration tests for graph hydration through the [`Engine`]
//! façade ([S-009], [FR-DB-05], [FR-DB-06], [NFR-PE-07]).
//!
//! These drive the public surface the navigation/metrics services will use —
//! `Engine::start` → `hydrate` / `advance_sync_stamp` / `hydration_stats` — over
//! a real on-disk WAL store, so the cache-key plumbing (`(scope, last_sync_at)`),
//! the RO-pool read path, and the invalidation-on-advance contract are all
//! exercised exactly as production wires them.

use std::sync::Arc;

use logos_core::graph_store::{BatchWriter, NewNode};
use logos_core::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use logos_core::{Engine, Granularity, HydrationConfig};
use petgraph::algo::tarjan_scc;
use tempfile::TempDir;

/// Start a long-lived engine rooted at a fresh temp dir.
fn engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let engine = Engine::start(dir.path()).expect("engine starts");
    (engine, dir)
}

/// Seed a 3-cycle of functions (a→b→c→a) via the writer actor, returning their
/// ids in order.
fn seed_cycle(engine: &Engine) -> [NodeId; 3] {
    engine
        .runtime()
        .expect("started engine has a runtime")
        .submit_write(|w: &BatchWriter<'_>| {
            let mut ids = Vec::new();
            for name in ["a", "b", "c"] {
                let sym = LogosSymbol::parse(&format!("local cyc_{name}"))?;
                let symbol_id = w.upsert_symbol(&sym)?;
                let id = w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, name))?;
                ids.push(id);
            }
            w.insert_edge(ids[0], ids[1], EdgeKind::Calls)?;
            w.insert_edge(ids[1], ids[2], EdgeKind::Calls)?;
            w.insert_edge(ids[2], ids[0], EdgeKind::Calls)?;
            Ok([ids[0], ids[1], ids[2]])
        })
        .expect("seed write commits")
}

#[test]
fn engine_hydrates_and_caches_a_view() {
    let (engine, _dir) = engine();
    seed_cycle(&engine);

    let first = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("first hydrate");
    let second = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("second hydrate");

    assert!(
        Arc::ptr_eq(&first, &second),
        "a repeated aggregate run at the same last_sync_at hits the cache (NFR-PE-07)"
    );
    let stats = engine.hydration_stats();
    assert_eq!((stats.hits, stats.misses), (1, 1));
}

#[test]
fn advancing_the_sync_stamp_forces_rehydration() {
    let (engine, _dir) = engine();
    seed_cycle(&engine);

    let before = engine.hydrate(Granularity::ExcludeContains).unwrap();
    let stamp = engine.advance_sync_stamp();
    assert_eq!(stamp, engine.sync_stamp(), "advance returns the new stamp");
    let after = engine.hydrate(Granularity::ExcludeContains).unwrap();

    assert!(
        !Arc::ptr_eq(&before, &after),
        "advancing last_sync_at invalidates the cache; the next hydrate rebuilds"
    );
}

#[test]
fn tarjan_scc_over_a_hydrated_engine_view_finds_the_cycle() {
    let (engine, _dir) = engine();
    seed_cycle(&engine);

    let view = engine.hydrate(Granularity::ExcludeContains).unwrap();
    let sccs = tarjan_scc(view.graph());
    assert_eq!(
        sccs.len(),
        1,
        "the whole graph is one strongly-connected cycle"
    );
    assert_eq!(sccs[0].len(), 3, "a, b, c form a single SCC");
}

#[test]
fn all_four_granularities_hydrate_through_the_engine() {
    let (engine, _dir) = engine();
    seed_cycle(&engine);

    for g in [
        Granularity::ExcludeContains,
        Granularity::Symbol,
        Granularity::File,
        Granularity::Module,
    ] {
        let view = engine.hydrate(g).expect("granularity hydrates on demand");
        assert!(view.node_count() > 0, "{g:?} produced a non-empty view");
    }
    assert_eq!(
        engine.hydration_stats().entries,
        4,
        "the four per-granularity views cache independently (FR-DB-06)"
    );
}

#[test]
fn start_with_hydration_config_wires_a_tight_bound_through_the_engine() {
    // The AQ-02/AA-04 seam: a custom HydrationConfig flows into the Engine's
    // cache. A 1-entry bound means hydrating a second granularity evicts the
    // first, observable through hydration_stats.
    let dir = TempDir::new().expect("temp dir");
    let engine = Engine::start_with_hydration_config(
        dir.path(),
        HydrationConfig {
            max_entries: Some(1),
            max_bytes: None,
        },
    )
    .expect("engine starts with a custom hydration config");
    seed_cycle(&engine);

    engine.hydrate(Granularity::ExcludeContains).unwrap();
    engine.hydrate(Granularity::Symbol).unwrap();
    assert_eq!(
        engine.hydration_stats().entries,
        1,
        "the tight entry bound configured at start is honoured through the Engine"
    );
}

/// Cross-story integration test (S-009 × S-010): `Engine::index` must advance
/// the sync stamp so that a hydrate() call following an index reflects the newly
/// persisted graph rather than a stale cached view ([ADR-04], [ADR-05]).
///
/// This test would have failed before the coherence fix: without the
/// `advance_sync_stamp()` call in `run_index`, the second hydrate() would return
/// the empty Arc cached at stamp 0 and report node_count() == 0.
#[cfg(feature = "lang-rust")]
#[test]
fn index_advances_the_sync_stamp_so_hydration_reflects_new_graph_data() {
    use std::fs;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let src = dir.path().join("solo.rs");
    fs::write(&src, "fn solo() {}\n").expect("write fixture");

    let engine = Engine::start(dir.path()).expect("engine starts");

    // Hydrate before indexing: the graph is empty, so the view has no vertices.
    let stamp_before = engine.sync_stamp();
    let empty_view = engine.hydrate(Granularity::ExcludeContains).unwrap();
    assert_eq!(
        empty_view.node_count(),
        0,
        "pre-index view is empty (graph not yet populated)"
    );

    // Index: populates the graph.
    let idx = engine.index();
    assert!(idx.nodes_created > 0, "index produced nodes");

    // The stamp must have advanced — the cache is now invalid.
    assert!(
        engine.sync_stamp() > stamp_before,
        "index() must advance the sync stamp so the cache invalidates (ADR-04)"
    );

    // A hydrate() after the index must reflect the new data.
    let populated_view = engine.hydrate(Granularity::ExcludeContains).unwrap();
    assert!(
        populated_view.node_count() > 0,
        "post-index hydration must reflect the indexed nodes; got 0 — the sync stamp \
         was not advanced after index(), so the stale empty view was returned from cache"
    );
    assert!(
        !std::sync::Arc::ptr_eq(&empty_view, &populated_view),
        "post-index view must be a different Arc than the pre-index one (cache miss)"
    );
}

#[test]
fn a_transient_engine_cannot_hydrate() {
    let dir = TempDir::new().expect("temp dir");
    // Engine::open is the store-free transient path — no RO pool to hydrate from.
    let engine = Engine::open(dir.path());
    let err = engine
        .hydrate(Granularity::ExcludeContains)
        .expect_err("a transient engine has no runtime");
    assert!(
        err.to_string().contains("long-lived engine"),
        "the error explains that hydration needs Engine::start: {err}"
    );
}
