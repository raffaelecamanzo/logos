//! Black-box integration tests for the core execution runtime (S-008,
//! [execution-runtime], [ADR-02]/[ADR-03]/[ADR-04]).
//!
//! Where `runtime::tests` drives the `Runtime` directly, these exercise the
//! story's acceptance criteria through the public `Engine` façade exactly as the
//! CLI/MCP surfaces will: start a long-lived engine, submit reads and writes
//! through its runtime, and assert serialization, rollback, and the cold-start
//! budget.
//!
//! [execution-runtime]: ../../docs/specs/architecture/components/execution-runtime.md
//! [ADR-02]: ../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-04]: ../../docs/specs/architecture/decisions/ADR-04.md

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tempfile::TempDir;

use logos_core::graph_store::{BatchWriter, NewNode};
use logos_core::model::{LogosSymbol, NodeKind};
use logos_core::Engine;

/// Insert one `function` node named `name` inside a write batch.
fn insert_function(w: &BatchWriter<'_>, symbol: &str, name: &str) -> Result<()> {
    let sym = LogosSymbol::parse(symbol)?;
    let symbol_id = w.upsert_symbol(&sym)?;
    w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, name))?;
    Ok(())
}

#[test]
fn start_brings_up_a_ready_long_lived_engine() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");

    // The canonical store was created under `.logos/`.
    let db = root.path().join(".logos").join("logos.db");
    assert!(db.exists(), "Engine::start creates .logos/logos.db");

    // The runtime is live and held for reuse across calls (ADR-04).
    assert!(
        engine.runtime().is_some(),
        "a started engine holds a runtime"
    );
    assert!(
        engine.runtime().unwrap().reader_pool_size() >= 1,
        "the read pool has at least one connection"
    );

    // The derived registry cache is built at startup and held — feature-agnostic
    // structural check (the Rust-grammar specifics are asserted separately under
    // `lang-rust`). With no grammar features the registry is an empty-but-present
    // cache, which is still the ADR-04 "built once, held" contract.
    assert!(
        engine.registry().is_some(),
        "a started engine caches the plugin registry as derived state (ADR-04)"
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn start_caches_the_plugin_registry_as_derived_state() {
    // ADR-04: the registry is built once at startup and held — a derived cache
    // reused across calls rather than rebuilt per operation.
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let registry = engine.registry().expect("registry cached at startup");
    assert!(
        registry.iter().any(|p| p.name() == "rust"),
        "the cached registry lists the compiled-in Rust grammar"
    );
}

#[test]
fn write_then_read_through_the_engine_runtime() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let runtime = engine.runtime().expect("runtime present");

    runtime
        .submit_write(|w| insert_function(w, "local e2e", "e2e_symbol"))
        .expect("write commits through the writer actor");

    let hits = runtime
        .submit_read(|store| Ok(store.search("e2e_symbol", None, 10)?.len()))
        .expect("read runs on the RO pool");
    assert_eq!(hits, 1, "the committed write is visible to the read pool");
}

#[test]
fn engine_survives_a_failed_write_batch_and_stays_consistent() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let runtime = engine.runtime().expect("runtime present");

    // A batch that errors after a partial write must roll back wholesale
    // (NFR-RA-07) and leave the engine usable.
    let failed: Result<()> = runtime.submit_write(|w| {
        insert_function(w, "local rollback", "rolled_back")?;
        Err(anyhow!("induced batch failure"))
    });
    assert!(failed.is_err());

    let leaked = runtime
        .submit_read(|store| Ok(store.search("rolled_back", None, 10)?.len()))
        .expect("read after rollback");
    assert_eq!(leaked, 0, "the rolled-back node never landed");

    // Engine remains consistent: the next write commits and reads back.
    runtime
        .submit_write(|w| insert_function(w, "local ok", "committed_ok"))
        .expect("the engine keeps serving writes after a rollback");
    let ok = runtime
        .submit_read(|store| Ok(store.search("committed_ok", None, 10)?.len()))
        .expect("read after recovery");
    assert_eq!(ok, 1);
}

#[test]
fn cold_start_to_ready_engine_is_within_pe05_budget() {
    // NFR-PE-05: cold start (build registry, open + migrate the store, bring up
    // the pools) completes in ≤ 500 ms before serving the first request. This
    // measures the *whole* ready-engine path through the public façade.
    //
    // The budget was revised 200 → 500 ms on 2026-06-14 to track the CR-009
    // grammar-set growth (5 → 12 compiled-in code languages): cold start scales
    // ≈ linearly with the compiled-in grammar count. See NFR-PE-05. The bound is
    // tolerance-banded via LOGOS_PERF_TOLERANCE so a loaded CI host can widen it
    // without editing the budget (a breach is re-run in isolation first).
    let root = TempDir::new().expect("temp root");

    let start = Instant::now();
    let engine = Engine::start(root.path()).expect("engine starts");
    let elapsed = start.elapsed();

    let budget = Duration::from_millis(500).mul_f64(perf_tolerance());
    assert!(engine.runtime().is_some(), "engine is ready to serve");
    assert!(
        elapsed < budget,
        "cold start to a ready Engine took {elapsed:?}, over the NFR-PE-05 ≤500ms budget \
         (tolerance-scaled to {budget:?})"
    );
}

/// Multiplier applied to every wall-clock budget so a loaded CI host can widen
/// the bands without editing the budget itself (S-024). `LOGOS_PERF_TOLERANCE`
/// defaults to `1.0`; a breach is re-run in isolation before being treated as a
/// regression.
fn perf_tolerance() -> f64 {
    std::env::var("LOGOS_PERF_TOLERANCE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v >= 1.0)
        .unwrap_or(1.0)
}
