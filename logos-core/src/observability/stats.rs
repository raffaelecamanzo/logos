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
//! # The attribution projections ([FR-OB-11])
//!
//! `calls_by_tool` groups by surface and tool with no origin; `calls_by_origin`
//! collapses origin to two buckets with no tool. Neither can answer *"which
//! navigation came from dev panes?"* — a caveat recorded verbatim in three
//! consecutive sprint records. Both columns already sit on the same `events` row,
//! so `calls_by_tool_origin` is one more `GROUP BY` over the rows already there,
//! not a capture change; `calls_by_class` rolls it up to the five-way
//! [`tool::ToolClass`] so a sprint dogfood table needs no manual classification.
//!
//! Both inherit `calls_by_origin`'s coverage: **raw events only** (no `origin` on
//! a rollup row) and legacy `NULL` origins folded into `"main"`. Those limits are
//! stated *in the payload* — `attribution_coverage` — rather than in
//! documentation, because an unlabelled figure is what [NFR-CC-04] forbids. The
//! per-tool `class` label rides the existing raw-plus-rollup `calls_by_tool`
//! counts instead, so **every** tool the read-model reports carries a class.
//!
//! The headline **tokens-saved figure is an estimate** — the dogfood metric
//! that says whether Logos earns its place ([NFR-OO-03]) — and is honestly
//! labeled as such ([NFR-CC-04]; the constants are SRS OQ-01).
//!
//! **Self-referential requests are excluded from every figure, on every
//! surface** ([FR-OB-09]). A request whose subject is Logos's own state — the
//! telemetry read-model itself, the shell's graph-state readout — says nothing
//! about the tool's value, and counting it means the measurement observes
//! itself. Every query below carries the [`tool::engine_query_predicate`]
//! fragment, so totals, the daily series, the dev-vs-`main` split, latency and
//! the estimate all reflect genuine tool use only.
//!
//! This replaces a blanket `surface <> 'web'` filter. That filter was written to
//! stop *viewing* the Statistics tab inflating the Statistics tab, which is
//! right in intent — but `surface` is stamped once per process
//! ([FR-OB-03]), so it could not tell a dashboard render from a graph query
//! issued by the same `serve --ui` process. It therefore discarded all SPA
//! navigation and every in-process chat-agent tool call as collateral, and the
//! remainder was reported as adoption ([CR-091]).
//!
//! **The exclusion is derived, not stored.** The predicate is built in Rust from
//! [`super::Tool`]'s exhaustive classification, so a new tool cannot escape it
//! and a per-row class column is unnecessary. That is also what makes the
//! correction retroactive: rows written *before* this classification existed are
//! filtered by the same rule as rows written after, which is the only way
//! re-running a historical window can report the navigation the raw store
//! actually holds. A stored per-event class could not have done that — and
//! `daily_rollup` is keyed `(day, surface, tool)`, so a class that is not a
//! function of `tool` could not survive the rollup either.
//!
//! [CR-091]: ../../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
//! [FR-OB-03]: ../../../docs/specs/requirements/FR-OB-03.md
//! [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
//!
//! [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
//! [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
//! [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
//! [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
//! [NFR-OO-04]: ../../../docs/specs/requirements/NFR-OO-04.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::tool;
use crate::models::quality::{
    AttributionCoverage, ClassUsage, DailyActivity, OriginUsage, StatsInfo, ToolOriginUsage,
    ToolUsage,
};

/// Default stats window in days ([FR-OB-04]: "default window 7 days").
pub(crate) const DEFAULT_WINDOW_DAYS: u32 = 7;

/// The dev-vs-`main` bucket, as a SQL expression over a row's `origin`
/// ([FR-OB-08]).
///
/// Defined once and interpolated into both projections that use it — the origin
/// split and the [FR-OB-11] cross-tab — for the same reason
/// [`tool::engine_query_predicate`] is: two copies of a bucketing rule can be
/// changed one at a time, and `calls_by_origin` and `calls_by_class` are
/// documented as covering the same rows under the same rule. A literal, never
/// user input, so interpolation carries no injection surface.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
const ORIGIN_BUCKET: &str =
    "CASE WHEN COALESCE(origin, 'main') = 'main' THEN 'main' ELSE 'dev' END";

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
            // Empty figures still carry their coverage labels: a consumer
            // rendering the (absent) cross-tab reads the same limits it would on
            // a populated store, rather than a silently zeroed struct.
            attribution_coverage: attribution_coverage(window_days),
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
    // The self-referential exclusion ([FR-OB-09]), derived once from the
    // exhaustive tool classification and interpolated into every query below —
    // raw events and rolled-up days alike, so the two sources can never apply
    // different rules to the same tool.
    let engine_query = tool::engine_query_predicate();

    // Usage counts: raw events in the window, plus rollup days the window
    // reaches back into (keyed map so the two sources merge per tool).
    let mut usage: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT surface, tool, count(*), sum(ok)
             FROM events WHERE at >= ?1 AND {engine_query} GROUP BY surface, tool",
        ))
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
        .prepare(&format!(
            "SELECT surface, tool, sum(calls), sum(ok_calls)
             FROM daily_rollup WHERE day >= date(?1, 'unixepoch') AND {engine_query}
             GROUP BY surface, tool",
        ))
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
        .prepare(&format!(
            "SELECT duration_ms FROM events
             WHERE at >= ?1 AND {engine_query} ORDER BY duration_ms",
        ))
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
        .prepare(&format!(
            "SELECT date(at, 'unixepoch'), count(*), sum(ok)
             FROM events WHERE at >= ?1 AND {engine_query} GROUP BY 1",
        ))
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
        .prepare(&format!(
            "SELECT day, sum(calls), sum(ok_calls)
             FROM daily_rollup WHERE day >= date(?1, 'unixepoch') AND {engine_query}
             GROUP BY day",
        ))
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
        .prepare(&format!(
            "SELECT {ORIGIN_BUCKET}, count(*), sum(ok)
             FROM events WHERE at >= ?1 AND {engine_query} GROUP BY 1",
        ))
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

    // Tool × origin cross-tab ([FR-OB-11]): the projection neither existing
    // breakdown can produce — `calls_by_tool` groups by surface and tool with no
    // origin, `calls_by_origin` by origin with no tool. Both columns already sit
    // on the same `events` row ([FR-OB-08]), so this is one more aggregation over
    // the rows already there, not a capture change.
    //
    // Raw events only and legacy NULLs folded into `main` — the *same*
    // `ORIGIN_BUCKET` expression as the origin split above, so the two can never
    // disagree; `attribution_coverage` below states both limits in the payload
    // ([NFR-CC-04]). Keyed `(tool, origin)` so the order is tool-then-origin with
    // `"dev"` before `"main"` ([NFR-RA-06]).
    let mut by_tool_origin: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT tool, {ORIGIN_BUCKET}, count(*), sum(ok)
             FROM events WHERE at >= ?1 AND {engine_query} GROUP BY 1, 2",
        ))
        .context("preparing the tool-by-origin cross-tab query")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .context("querying the tool-by-origin cross-tab")?;
    for row in rows {
        let (tool, origin, calls, ok_calls) = row.context("reading a cross-tab row")?;
        let entry = by_tool_origin.entry((tool, origin)).or_default();
        entry.0 += calls.max(0) as u64;
        entry.1 += ok_calls.max(0) as u64;
    }

    let calls_by_tool_origin: Vec<ToolOriginUsage> = by_tool_origin
        .into_iter()
        .map(|((tool, origin), (calls, ok_calls))| ToolOriginUsage {
            class: tool::class_of_wire(&tool).to_string(),
            tool,
            origin,
            calls,
            ok_calls,
        })
        .collect();

    // The class × origin rollup — the dogfood table. Folded in Rust from the
    // cross-tab rather than queried again, so the two cannot disagree and it
    // inherits exactly the cross-tab's coverage.
    let mut by_class_origin: BTreeMap<(&str, &str), (u64, u64)> = BTreeMap::new();
    for cell in &calls_by_tool_origin {
        let entry = by_class_origin
            .entry((cell.class.as_str(), cell.origin.as_str()))
            .or_default();
        entry.0 += cell.calls;
        entry.1 += cell.ok_calls;
    }
    let calls_by_class: Vec<ClassUsage> = by_class_origin
        .into_iter()
        .map(|((class, origin), (calls, ok_calls))| ClassUsage {
            class: class.to_string(),
            origin: origin.to_string(),
            calls,
            ok_calls,
        })
        .collect();

    let calls_by_tool: Vec<ToolUsage> = usage
        .into_iter()
        .map(|((surface, tool), (calls, ok_calls))| ToolUsage {
            class: tool::class_of_wire(&tool).to_string(),
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
        calls_by_tool_origin,
        calls_by_class,
        attribution_coverage: attribution_coverage(window_days),
        warnings: Vec::new(),
    })
}

/// What the two attribution projections cover, stated in the payload
/// ([FR-OB-11], [NFR-CC-04]).
///
/// The limits are structural, not data-dependent — they follow from
/// `daily_rollup`'s schema and the [FR-OB-08] migration — so this is a pure
/// function of the requested window rather than something measured per query.
///
/// `covered_window_days` is therefore a **guaranteed floor, not a measurement**.
/// Pruning is flush-triggered ([`super::db::rollup_and_prune`] runs from the
/// writer, not from a clock), so a long-lived process can still hold raw events
/// older than the retention horizon and the projections then cover more than
/// this claims. Under-stating is the safe direction ([NFR-CC-04]) and measuring
/// instead would be worse: `MIN(at)` on a three-day-old store would report
/// "covers 3 of 7 days", conflating how much history exists with how much of the
/// window these projections are able to see.
///
/// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn attribution_coverage(window_days: u32) -> AttributionCoverage {
    let covered_window_days = window_days.min(super::db::RETENTION_DAYS);
    let truncated_by_retention = covered_window_days < window_days;
    let mut notes = vec![
        "`calls_by_tool_origin` and `calls_by_class` are computed from raw events only: \
         `daily_rollup` is keyed (day, surface, tool) and carries no origin column, so a \
         rolled-up day is absent from both rather than mis-attributed. `calls_by_origin` \
         shares this limit; the totals, `calls_by_tool` and `activity_by_day` do not."
            .to_string(),
        format!(
            "Raw events are retained for about {} days, so these projections are guaranteed \
             to cover only the most recent {} days of any window.",
            super::db::RETENTION_DAYS,
            super::db::RETENTION_DAYS,
        ),
        "Events written before the origin stamp existed have origin IS NULL and fold into \
         \"main\", which inflates the historical \"main\" bucket."
            .to_string(),
    ];
    if truncated_by_retention {
        notes.push(format!(
            "The requested window reaches past raw retention: `calls_total`, `calls_by_tool` \
             and `activity_by_day` cover all {window_days} days, while the raw-events-only \
             projections are guaranteed only the most recent {covered_window_days}. They may \
             cover more — pruning is flush-triggered, not time-driven — so treat \
             `covered_window_days` as a floor.",
        ));
    }
    AttributionCoverage {
        raw_events_only: true,
        requested_window_days: window_days,
        covered_window_days,
        truncated_by_retention,
        legacy_null_origin_folds_into_main: true,
        notes,
    }
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
///
/// This weight table is where the tool distinction used to live *only*, which is
/// what [FR-OB-11] promotes onto the read-model as [`tool::ToolClass`]. The two
/// stay deliberately separate: the class says what kind of work a call was, the
/// weights say how much reading one of them replaces, and those are different
/// numbers (an `implements` call is navigation but carries no ratified weight).
/// The one binding between them is directional and asserted by
/// `every_weighted_tool_is_classified_navigation`: a non-zero weight implies
/// [`tool::ToolClass::Navigation`]. The weights themselves are ratified constants
/// (SRS OQ-01) and are unchanged by that promotion.
///
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
pub(super) fn reads_saved_per_call(tool: &str) -> u64 {
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
