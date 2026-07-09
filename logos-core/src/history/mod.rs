//! history-engine — the `.logos/history.db` evidence store and its incremental
//! git miner ([history-engine], [ADR-22], [CR-006]).
//!
//! This module owns the **second-store substrate** [S-046] establishes: a
//! separate `history.db` with its own forward-only migration track ([db]), and
//! the incremental, HEAD-anchored `git log --numstat -M -z` miner ([miner]) that
//! populates it. [S-047] adds the per-file temporal metrics ([temporal]) and the
//! append-only snapshot series on the same track; the hotspot surface ([S-048])
//! and the coverage tables ([S-049]) ride it in later iterations.
//!
//! # The non-dependency that defines the tier
//! Mining is **lazy**: [`mine`] is reached only from a temporal-surface read
//! ([S-048]), never from `gate`/`sync`/navigation ([BR-26], [FR-GH-02]). The
//! gated metric path holds no connection to `history.db` and never `ATTACH`-es
//! it, so "the gate cannot see history" is physical, not conventional
//! ([UAT-GH-02]). Nothing in this module is wired into those hot paths.
//!
//! [history-engine]: ../../../docs/specs/architecture/components/history-engine.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [CR-006]: ../../../docs/requests/CR-006-git-history-analytics.md
//! [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [FR-GH-02]: ../../../docs/specs/requirements/FR-GH-02.md
//! [UAT-GH-02]: ../../../docs/specs/requirements/UAT-GH-02.md
//! [S-046]: ../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner
//! [S-047]: ../../../docs/planning/journal.md#s-047-temporal-metrics-co-change-and-defect-heuristic
//! [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
//! [S-049]: ../../../docs/planning/journal.md#s-049-coverage-store-parsers-and-ingest-pipeline

pub mod coverage;
mod db;
mod hotspot;
mod miner;
mod temporal;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::EffectiveHistory;

pub use coverage::{
    coverage_cross, CoverageCrossReport, CoverageFileStatus, CoverageFormat,
    CoverageRefreshSummary, CoverageStatus, CrossSymbol, CrossSymbolInput, CrossTotals,
    IngestSummary, Quadrant, FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE,
};
pub use hotspot::{
    aggregate_complexity, rank, test_only_files, CoverageCell, FileCoverage, Hotspot,
    HotspotReport, COVERAGE_BASIS, DEFECT_LABEL, FIRST_MINE_NOTICE, STATIC_BASIS,
    STATIC_FALLBACK_LABEL, TIER_LABEL,
};
pub use miner::{DegradedReason, MineOutcome};
pub use temporal::{FileTemporal, TemporalReport};

/// `history.db` lives beside `logos.db` and `telemetry.db` under the resolved
/// worktree root ([FR-GH-01]); in a linked worktree that is *that* worktree's
/// `.logos/` ([FR-WT-01]).
const HISTORY_DB_RELPATH: &str = ".logos/history.db";

/// The resolved path of the history store for a worktree `root`.
pub(crate) fn db_path(root: &Path) -> PathBuf {
    root.join(HISTORY_DB_RELPATH)
}

/// Open (creating if absent) and migrate the history store under `root`.
///
/// Creates the `.logos/` parent if needed so a first temporal read on a brand
/// new project still works; the store is on its **own** migration track, never
/// attached to `logos.db` ([FR-GH-01]).
///
/// # Errors
/// Returns an error if the directory or file cannot be created/opened, or a
/// migration fails.
pub(crate) fn open(root: &Path) -> Result<rusqlite::Connection> {
    let path = db_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the history store", parent.display()))?;
    }
    db::open(&path)
}

/// Lazily mine the local git history at `root` into `history.db`, bounded by the
/// HEAD-anchored window in `cfg` ([FR-GH-02], [ADR-22]).
///
/// The single entrypoint the temporal surfaces ([S-048]) call. Incremental:
/// after the first mine only commits since the last mined SHA are read, and a
/// second mine at an unchanged HEAD reads zero. Degraded repository states
/// (non-git, `git` absent, shallow) resolve to a [`MineOutcome`] carrying the
/// reason — never an error ([NFR-RA-05]).
///
/// # Errors
/// Returns an error only on an unexpected git/store failure.
///
/// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub fn mine(root: &Path, cfg: &EffectiveHistory) -> Result<MineOutcome> {
    let mut conn = open(root)?;
    miner::mine_incremental(&mut conn, root, cfg)
}

/// Lazily mine, then compute the per-file temporal report and append a snapshot
/// row to the series ([FR-GH-03]..[FR-GH-05], [FR-GH-09]).
///
/// The temporal-tier entrypoint the surfaces ([S-048]) read through (via
/// [`Engine::temporal_report`](crate::Engine::temporal_report)). It mines
/// incrementally (lazy — only reached on a temporal read, [BR-26]), recomputes
/// the metrics deterministically from the stored facts at the *current* HEAD and
/// config ([BR-27], [NFR-RA-06]), and — for a real evaluation — appends one
/// append-only [`temporal_snapshots`](db) row carrying the mined-through SHA, the
/// local git version, and the effective `[history]` config hash ([FR-GH-09]). A
/// degraded repository (non-git / `git` absent / shallow) yields an `n/a` report
/// and writes no snapshot — never fabricating ([NFR-RA-05]).
///
/// # Errors
/// Returns an error only on an unexpected git/store failure or a non-compiling
/// `defect_patterns` (a `[history]` misconfiguration).
pub fn temporal_report(root: &Path, cfg: &EffectiveHistory) -> Result<TemporalReport> {
    let mut conn = open(root)?;
    let outcome = miner::mine_incremental(&mut conn, root, cfg)?;
    let git_version = db::stored_git_version(&conn)?;
    let (report, snapshot) = temporal::compute_temporal(&conn, cfg, &outcome, &git_version)?;
    if let Some(row) = snapshot {
        db::persist_temporal_snapshot(&conn, &row)?;
    }
    Ok(report)
}

/// The **read-only** temporal report: recompute the per-file temporal metrics
/// from the **already-mined** facts at the stored cursor, **without mining and
/// without appending a snapshot** ([CR-018], [ADR-28], S-082).
///
/// This is the non-persisting twin of [`temporal_report`] the web dashboard's
/// Commits and Hotspots views read through, so a page GET reflects the last
/// `logos hotspots`/`scan` mine and never triggers a new
/// mine-and-persist — the `temporal_snapshots` series (and `commits`/
/// `file_changes`) are left byte-for-byte unchanged. The HEAD is taken from the
/// persisted [`mine_cursor`](db::mine_cursor), never re-resolved from `git`; a
/// never-mined store yields the same empty `n/a` report a degraded repo does
/// ([NFR-RA-05]). Output is the same deterministic function of (stored facts,
/// HEAD timestamp, effective config) as `temporal_report` ([BR-27], [NFR-RA-06]).
///
/// # Errors
/// Returns an error only on an unexpected store read failure or a non-compiling
/// `defect_patterns` — exactly the read-time failure modes of `temporal_report`.
///
/// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
pub fn latest_temporal_report(root: &Path, cfg: &EffectiveHistory) -> Result<TemporalReport> {
    let conn = open(root)?;
    let git_version = db::stored_git_version(&conn)?;
    // The stored cursor stands in for a mine: it pins the last-mined HEAD and
    // mined-through SHA without spawning git or writing a row. A never-mined
    // store has no cursor → a headless outcome → the empty `n/a` report.
    let outcome = match db::mine_cursor(&conn)? {
        Some(cursor) => MineOutcome {
            commits_read: 0,
            first_mine: false,
            head_sha: Some(cursor.head_sha),
            mined_through: Some(cursor.mined_through),
            degraded: None,
        },
        None => MineOutcome {
            commits_read: 0,
            first_mine: false,
            head_sha: None,
            mined_through: None,
            degraded: None,
        },
    };
    // Discard the would-be snapshot row — the read-only path never persists.
    let (report, _snapshot) = temporal::compute_temporal(&conn, cfg, &outcome, &git_version)?;
    Ok(report)
}

/// The history store's current migration version — used by the re-index
/// survival test to assert it is **independent** of `logos.db`'s version
/// ([FR-GH-01]). Test-only for now; [S-047] promotes it when a production
/// reader (e.g. a history-tier health surface) needs it.
///
/// # Errors
/// Returns an error if the store cannot be opened or its version read.
///
/// [S-047]: ../../../docs/planning/journal.md#s-047-temporal-metrics-co-change-and-defect-heuristic
#[cfg(test)]
pub(crate) fn schema_version(root: &Path) -> Result<i64> {
    let conn = open(root)?;
    db::current_version(&conn)
}

/// The newest migration version the embedded history ledger defines — the value
/// a fully migrated `history.db` reports, on its own track ([FR-GH-01]).
/// Test-only for now (see [`schema_version`]).
#[cfg(test)]
pub(crate) fn latest_schema_version() -> i64 {
    db::latest_version()
}
