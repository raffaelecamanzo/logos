//! The `stats` read-models over `telemetry.db` ([FR-OB-04], [NFR-OO-03]).
//!
//! Usage counts come from raw `events` within the window **plus** any
//! `daily_rollup` rows the window reaches back into (so a 365-day window still
//! counts calls whose raw events aged out, [NFR-OO-04]). Latency percentiles
//! are computed from raw events only — rollups deliberately do not carry
//! distribution data, and the raw retention window (90 days) comfortably
//! covers the default 7-day stats window.
//!
//! The daily-activity series folds the same raw-plus-rollup dual source as the
//! usage counts; the dev-vs-`main` split ([FR-OB-08]) collapses every non-`main`
//! origin (each a worktree branch) into a single cumulative `"dev"` bucket and is
//! raw-events-only, since `daily_rollup` carries no `origin` column — an honest
//! omission over a fabricated attribution ([NFR-CC-04]).
//!
//! The headline **tokens-saved figure is an estimate** — the dogfood metric
//! that says whether Logos earns its place ([NFR-OO-03]) — and is honestly
//! labeled as such ([NFR-CC-04]; the constants are SRS OQ-01).
//!
//! **Web-dashboard activity is excluded from every figure.** Each request the
//! `serve --ui` surface answers emits a `surface="web"` telemetry event, so
//! *viewing* the stats would otherwise inflate them — self-referential noise
//! that says nothing about the tool's real value (structural navigation over
//! CLI/MCP). Every query below filters `surface <> 'web'`, so totals, the daily
//! series, the dev-vs-`main` split, latency, and the estimate all reflect genuine tool
//! use only. The `'web'` SQL literal is pinned to [`super::Surface::Web`] by a
//! guard test so an enum rename cannot silently defeat the filter.
//!
//! [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
//! [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
//! [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
//! [NFR-OO-04]: ../../../docs/specs/requirements/NFR-OO-04.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::models::quality::{DailyActivity, OriginUsage, StatsInfo, ToolUsage};

/// Default stats window in days ([FR-OB-04]: "default window 7 days").
pub(crate) const DEFAULT_WINDOW_DAYS: u32 = 7;

/// Aggregate usage/perf stats for the project rooted at `root`.
///
/// The store resolves through [`super::telemetry_logos_dir`], so a linked
/// worktree reads the **primary** repo's shared `.logos/telemetry.db` — the
/// same directory the write path ([`super::init`]) targets ([ADR-50],
/// [FR-OB-07]) — and reports repository-wide usage, not an empty per-worktree
/// store.
///
/// A missing `telemetry.db` (telemetry never ran here) degrades to an empty
/// read-model carrying the reason in `warnings` — the infallible-surface
/// posture, never an error to the caller.
///
/// # Errors
/// Returns an error only on an unreadable/corrupt store; the Engine surface
/// converts that to a warning-carrying default.
///
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-07]: ../../../docs/specs/requirements/FR-OB-07.md
pub(crate) fn stats(root: &Path, window_days: Option<u32>) -> Result<StatsInfo> {
    let window_days = window_days.unwrap_or(DEFAULT_WINDOW_DAYS);
    let db_path = super::telemetry_logos_dir(root).join(super::TELEMETRY_DB_FILENAME);
    if !db_path.is_file() {
        return Ok(StatsInfo {
            window_days,
            warnings: vec!["no telemetry recorded yet (telemetry.db not found)".to_string()],
            ..StatsInfo::default()
        });
    }
    let conn = super::db::open_readonly(&db_path)?;
    stats_from(&conn, window_days, now_unix())
}

/// The computation under [`stats`], on an explicit connection and clock —
/// the testable seam.
pub(crate) fn stats_from(conn: &Connection, window_days: u32, now_unix: i64) -> Result<StatsInfo> {
    let cutoff = now_unix - i64::from(window_days) * 86_400;

    // Usage counts: raw events in the window, plus rollup days the window
    // reaches back into (keyed map so the two sources merge per tool).
    let mut usage: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT surface, tool, count(*), sum(ok)
             FROM events WHERE at >= ?1 AND surface <> 'web' GROUP BY surface, tool",
        )
        .context("preparing the usage query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .context("querying raw usage")?;
    for row in rows {
        let (surface, tool, calls, ok_calls) = row.context("reading a usage row")?;
        let entry = usage.entry((surface, tool)).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }
    let mut stmt = conn
        .prepare(
            "SELECT surface, tool, sum(calls), sum(ok_calls)
             FROM daily_rollup WHERE day >= date(?1, 'unixepoch') AND surface <> 'web'
             GROUP BY surface, tool",
        )
        .context("preparing the rollup usage query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .context("querying rollup usage")?;
    for row in rows {
        let (surface, tool, calls, ok_calls) = row.context("reading a rollup row")?;
        let entry = usage.entry((surface, tool)).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }

    // Latency percentiles over the window's raw durations (nearest-rank).
    let mut stmt = conn
        .prepare(
            "SELECT duration_ms FROM events
             WHERE at >= ?1 AND surface <> 'web' ORDER BY duration_ms",
        )
        .context("preparing the latency query")?;
    let durations: Vec<u64> = stmt
        .query_map([cutoff], |r| r.get::<_, i64>(0))
        .context("querying latencies")?
        .map(|d| d.map(|ms| ms.max(0) as u64))
        .collect::<std::result::Result<_, _>>()
        .context("reading a latency row")?;

    // Daily activity: raw events grouped by UTC day, merged with the rollup
    // days the window reaches back into — the same dual source as usage above,
    // so aged-out days still contribute. The `BTreeMap<day,_>` key is a
    // `'YYYY-MM-DD'` string, whose lexical order is chronological → oldest first.
    let mut by_day: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT date(at, 'unixepoch'), count(*), sum(ok)
             FROM events WHERE at >= ?1 AND surface <> 'web' GROUP BY 1",
        )
        .context("preparing the daily-activity query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .context("querying raw daily activity")?;
    for row in rows {
        let (day, calls, ok_calls) = row.context("reading a daily-activity row")?;
        let entry = by_day.entry(day).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }
    let mut stmt = conn
        .prepare(
            "SELECT day, sum(calls), sum(ok_calls)
             FROM daily_rollup WHERE day >= date(?1, 'unixepoch') AND surface <> 'web'
             GROUP BY day",
        )
        .context("preparing the rollup daily-activity query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .context("querying rollup daily activity")?;
    for row in rows {
        let (day, calls, ok_calls) = row.context("reading a rollup daily-activity row")?;
        let entry = by_day.entry(day).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }
    let activity_by_day: Vec<DailyActivity> = by_day
        .into_iter()
        .map(|(day, (calls, ok_calls))| DailyActivity {
            day,
            calls,
            ok_calls,
        })
        .collect();

    // Dev-vs-`main` split: every non-`main` origin (each a worktree branch) folds
    // into a single cumulative `"dev"` bucket so the card is a two-way comparison —
    // all development-increment work combined vs `main` — not one bar per stale
    // branch. Raw events only: `daily_rollup` carries no `origin`, so a rolled-up
    // day is deliberately absent here rather than mis-attributed (NFR-CC-04).
    // Legacy NULL rows and the primary checkout both fold into `"main"` via COALESCE.
    let mut by_origin: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT CASE WHEN COALESCE(origin, 'main') = 'main' THEN 'main' ELSE 'dev' END,
                    count(*), sum(ok)
             FROM events WHERE at >= ?1 AND surface <> 'web' GROUP BY 1",
        )
        .context("preparing the origin-breakdown query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .context("querying origin breakdown")?;
    for row in rows {
        let (origin, calls, ok_calls) = row.context("reading an origin-breakdown row")?;
        let entry = by_origin.entry(origin).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }
    let calls_by_origin: Vec<OriginUsage> = by_origin
        .into_iter()
        .map(|(origin, (calls, ok_calls))| OriginUsage {
            origin,
            calls,
            ok_calls,
        })
        .collect();

    let calls_by_tool: Vec<ToolUsage> = usage
        .into_iter()
        .map(|((surface, tool), (calls, ok_calls))| ToolUsage {
            surface,
            tool,
            calls,
            ok_calls,
        })
        .collect();
    let calls_total = calls_by_tool.iter().map(|u| u.calls).sum();
    let (reads_saved_estimate, tokens_saved_estimate) = saved_estimates(&calls_by_tool);

    Ok(StatsInfo {
        window_days,
        calls_total,
        calls_by_tool,
        latency_p50_ms: percentile(&durations, 50),
        latency_p95_ms: percentile(&durations, 95),
        latency_p99_ms: percentile(&durations, 99),
        reads_saved_estimate,
        tokens_saved_estimate,
        // Cross-artifact binding counts are a live-graph property merged by
        // `Engine::stats` (CR-011); the telemetry layer leaves them empty.
        artifact_bindings: std::collections::BTreeMap::new(),
        activity_by_day,
        calls_by_origin,
        warnings: Vec::new(),
    })
}

/// Nearest-rank percentile over an ascending-sorted slice (`0` when empty).
fn percentile(sorted: &[u64], pct: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    // ceil(pct/100 * n) as a 1-based rank, clamped into the slice.
    let rank = (pct * sorted.len() as u64).div_ceil(100).max(1) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

// ── The tokens-saved dogfood estimate (NFR-OO-03, SRS OQ-01) ───────────────
//
// OQ-01 resolution (ratified by the maintainer, S-019): per-tool read weights
// — bundle tools (context/explore/impact) replace more ad-hoc exploration than
// point queries — at a net 1,500 tokens per avoided read. Revisit against
// real dogfood data once the MCP surface has been in use (NFR-CC-04).

/// Estimated `(reads_saved, tokens_saved)` across the window's usage.
///
/// **This is an estimate, honestly labeled ([NFR-CC-04]).** The model: each
/// *navigation* call replaces the ad-hoc file reads an agent would otherwise
/// burn exploring the codebase (AS-02, the token-saving thesis); pipeline and
/// bookkeeping calls save nothing.
fn saved_estimates(usage: &[ToolUsage]) -> (u64, u64) {
    let reads_saved: u64 = usage
        .iter()
        .map(|u| u.calls * reads_saved_per_call(&u.tool))
        .sum();
    (reads_saved, reads_saved * TOKENS_PER_AVOIDED_READ)
}

/// Net tokens an avoided ad-hoc file read would have cost.
const TOKENS_PER_AVOIDED_READ: u64 = 1_500;

/// Estimated file reads replaced by one call of `tool` (0 for non-navigation
/// tools — indexing and bookkeeping save nothing by themselves).
fn reads_saved_per_call(tool: &str) -> u64 {
    match tool {
        "context" => 5, // bundle: replaces a whole exploration
        "explore" => 4, // grouped neighbourhood read
        "impact" => 3,  // transitive closure vs. manual chasing
        "search" | "node" | "callers" | "callees" => 2, // point query vs. grep + open
        _ => 0,         // index/sync/stats save nothing
    }
}

/// Seconds since the Unix epoch (0 on a pre-1970 clock — best-effort).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
