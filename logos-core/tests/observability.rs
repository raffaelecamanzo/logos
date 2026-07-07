//! Black-box integration tests for the observability layer (S-019,
//! [observability], [ADR-13]).
//!
//! These drive the story's acceptance criteria through the public surface
//! exactly as the CLI/MCP adapters wire it — `observability::init` →
//! instrumented `Engine` calls → `telemetry.db` → `Engine::stats`:
//! - telemetry-tagged events persist to `.logos/telemetry.db`
//!   ([FR-OB-03](../../docs/specs/requirements/FR-OB-03.md),
//!   [UAT-OB-01](../../docs/specs/requirements/UAT-OB-01.md));
//! - the store **survives a full reindex** — deleting `logos.db` and
//!   re-indexing must not touch it
//!   ([NFR-OO-05](../../docs/specs/requirements/NFR-OO-05.md));
//! - `stats` reports usage counts, latency percentiles, and the tokens-saved
//!   estimate over what was recorded
//!   ([FR-OB-04](../../docs/specs/requirements/FR-OB-04.md),
//!   [NFR-OO-03](../../docs/specs/requirements/NFR-OO-03.md),
//!   [UAT-OB-02](../../docs/specs/requirements/UAT-OB-02.md)).
//!
//! Everything lives in **one** test function: `init` installs the *global*
//! subscriber, so a second parallel test in this binary would record its own
//! engine calls into the same telemetry store and perturb the counts.
//!
//! Gated on `lang-rust`: the index fixture needs the Rust grammar.
//!
//! [observability]: ../../docs/specs/architecture/components/observability.md
//! [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
#![cfg(feature = "lang-rust")]

use std::fs;

use tempfile::TempDir;

use logos_core::observability::{self, Surface};
use logos_core::Engine;

#[test]
fn telemetry_persists_survives_reindex_and_feeds_stats() {
    // ── stats degrades when telemetry never ran (no .logos at all) ──────────
    let bare = TempDir::new().expect("temp dir");
    let info = Engine::open(bare.path()).stats(None);
    assert_eq!(info.window_days, 7, "FR-OB-04 default window");
    assert_eq!(info.calls_total, 0);
    assert!(
        info.warnings.iter().any(|w| w.contains("no telemetry")),
        "missing telemetry.db is a warning, never an error: {:?}",
        info.warnings
    );

    // ── a real project: init telemetry, index, navigate ────────────────────
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    fs::create_dir_all(root.join(".logos")).expect("pre-create .logos");
    fs::write(
        root.join("lib.rs"),
        "pub fn alpha() { beta(); }\npub fn beta() {}\n",
    )
    .expect("write fixture");

    // The adapter wiring: install the global subscriber + telemetry writer.
    let guard = observability::init(Surface::Cli, root);

    {
        let engine = Engine::start(root).expect("engine starts");
        let indexed = engine.index();
        assert_eq!(indexed.files_indexed, 1, "the fixture file indexed");
        let found = engine.search("alpha", None, None);
        assert!(
            found.warnings.is_empty(),
            "search served: {:?}",
            found.warnings
        );
    }
    // Flush the last telemetry batch exactly as a process exit would.
    drop(guard);

    let telemetry_db = root.join(".logos").join("telemetry.db");
    assert!(telemetry_db.is_file(), "telemetry.db created (FR-OB-03)");

    // ── stats reports what was recorded (FR-OB-04, NFR-OO-03) ──────────────
    let stats = Engine::open(root).stats(None);
    assert!(stats.calls_total > 0, "usage recorded");
    let tools: Vec<&str> = stats
        .calls_by_tool
        .iter()
        .map(|u| u.tool.as_str())
        .collect();
    for expected in ["index", "extract", "resolve", "annotate", "search"] {
        assert!(
            tools.contains(&expected),
            "the {expected} instrumentation point recorded (got {tools:?})"
        );
    }
    assert!(
        stats.calls_by_tool.iter().all(|u| u.surface == "cli"),
        "every record is stamped with the installing surface"
    );
    assert!(
        stats.latency_p95_ms >= stats.latency_p50_ms,
        "percentiles are ordered"
    );
    assert!(
        stats.tokens_saved_estimate > 0,
        "the search call yields a non-zero tokens-saved estimate"
    );

    // ── the survival contract (NFR-OO-05): wipe logos.db, reindex ──────────
    let events_before = rusqlite_count(&telemetry_db);
    assert!(events_before > 0, "events persisted before the wipe");

    for db_file in ["logos.db", "logos.db-wal", "logos.db-shm"] {
        let path = root.join(".logos").join(db_file);
        if path.exists() {
            fs::remove_file(&path).expect("remove graph store file");
        }
    }
    {
        let engine = Engine::start(root).expect("engine restarts on a fresh logos.db");
        let reindexed = engine.index();
        assert_eq!(reindexed.files_indexed, 1, "full reindex rebuilt the graph");
    }

    assert!(
        telemetry_db.is_file(),
        "telemetry.db survives the logos.db wipe + reindex (NFR-OO-05)"
    );
    let events_after = rusqlite_count(&telemetry_db);
    assert!(
        events_after >= events_before,
        "no prior telemetry was lost ({events_before} before, {events_after} after)"
    );
}

/// Count rows in the events table of the telemetry store at `path`.
fn rusqlite_count(path: &std::path::Path) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("telemetry store opens read-only");
    conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .expect("events table readable")
}
