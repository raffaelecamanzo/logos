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
use super::stats::stats_from;
use super::{
    db, telemetry_logos_dir, telemetry_origin, traced, EventRecord, Surface, TELEMETRY_TARGET,
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
    assert_eq!(versions, 2, "each ledger migration recorded exactly once");
    // The v2 `origin` column is present on the events table.
    let has_origin: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('events') WHERE name = 'origin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_origin, 1, "v2 added the nullable origin column");
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

    let (tool, duration_ms, ok, surface, origin): (String, i64, i64, String, String) = conn
        .query_row(
            "SELECT tool, duration_ms, ok, surface, origin FROM events WHERE tool = 'index'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (tool.as_str(), duration_ms, ok, surface.as_str(), origin.as_str()),
        ("index", 900, 0, "cli", "main")
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
    db::migrate(&mut conn).expect("v2 migration applies over a v1 store");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2, "the store advanced to v2");

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
        .with(TelemetryLayer::new(Surface::Mcp, "feature".to_string(), sink));

    tracing::subscriber::with_default(subscriber, || {
        let ok = traced("callers", || Ok(42)).unwrap();
        assert_eq!(ok, 42, "traced is transparent to the wrapped result");
        let err = traced("impact", || Err::<(), _>(anyhow::anyhow!("boom")));
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

/// The S-022 watcher attribution: a telemetry event carrying the sanctioned
/// `surface = "watcher"` override records under that surface (the watcher
/// runs *inside* the `serve --mcp` process, whose default is `mcp`), while
/// an unsanctioned override value is ignored.
#[test]
fn watcher_surface_override_is_honoured_and_bounded() {
    let (sink, rx) = TelemetrySink::with_capacity(4);
    let subscriber = tracing_subscriber::registry()
        .with(TelemetryLayer::new(Surface::Mcp, "feature".to_string(), sink));
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
        .with(TelemetryLayer::new(Surface::Cli, "main".to_string(), sink));
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

/// Web-dashboard activity (`surface="web"`) is excluded from **every** figure —
/// totals, per-tool usage, the daily series, the origin split, latency, and the
/// saved estimate (HF-1). Viewing the stats emits `surface="web"` events, so
/// counting them would be self-referential noise. The web rows are seeded with
/// large durations, a navigation tool, and a distinct origin so that a dropped
/// filter on *any* query would visibly leak; a non-web (`mcp`) rollup row on the
/// same day proves the exclusion is web-specific, not a blanket rollup drop.
#[test]
fn web_surface_activity_is_excluded_from_all_stats() {
    // The SQL filter pins the literal `'web'` to the enum — guard against drift.
    assert_eq!(Surface::Web.as_str(), "web");

    let mut conn = db::open_in_memory();
    // Real tool use: 3 cli `search` calls, origin "main".
    db::write_batch(
        &mut conn,
        &[
            record("search", 10, true, NOW - 60),
            record("search", 20, true, NOW - 60),
            record("search", 30, true, NOW - 60),
        ],
    )
    .unwrap();
    // Dashboard noise: 4 web `context` calls (5 reads each if leaked), a huge
    // duration (would dominate percentiles), and a web-only origin (would add a
    // group). Every one must be excluded.
    let web: Vec<EventRecord> = (0..4)
        .map(|_| EventRecord {
            at: NOW - 60,
            surface: "web",
            tool: "context".to_string(),
            duration_ms: 5_000,
            ok: true,
            origin: "web-only".to_string(),
        })
        .collect();
    db::write_batch(&mut conn, &web).unwrap();
    // A rollup day in the window: a web row (excluded) beside an mcp row (kept),
    // so the daily series and usage counts see only the mcp contribution.
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'web', 'impact', 9, 9, 900, 500)",
        [NOW - 86_400],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO daily_rollup (day, surface, tool, calls, ok_calls,
                                   total_duration_ms, max_duration_ms)
         VALUES (date(?1, 'unixepoch'), 'mcp', 'node', 2, 2, 40, 30)",
        [NOW - 86_400],
    )
    .unwrap();

    let info = stats_from(&conn, 7, NOW).expect("stats compute");

    // Totals & per-tool: cli search (3) + mcp node rollup (2) only.
    assert_eq!(info.calls_total, 5, "web raw (4) and web rollup (9) excluded");
    assert!(
        info.calls_by_tool.iter().all(|u| u.surface != "web"),
        "no web surface in the per-tool breakdown: {:?}",
        info.calls_by_tool
    );
    assert!(
        info.calls_by_tool
            .iter()
            .all(|u| u.tool != "context" && u.tool != "impact"),
        "web-only tools never appear"
    );
    // Origin split: only the cli "main" events; the web-only origin is gone.
    assert_eq!(info.calls_by_origin.len(), 1);
    assert_eq!(info.calls_by_origin[0].origin, "main");
    assert_eq!(info.calls_by_origin[0].calls, 3);
    // Daily series: raw cli day (3) + rollup mcp day (2); no web anywhere.
    let day_total: u64 = info.activity_by_day.iter().map(|d| d.calls).sum();
    assert_eq!(info.activity_by_day.len(), 2, "two distinct days");
    assert_eq!(day_total, 5, "web calls excluded from the daily series");
    // Latency: only the cli durations (10/20/30); the 5000 ms web calls are gone.
    assert!(info.latency_p99_ms <= 30, "web latency excluded");
    // Estimate: search 3×2 + node 2×2 = 10 reads; web `context` (4×5) excluded.
    assert_eq!(info.reads_saved_estimate, 10);
    assert_eq!(info.tokens_saved_estimate, 10 * 1_500);
    assert!(info.warnings.is_empty());
}

/// Events outside the window are excluded from counts, percentiles, **and the
/// S-233 additive fields** — the daily series and origin split carry their own
/// `WHERE at >= ?1` / `day >= date(cutoff)` predicates, so this locks each
/// against a dropped window filter (a distinct out-of-window origin and an
/// out-of-window rollup day would both leak otherwise).
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
    db::migrate(&mut conn).expect("v2 migration");
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
