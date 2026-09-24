//! Unit tests for the observability seam: store, layer, sink, writer, stats.
//!
//! The cross-cutting contracts (telemetry survives a reindex of `logos.db`,
//! stdout stays clean through a real binary run) live in
//! `logos-core/tests/observability.rs` and `cli/tests/stdout_safety.rs`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use tracing_subscriber::layer::SubscriberExt;

use super::layer::{spawn_writer, TelemetryLayer, TelemetrySink};
use super::stats::{reads_saved_per_call, stats_from};
use super::tool::{
    class_of_wire, compose_predicate, engine_query_predicate, not_in, self_referential_tools,
    EventClass, Tool, ToolClass, UNREGISTERED_CLASS,
};
use super::{
    db, generate_session_id, in_surface, self_referential_surfaces, telemetry_logos_dir,
    telemetry_origin, traced, EventRecord, Surface, TELEMETRY_TARGET,
};

/// An event record `secs_ago` seconds before the fixed "now" used in tests.
/// Defaults `origin` to `"main"` — the primary-checkout increment; the
/// `origin`-specific tests construct records explicitly.
fn record(tool: &str, duration_ms: u64, ok: bool, at: i64) -> EventRecord {
    EventRecord {
        at,
        surface: "cli",
        tool: tool.to_string(),
        duration_ms,
        ok,
        origin: "main".to_string(),
        session_id: "test-session".to_string(),
    }
}

const NOW: i64 = 1_780_000_000; // an arbitrary fixed unix-seconds clock

// ── Store (db.rs) ──────────────────────────────────────────────────────────

/// The migrated schema holds both documented tables (FR-OB-03) and re-opening
/// (re-migrating) an existing store is a no-op, not an error.
#[test]
fn telemetry_schema_migrates_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telemetry.db");
    {
        let conn = db::open(&path).expect("first open migrates");
        for table in ["events", "daily_rollup", "schema_versions"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table} exists after migration");
        }
    }
    // Second open re-runs the ledger against user_version — applies nothing.
    let conn = db::open(&path).expect("re-open is idempotent");
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, 3, "each ledger migration recorded exactly once");
    // The v2 `origin` column is present on the events table.
    let has_origin: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('events') WHERE name = 'origin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_origin, 1, "v2 added the nullable origin column");
    // The v3 `session_id` column is present on the events table.
    let has_session_id: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('events') WHERE name = 'session_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_session_id, 1, "v3 added the nullable session_id column");
}

/// A batch lands atomically with every field intact.
#[test]
fn write_batch_persists_event_fields() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[
            record("search", 12, true, NOW),
            record("index", 900, false, NOW),
        ],
    )
    .expect("batch commits");

    let (tool, duration_ms, ok, surface, origin, session_id): (
        String,
        i64,
        i64,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT tool, duration_ms, ok, surface, origin, session_id FROM events WHERE tool = 'index'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(
        (
            tool.as_str(),
            duration_ms,
            ok,
            surface.as_str(),
            origin.as_str(),
            session_id.as_str(),
        ),
        ("index", 900, 0, "cli", "main", "test-session")
    );
    let n: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

/// `write_batch` persists the per-event `origin` verbatim (FR-OB-08) — a
/// worktree-branch stamp survives the round-trip alongside `surface`.
#[test]
fn write_batch_persists_the_origin_stamp() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[EventRecord {
            at: NOW,
            surface: "mcp",
            tool: "context".to_string(),
            duration_ms: 5,
            ok: true,
            origin: "sprint-40-I2-S1".to_string(),
            session_id: "test-session".to_string(),
        }],
    )
    .expect("batch commits");

    let origin: String = conn
        .query_row("SELECT origin FROM events WHERE tool = 'context'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(origin, "sprint-40-I2-S1", "the branch origin is stored as-is");
}

/// The v2 forward migration applies cleanly over a seeded v1 store: the
/// pre-`origin` row survives and reads back as `'main'` via
/// `COALESCE(origin,'main')`, while a post-migration write carries its stamp
/// (FR-OB-08). This is the migration-ledger discipline the story requires.
#[test]
fn v2_migration_over_a_v1_store_reads_legacy_rows_as_main() {
    let mut conn = db::open_in_memory_v1();
    // A legacy row written under the v1 schema (events has no `origin` column).
    conn.execute(
        "INSERT INTO events (at, surface, tool, duration_ms, ok)
         VALUES (?1, 'cli', 'search', 12, 1)",
        [NOW],
    )
    .expect("legacy insert");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1, "the seeded store is at v1");

    // Apply the forward ledger — the exact production path `db::open` runs.
    // `migrate` applies every pending migration, so a v1 seed now lands on the
    // current head (v3) in one call, not just v2.
    db::migrate(&mut conn).expect("the forward ledger applies over a v1 store");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3, "the store advanced to the ledger head");

    // The legacy row is intact and its NULL origin reads as 'main'.
    let (legacy_origin, coalesced): (Option<String>, String) = conn
        .query_row(
            "SELECT origin, COALESCE(origin, 'main') FROM events WHERE tool = 'search'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy_origin, None, "the legacy row's origin is NULL");
    assert_eq!(coalesced, "main", "a legacy NULL origin reads as main");

    // A post-migration write stamps its origin normally.
    db::write_batch(
        &mut conn,
        &[EventRecord {
            at: NOW,
            surface: "cli",
            tool: "impact".to_string(),
            duration_ms: 3,
            ok: true,
            origin: "feature".to_string(),
            session_id: "test-session".to_string(),
        }],
    )
    .expect("post-migration write");
    let new_origin: String = conn
        .query_row("SELECT origin FROM events WHERE tool = 'impact'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(new_origin, "feature");
}

/// `write_batch` persists the per-event `session_id` verbatim (FR-OB-12) —
/// an opaque per-process stamp survives the round-trip alongside `origin` and
/// `surface`, each independently.
#[test]
fn write_batch_persists_the_session_id_stamp() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[EventRecord {
            at: NOW,
            surface: "mcp",
            tool: "context".to_string(),
            duration_ms: 5,
            ok: true,
            origin: "sprint-40-I2-S1".to_string(),
            session_id: "a1b2c3d4e5f60718".to_string(),
        }],
    )
    .expect("batch commits");

    let session_id: String = conn
        .query_row(
            "SELECT session_id FROM events WHERE tool = 'context'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        session_id, "a1b2c3d4e5f60718",
        "the opaque per-process stamp is stored as-is"
    );
}

/// The v3 forward migration applies cleanly over a seeded **v2** store: the
/// pre-`session_id` row survives and reads back as `'unattributed'` — never
/// folded into a real session, unlike `origin`'s `'main'` fallback — while a
/// post-migration write carries its own stamp (FR-OB-12). This is the
/// migration-ledger discipline the story requires, over the exact predecessor
/// version the story names.
#[test]
fn v3_migration_over_a_v2_store_reads_legacy_rows_as_unattributed() {
    let mut conn = db::open_in_memory_v2();
    // A legacy row written under the v2 schema (events has `origin` but no
    // `session_id` column), already carrying a real origin stamp.
    conn.execute(
        "INSERT INTO events (at, surface, tool, duration_ms, ok, origin)
         VALUES (?1, 'cli', 'search', 12, 1, 'main')",
        [NOW],
    )
    .expect("legacy v2 insert");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2, "the seeded store is at v2");

    // Apply the forward ledger — the exact production path `db::open` runs.
    db::migrate(&mut conn).expect("v3 migration applies over a v2 store");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3, "the store advanced to v3");

    // The legacy row is intact, its NULL session_id is never coalesced into a
    // real session, and `COALESCE(session_id, 'unattributed')` is how a reader
    // is expected to name that: a sentinel no real session id can collide with.
    let (legacy_session, coalesced, origin): (Option<String>, String, String) = conn
        .query_row(
            "SELECT session_id, COALESCE(session_id, 'unattributed'), origin
             FROM events WHERE tool = 'search'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(legacy_session, None, "the legacy row's session_id is NULL");
    assert_eq!(
        coalesced, "unattributed",
        "a legacy NULL session_id reads as unattributed, never a real session"
    );
    assert_eq!(
        origin, "main",
        "the pre-existing origin stamp is untouched by the v3 migration"
    );

    // A post-migration write stamps its session_id normally.
    db::write_batch(
        &mut conn,
        &[EventRecord {
            at: NOW,
            surface: "cli",
            tool: "impact".to_string(),
            duration_ms: 3,
            ok: true,
            origin: "main".to_string(),
            session_id: "freshly-generated".to_string(),
        }],
    )
    .expect("post-migration write");
    let new_session_id: String = conn
        .query_row(
            "SELECT session_id FROM events WHERE tool = 'impact'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_session_id, "freshly-generated");
}

/// Events older than the retention window fold into `daily_rollup` and are
/// deleted; recent events stay raw (NFR-OO-04). A second prune never
/// double-counts (the raw rows it read are gone).
#[test]
fn rollup_and_prune_bounds_raw_retention() {
    let mut conn = db::open_in_memory();
    let old = NOW - 100 * 86_400; // beyond the 90-day window
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, old),
            record("search", 30, true, old),
            record("context", 50, true, NOW - 60), // recent: stays raw
        ],
    )
    .unwrap();

    db::rollup_and_prune(&mut conn, NOW, db::RETENTION_DAYS).expect("rollup commits");

    let raw: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw, 1, "only the recent event survives raw");
    let (calls, total_ms): (i64, i64) = conn
        .query_row(
            "SELECT calls, total_duration_ms FROM daily_rollup WHERE tool = 'search'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((calls, total_ms), (2, 40), "aged events aggregated");

    // Idempotence: pruning again with no aged raws changes nothing.
    db::rollup_and_prune(&mut conn, NOW, db::RETENTION_DAYS).unwrap();
    let calls_after: i64 = conn
        .query_row(
            "SELECT calls FROM daily_rollup WHERE tool = 'search'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(calls_after, 2, "a re-prune never double-counts");
}

// ── Sink (layer.rs): the never-blocks contract (NFR-OO-02) ────────────────

/// With a stalled (never-draining) consumer, recording past the queue bound
/// returns immediately — events are dropped, the caller is never blocked.
/// This is the structural guarantee behind "a telemetry write can never
/// block a query or fail a command".
#[test]
fn a_full_queue_drops_instead_of_blocking() {
    let (sink, _rx) = TelemetrySink::with_capacity(4); // _rx held but never drained
    let start = Instant::now();
    for i in 0..10_000 {
        sink.record(record("search", i, true, NOW));
    }
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "10k records against a stalled writer must return ~instantly, took {:?}",
        start.elapsed()
    );
}

// ── Layer + emission helper: the single emission point (NFR-OO-01) ────────

/// `traced` emits one telemetry-tagged event the layer turns into a full
/// record (tool, duration, ok, surface); non-telemetry events are ignored.
#[test]
fn traced_emits_one_record_through_the_layer() {
    let (sink, rx) = TelemetrySink::with_capacity(16);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "feature".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        let ok = traced(Tool::Callers, || Ok(42)).unwrap();
        assert_eq!(ok, 42, "traced is transparent to the wrapped result");
        let err = traced(Tool::Impact, || Err::<(), _>(anyhow::anyhow!("boom")));
        assert!(err.is_err(), "traced propagates the error untouched");
        // A human-log event without the telemetry target must not record.
        tracing::warn!(tool = "not-telemetry", "plain log line");
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2, "exactly one record per traced call");
    assert_eq!(records[0].tool, "callers");
    assert!(records[0].ok);
    assert_eq!(records[0].surface, "mcp");
    assert_eq!(
        records[0].origin, "feature",
        "the per-process origin stamp rides every record (FR-OB-08)"
    );
    assert_eq!(records[1].tool, "impact");
    assert!(!records[1].ok, "a failed call records ok = false");
    assert_eq!(records[1].origin, "feature");
}

/// The workspace-tier config seam (S-450, [ADR-67]) books each call exactly as
/// the façade method it stands in for: one `config_read` / `config_write` /
/// `config_write_secret` record per call, `ok` tracking the result — so a
/// workspace-root save is as visible to `logos stats` as a member's, with no
/// engine constructed to emit it.
///
/// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
#[test]
fn the_workspace_tier_seam_emits_the_facade_config_events() {
    let dir = tempfile::tempdir().expect("temp workspace root");
    let root = dir.path();
    let (sink, rx) = TelemetrySink::with_capacity(16);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "feature".to_string(), "test-session".to_string(), sink));

    // The load fingerprint of the absent tier: the empty document's (S-451 T2).
    let absent = crate::federation::manifest::fingerprint(b"");
    tracing::subscriber::with_default(subscriber, || {
        crate::config::write_workspace_config(root, "[chat]\nmodel = \"ws/m\"\n", &absent).expect("writes");
        crate::config::write_workspace_secret(root, "sk-seam-key-sm01").expect("writes");
        crate::config::read_workspace_documents(root).expect("reads");
        assert!(crate::config::write_workspace_config(root, "modle = 1\n", &absent).is_err());
    });

    let records: Vec<(String, bool)> = rx.try_iter().map(|r| (r.tool, r.ok)).collect();
    assert_eq!(
        records,
        [
            ("config_write".to_string(), true),
            ("config_write_secret".to_string(), true),
            ("config_read".to_string(), true),
            ("config_write".to_string(), false),
        ],
        "one façade-named record per workspace-tier call"
    );
    assert!(!root.join(".logos/logos.db").exists(), "and no graph store behind them");
}

/// The S-022 watcher attribution: a telemetry event carrying the sanctioned
/// `surface = "watcher"` override records under that surface (the watcher
/// runs *inside* the `serve --mcp` process, whose default is `mcp`), while
/// an unsanctioned override value is ignored.
#[test]
fn watcher_surface_override_is_honoured_and_bounded() {
    let (sink, rx) = TelemetrySink::with_capacity(4);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "feature".to_string(), "test-session".to_string(), sink));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            target: TELEMETRY_TARGET,
            tool = "watch_sync",
            surface = "watcher",
            duration_ms = 12u64,
            ok = true,
            "watcher sync"
        );
        tracing::info!(
            target: TELEMETRY_TARGET,
            tool = "search",
            surface = "made-up",
            duration_ms = 3u64,
            ok = true,
            "bogus override"
        );
    });
    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].surface, "watcher", "sanctioned override applies");
    assert_eq!(records[0].tool, "watch_sync");
    assert_eq!(records[1].surface, "mcp", "unknown override falls back");
    // origin is orthogonal to the surface override (FR-OB-08): the watcher
    // event still carries the process-wide increment stamp.
    assert_eq!(records[0].origin, "feature", "surface override leaves origin");
    assert_eq!(records[1].origin, "feature");
}

/// An event on the telemetry target missing the helper's full field shape is
/// dropped, never half-recorded.
#[test]
fn a_malformed_telemetry_event_is_dropped() {
    let (sink, rx) = TelemetrySink::with_capacity(4);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Cli, "main".to_string(), "test-session".to_string(), sink));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: TELEMETRY_TARGET, tool = "search", "no duration, no ok");
    });
    assert_eq!(rx.try_iter().count(), 0, "incomplete events are dropped");
}

// ── Writer thread: async/batched persistence + flush-on-drop ──────────────

/// Records queued through the real writer land in `telemetry.db`; dropping
/// the guard flushes the final batch before the process would exit.
#[test]
fn writer_persists_and_guard_flushes_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telemetry.db");
    let (sink, guard) = spawn_writer(path.clone());
    for i in 0..50 {
        sink.record(record("context", i, true, NOW));
    }
    drop(guard); // shutdown → drain → final batch commit → join

    let conn = db::open(&path).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 50, "every queued record was flushed by the guard");
}

/// A writer whose store cannot open (unwritable path) exits silently; the
/// guard still drops without hanging and the caller never sees a failure —
/// best-effort end to end (NFR-CC-03).
#[test]
fn an_unopenable_store_degrades_silently() {
    let path = std::path::PathBuf::from("/nonexistent-root/.logos/telemetry.db");
    let (sink, guard) = spawn_writer(path);
    sink.record(record("search", 1, true, NOW)); // dropped on the floor
    drop(guard); // must not hang or panic
}

// ── Stats (stats.rs): FR-OB-04 / NFR-OO-03 ────────────────────────────────

/// Usage counts, percentiles, and the saved estimates over seeded events —
/// including rollup rows the window reaches back into.
#[test]
fn stats_reports_usage_percentiles_and_saved_estimates() {
    let mut conn = db::open_in_memory();
    // 100 search calls with durations 1..=100 ms, one of them failed.
    let batch: Vec<EventRecord> = (1..=100)
        .map(|i| record("search", i, i != 7, NOW - 60))
        .collect();
    db::write_batch(&mut conn, &batch).unwrap();
    // 2 context calls inside the window.
    db::write_batch(
        &mut conn,
        &[
            record("context", 40, true, NOW - 120),
            record("context", 60, true, NOW - 120),
        ],
    )
    .unwrap();
    // A rollup day inside the 7-day window (raw events already aged out).
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'mcp', 'impact', 3, 3, 90, 50)",
        [NOW - 86_400],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    assert_eq!(info.window_days, 7);
    assert_eq!(info.calls_total, 105, "raw (102) + rollup (3) calls");
    let search = info
        .calls_by_tool
        .iter()
        .find(|u| u.tool == "search")
        .expect("search usage listed");
    assert_eq!((search.calls, search.ok_calls), (100, 99));
    let impact = info
        .calls_by_tool
        .iter()
        .find(|u| u.tool == "impact")
        .expect("rollup usage merged in");
    assert_eq!((impact.surface.as_str(), impact.calls), ("mcp", 3));

    // Nearest-rank percentiles over the 102 raw durations.
    assert!(info.latency_p50_ms >= 50 && info.latency_p50_ms <= 52);
    assert!(info.latency_p95_ms >= 95 && info.latency_p95_ms <= 97);
    assert!(info.latency_p99_ms >= 99 && info.latency_p99_ms <= 100);

    // Saved estimates (OQ-01 ratified weights): 100 search × 2 + 2 context × 5
    // + 3 impact × 3 = 219 reads; × 1500 tokens each.
    assert_eq!(info.reads_saved_estimate, 219);
    assert_eq!(info.tokens_saved_estimate, 219 * 1_500);
    assert!(info.warnings.is_empty());
}

// ── Per-event classification (FR-OB-09) + the chat surface (FR-OB-10) ──────

/// Every registered tool carries a classification, and every wire name is a
/// bare snake_case identifier.
///
/// The second half is what lets [`tool::engine_query_predicate`] interpolate
/// the excluded names into SQL rather than bind them: the values are compile-
/// time literals from a closed enum, and this pins them to a shape that cannot
/// carry a quote. If a future wire name breaks that shape, this fails before the
/// interpolation can.
#[test]
fn every_registered_tool_is_classified_and_sql_safe() {
    assert!(!Tool::ALL.is_empty(), "the registry is populated");

    let mut names: Vec<&str> = Tool::ALL.iter().map(|t| t.as_str()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "wire names are unique");

    for tool in Tool::ALL {
        let name = tool.as_str();
        assert!(!name.is_empty(), "{tool:?} has a wire name");
        let mut chars = name.chars();
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_lowercase()),
            "{name} starts with a lowercase ascii letter"
        );
        assert!(
            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name} is [a-z0-9_] throughout — SQL-safe to interpolate"
        );
        // The classification is total by construction (an exhaustive match), so
        // what this asserts is that calling it is infallible for every variant.
        let _: EventClass = tool.event_class();
    }

    // The two AC-named exclusions are present, and the set stays deliberately
    // small — the exclusion corrects an overstatement, it must not create an
    // understatement (NFR-CC-04).
    let excluded = self_referential_tools();
    assert!(excluded.contains(&"stats"), "got {excluded:?}");
    assert!(excluded.contains(&"status"), "got {excluded:?}");
    assert_eq!(
        excluded,
        vec!["stats", "status"],
        "the set is exactly the two the acceptance criteria name — the exclusion \
         corrects an overstatement and must not create an understatement \
         (NFR-CC-04); widening it is a specification decision, not a code one"
    );
}

/// The predicate's **string** — the one thing every stats query depends on and
/// nothing asserted.
///
/// Six queries interpolate this fragment, and a review showed the gap was real
/// rather than theoretical: the `"1 = 1"` fallback could be replaced with
/// `"THIS IS NOT SQL"` and all 2241 tests in the crate still passed. The
/// composition is covered by behaviour elsewhere (swapping `AND` for `OR` reds
/// nine tests), but the shape itself — which axes appear, how they join, and
/// what an empty axis does — was only ever asserted by reading it.
///
/// The fallback branches matter precisely because they are unreachable today:
/// both axes are non-empty, so the only thing standing between a future empty
/// classification and invalid SQL (`NOT IN ()`, or a dangling `WHERE … AND`) is
/// code nothing exercises.
#[test]
fn the_engine_query_predicate_composes_both_axes() {
    let predicate = engine_query_predicate();
    assert_eq!(
        predicate, "tool NOT IN ('stats', 'status') AND surface NOT IN ('shell')",
        "the shipped shape: both axes, AND-joined, in registration order"
    );
    // No parentheses of its own — the callers compose it with their own `AND`
    // (`WHERE at >= ?1 AND {engine_query}`), which is only safe while this
    // fragment is `AND` throughout. The doc says so; this pins it.
    assert!(
        !predicate.contains(" OR ") && !predicate.starts_with('('),
        "no OR, and no grouping parens wrapping the fragment — the parens it \
         does carry are `NOT IN (…)`'s own: {predicate}"
    );

    // Each axis on its own, and the empty case, through the helper the
    // predicate is built from — the branches the live classification cannot
    // reach.
    assert_eq!(
        not_in("tool", vec!["stats", "status"]).as_deref(),
        Some("tool NOT IN ('stats', 'status')")
    );
    assert_eq!(
        not_in("surface", vec!["shell"]).as_deref(),
        Some("surface NOT IN ('shell')")
    );
    assert_eq!(
        not_in("surface", vec![]),
        None,
        "an axis that excludes nothing contributes no clause — never `NOT IN ()`"
    );

    // And the both-empty degenerate case — reachable only through the composer,
    // since the live classification never yields zero clauses — stays valid SQL
    // in the caller's `AND` composition rather than truncating the `WHERE`.
    let degenerate = compose_predicate(Vec::new());
    assert_eq!(degenerate, "1 = 1", "an always-true predicate, not an empty string");
    let conn = db::open_in_memory();
    for fragment in [&degenerate, &predicate] {
        conn.query_row(
            &format!("SELECT count(*) FROM events WHERE at >= 0 AND {fragment}"),
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or_else(|e| panic!("`{fragment}` composes into valid SQL: {e}"));
    }
}

/// **An unclassified tool must fail the build**, on *both* classification axes:
/// `Tool::event_class` ([FR-OB-09]) and `Tool::tool_class` ([FR-OB-11], whose
/// criteria repeat the requirement verbatim — "an unclassified tool fails the
/// build rather than defaulting silently"). Each is an exhaustive `match` with
/// no wildcard arm, so the compiler refuses to build until a newly-registered
/// tool is classified twice.
///
/// A single `_ =>` arm silently destroys that while still compiling, restoring
/// exactly the closed-list defect CR-091 was filed about. Nothing in rustc
/// guards against *adding* a wildcard, so this scans the source and does — for
/// both matches, because a guard that named only the first would let the second
/// acquire the very defect this test exists to prevent.
///
/// # The guard is set equality, not a count
///
/// An earlier form asserted `body.matches("Tool::").count() >= Tool::ALL.len()`
/// and rejected the two literal spellings `_ =>` / `_=>`. Both halves were
/// evadable, and by the same developer action:
///
/// - a **named** catch-all (`other => ToolClass::EngineInternal`) is not a `_`,
///   so the literal scan missed it, and
/// - the `>=` count could be restored to the new floor by a *comment* naming the
///   unclassified variant — a mention is not a classification.
///
/// So the body is stripped of comments and the set of variants **named in code**
/// is compared for equality with the registry. A variant that reaches a fallback
/// arm is then simply absent from the set, whatever the fallback is spelled, and
/// prose cannot put it back. The wildcard scan is kept and widened to any
/// lone-`_` or lone-lowercase-identifier arm pattern, so the failure names the
/// cause rather than only the symptom.
#[test]
fn unclassified_tool_fails_the_build() {
    /// Everything from `//` to end-of-line, removed. Neither body contains a
    /// string literal, so there is no `//`-inside-a-string case to mishandle.
    fn strip_comments(body: &str) -> String {
        body.lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every `Tool::<Variant>` named in `code`, as a set.
    fn variants_named_in(code: &str) -> std::collections::BTreeSet<String> {
        code.match_indices("Tool::")
            .map(|(at, marker)| {
                code[at + marker.len()..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .filter(|ident| !ident.is_empty())
            .collect()
    }

    let source = include_str!("tool.rs");
    let registry: std::collections::BTreeSet<String> =
        Tool::ALL.iter().map(|t| format!("{t:?}")).collect();
    let matches: [(&str, &str); 2] = [
        (
            "Tool::event_class",
            "pub(crate) const fn event_class(self) -> EventClass {",
        ),
        (
            "Tool::tool_class",
            "pub(crate) const fn tool_class(self) -> ToolClass {",
        ),
    ];

    for (name, signature) in matches {
        let body = source
            .split_once(signature)
            .unwrap_or_else(|| panic!("{name} is where this test believes it is"))
            .1;
        // The match arms close at 8-space indent, so `"\n    }\n"` is the
        // function's own closing brace and nothing inside it. If either function
        // ever moves to module level this split would run past the end — hence
        // the set equality below, which fails loudly instead of drifting.
        let body = body
            .split_once("\n    }\n")
            .unwrap_or_else(|| panic!("{name} is brace-delimited"))
            .0;
        let code = strip_comments(body);

        // No fallback arm, however it is spelled: a lone `_` or a lone lowercase
        // binding on the left of `=>` matches every remaining variant.
        for line in code.lines() {
            let Some((pattern, _)) = line.split_once("=>") else {
                continue;
            };
            let pattern = pattern.trim().trim_start_matches('|').trim();
            let is_catch_all = pattern == "_"
                || (!pattern.is_empty()
                    && pattern.starts_with(|c: char| c.is_ascii_lowercase())
                    && pattern.chars().all(|c| c.is_alphanumeric() || c == '_'));
            assert!(
                !is_catch_all,
                "`{pattern} =>` in {name} is a catch-all arm; it defeats the \
                 build-time completeness guarantee (FR-OB-09, FR-OB-11) by giving \
                 a new tool a silent default instead of failing to compile"
            );
        }

        // Every registered variant is named in an arm, and nothing else is. A
        // variant left to a fallback is absent from this set; a retired variant
        // still named here is a leftover. Comments are stripped, so prose about
        // a variant cannot stand in for classifying it.
        assert_eq!(
            variants_named_in(&code),
            registry,
            "{name} must name exactly the {} registered tools",
            registry.len()
        );
    }
}

/// **An unclassified surface must fail the build** ([BR-42]) — the second half
/// of the rule `unclassified_tool_fails_the_build` pins for tools.
///
/// The requirement is written about *reads*, not about tools: "adding such a
/// read must register its classification; an unclassified read is a build
/// failure, never a silent inclusion". Since [CR-097] an adapter registers by
/// naming a [`Surface`], so `Surface::event_class` is now a place a wildcard
/// would reinstate the silent default — and a wildcard there is *worse* than
/// one in `Tool::event_class`, because `None` ("the tool decides") is the arm a
/// careless fallback would pick, quietly putting a new adapter's chrome back
/// into the figures.
///
/// Same construction as the tool guard, and deliberately so: comments stripped,
/// set equality against the vocabulary, plus a scan for a fallback arm however
/// it is spelled. See that test for why each half is there.
///
/// [BR-42]: ../../../docs/specs/software-spec.md#316-observability--telemetry
/// [CR-097]: ../../../docs/requests/CR-097-header-graph-state-readout.md
#[test]
fn unclassified_surface_fails_the_build() {
    fn strip_comments(body: &str) -> String {
        body.lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn variants_named_in(code: &str) -> std::collections::BTreeSet<String> {
        code.match_indices("Surface::")
            .map(|(at, marker)| {
                code[at + marker.len()..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .filter(|ident| !ident.is_empty())
            .collect()
    }

    /// The variants the `Surface` enum *declares*, read from its own source.
    ///
    /// `Surface::ALL` would be the obvious roster and is the wrong one here:
    /// [`Surface::Chat`] is `#[cfg(feature = "agents")]`, so under a bare
    /// `cargo test -p logos-core` it is absent from `ALL` while still being
    /// named — correctly — in the match. Comparing source against source makes
    /// the guard feature-independent, and widens it: a variant declared but
    /// left to a fallback fails here under *either* feature configuration.
    fn variants_declared_in(source: &str) -> std::collections::BTreeSet<String> {
        let body = source
            .split_once("pub enum Surface {")
            .expect("the Surface enum is where this test believes it is")
            .1
            .split_once("\n}\n")
            .expect("the Surface enum is brace-delimited")
            .0;
        strip_comments(body)
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_suffix(','))
            .filter(|ident| {
                ident.starts_with(|c: char| c.is_ascii_uppercase())
                    && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
            })
            .map(str::to_string)
            .collect()
    }

    /// The variants declared *without* a `#[cfg(…)]` gate, so they are present in
    /// every feature configuration and must therefore be in `Surface::ALL`.
    fn unconditional_variants_in(source: &str) -> std::collections::BTreeSet<String> {
        let body = source
            .split_once("pub enum Surface {")
            .expect("the Surface enum is where this test believes it is")
            .1
            .split_once("\n}\n")
            .expect("the Surface enum is brace-delimited")
            .0;
        let mut out = std::collections::BTreeSet::new();
        let mut gated = false;
        for line in strip_comments(body).lines().map(str::trim) {
            if line.is_empty() {
                continue;
            }
            if line.starts_with("#[cfg(") {
                gated = true;
                continue;
            }
            if let Some(ident) = line.strip_suffix(',') {
                if ident.starts_with(|c: char| c.is_ascii_uppercase())
                    && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                {
                    if !gated {
                        out.insert(ident.to_string());
                    }
                    gated = false;
                }
            }
        }
        out
    }

    let source = include_str!("mod.rs");
    let vocabulary = variants_declared_in(source);
    assert!(
        vocabulary.contains("Shell"),
        "the CR-097 shell-chrome surface is declared: {vocabulary:?}"
    );

    // `Surface::ALL` is hand-maintained, and until now nothing tied it to the
    // declaration. The exhaustive matches below are enforced by rustc, so a new
    // variant must be classified — but `ALL` is a plain slice literal, and
    // `self_referential_surfaces()` and `from_wire()` are both derived from it.
    //
    // A variant declared AND classified self-referential, but simply left out of
    // `ALL`, therefore ships with every guard in this module green: it is absent
    // from the `surface NOT IN (…)` exclusion, so every event it emits is counted
    // as real usage — the exact misattribution BR-42 forbids and S-316 exists to
    // close — and `from_wire` refuses its wire name, so a per-event override
    // naming it falls back to the process surface without a word. Proven by
    // adding one: 62 observability tests stayed green.
    let rostered: std::collections::BTreeSet<String> =
        Surface::ALL.iter().map(|s| format!("{s:?}")).collect();
    let unconditional = unconditional_variants_in(source);
    assert!(
        unconditional.is_subset(&rostered),
        "every ungated Surface variant must be in Surface::ALL, or it is silently \
         absent from the self-referential exclusion and from `from_wire` \
         (FR-OB-03, FR-OB-09, BR-42). Declared but unrostered: {:?}",
        &unconditional - &rostered
    );
    assert!(
        rostered.is_subset(&vocabulary),
        "Surface::ALL names only declared variants; stale entries: {:?}",
        &rostered - &vocabulary
    );
    let signature = "pub(crate) const fn event_class(self) -> Option<EventClass> {";
    let body = source
        .split_once(signature)
        .expect("Surface::event_class is where this test believes it is")
        .1;
    let body = body
        .split_once("\n    }\n")
        .expect("Surface::event_class is brace-delimited")
        .0;
    let code = strip_comments(body);

    for line in code.lines() {
        let Some((pattern, _)) = line.split_once("=>") else {
            continue;
        };
        let pattern = pattern.trim().trim_start_matches('|').trim();
        let is_catch_all = pattern == "_"
            || (!pattern.is_empty()
                && pattern.starts_with(|c: char| c.is_ascii_lowercase())
                && pattern.chars().all(|c| c.is_alphanumeric() || c == '_'));
        assert!(
            !is_catch_all,
            "`{pattern} =>` in Surface::event_class is a catch-all arm; it defeats \
             the build-time completeness guarantee (FR-OB-09, BR-42) by giving a \
             new surface a silent default instead of failing to compile"
        );
    }

    assert_eq!(
        variants_named_in(&code),
        vocabulary,
        "Surface::event_class must name exactly the {} declared surfaces",
        vocabulary.len()
    );
}

/// Every registered tool carries one of the **five** [FR-OB-11] classes, the
/// labels are exactly that vocabulary, and none of the five is dead.
///
/// Totality is the compiler's job (an exhaustive match); what this adds is that
/// the *wire* vocabulary the read-model publishes is the closed set the
/// requirement names — a sixth label would reach the Statistics tab and a
/// dogfood table as a silently new category — and that every class is actually
/// reachable, which catches a class that exists only in the enum.
#[test]
fn every_registered_tool_carries_one_of_the_five_classes() {
    const VOCABULARY: [&str; 5] = [
        "navigation",
        "quality-gate",
        "session-gate",
        "engine-internal",
        "read-model",
    ];

    let mut seen: Vec<&str> = Vec::new();
    for tool in Tool::ALL {
        let label = tool.tool_class().as_str();
        assert!(
            VOCABULARY.contains(&label),
            "{} is classified {label:?}, outside the FR-OB-11 vocabulary {VOCABULARY:?}",
            tool.as_str()
        );
        // The read-model does not call `tool_class()` — it calls `class_of_wire`
        // on a name read back out of the store, which round-trips through
        // `Tool::from_wire`. Assert the wire path agrees for *every* registered
        // tool, not just the handful the fixtures happen to emit: a name that
        // failed to resolve would be labelled `unregistered` while the tool is
        // very much registered.
        assert_eq!(
            class_of_wire(tool.as_str()),
            label,
            "the wire path disagrees with the enum path for {}",
            tool.as_str()
        );
        if !seen.contains(&label) {
            seen.push(label);
        }
    }
    for class in VOCABULARY {
        assert!(
            seen.contains(&class),
            "no registered tool is classified {class:?} — a dead class in the \
             read-model's vocabulary"
        );
    }
    assert!(
        !VOCABULARY.contains(&UNREGISTERED_CLASS),
        "the fallback label for a retired tool name must stay outside the five \
         classes, or a live tool could fall into it"
    );
}

/// The two classifications are independent axes, with one invariant binding
/// them: **everything excluded as self-referential is a read-model call.**
///
/// The converse deliberately does not hold — `languages` is a `read-model` class
/// yet an `EngineQuery` event (S-304 reclassified it after the exclusion set was
/// shown to be an understatement). Asserting the direction that *is* required
/// stops the two matches drifting into contradiction without forcing them to
/// answer the same question.
#[test]
fn the_two_classifications_agree_where_they_must() {
    for tool in Tool::ALL {
        if matches!(tool.event_class(), EventClass::ReadModelRequest) {
            assert_eq!(
                tool.tool_class(),
                ToolClass::ReadModel,
                "{} is excluded as self-referential but not classed read-model",
                tool.as_str()
            );
        }
    }
    // …and the converse is genuinely false, so the test above is not vacuously
    // asserting an identity between the two axes.
    assert_eq!(Tool::Languages.tool_class(), ToolClass::ReadModel);
    assert_eq!(Tool::Languages.event_class(), EventClass::EngineQuery);
}

/// Every tool the reads-saved weight table pays for is classified `navigation`.
///
/// [FR-OB-11] promotes the distinction that lived *only* in that table onto the
/// read-model. The two are not the same list — `implements` is navigation and
/// carries no ratified weight — so the binding is directional: a non-zero weight
/// implies the navigation class. Without this, the weights (a `&str` match with
/// a wildcard, by necessity — it also sees retired names) could pay a tool the
/// class calls engine-internal, and the dogfood table would contradict the
/// headline estimate computed from the same window.
#[test]
fn every_weighted_tool_is_classified_navigation() {
    let mut weighted = 0;
    for tool in Tool::ALL {
        if reads_saved_per_call(tool.as_str()) > 0 {
            weighted += 1;
            assert_eq!(
                tool.tool_class(),
                ToolClass::Navigation,
                "{} carries a reads-saved weight but is classified {:?}",
                tool.as_str(),
                tool.tool_class()
            );
        }
    }
    assert!(weighted >= 7, "the weight table is populated ({weighted} tools)");
}

/// The [FR-OB-09] headline: a graph query issued through the SPA **appears** in
/// the usage figures; a Statistics-tab render does not; and a CLI `logos stats`
/// invocation does not either — the exclusion is per event and applies on every
/// surface, not to a surface.
///
/// The old blanket `surface <> 'web'` filter got two of these three wrong: it
/// discarded the SPA's navigation and counted the CLI's self-measurement.
#[test]
fn spa_navigation_counts_while_self_referential_reads_do_not() {
    let mut conn = db::open_in_memory();
    let event = |surface: &'static str, tool: &str| EventRecord {
        at: NOW - 60,
        surface,
        tool: tool.to_string(),
        duration_ms: 10,
        ok: true,
        origin: "main".to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            // A graph query the user issued through the SPA.
            event("web", "search"),
            // The Statistics tab rendering itself.
            event("web", "stats"),
            // The app shell's header graph-state readout (FR-UI-34) — issued by
            // navigation, asked for by nobody.
            event("web", "status"),
            // `logos stats` from the CLI: no less self-referential.
            event("cli", "stats"),
            // A genuine CLI navigation call.
            event("cli", "callers"),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    let counted: Vec<(&str, &str)> = info
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    assert!(
        counted.contains(&("web", "search")),
        "SPA navigation is real tool use: {counted:?}"
    );
    assert!(
        counted.contains(&("cli", "callers")),
        "CLI navigation still counts: {counted:?}"
    );
    assert!(
        !counted.iter().any(|(_, tool)| *tool == "stats"),
        "no stats read counts, on any surface: {counted:?}"
    );
    assert!(
        !counted.iter().any(|(_, tool)| *tool == "status"),
        "the shell's status readout counts nowhere: {counted:?}"
    );
    assert_eq!(info.calls_total, 2, "exactly the two navigation calls");
}

/// The same exclusion reaches `daily_rollup`, whose rows carry `tool` too — so a
/// window long enough to touch rolled-up days applies one rule, not two.
#[test]
fn the_self_referential_exclusion_reaches_rollup_rows() {
    let conn = db::open_in_memory();
    for (surface, tool, calls) in [("web", "stats", 40), ("web", "impact", 6)] {
        conn.execute(
            "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                       total_duration_ms, max_duration_ms)
             VALUES (date(?1, 'unixepoch'), ?2, ?3, ?4, ?4, 100, 50)",
            rusqlite::params![NOW - 86_400, surface, tool, calls],
        )
        .unwrap();
    }

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    assert_eq!(info.calls_total, 6, "the rolled-up stats reads are excluded");
    assert_eq!(info.calls_by_tool.len(), 1);
    assert_eq!(info.calls_by_tool[0].tool, "impact");
    let day_calls: u64 = info.activity_by_day.iter().map(|d| d.calls).sum();
    assert_eq!(day_calls, 6, "and the daily series applies the same rule");
}
/// [FR-OB-09] widened by [CR-097]: the app shell's own chrome is a
/// **surface**, and every event it carries is self-referential by construction.
///
/// The tool arm alone cannot express this. The header's readout happens to call
/// `status`, which is already excluded as a tool — but that exclusion is
/// incidental to *which engine method the handler picked*, and it says nothing
/// about the next chrome read. So the fixture seeds a chrome-issued `search`:
/// a wire name the tool arm **counts**, excluded here only because the shell
/// declared the surface at its adapter boundary ([BR-42]). Flip the surface arm
/// off and that row alone reappears in the figures.
#[test]
fn shell_chrome_is_excluded_by_the_surface_arm_of_the_classification() {
    let mut conn = db::open_in_memory();
    let event = |surface: &'static str, tool: &str, duration_ms: u64| EventRecord {
        at: NOW - 60,
        surface,
        tool: tool.to_string(),
        duration_ms,
        ok: true,
        origin: "main".to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            // A graph query the user issued through the SPA — real use.
            event("web", "search", 500),
            // The header's graph-state readout (FR-UI-34), one per navigation.
            event("shell", "status", 5),
            // Chrome reading the graph itself: the tool arm would count this.
            event("shell", "search", 5),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    let counted: Vec<(&str, &str)> = info
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    assert_eq!(
        counted,
        vec![("web", "search")],
        "only the user's own query counts"
    );
    assert_eq!(info.calls_total, 1, "the shell's two reads count nowhere");
    // AC5 names four figures; the latency percentile is the one with no other
    // coverage on this axis, and it is the most leak-prone — a chrome read is
    // cheap, so a leak *lowers* every percentile and looks like an improvement
    // rather than a fault. The seeded chrome rows carry a duration two orders
    // of magnitude off the real one so a leak is unmistakable.
    assert_eq!(
        (info.latency_p50_ms, info.latency_p95_ms, info.latency_p99_ms),
        (500, 500, 500),
        "every percentile is the user's own call alone, never the chrome reads'"
    );

    // Nothing was destroyed — the rows are still in the raw store, only
    // attributed away from the usage figures ([NFR-CC-04]).
    let stored: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, 3, "every event survives; only its attribution changed");
}

/// The same exclusion reaches `daily_rollup`, whose rows carry `surface` too —
/// so a window long enough to touch rolled-up days applies one rule, not two.
///
/// This is why the classification is derived from the stored `surface` rather
/// than from a per-row class column: the rollup aggregates away everything but
/// `(day, surface, tool)`, and a class that lived anywhere else could not
/// survive it.
#[test]
fn the_shell_chrome_exclusion_reaches_rollup_rows() {
    let conn = db::open_in_memory();
    // `impact` deliberately, not `status`: a rolled-up `status` row is already
    // excluded by the tool arm, so seeding one would prove nothing about the
    // surface arm this test exists for.
    for (surface, tool, calls) in [("shell", "impact", 40), ("web", "impact", 6)] {
        conn.execute(
            "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                       total_duration_ms, max_duration_ms)
             VALUES (date(?1, 'unixepoch'), ?2, ?3, ?4, ?4, 100, 50)",
            rusqlite::params![NOW - 86_400, surface, tool, calls],
        )
        .unwrap();
    }

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    assert_eq!(info.calls_total, 6, "the rolled-up chrome reads are excluded");
    assert_eq!(info.calls_by_tool.len(), 1);
    assert_eq!(info.calls_by_tool[0].tool, "impact");
    let day_calls: u64 = info.activity_by_day.iter().map(|d| d.calls).sum();
    assert_eq!(day_calls, 6, "and the daily series applies the same rule");
}

/// [FR-UI-34] end to end through the emission path: a read entered under
/// [`Surface::Shell`] is stored as `shell` and never reaches a usage figure,
/// while the same tool called outside that scope still counts.
///
/// Mirrors `chat_agent_calls_are_separable_from_web_and_mcp` — the two are the
/// same mechanism (an adapter naming itself at its boundary) pointed at
/// opposite conclusions: the chat agent's calls are separable **and counted**,
/// the shell's are separable **and excluded**.
#[test]
fn shell_chrome_reads_are_separable_from_the_web_surface_that_hosts_them() {
    assert_eq!(Surface::Shell.as_str(), "shell", "the CR-097 wire value");

    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        // The shell runs *inside* `serve --ui`, whose process surface is web.
        .with(TelemetryLayer::new(Surface::Web, "main".to_string(), "test-session".to_string(), sink));
    tracing::subscriber::with_default(subscriber, || {
        in_surface(Surface::Shell, || {
            traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
        });
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].surface, "shell", "the header's own read");
    assert_eq!(records[1].surface, "web", "a human browsing the dashboard");

    let mut conn = db::open_in_memory();
    let rows: Vec<EventRecord> = records
        .into_iter()
        .map(|r| EventRecord { at: NOW - 60, ..r })
        .collect();
    db::write_batch(&mut conn, &rows).unwrap();
    let info = stats_from(&conn, 7, NOW).expect("stats compute");
    assert_eq!(info.calls_total, 1, "only the human's call is usage");
    assert_eq!(info.calls_by_tool[0].surface, "web");
}

/// Every surface is classified, its wire name is unique and SQL-safe to
/// interpolate, and the self-referential set is exactly the one the acceptance
/// criteria name.
///
/// The SQL-safety half matters for the same reason it does for tools:
/// [`engine_query_predicate`] interpolates these names into a `NOT IN (…)`
/// list, and the guard is what makes that interpolation provably quote-free.
#[test]
fn every_surface_is_classified_and_sql_safe() {
    let mut names: Vec<&str> = Surface::ALL.iter().map(|s| s.as_str()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "surface wire names are unique");

    for surface in Surface::ALL {
        let name = surface.as_str();
        assert!(!name.is_empty(), "{surface:?} has a wire name");
        // The same shape the tool guard pins, for the same reason: both lists
        // are interpolated into the one predicate.
        let mut chars = name.chars();
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_lowercase()),
            "{name} starts with a lowercase ascii letter"
        );
        assert!(
            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name} is [a-z0-9_] throughout — SQL-safe to interpolate"
        );
        assert_eq!(
            Surface::from_wire(name),
            Some(*surface),
            "{name} round-trips through the wire vocabulary"
        );
        // `Option<EventClass>` can represent `Some(EngineQuery)` — "this
        // surface fixes every event as real usage" — which nothing means and
        // which `self_referential_surfaces` would silently treat as `None`.
        // The shape is kept for symmetry with the tool axis, so the state it
        // admits is excluded here instead.
        assert!(
            !matches!(surface.event_class(), Some(EventClass::EngineQuery)),
            "{surface:?} fixes EngineQuery — a surface either forces the \
             self-referential class or leaves the class to the tool; it can \
             never force an event to count"
        );
    }

    assert_eq!(
        self_referential_surfaces(),
        vec!["shell"],
        "the shell is the one surface whose every event is self-referential by \
         construction; `web` carries both a user's query and a Statistics-tab \
         render, which is exactly why the blanket surface filter had to go"
    );
}

/// The exclusion holds across **every** query the read-model runs, not just the
/// usage counts — latency, the raw daily series, and the origin split each carry
/// their own copy of the predicate.
///
/// This is a regression guard with teeth: `stats_from` interpolates
/// `{engine_query}` at **six** independent sites, and dropping it from any one
/// is a plausible edit. Mutation-testing the suite showed the latency query, the
/// raw daily-activity query and the origin breakdown could each lose the
/// predicate with every other test still green — the three the retired
/// `web_surface_activity_is_excluded_from_all_stats` used to cover. The seeded
/// self-referential rows are therefore given a *distinguishing* duration and a
/// *distinguishing* origin, so a leak in any query is visible in that query's
/// own output rather than only in the totals.
#[test]
fn the_exclusion_applies_to_every_stats_query() {
    let mut conn = db::open_in_memory();
    // Real use: three cli `search` calls, origin "main", durations 10/20/30.
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, NOW - 60),
            record("search", 20, true, NOW - 60),
            record("search", 30, true, NOW - 60),
        ],
    )
    .unwrap();
    // Self-referential noise on the same day: a huge duration (would dominate
    // every percentile), a distinct origin (would add a whole `dev` bucket) and
    // four extra calls (would inflate that day's series entry).
    let noise: Vec<EventRecord> = (0..4)
        .map(|_| EventRecord {
            at: NOW - 60,
            surface: "web",
            tool: "stats".to_string(),
            duration_ms: 5_000,
            ok: true,
            origin: "some-worktree-branch".to_string(),
            session_id: "test-session".to_string(),
        })
        .collect();
    db::write_batch(&mut conn, &noise).unwrap();
    // And on a rolled-up day inside the window, beside a real rollup row.
    for (tool, calls) in [("status", 99), ("node", 2)] {
        conn.execute(
            "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                       total_duration_ms, max_duration_ms)
             VALUES (date(?1, 'unixepoch'), 'web', ?2, ?3, ?3, 100, 50)",
            rusqlite::params![NOW - 86_400, tool, calls],
        )
        .unwrap();
    }

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    // Query 1 + 2 — usage counts, raw and rolled up.
    assert_eq!(info.calls_total, 5, "3 cli search + 2 rollup node");
    assert!(
        info.calls_by_tool
            .iter()
            .all(|u| u.tool != "stats" && u.tool != "status"),
        "no self-referential tool in the breakdown: {:?}",
        info.calls_by_tool
    );

    // Query 3 — latency. The 5000 ms reads must not reach the percentiles.
    assert!(
        info.latency_p99_ms <= 30,
        "self-referential latency leaked into the percentiles: p99 = {}",
        info.latency_p99_ms
    );

    // Query 4 + 5 — the daily series, raw and rolled up.
    let day_of = |secs: i64| -> String {
        conn.query_row("SELECT date(?1, 'unixepoch')", [secs], |r| r.get(0))
            .unwrap()
    };
    let today = day_of(NOW - 60);
    let raw_day = info
        .activity_by_day
        .iter()
        .find(|d| d.day == today)
        .expect("the raw day is in the series");
    assert_eq!(
        raw_day.calls, 3,
        "the raw daily series counted the self-referential reads"
    );
    let rolled = info
        .activity_by_day
        .iter()
        .find(|d| d.day == day_of(NOW - 86_400))
        .expect("the rolled-up day is in the series");
    assert_eq!(rolled.calls, 2, "the rolled-up day counted `status`");

    // Query 6 — the origin split. The noise carried its own branch origin, so a
    // leak would show up as a whole extra `dev` bucket.
    assert_eq!(
        info.calls_by_origin.len(),
        1,
        "a self-referential origin leaked into the split: {:?}",
        info.calls_by_origin
    );
    assert_eq!(info.calls_by_origin[0].origin, "main");
    assert_eq!(info.calls_by_origin[0].calls, 3);
}

/// The regression the story names: re-running a historical window reports the
/// navigation the raw store actually holds.
///
/// The Sprint 60 record read **4** navigation calls from a day whose store held
/// **82**, because 78 of them were stamped `web` — the surface the read-model
/// discarded wholesale. Nothing about those rows changed; the *rule* did, which
/// is only possible because the classification is derived from `tool` at read
/// time rather than stored per row.
#[test]
fn a_historical_window_reports_the_navigation_the_store_holds() {
    let mut nav = Vec::new();
    let seed = |surface: &'static str, tool: &str, n: usize| -> Vec<EventRecord> {
        (0..n)
            .map(|_| EventRecord {
                at: NOW - 3_600,
                surface,
                tool: tool.to_string(),
                duration_ms: 8,
                ok: true,
                origin: "main".to_string(),
                session_id: "test-session".to_string(),
            })
            .collect()
    };
    // 4 CLI/MCP navigation calls — all the old filter could see.
    nav.extend(seed("mcp", "search", 4));
    // 78 more on the web surface: the SPA and the chat agent driving the graph.
    nav.extend(seed("web", "search", 27));
    nav.extend(seed("web", "explore", 10));
    nav.extend(seed("web", "impact", 7));
    nav.extend(seed("web", "context", 8));
    nav.extend(seed("web", "callers", 7));
    nav.extend(seed("web", "node", 5));
    nav.extend(seed("web", "evolution", 11));
    nav.extend(seed("web", "dsm", 3));
    // Plus the self-referential reads that day, which must stay excluded.
    nav.extend(seed("web", "stats", 120));
    nav.extend(seed("cli", "stats", 5));

    let mut conn = db::open_in_memory();
    db::write_batch(&mut conn, &nav).unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");
    assert_eq!(
        info.calls_total, 82,
        "82 real calls, not the 4 the surface filter left"
    );
    let searches: u64 = info
        .calls_by_tool
        .iter()
        .filter(|u| u.tool == "search")
        .map(|u| u.calls)
        .sum();
    assert_eq!(searches, 31, "both surfaces' search calls are one figure");
}

/// A tool name in the store that today's registry does not know — written by an
/// older build, since retired — is **counted**, not dropped. History is reported
/// as it was recorded rather than silently rewritten by the current registry
/// ([NFR-CC-04]).
#[test]
fn an_unregistered_historical_tool_is_still_counted() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[record("a_retired_tool", 5, true, NOW - 60), record("search", 5, true, NOW - 60)],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();
    assert_eq!(info.calls_total, 2);
    assert!(
        info.calls_by_tool.iter().any(|u| u.tool == "a_retired_tool"),
        "an unknown tool is kept: {:?}",
        info.calls_by_tool
    );
}

// ── The generalised per-event surface override (FR-OB-03, FR-OB-09) ────────

/// [`in_surface`] attributes every event emitted inside it to the scoped
/// surface, leaving the process stamp untouched outside — the adapter-boundary
/// seam the chat agent enters once per tool call.
#[test]
fn the_surface_scope_attributes_only_what_it_wraps() {
    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Web, "main".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
        in_surface(Surface::Watcher, || {
            traced(Tool::Sync, || Ok::<_, anyhow::Error>(())).unwrap();
        });
        traced(Tool::Node, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    let seen: Vec<(&str, &str)> = records
        .iter()
        .map(|r| (r.tool.as_str(), r.surface))
        .collect();
    assert_eq!(
        seen,
        vec![("search", "web"), ("sync", "watcher"), ("node", "web")],
        "the scope covers exactly its own call, and the process stamp resumes"
    );
    assert!(
        records.iter().all(|r| r.origin == "main"),
        "origin is orthogonal to the surface override (FR-OB-08)"
    );
}

/// Nesting restores the **outer** scope, not the process stamp.
///
/// [`in_surface`]'s contract says so explicitly, and the difference is only
/// visible when a scope is entered from inside another one: a guard that reset
/// to `None` instead of the saved value would pass every other scope test here,
/// because they all start unscoped.
#[test]
fn a_nested_surface_scope_restores_the_outer_one() {
    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "main".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        in_surface(Surface::Web, || {
            traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
            in_surface(Surface::Watcher, || {
                traced(Tool::Sync, || Ok::<_, anyhow::Error>(())).unwrap();
            });
            // Back in the OUTER scope — web, not the mcp process stamp.
            traced(Tool::Node, || Ok::<_, anyhow::Error>(())).unwrap();
        });
        // Outside every scope — the process stamp.
        traced(Tool::Impact, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let seen: Vec<(String, &str)> = rx
        .try_iter()
        .map(|r| (r.tool, r.surface))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("search".to_string(), "web"),
            ("sync".to_string(), "watcher"),
            ("node".to_string(), "web"),
            ("impact".to_string(), "mcp"),
        ],
        "the inner scope pops back to the outer one, then to the process stamp"
    );
}

/// The scope is restored even when the scoped call unwinds — a panicking tool
/// call must not leave the thread mis-attributing every later event on it.
#[test]
fn the_surface_scope_is_restored_after_a_panic() {
    let (sink, rx) = TelemetrySink::with_capacity(4);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "main".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            in_surface(Surface::Watcher, || panic!("a tool call blew up"));
        }));
        assert!(unwound.is_err(), "the panic propagated");
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].surface, "mcp",
        "the process stamp is back after the unwind"
    );
}

/// The watcher's two telemetry points record a **full** record in both
/// outcomes — the regression guard for a defect that was silent for its whole
/// lifetime.
///
/// `watch_coverage_ingest` was emitted without `duration_ms`/`ok`, so
/// [`TelemetryVisitor::into_record`] dropped it as malformed and it had never
/// written a row. Fixing only the success arm would have been worse than the
/// bug: the tool would then report `ok_calls == calls` forever, because failures
/// still never reached the store — a fabricated 100 % success rate
/// ([NFR-CC-04]). This asserts the exact field shape both arms emit, so the
/// event cannot drift back to being dropped and the failure arm cannot go quiet.
#[test]
fn the_watcher_records_both_outcomes_with_a_full_field_shape() {
    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        // The watcher runs inside `serve --mcp`, whose process surface is mcp.
        .with(TelemetryLayer::new(Surface::Mcp, "main".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        // Mirrors the success arm of watch/mod.rs's coverage-ingest loop.
        tracing::info!(
            target: TELEMETRY_TARGET,
            tool = Tool::WatchCoverageIngest.as_str(),
            surface = Surface::Watcher.as_str(),
            duration_ms = 12u64,
            ok = true,
            matched_files = 3usize,
            "watcher auto-ingested a coverage artifact",
        );
        // …and the failure arm.
        tracing::info!(
            target: TELEMETRY_TARGET,
            tool = Tool::WatchCoverageIngest.as_str(),
            surface = Surface::Watcher.as_str(),
            duration_ms = 4u64,
            ok = false,
            "watcher coverage ingest failed",
        );
        // …and the sync trigger.
        tracing::info!(
            target: TELEMETRY_TARGET,
            tool = Tool::WatchSync.as_str(),
            surface = Surface::Watcher.as_str(),
            duration_ms = 7u64,
            ok = true,
            files = 2u64,
            "watcher sync",
        );
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    let seen: Vec<(&str, &str, bool)> = records
        .iter()
        .map(|r| (r.tool.as_str(), r.surface, r.ok))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("watch_coverage_ingest", "watcher", true),
            ("watch_coverage_ingest", "watcher", false),
            ("watch_sync", "watcher", true),
        ],
        "every watcher emission records, and a failure records as a failure"
    );
}

/// An event that names its own surface is the most specific statement
/// available, so it wins over an ambient scope. This is what keeps the
/// watcher's own attribution correct if it is ever driven from inside another
/// adapter's scope.
#[test]
fn an_event_field_override_wins_over_the_ambient_scope() {
    let (sink, rx) = TelemetrySink::with_capacity(4);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Web, "main".to_string(), "test-session".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        in_surface(Surface::Web, || {
            tracing::info!(
                target: TELEMETRY_TARGET,
                tool = Tool::WatchSync.as_str(),
                surface = Surface::Watcher.as_str(),
                duration_ms = 4u64,
                ok = true,
                "watcher sync inside another scope"
            );
        });
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].surface, "watcher");
}

/// [FR-OB-10]: a chat-agent tool call is stored under `surface = "chat"`,
/// separable from both `web` (the process it runs inside) and `mcp` — and it
/// counts as an engine query, not as a self-referential read.
///
/// The surface is gated with the agent substrate it describes, so in a build
/// without `agents` the variant does not exist and neither does this test.
#[cfg(feature = "agents")]
#[test]
fn chat_agent_calls_are_separable_from_web_and_mcp() {
    assert_eq!(Surface::Chat.as_str(), "chat", "the FR-OB-10 wire value");

    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        // The chat agent runs *inside* `serve --ui`, whose process surface is web.
        .with(TelemetryLayer::new(Surface::Web, "main".to_string(), "test-session".to_string(), sink));
    tracing::subscriber::with_default(subscriber, || {
        in_surface(Surface::Chat, || {
            traced(Tool::Impact, || Ok::<_, anyhow::Error>(())).unwrap();
        });
        traced(Tool::Impact, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].surface, "chat", "the agent's own call");
    assert_eq!(records[1].surface, "web", "a human browsing the dashboard");

    // And it survives into the read-model as its own surface, counted.
    let mut conn = db::open_in_memory();
    let mut rows: Vec<EventRecord> = records
        .into_iter()
        .map(|r| EventRecord { at: NOW - 60, ..r })
        .collect();
    // FR-OB-10 AC1 names BOTH: separable from `web` *and* from `mcp`. The two
    // are separate claims — `web` is the process the agent runs inside, `mcp`
    // is the other agent-facing surface it would otherwise be summed with — so
    // an `mcp` row joins the fixture rather than being assumed.
    rows.push(EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: "impact".to_string(),
        duration_ms: 3,
        ok: true,
        origin: "main".to_string(),
        session_id: "test-session".to_string(),
    });
    db::write_batch(&mut conn, &rows).unwrap();
    let info = stats_from(&conn, 7, NOW).unwrap();
    let surfaces: Vec<&str> = info
        .calls_by_tool
        .iter()
        .map(|u| u.surface.as_str())
        .collect();
    for expected in ["chat", "web", "mcp"] {
        assert!(
            surfaces.contains(&expected),
            "`{expected}` is its own group, not folded into another: {surfaces:?}"
        );
    }
    assert_eq!(
        info.calls_by_tool.len(),
        3,
        "one `impact` row per surface — chat is never summed with web or mcp: {:?}",
        info.calls_by_tool
    );
    assert_eq!(info.calls_total, 3, "agent-issued navigation is real usage");
}

/// [FR-OB-13]: Logos's own wiki generation pass is stored under
/// `surface = "wikigen"`, separable from `web` (the process it runs inside),
/// from `mcp`, and from `chat` (the *other* in-process agent) — and it counts
/// as an engine query, because its subject is the indexed code.
///
/// The sibling of [`chat_agent_calls_are_separable_from_web_and_mcp`], pointed
/// at a third in-process caller, and the unit half of the pair
/// `web/tests/wikigen_surface.rs` asserts end to end through the real route.
///
/// # Why this one is not `#[cfg(feature = "agents")]`
///
/// `Chat` is gated with the agent substrate it describes. The materialize pass
/// this names is deterministic and offline ([FR-WK-20], [NFR-SE-01]), so there
/// is no egress substrate to gate it with — it joins `Watcher` and `Shell` as an
/// ungated override-only variant. The `chat` arm of the separability claim is
/// the only part that needs the feature, and only that part carries the gate.
///
/// [FR-OB-13]: ../../../docs/specs/requirements/FR-OB-13.md
/// [FR-WK-20]: ../../../docs/specs/requirements/FR-WK-20.md
/// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
#[test]
fn wiki_generation_calls_are_separable_from_the_surfaces_they_would_be_summed_with() {
    assert_eq!(Surface::WikiGen.as_str(), "wikigen", "the FR-OB-13 wire value");

    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry()
        // The generation pass runs *inside* `serve --ui`, whose process surface
        // is web.
        .with(TelemetryLayer::new(Surface::Web, "main".to_string(), "test-session".to_string(), sink));
    tracing::subscriber::with_default(subscriber, || {
        in_surface(Surface::WikiGen, || {
            traced(Tool::WikiMaterialize, || Ok::<_, anyhow::Error>(())).unwrap();
        });
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].surface, "wikigen", "the generator's own pass");
    assert_eq!(records[1].surface, "web", "a human browsing the dashboard");

    // And it survives into the read-model as its own group, counted. The three
    // surfaces it must not be folded into are named separately, because they are
    // three separate claims: `web` is the process it runs inside, `mcp` the other
    // agent-facing surface, `chat` the other in-process agent — the one reuse
    // CR-139 decision 1 rejected, since collapsing them would recreate one
    // conflation while removing another.
    let mut conn = db::open_in_memory();
    let mut rows: Vec<EventRecord> = records
        .into_iter()
        .map(|r| EventRecord { at: NOW - 60, ..r })
        .collect();
    #[cfg_attr(not(feature = "agents"), allow(unused_mut))]
    let mut expected_surfaces = vec!["wikigen", "web", "mcp"];
    rows.push(EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: "wiki_materialize".to_string(),
        duration_ms: 3,
        ok: true,
        origin: "main".to_string(),
        session_id: "test-session".to_string(),
    });
    #[cfg(feature = "agents")]
    {
        expected_surfaces.push(Surface::Chat.as_str());
        rows.push(EventRecord {
            at: NOW - 60,
            surface: Surface::Chat.as_str(),
            tool: "wiki_materialize".to_string(),
            duration_ms: 3,
            ok: true,
            origin: "main".to_string(),
            session_id: "test-session".to_string(),
        });
    }
    db::write_batch(&mut conn, &rows).unwrap();
    let info = stats_from(&conn, 7, NOW).unwrap();
    let seen: Vec<(&str, &str)> = info
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    for expected in &expected_surfaces {
        assert!(
            seen.iter().any(|(surface, _)| surface == expected),
            "`{expected}` is its own group, not folded into another: {seen:?}"
        );
    }
    assert_eq!(
        seen.iter().filter(|(_, tool)| *tool == "wiki_materialize").count(),
        expected_surfaces.len() - 1,
        "one `wiki_materialize` row per surface — the generator's pass is never \
         summed with mcp's or the chat agent's: {seen:?}"
    );
    assert!(
        seen.contains(&("wikigen", "wiki_materialize")),
        "the generation pass's subject is the indexed code, so it is real engine \
         work and stays counted — this variant separates it from a developer's \
         browsing, it does not exclude it: {seen:?}"
    );
    assert_eq!(
        self_referential_surfaces(),
        vec!["shell"],
        "and adding it left the self-referential set alone: the shell is still \
         the only surface whose every event is self-referential by construction"
    );
}

/// Events outside the window are excluded from counts, percentiles, **and every
/// additive projection** — the S-233 daily series and origin split, and the
/// [FR-OB-11] cross-tab and class breakdown. Each carries its own
/// `WHERE at >= ?1` / `day >= date(cutoff)` predicate, so this locks each
/// against a dropped window filter (a distinct out-of-window origin and an
/// out-of-window rollup day would both leak otherwise).
///
/// **This roster must grow with the surface it protects.** It is the only test
/// that exercises the window boundary, so a projection added later and not
/// asserted here can lose its `at >= ?1` and pass the entire suite.
#[test]
fn stats_respects_the_window() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, NOW - 60), // inside, origin "main"
            // Outside 7d and a *distinct* origin: if either the by-day or the
            // origin query lost its window predicate, this would leak in.
            EventRecord {
                at: NOW - 10 * 86_400,
                surface: "cli",
                tool: "search".to_string(),
                duration_ms: 9_999,
                ok: true,
                origin: "feature".to_string(),
                session_id: "test-session".to_string(),
            },
        ],
    )
    .unwrap();
    // A rollup day well outside the window — must not reach the daily series.
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'mcp', 'impact', 5, 5, 100, 40)",
        [NOW - 30 * 86_400],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();
    assert_eq!(info.calls_total, 1);
    assert_eq!(
        info.latency_p99_ms, 10,
        "the out-of-window duration is excluded"
    );
    // The additive fields honor the same window: only the in-window main day.
    assert_eq!(info.activity_by_day.len(), 1, "only the in-window day");
    assert_eq!(info.activity_by_day[0].calls, 1);
    assert_eq!(
        info.calls_by_origin.len(),
        1,
        "the out-of-window feature origin and rollup day are excluded"
    );
    assert_eq!(info.calls_by_origin[0].origin, "main");
    assert_eq!(info.calls_by_origin[0].calls, 1);
    // The FR-OB-11 projections honor it too. The out-of-window event carries a
    // *distinct* origin, so a dropped predicate on the cross-tab would surface a
    // whole extra `("search", "dev")` cell and a `("navigation", "dev")` class row.
    let cross_tab: Vec<(&str, &str, u64)> = info
        .calls_by_tool_origin
        .iter()
        .map(|c| (c.tool.as_str(), c.origin.as_str(), c.calls))
        .collect();
    assert_eq!(
        cross_tab,
        vec![("search", "main", 1)],
        "the out-of-window dev origin is excluded from the cross-tab"
    );
    let by_class: Vec<(&str, &str, u64)> = info
        .calls_by_class
        .iter()
        .map(|c| (c.class.as_str(), c.origin.as_str(), c.calls))
        .collect();
    assert_eq!(by_class, vec![("navigation", "main", 1)]);
}

/// An empty store yields a zeroed read-model, not an error — including the
/// S-233 additive series/breakdown, which are empty rather than fabricated
/// (NFR-CC-04).
#[test]
fn stats_on_an_empty_store_is_zeroed() {
    let conn = db::open_in_memory();
    let info = stats_from(&conn, 7, NOW).unwrap();
    assert_eq!(info.calls_total, 0);
    assert_eq!(info.latency_p50_ms, 0);
    assert_eq!(info.tokens_saved_estimate, 0);
    assert!(info.calls_by_tool.is_empty());
    assert!(info.activity_by_day.is_empty(), "no days recorded");
    assert!(info.calls_by_origin.is_empty(), "no origins recorded");
    assert!(info.warnings.is_empty());
}

/// The S-233 read-model additions: a per-UTC-day activity series (raw events
/// folded with the rollup days the window reaches) and a per-`origin` usage
/// split grouped by `COALESCE(origin,'main')` (FR-OB-04, FR-OB-08). The origin
/// split is raw-events-only — a rollup day (no `origin` column) contributes to
/// the daily series but never to the origin breakdown, so its call sum can lag
/// `calls_total` honestly (NFR-CC-04).
#[test]
fn stats_reports_daily_activity_and_origin_breakdown() {
    let mut conn = db::open_in_memory();
    // Day D0 (now): three "main" search calls, one of them failed.
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, NOW - 60),
            record("search", 20, false, NOW - 60),
            record("search", 30, true, NOW - 60),
        ],
    )
    .unwrap();
    // Day D-1: two calls from a worktree branch `feature`.
    let feature = |tool: &str, at: i64| EventRecord {
        at,
        surface: "mcp",
        tool: tool.to_string(),
        duration_ms: 5,
        ok: true,
        origin: "feature".to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            feature("context", NOW - 86_400 - 60),
            feature("impact", NOW - 86_400 - 60),
        ],
    )
    .unwrap();
    // Day D-2: a rollup day inside the window (raw events already aged out).
    // It carries no `origin`, so it lands in the daily series but not the split.
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'mcp', 'search', 4, 4, 120, 40)",
        [NOW - 2 * 86_400],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    // Resolve the three expected calendar days from the same clock/DB function
    // so the assertion is timezone-agnostic (the query groups on UTC dates).
    let day_of = |secs: i64| -> String {
        conn.query_row("SELECT date(?1, 'unixepoch')", [secs], |r| r.get(0))
            .unwrap()
    };
    let (d0, d1, d2) = (
        day_of(NOW - 60),
        day_of(NOW - 86_400 - 60),
        day_of(NOW - 2 * 86_400),
    );

    // Daily series: three distinct days, oldest first (D-2, D-1, D0).
    assert_eq!(info.activity_by_day.len(), 3, "one entry per active day");
    let days: Vec<&str> = info.activity_by_day.iter().map(|d| d.day.as_str()).collect();
    assert_eq!(days, vec![d2.as_str(), d1.as_str(), d0.as_str()], "oldest first");
    assert_eq!(
        (info.activity_by_day[0].calls, info.activity_by_day[0].ok_calls),
        (4, 4),
        "D-2 comes from the rollup day"
    );
    assert_eq!(
        (info.activity_by_day[1].calls, info.activity_by_day[1].ok_calls),
        (2, 2),
        "D-1 is the two feature events"
    );
    assert_eq!(
        (info.activity_by_day[2].calls, info.activity_by_day[2].ok_calls),
        (3, 2),
        "D0 is three search calls, one failed"
    );
    // The series total matches calls_total (raw 5 + rollup 4).
    let series_calls: u64 = info.activity_by_day.iter().map(|d| d.calls).sum();
    assert_eq!(series_calls, info.calls_total);
    assert_eq!(info.calls_total, 9);

    // Dev-vs-main split: the `feature` branch folds into `"dev"`, sorted before
    // `"main"`; raw events only (rollup excluded).
    assert_eq!(info.calls_by_origin.len(), 2, "dev + main");
    assert_eq!(info.calls_by_origin[0].origin, "dev");
    assert_eq!(
        (info.calls_by_origin[0].calls, info.calls_by_origin[0].ok_calls),
        (2, 2)
    );
    assert_eq!(info.calls_by_origin[1].origin, "main");
    assert_eq!(
        (info.calls_by_origin[1].calls, info.calls_by_origin[1].ok_calls),
        (3, 2)
    );
    let origin_calls: u64 = info.calls_by_origin.iter().map(|o| o.calls).sum();
    assert_eq!(
        origin_calls, 5,
        "origin split covers raw events only; the rollup day is honestly absent"
    );
    assert!(info.warnings.is_empty());
}

/// A legacy row written before the v2 migration (NULL `origin`) folds into the
/// `"main"` group via `COALESCE`, alongside genuinely-`"main"`-stamped rows
/// (FR-OB-08) — the read-model never leaks a NULL origin.
#[test]
fn origin_breakdown_folds_legacy_null_into_main() {
    let mut conn = db::open_in_memory_v1();
    // Two legacy rows under the v1 schema (no `origin` column).
    conn.execute(
        "INSERT INTO events (at, surface, tool, duration_ms, ok)
         VALUES (?1, 'cli', 'search', 12, 1), (?1, 'cli', 'node', 8, 1)",
        [NOW - 60],
    )
    .unwrap();
    db::migrate(&mut conn).expect("the forward ledger applies");
    // A post-migration row explicitly stamped "main".
    db::write_batch(&mut conn, &[record("impact", 3, true, NOW - 60)]).unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();
    assert_eq!(info.calls_by_origin.len(), 1, "all fold into a single group");
    assert_eq!(info.calls_by_origin[0].origin, "main");
    assert_eq!(
        (info.calls_by_origin[0].calls, info.calls_by_origin[0].ok_calls),
        (3, 3),
        "two legacy NULL rows + one stamped main"
    );
}

/// The dev-vs-`main` collapse: several *distinct* worktree branches in the window
/// fold into a single cumulative `"dev"` bucket summing their calls, so the card
/// never grows one bar per stale branch (FR-OB-08). `"main"` stays its own bucket.
#[test]
fn origin_breakdown_collapses_all_branches_into_dev() {
    let mut conn = db::open_in_memory();
    let on = |branch: &str, tool: &str, ok: bool| EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: tool.to_string(),
        duration_ms: 5,
        ok,
        origin: branch.to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            // Two events on branch A (one failed), one event on branch B, plus main.
            on("sprint-40-I1-S1", "context", true),
            on("sprint-40-I1-S1", "impact", false),
            on("sprint-41-I2-S3", "search", true),
            record("node", 8, true, NOW - 60),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    // Exactly two buckets, `"dev"` before `"main"`.
    assert_eq!(info.calls_by_origin.len(), 2, "dev + main, never per-branch");
    assert_eq!(info.calls_by_origin[0].origin, "dev");
    assert_eq!(
        (info.calls_by_origin[0].calls, info.calls_by_origin[0].ok_calls),
        (3, 2),
        "both branches summed: 3 calls, 2 ok"
    );
    assert_eq!(info.calls_by_origin[1].origin, "main");
    assert_eq!(
        (info.calls_by_origin[1].calls, info.calls_by_origin[1].ok_calls),
        (1, 1)
    );
}

/// The read-model is **monotone in the window**: a wider window never reports
/// fewer calls ([NFR-CC-04]).
///
/// Not a theoretical property. The two `daily_rollup` queries filter on
/// `day >= date(?1, 'unixepoch')`, and SQLite's `date()` returns **NULL** once the
/// cutoff falls below its julian-day floor; `day >= NULL` is NULL, so every rollup
/// row stops matching and the rolled-up half of `calls_total`, `calls_by_tool` and
/// `activity_by_day` silently disappears. Before the cutoff was clamped at the
/// epoch, `window = u32::MAX` returned *one* call where `window = 365` returned
/// 101 — a larger window answering with a smaller total, and no warning.
///
/// Reachable from both `?window=` endpoints, neither of which bounds the value.
#[test]
fn a_wider_window_never_reports_fewer_calls() {
    let mut conn = db::open_in_memory();
    db::write_batch(&mut conn, &[record("search", 10, true, NOW - 120)]).unwrap();
    // A rolled-up day 200 days back — only a long window reaches it, and only a
    // rollup row can exercise the `date()` predicate.
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'cli', 'search', 100, 100, 3000, 50)",
        [NOW - 200 * 86_400],
    )
    .unwrap();

    let windows = [7u32, 365, 1_000_000, u32::MAX];
    let totals: Vec<u64> = windows
        .iter()
        .map(|w| stats_from(&conn, *w, NOW).expect("stats compute").calls_total)
        .collect();

    assert_eq!(totals[0], 1, "the 7-day window sees only the raw event");
    assert!(
        totals[1..].iter().all(|&t| t == 101),
        "every window reaching the rolled-up day counts it: {windows:?} -> {totals:?}"
    );
    for pair in totals.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "a wider window returned fewer calls: {windows:?} -> {totals:?}"
        );
    }
}

/// An **unreadable** store is an `Err` out of [`super::stats`], not the
/// warning-carrying empty model an *absent* one produces.
///
/// The sibling of `stats_without_a_telemetry_db_degrades_with_a_warning`, and
/// the half that was unpinned: `stats` now delegates to `read_stats`, so the
/// `?` that propagates a real failure is one edit away from becoming an
/// `unwrap_or(None)` that swallows it. That edit would report a corrupt store to
/// every `Engine::stats` caller as "no telemetry recorded yet" — a fault
/// rendered as a routine state, which is the worse direction and exactly what
/// the empty/absent distinction exists to keep apart ([NFR-CC-04]).
#[test]
fn stats_over_an_unreadable_store_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let logos = dir.path().join(".logos");
    std::fs::create_dir_all(&logos).unwrap();
    std::fs::write(logos.join("telemetry.db"), b"not a database").unwrap();

    let err = super::stats(dir.path(), None)
        .expect_err("a present, unopenable store must not degrade to the empty model");
    let rendered = format!("{err:#}");
    assert!(
        !rendered.contains("no telemetry recorded yet"),
        "a fault must not be reported as an absence: {rendered}"
    );
}

/// A store that is **present but not a regular file** is likewise an error, not
/// an absence — `is_file()` alone cannot tell the two apart, because it answers
/// `false` for every `stat` outcome rather than only for "not there".
#[test]
fn stats_over_a_non_file_store_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos").join("telemetry.db")).unwrap();

    let err = super::stats(dir.path(), None)
        .expect_err("a directory named telemetry.db is a broken checkout, not an empty one");
    assert!(
        format!("{err:#}").contains("not a regular file"),
        "the diagnostic names what is actually wrong: {err:#}"
    );
}

/// A project that never recorded telemetry degrades to a warning-carrying
/// default through the path-level entry point.
#[test]
fn stats_without_a_telemetry_db_degrades_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let info = super::stats(dir.path(), None).expect("missing store is not an error");
    assert_eq!(info.window_days, 7, "FR-OB-04 default window");
    assert_eq!(info.calls_total, 0);
    assert!(
        info.warnings.iter().any(|w| w.contains("no telemetry")),
        "the degradation reason is surfaced: {:?}",
        info.warnings
    );
    assert!(
        info.activity_by_day.is_empty() && info.calls_by_origin.is_empty(),
        "the additive series/breakdown default empty, not fabricated (NFR-CC-04)"
    );
    assert!(
        info.calls_by_tool_origin.is_empty() && info.calls_by_class.is_empty(),
        "the attribution projections default empty too"
    );
    // The coverage labels are structural, not measured, so a degraded read-model
    // still states them — a consumer rendering an empty cross-tab must not be
    // told `raw_events_only: false`, which would be a claim rather than an
    // absence (FR-OB-11, NFR-CC-04).
    assert!(info.attribution_coverage.raw_events_only);
    assert!(info.attribution_coverage.legacy_null_origin_folds_into_main);
    assert_eq!(info.attribution_coverage.requested_window_days, 7);
    assert_eq!(info.attribution_coverage.covered_window_days, 7);
    assert_eq!(info.attribution_coverage.notes.len(), 3);
}

// ── Shared telemetry-store resolution (ADR-50, FR-OB-07) ───────────────────
//
// `init` (write) and `stats` (read) both resolve the store directory through
// the single `telemetry_logos_dir` helper — so proving the helper's matrix
// proves the two paths can never target different stores. The full
// write-through + `git worktree remove` durability end-to-end is S-232.

/// Run a git command in `cwd`, panicking on failure — fixtures only.
fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A committed repo at `<tmp>/main`; returns (tmp, primary_root).
fn repo_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("temp root");
    let main = tmp.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("f.rs"), "pub fn a() {}\n").unwrap();
    sh_git(&main, &["init", "-q", "-b", "main"]);
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "initial"]);
    (tmp, main)
}

/// Add a linked worktree at `<tmp>/wt` on a new branch; returns its root.
fn add_worktree(tmp: &tempfile::TempDir, main: &Path) -> std::path::PathBuf {
    let wt = tmp.path().join("wt");
    sh_git(
        main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feature"],
    );
    wt
}

/// From a linked worktree whose primary already has `.logos/`, the store
/// resolves to the PRIMARY's `.logos/` — write-through durability (FR-OB-07).
#[test]
fn telemetry_dir_from_a_worktree_is_the_primary_logos() {
    let (tmp, main) = repo_fixture();
    std::fs::create_dir_all(main.join(".logos")).unwrap();
    let wt = add_worktree(&tmp, &main);

    let resolved = telemetry_logos_dir(&wt);
    assert_eq!(
        resolved.canonicalize().unwrap(),
        main.join(".logos").canonicalize().unwrap(),
        "a worktree writes/reads through the primary's .logos, not its own"
    );
}

/// When the primary has no `.logos/` yet, a worktree falls back to its own
/// local `.logos/` — a read command must never CREATE state in another
/// checkout (ADR-50).
#[test]
fn telemetry_dir_falls_back_to_local_when_primary_uninitialised() {
    let (tmp, main) = repo_fixture();
    let wt = add_worktree(&tmp, &main);

    let resolved = telemetry_logos_dir(&wt);
    assert_eq!(
        resolved,
        wt.join(".logos"),
        "no primary .logos → stay local, seed nothing in main"
    );
    assert!(
        !main.join(".logos").exists(),
        "resolution must not create a .logos in the primary"
    );
}

/// From the primary checkout, resolution is the identity — the store path and
/// existing data are unchanged for the non-worktree case.
#[test]
fn telemetry_dir_from_the_primary_is_the_identity() {
    let (_tmp, main) = repo_fixture();
    std::fs::create_dir_all(main.join(".logos")).unwrap();
    assert_eq!(telemetry_logos_dir(&main), main.join(".logos"));
}

/// Outside any git repo, resolution is the local `.logos/` (no primary, no
/// panic) — the degrade-gracefully posture.
#[test]
fn telemetry_dir_outside_git_is_local() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(telemetry_logos_dir(dir.path()), dir.path().join(".logos"));
}

// ── Per-process origin stamp (FR-OB-08) ────────────────────────────────────
//
// `telemetry_origin` decides the dev-vs-main attribution once at init, reusing
// the same primary-vs-worktree distinction as the store resolution above.

/// From a linked worktree, the origin is that worktree's branch name — the
/// increment being built (FR-OB-08). `add_worktree` creates it on `feature`.
#[test]
fn telemetry_origin_from_a_worktree_is_the_branch() {
    let (tmp, main) = repo_fixture();
    let wt = add_worktree(&tmp, &main);
    assert_eq!(
        telemetry_origin(&wt),
        "feature",
        "a worktree attributes its events to its branch"
    );
}

/// From the primary checkout, the origin is `"main"` — there is no distinct
/// primary to point back at, so the increment is main (FR-OB-08).
#[test]
fn telemetry_origin_from_the_primary_is_main() {
    let (_tmp, main) = repo_fixture();
    assert_eq!(telemetry_origin(&main), "main");
}

/// Outside any git repo the origin degrades to `"main"` (no primary, no
/// branch, no panic) — the same graceful posture as the store resolution.
#[test]
fn telemetry_origin_outside_git_is_main() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(telemetry_origin(dir.path()), "main");
}

/// A linked worktree with a **detached HEAD** has a primary but no nameable
/// branch — the `Some(primary)` arm's fallback lands on `"main"` (the one
/// FR-OB-08 degrade case the primary-checkout tests don't exercise).
#[test]
fn telemetry_origin_from_a_detached_worktree_is_main() {
    let (tmp, main) = repo_fixture();
    let wt = add_worktree(&tmp, &main);
    // Detach the worktree's HEAD onto its own commit.
    let head = {
        let out = Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    sh_git(&wt, &["checkout", "-q", &head]);
    assert_eq!(
        telemetry_origin(&wt),
        "main",
        "a detached worktree HEAD degrades to main, not the literal HEAD"
    );
}

// ── Per-process session id (FR-OB-12) ──────────────────────────────────────
//
// `generate_session_id` decides the per-process stamp once at init — unlike
// `telemetry_origin`, it needs no root or git resolution at all, since it
// carries no environment identity by design (NFR-CC-03).

/// Two separate processes get two distinct ids. There is no shared process
/// state to derive it from — each call represents one process's own
/// [`init`](super::init) — so this asserts what makes the value usable as a
/// session key at all: no collision between independently-generated stamps.
#[test]
fn distinct_processes_get_distinct_session_ids() {
    let a = generate_session_id();
    let b = generate_session_id();
    assert_ne!(a, b, "each process draws its own random stamp");
    assert!(!a.is_empty() && !b.is_empty());
}

/// The value carries no user, machine or account identity (NFR-CC-03): calling
/// it many times never reproduces a previous value (it is not seeded from
/// anything stable like a hostname or username) and it is plain hex — never a
/// path, an email-shaped string, or anything else that could leak identity by
/// accident.
#[test]
fn the_session_id_is_opaque_and_never_repeats() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        let id = generate_session_id();
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "{id} is not plain hex"
        );
        assert!(seen.insert(id), "a session id repeated across 64 draws");
    }
}

/// Two events emitted through the same process's layer share a `session_id` —
/// the per-process stamp is computed once and copied onto every record, not
/// redrawn per event ([NFR-OO-02]: no hot-path cost).
#[test]
fn events_from_one_process_share_a_session_id() {
    let (sink, rx) = TelemetrySink::with_capacity(8);
    let session_id = generate_session_id();
    let subscriber = tracing_subscriber::registry().with(TelemetryLayer::new(
        Surface::Mcp,
        "main".to_string(),
        session_id.clone(),
        sink,
    ));

    tracing::subscriber::with_default(subscriber, || {
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
        traced(Tool::Node, || Ok::<_, anyhow::Error>(())).unwrap();
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert!(
        records.iter().all(|r| r.session_id == session_id),
        "both events from this process share the one stamp: {records:?}"
    );
}

/// `session_id` is orthogonal to both `surface` and `origin` (FR-OB-12): it
/// stays fixed while a surface override changes the attributed surface for
/// one call, and it is independent of whatever `origin` string the layer
/// carries — none of the three tracks the others.
#[test]
fn the_session_id_is_orthogonal_to_surface_and_origin() {
    let (sink, rx) = TelemetrySink::with_capacity(8);
    let subscriber = tracing_subscriber::registry().with(TelemetryLayer::new(
        Surface::Mcp,
        "feature-branch".to_string(),
        "fixed-session".to_string(),
        sink,
    ));

    tracing::subscriber::with_default(subscriber, || {
        traced(Tool::Search, || Ok::<_, anyhow::Error>(())).unwrap();
        in_surface(Surface::Watcher, || {
            traced(Tool::Sync, || Ok::<_, anyhow::Error>(())).unwrap();
        });
    });

    let records: Vec<EventRecord> = rx.try_iter().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].surface, "mcp");
    assert_eq!(records[1].surface, "watcher", "the surface override took effect");
    assert!(
        records.iter().all(|r| r.session_id == "fixed-session"),
        "session_id is unaffected by the surface override: {records:?}"
    );
    assert!(
        records.iter().all(|r| r.origin == "feature-branch"),
        "origin is unaffected by the surface override too: {records:?}"
    );
}

/// The store can answer *"what fraction of sessions made at least one
/// navigation call?"* — the question `origin` alone could not, because every
/// process in a worktree collapses into one branch bucket. Two sessions issue
/// a navigation call each, a third issues only bookkeeping calls, and a
/// pre-migration (legacy `NULL`) row contributes to neither the numerator nor
/// a fabricated denominator: it reads as its own `'unattributed'` bucket, one
/// query, `COALESCE(session_id, 'unattributed')` grouping.
#[test]
fn fraction_of_sessions_with_a_navigation_call_is_derivable() {
    let mut conn = db::open_in_memory();
    let on = |session: &str, tool: &str| EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: tool.to_string(),
        duration_ms: 5,
        ok: true,
        origin: "main".to_string(),
        session_id: session.to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            on("session-a", "search"),   // navigation
            on("session-a", "sync"),     // bookkeeping, same session
            on("session-b", "context"),  // navigation, distinct session
            on("session-c", "sync"),     // bookkeeping only — no navigation
        ],
    )
    .unwrap();
    // A pre-migration row: NULL session_id, must not be folded into any of the
    // three sessions above nor silently counted as a fourth real one.
    conn.execute(
        "INSERT INTO events (at, surface, tool, duration_ms, ok, origin)
         VALUES (?1, 'cli', 'search', 12, 1, 'main')",
        [NOW - 60],
    )
    .unwrap();

    let navigation_tools: Vec<&str> = Tool::ALL
        .iter()
        .filter(|t| t.tool_class() == ToolClass::Navigation)
        .map(|t| t.as_str())
        .collect();
    let placeholders = navigation_tools
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    let total_sessions: i64 = conn
        .query_row(
            "SELECT count(DISTINCT COALESCE(session_id, 'unattributed')) FROM events",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        total_sessions, 4,
        "3 real sessions + the unattributed bucket, never folded together"
    );

    let sessions_with_nav: i64 = conn
        .query_row(
            &format!(
                "SELECT count(DISTINCT session_id) FROM events
                 WHERE session_id IS NOT NULL AND tool IN ({placeholders})"
            ),
            rusqlite::params_from_iter(navigation_tools.iter()),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        sessions_with_nav, 2,
        "session-a and session-b each made a navigation call"
    );

    // The fraction itself: real sessions only — an unattributed legacy row is
    // not a session that could have made a navigation call.
    let real_sessions = total_sessions - 1;
    assert_eq!(
        (sessions_with_nav as f64) / (real_sessions as f64),
        2.0 / 3.0,
        "2 of the 3 real sessions made at least one navigation call"
    );
}

// ── The tool × origin cross-tab and the tool class (FR-OB-11) ──────────────

/// **The headline [FR-OB-11] criterion.** `logos stats --json` reports per-tool
/// counts split by dev/`main` origin, and a class for every tool.
///
/// Neither existing shape can answer *"which navigation came from dev panes?"*:
/// `calls_by_tool` groups by surface and tool with no origin, `calls_by_origin`
/// by origin with no tool. The fixture makes that concrete — `search` runs on
/// **both** sides and `index` only on `main`, so a cross-tab that merely echoed
/// either existing breakdown would fail here. Both are asserted still populated:
/// the Statistics tab reads them and must keep working across this change.
#[test]
fn the_cross_tab_splits_each_tool_by_dev_and_main_origin() {
    let mut conn = db::open_in_memory();
    let on = |branch: &str, tool: &str, ok: bool| EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: tool.to_string(),
        duration_ms: 5,
        ok,
        origin: branch.to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            // Dev panes: two branches, three `search` calls (one failed) + `context`.
            on("sprint-64-I2-S2", "search", true),
            on("sprint-64-I2-S2", "search", false),
            on("sprint-64-I1-S3", "search", true),
            on("sprint-64-I1-S3", "context", true),
            // main: one `search` and one `index`.
            record("search", 10, true, NOW - 60),
            record("index", 20, true, NOW - 60),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    // The cross-tab: (tool, origin) cells, tool-then-origin ordered.
    let cells: Vec<(&str, &str, &str, u64, u64)> = info
        .calls_by_tool_origin
        .iter()
        .map(|c| {
            (
                c.tool.as_str(),
                c.origin.as_str(),
                c.class.as_str(),
                c.calls,
                c.ok_calls,
            )
        })
        .collect();
    assert_eq!(
        cells,
        vec![
            ("context", "dev", "navigation", 1, 1),
            ("index", "main", "engine-internal", 1, 1),
            ("search", "dev", "navigation", 3, 2),
            ("search", "main", "navigation", 1, 1),
        ],
        "one cell per (tool, origin), each carrying its class"
    );

    // The question the caveat said was unanswerable, now answerable from the
    // payload alone: navigation issued from dev panes.
    let dev_navigation: u64 = info
        .calls_by_tool_origin
        .iter()
        .filter(|c| c.origin == "dev" && c.class == "navigation")
        .map(|c| c.calls)
        .sum();
    assert_eq!(dev_navigation, 4, "3 search + 1 context, all from worktrees");

    // Both pre-existing shapes survive unchanged in meaning — the Statistics tab
    // reads these and must keep working (FR-OB-11 AC 5).
    let by_surface_tool: Vec<(&str, &str)> = info
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    assert_eq!(
        by_surface_tool,
        vec![
            ("cli", "index"),
            ("cli", "search"),
            ("mcp", "context"),
            ("mcp", "search"),
        ],
        "still surface × tool with no origin — the shape the Statistics tab reads"
    );
    assert_eq!(info.calls_by_origin.len(), 2, "dev + main, as before");
    assert_eq!(info.calls_by_origin[0].origin, "dev");
    assert_eq!(info.calls_by_origin[0].calls, 4);
    assert_eq!(info.calls_by_origin[1].calls, 2);
    assert_eq!(info.calls_total, 6);

    // The cross-tab and the origin split bucket identically — both interpolate
    // `stats::ORIGIN_BUCKET`, and this makes the agreement observable rather
    // than merely structural. Both are raw-events-only over the same rows, so
    // the cross-tab summed by origin must reproduce the split exactly; a bucket
    // rule changed in one place and not the other would show up here.
    for split in &info.calls_by_origin {
        let summed: u64 = info
            .calls_by_tool_origin
            .iter()
            .filter(|c| c.origin == split.origin)
            .map(|c| c.calls)
            .sum();
        assert_eq!(
            summed, split.calls,
            "the cross-tab and calls_by_origin disagree on the {:?} bucket",
            split.origin
        );
    }

    // …and every tool in the existing breakdown now carries a class too, so the
    // label covers the full raw-plus-rollup coverage, not only the cross-tab.
    let classed: Vec<(&str, &str)> = info
        .calls_by_tool
        .iter()
        .map(|u| (u.tool.as_str(), u.class.as_str()))
        .collect();
    assert!(classed.contains(&("search", "navigation")), "got {classed:?}");
    assert!(classed.contains(&("index", "engine-internal")), "got {classed:?}");
    assert!(classed.contains(&("context", "navigation")), "got {classed:?}");
}

/// **Reproducing a hand-written sprint dogfood table requires no manual
/// classification** — the story's closing criterion. `calls_by_class` is that
/// table: class × origin, straight out of `stats`.
///
/// The fixture spans four of the five classes on both sides of the split, so a
/// rollup that lost the class or the origin dimension collapses visibly — and
/// one dev-pane navigation call **fails**, so the rollup's `ok_calls`
/// accumulator is pinned separately from `calls`. Folding `calls` into both (the
/// half-fix S-304's review caught on `watch_coverage_ingest`) would otherwise
/// report a fabricated 100 % success rate forever ([NFR-CC-04]).
#[test]
fn the_class_breakdown_is_the_dogfood_table() {
    let mut conn = db::open_in_memory();
    let on = |branch: &str, tool: &str, ok: bool| EventRecord {
        at: NOW - 60,
        surface: "mcp",
        tool: tool.to_string(),
        duration_ms: 5,
        ok,
        origin: branch.to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            // Dev pane: two navigation calls (one failed), a session gate and a
            // quality gate.
            on("sprint-64-I2-S2", "context", true),
            on("sprint-64-I2-S2", "callers", false),
            on("sprint-64-I2-S2", "session_end", true),
            on("sprint-64-I2-S2", "check_rules", true),
            // main: one navigation call and the indexing that fed it.
            record("search", 10, true, NOW - 60),
            record("sync", 20, true, NOW - 60),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    let table: Vec<(&str, &str, u64, u64)> = info
        .calls_by_class
        .iter()
        .map(|c| (c.class.as_str(), c.origin.as_str(), c.calls, c.ok_calls))
        .collect();
    assert_eq!(
        table,
        vec![
            ("engine-internal", "main", 1, 1),
            // Two dev-pane navigation calls, one of them successful — `ok_calls`
            // accumulates from `ok_calls`, never from `calls`.
            ("navigation", "dev", 2, 1),
            ("navigation", "main", 1, 1),
            ("quality-gate", "dev", 1, 1),
            ("session-gate", "dev", 1, 1),
        ],
        "class × origin with an honest ok-rate, deterministically ordered (NFR-RA-06)"
    );

    // The rollup is exactly the cross-tab's rollup — same rows, same coverage.
    let cross_tab_total: u64 = info.calls_by_tool_origin.iter().map(|c| c.calls).sum();
    let class_total: u64 = info.calls_by_class.iter().map(|c| c.calls).sum();
    assert_eq!(class_total, cross_tab_total, "derived, not separately queried");
    assert_eq!(class_total, 6);
}

/// **The payload states its own coverage limits** ([FR-OB-11], [NFR-CC-04]) —
/// and the raw-events-only claim is *true*, not merely asserted: the fixture
/// puts a rollup day inside the window and shows it counted in `calls_total`
/// while absent from both attribution projections.
///
/// An unlabelled figure is what NFR-CC-04 forbids; a labelled one whose label
/// is wrong is worse, so the claim and the behaviour are checked together.
#[test]
fn the_attribution_projections_state_their_coverage_limits() {
    let mut conn = db::open_in_memory();
    db::write_batch(&mut conn, &[record("search", 10, true, NOW - 60)]).unwrap();
    // A rolled-up day inside the window: no `origin` column, so it can appear in
    // the totals but must not be attributed to either bucket.
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'mcp', 'context', 4, 4, 120, 40)",
        [NOW - 2 * 86_400],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();
    let coverage = &info.attribution_coverage;

    // The claim.
    assert!(coverage.raw_events_only);
    assert!(coverage.legacy_null_origin_folds_into_main);
    assert_eq!(coverage.requested_window_days, 7);
    assert_eq!(coverage.covered_window_days, 7, "7 days is inside retention");
    assert!(!coverage.truncated_by_retention);
    let prose = coverage.notes.join(" ");
    assert!(prose.contains("raw events only"), "got {:?}", coverage.notes);
    assert!(prose.contains("daily_rollup"), "got {:?}", coverage.notes);
    assert!(prose.contains("origin IS NULL"), "got {:?}", coverage.notes);

    // The behaviour the claim describes: the rollup day is in the totals…
    assert_eq!(info.calls_total, 5, "1 raw + 4 rolled up");
    assert!(info.calls_by_tool.iter().any(|u| u.tool == "context"));
    // …and absent from both attribution projections rather than mis-attributed.
    let cross_tab: Vec<&str> = info
        .calls_by_tool_origin
        .iter()
        .map(|c| c.tool.as_str())
        .collect();
    assert_eq!(cross_tab, vec!["search"], "the rolled-up tool is honestly absent");
    let attributed: u64 = info.calls_by_class.iter().map(|c| c.calls).sum();
    assert_eq!(attributed, 1, "attribution covers raw events only");
}

/// When the requested window reaches past raw retention, the payload **names the
/// window the attribution projections actually cover** rather than implying they
/// span the request ([FR-OB-11] AC 2).
#[test]
fn a_window_past_retention_reports_the_window_it_actually_covers() {
    let conn = db::open_in_memory();
    let info = stats_from(&conn, 365, NOW).unwrap();
    let coverage = &info.attribution_coverage;

    assert_eq!(coverage.requested_window_days, 365);
    assert_eq!(
        coverage.covered_window_days,
        db::RETENTION_DAYS,
        "capped at the raw-retention horizon"
    );
    assert!(coverage.truncated_by_retention);
    let prose = coverage.notes.join(" ");
    assert!(
        prose.contains("guaranteed only the most recent 90"),
        "the covered window is named, not merely flagged: {:?}",
        coverage.notes
    );
    assert!(
        prose.contains("reaches past raw retention"),
        "the divergence from the rest of the read-model is stated: {:?}",
        coverage.notes
    );
    // …and named as a floor, because pruning is flush-triggered: a long-lived
    // process may still hold older raw events, so the projections can cover more
    // than the figure claims. Under-stating is the safe direction (NFR-CC-04),
    // but the payload must not present a bound as a measurement.
    assert!(
        prose.contains("treat `covered_window_days` as a floor"),
        "the figure is labelled a floor, not the measured coverage: {:?}",
        coverage.notes
    );

    // A window inside retention says so without the extra caveat.
    let inside = stats_from(&conn, 7, NOW).unwrap().attribution_coverage;
    assert_eq!(inside.notes.len(), 3, "no truncation note: {:?}", inside.notes);
}

/// Legacy `NULL` origins fold into `"main"` in the cross-tab exactly as they do
/// in `calls_by_origin` (FR-OB-08) — the projection never leaks a NULL origin,
/// and `attribution_coverage` says so in the payload.
#[test]
fn the_cross_tab_folds_legacy_null_origins_into_main() {
    let mut conn = db::open_in_memory_v1();
    // Two rows written under the v1 schema, before the `origin` column existed.
    conn.execute(
        "INSERT INTO events (at, surface, tool, duration_ms, ok)
         VALUES (?1, 'cli', 'search', 12, 1), (?1, 'cli', 'search', 8, 1)",
        [NOW - 60],
    )
    .unwrap();
    db::migrate(&mut conn).expect("the forward ledger applies");
    // A post-migration row explicitly stamped "main".
    db::write_batch(&mut conn, &[record("search", 10, true, NOW - 60)]).unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    assert_eq!(info.calls_by_tool_origin.len(), 1, "all fold into one cell");
    assert_eq!(info.calls_by_tool_origin[0].tool, "search");
    assert_eq!(info.calls_by_tool_origin[0].origin, "main");
    assert_eq!(info.calls_by_tool_origin[0].calls, 3, "2 legacy NULL + 1 stamped");
    assert!(
        info.attribution_coverage.legacy_null_origin_folds_into_main,
        "and the payload says the historical main bucket is inflated"
    );
}

/// A tool name today's registry does not know — written by an older build and
/// since retired — is **counted and labelled `unregistered`**, not dropped and
/// not guessed into one of the five classes.
///
/// This is the read-side twin of `an_unregistered_historical_tool_is_still_counted`:
/// history is reported as recorded, and the one thing the read-model cannot know
/// about it (its class) is stated as unknown ([NFR-CC-04]).
#[test]
fn a_retired_tool_name_is_counted_and_labelled_unregistered() {
    let mut conn = db::open_in_memory();
    db::write_batch(
        &mut conn,
        &[
            record("a_tool_from_an_older_build", 10, true, NOW - 60),
            record("search", 10, true, NOW - 60),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    assert_eq!(info.calls_total, 2, "the retired name is counted, not dropped");
    let retired = info
        .calls_by_tool_origin
        .iter()
        .find(|c| c.tool == "a_tool_from_an_older_build")
        .expect("the retired tool has a cross-tab cell");
    assert_eq!(retired.class, UNREGISTERED_CLASS);
    assert_eq!(retired.origin, "main");
    // …and it rolls up under its own label rather than inflating a real class.
    // Asserted as the exact table, so the retired call cannot be silently added
    // to `navigation` — and so its ordering (last, after `session-gate`) is
    // pinned rather than merely its presence.
    let table: Vec<(&str, &str, u64, u64)> = info
        .calls_by_class
        .iter()
        .map(|c| (c.class.as_str(), c.origin.as_str(), c.calls, c.ok_calls))
        .collect();
    assert_eq!(
        table,
        vec![("navigation", "main", 1, 1), (UNREGISTERED_CLASS, "main", 1, 1)]
    );
    assert!(
        info.calls_by_tool
            .iter()
            .any(|u| u.tool == "a_tool_from_an_older_build" && u.class == UNREGISTERED_CLASS),
        "the label reaches the existing shape too: {:?}",
        info.calls_by_tool
    );
}

/// The self-referential exclusion reaches the two new projections too — a leak
/// here would put `stats`/`status` back into the usage figures through a side
/// door, which is the whole defect CR-091 was filed about ([FR-OB-09]).
#[test]
fn the_exclusion_reaches_the_cross_tab_and_the_class_breakdown() {
    let mut conn = db::open_in_memory();
    let noise = |tool: &str| EventRecord {
        at: NOW - 60,
        surface: "web",
        tool: tool.to_string(),
        duration_ms: 5,
        ok: true,
        // A distinct origin, so a leak would add a whole `dev` column.
        origin: "some-worktree-branch".to_string(),
        session_id: "test-session".to_string(),
    };
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, NOW - 60),
            noise("stats"),
            noise("status"),
        ],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).unwrap();

    let tools: Vec<&str> = info
        .calls_by_tool_origin
        .iter()
        .map(|c| c.tool.as_str())
        .collect();
    assert_eq!(tools, vec!["search"], "self-referential rows leaked: {tools:?}");
    let classes: Vec<(&str, &str)> = info
        .calls_by_class
        .iter()
        .map(|c| (c.class.as_str(), c.origin.as_str()))
        .collect();
    assert_eq!(
        classes,
        vec![("navigation", "main")],
        "the read-model class must not appear via the class breakdown"
    );
}

/// **Both degradations state the same limits.** A missing `telemetry.db` and an
/// unreadable one reach the read-model by different paths — `stats()`'s early
/// return and `Engine::stats`'s `unwrap_or_else` — and only the first was
/// wired to `attribution_coverage` at first pass.
///
/// The limits are structural (properties of `daily_rollup`'s schema, not of the
/// data), so they hold on both, and a surface rendering `notes` must not fall
/// silent on exactly the payload least worth trusting ([NFR-CC-04]). This pins
/// the pure function both paths now share, and the `Default` that backstops it.
#[test]
fn a_degraded_read_model_still_states_its_coverage_limits() {
    // The shared pure function: the caller's window is echoed, not zeroed.
    let coverage = super::attribution_coverage(30);
    assert!(coverage.raw_events_only);
    assert!(coverage.legacy_null_origin_folds_into_main);
    assert_eq!(coverage.requested_window_days, 30);
    assert_eq!(coverage.covered_window_days, 30);
    assert_eq!(coverage.notes.len(), 3, "the prose survives degradation");

    // The `Default` backstop: `Engine::stats` builds its fallback with
    // `..StatsInfo::default()`, so a derived `Default` would answer
    // `raw_events_only: false` — a *claim* about the schema, not an absence,
    // which is the failure NFR-CC-04 names. Zeroed windows are correct there
    // (no window was covered); the two structural booleans are not.
    let zeroed = crate::models::quality::AttributionCoverage::default();
    assert!(
        zeroed.raw_events_only && zeroed.legacy_null_origin_folds_into_main,
        "a zeroed coverage struct states an absence, never `raw_events_only: false`"
    );
    assert_eq!((zeroed.requested_window_days, zeroed.covered_window_days), (0, 0));
    assert!(!zeroed.truncated_by_retention);
}
