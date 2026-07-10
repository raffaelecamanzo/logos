//! The coverage **read** path — the latest snapshot and per-file freshness
//! ([FR-CV-05], [FR-CV-06], [ADR-23]).
//!
//! Where [`super::store`] writes the coverage tables, this module reads them back
//! for the [S-051] surfaces (`coverage status`, the untested-hotspots join). It
//! reads the most-recent snapshot (the highest `coverage_snapshots.id` — the
//! store is append-only and a new HEAD always opens a new row, [FR-CV-04]) and
//! computes **hash-based** freshness per file: a covered file whose *current*
//! content blake3 still equals the anchored [`coverage_files.content_hash`]
//! ([FR-CV-02]) is **fresh** and reports its derived line-coverage; a file whose
//! content moved is **stale** and reports **no** line data — old line numbers are
//! never applied to a shifted tree ([FR-CV-05], [NFR-RA-05]).
//!
//! File-level coverage % is derived on read (never materialised): instrumented =
//! `COUNT(*)`, covered = `COUNT(hits > 0)` over `coverage_lines` ([FR-CV-02]
//! schema note). Reads are ordered by `path` so every surface is byte-identical
//! across runs ([NFR-RA-06]).
//!
//! [FR-CV-02]: ../../../../docs/specs/requirements/FR-CV-02.md
//! [FR-CV-04]: ../../../../docs/specs/requirements/FR-CV-04.md
//! [FR-CV-05]: ../../../../docs/specs/requirements/FR-CV-05.md
//! [FR-CV-06]: ../../../../docs/specs/requirements/FR-CV-06.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [S-051]: ../../../../docs/planning/journal.md#s-051-coverage-surfaces-and-untested-hotspots

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// The latest coverage snapshot resolved against the working tree — the read-side
/// twin of an ingest. Provenance ([FR-CV-06]) plus the per-file freshness rows
/// ([FR-CV-05]), in canonical `path` order.
pub(crate) struct CoverageView {
    /// The ingest-time HEAD SHA the snapshot is anchored to ([FR-CV-02]).
    pub(crate) head_sha: String,
    /// The effective `[coverage]` config hash recorded at ingest ([FR-CV-09]).
    pub(crate) config_hash: String,
    /// Distinct report formats merged into the snapshot, sorted.
    pub(crate) formats: Vec<String>,
    /// Number of reports merged into the snapshot ([FR-CV-06] "report count").
    pub(crate) report_count: usize,
    /// One row per covered file, ordered by `path`.
    pub(crate) files: Vec<CoveredFile>,
}

/// One covered file's freshness-resolved coverage ([FR-CV-05]).
pub(crate) struct CoveredFile {
    /// Repo-relative indexed path the report bound to ([FR-CV-03]).
    pub(crate) path: String,
    /// `true` when the file's current content still matches the anchor; `false`
    /// (**stale**) when it moved — stale files carry no line data ([FR-CV-05]).
    pub(crate) fresh: bool,
    /// Instrumented line count (`COUNT(*)`), `0` when stale (never rendered).
    pub(crate) instrumented_lines: i64,
    /// Covered line count (`COUNT(hits > 0)`), `0` when stale.
    pub(crate) covered_lines: i64,
}

impl CoveredFile {
    /// Line coverage in basis points (0–10000), or `None` when the file is stale
    /// — stale coverage is a label, never a (shifted) number ([FR-CV-05]).
    /// Rounds to the nearest `bp` ([ADR-08]); a file with zero instrumented lines
    /// reports `0`.
    pub(crate) fn coverage_bp(&self) -> Option<i64> {
        if !self.fresh {
            return None;
        }
        if self.instrumented_lines <= 0 {
            return Some(0);
        }
        // Nearest-bp rounding on integers (deterministic across targets, ADR-08).
        Some(
            (self.covered_lines * super::BP_SCALE + self.instrumented_lines / 2)
                / self.instrumented_lines,
        )
    }
}

/// The latest coverage snapshot's metadata plus its covered-file rows with
/// freshness resolved against the current tree — the shared preamble of
/// [`read_latest`]. Opening the store migrates it
/// on the `history.db` track; `logos.db` is never touched ([BR-28]).
struct LatestSnapshot {
    /// The open `history.db` connection (the callers read more from it).
    conn: Connection,
    /// The newest snapshot's id ([FR-CV-04]).
    snapshot_id: i64,
    /// The ingest-time HEAD SHA the snapshot is anchored to ([FR-CV-02]).
    head_sha: String,
    /// The effective `[coverage]` config hash recorded at ingest ([FR-CV-09]).
    config_hash: String,
    /// `(file_id, path, fresh)` per covered file in `path` order; `fresh` is the
    /// hash-based freshness verdict ([FR-CV-05]).
    files: Vec<(i64, String, bool)>,
}

/// Open the store and read the most-recent coverage snapshot's metadata + its
/// covered-file rows with freshness resolved ([FR-CV-04], [FR-CV-05]). `Ok(None)`
/// when no coverage has ever been ingested (no snapshot row).
fn latest_snapshot(root: &Path) -> Result<Option<LatestSnapshot>> {
    let conn = super::super::open(root)?;
    // The newest snapshot: the store is append-only and a new HEAD always opens a
    // higher id, so MAX(id) is the most-recently-ingested snapshot ([FR-CV-04]).
    let snapshot: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT id, head_sha, config_hash FROM coverage_snapshots
             ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .context("reading the latest coverage snapshot")?;
    let Some((snapshot_id, head_sha, config_hash)) = snapshot else {
        return Ok(None);
    };

    let files = {
        let mut file_stmt = conn
            .prepare(
                "SELECT id, path, content_hash FROM coverage_files
                 WHERE snapshot_id = ?1 ORDER BY path",
            )
            .context("preparing the coverage file read")?;
        let rows: Vec<(i64, String, String)> = file_stmt
            .query_map([snapshot_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .context("reading covered files")?
            .collect::<rusqlite::Result<_>>()
            .context("collecting covered files")?;
        rows.into_iter()
            .map(|(file_id, path, anchor)| {
                // Hash-based freshness: the file's CURRENT content vs the anchor
                // ([FR-CV-05]). A file that cannot be read on disk can no longer
                // match its anchor → stale, never fabricated as fresh ([NFR-RA-05]).
                let fresh =
                    super::content_hash(&root.join(&path)).as_deref() == Some(anchor.as_str());
                (file_id, path, fresh)
            })
            .collect()
    };

    Ok(Some(LatestSnapshot {
        conn,
        snapshot_id,
        head_sha,
        config_hash,
        files,
    }))
}

/// Read the most-recent coverage snapshot under `root` and resolve each covered
/// file's freshness against the current tree ([FR-CV-05]).
///
/// Returns `Ok(None)` when no coverage has ever been ingested (no snapshot row) —
/// the surfaces turn that into an `n/a` + notice ([FR-CV-06]). Opening the store
/// migrates it to the latest version on the `history.db` track; `logos.db` is
/// never touched ([BR-28]).
///
/// # Errors
/// Returns an error only on an unexpected store failure.
///
/// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
pub(crate) fn read_latest(root: &Path) -> Result<Option<CoverageView>> {
    let Some(LatestSnapshot {
        conn,
        snapshot_id,
        head_sha,
        config_hash,
        files: file_rows,
    }) = latest_snapshot(root)?
    else {
        return Ok(None);
    };

    let mut format_stmt = conn
        .prepare(
            "SELECT DISTINCT format FROM coverage_reports WHERE snapshot_id = ?1 ORDER BY format",
        )
        .context("preparing the coverage format read")?;
    let formats: Vec<String> = format_stmt
        .query_map([snapshot_id], |row| row.get::<_, String>(0))
        .context("reading coverage report formats")?
        .collect::<rusqlite::Result<_>>()
        .context("collecting coverage report formats")?;

    let report_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM coverage_reports WHERE snapshot_id = ?1",
            [snapshot_id],
            |row| row.get(0),
        )
        .context("counting coverage reports")?;

    let mut files = Vec::with_capacity(file_rows.len());
    for (file_id, path, fresh) in file_rows {
        let (instrumented_lines, covered_lines) = if fresh {
            line_counts(&conn, file_id)?
        } else {
            // Stale: do NOT read line data — it would be applied to a shifted tree.
            (0, 0)
        };
        files.push(CoveredFile {
            path,
            fresh,
            instrumented_lines,
            covered_lines,
        });
    }

    Ok(Some(CoverageView {
        head_sha,
        config_hash,
        formats,
        report_count: report_count as usize,
        files,
    }))
}

/// The **overall line-coverage aggregate** ([FR-CV-06], [CR-021]): covered ÷
/// instrumented lines summed over the **fresh** files only, in basis points
/// (0–10000), nearest-`bp` rounded on integers ([ADR-08]). Stale files carry no
/// line data ([FR-CV-05]) so they never contribute to either total.
///
/// Returns `None` (`n/a`) when no fresh covered lines exist — an all-stale or
/// no-covered snapshot has nothing to aggregate ([FR-CV-06]). Because a file's
/// covered lines never exceed its instrumented lines, a positive covered total
/// implies a positive instrumented total, so the division is panic-proof. The
/// figure is raw — never graded, no thresholds ([BR-28], [NFR-CC-04]).
///
/// [FR-CV-06]: ../../../../docs/specs/requirements/FR-CV-06.md
/// [CR-021]: ../../../../docs/requests/CR-021-dashboard-redesign-quality-coverage-rollups.md
/// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
pub(crate) fn overall_coverage_bp(files: &[CoveredFile]) -> Option<i64> {
    let mut instrumented_total: i64 = 0;
    let mut covered_total: i64 = 0;
    for f in files.iter().filter(|f| f.fresh) {
        instrumented_total += f.instrumented_lines;
        covered_total += f.covered_lines;
    }
    if covered_total <= 0 {
        return None;
    }
    // Nearest-bp rounding on integers (deterministic across targets, ADR-08).
    Some((covered_total * super::BP_SCALE + instrumented_total / 2) / instrumented_total)
}

/// `(instrumented, covered)` line counts for one covered file, derived on read
/// ([FR-CV-02]): instrumented = `COUNT(*)`, covered = `COUNT(hits > 0)`.
fn line_counts(conn: &Connection, file_id: i64) -> Result<(i64, i64)> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(hits > 0), 0) FROM coverage_lines WHERE file_id = ?1",
        [file_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .context("deriving file-level coverage counts")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(fresh: bool, instrumented: i64, covered: i64) -> CoveredFile {
        CoveredFile {
            path: "x".into(),
            fresh,
            instrumented_lines: instrumented,
            covered_lines: covered,
        }
    }

    /// Coverage % is nearest-bp rounded on integers ([ADR-08]) and a stale file
    /// reports no number ([FR-CV-05]).
    #[test]
    fn coverage_bp_rounds_to_nearest_and_stale_is_none() {
        assert_eq!(
            covered(true, 4, 3).coverage_bp(),
            Some(7500),
            "3/4 = 75.00%"
        );
        // 2/3 = 66.666…% → 6667 bp (nearest).
        assert_eq!(covered(true, 3, 2).coverage_bp(), Some(6667));
        assert_eq!(
            covered(true, 1, 0).coverage_bp(),
            Some(0),
            "uncovered fresh file"
        );
        assert_eq!(
            covered(true, 0, 0).coverage_bp(),
            Some(0),
            "no instrumented lines"
        );
        assert_eq!(
            covered(false, 4, 3).coverage_bp(),
            None,
            "stale coverage is a label, never a number"
        );
    }

    /// The overall aggregate sums covered ÷ instrumented over the fresh files and
    /// nearest-bp rounds ([FR-CV-06], [ADR-08]); stale files contribute nothing.
    #[test]
    fn overall_aggregate_sums_fresh_files_and_rounds() {
        // 3/4 and 3/6 fresh → (3+3)/(4+6) = 6/10 = 60.00%.
        let files = vec![covered(true, 4, 3), covered(true, 6, 3)];
        assert_eq!(overall_coverage_bp(&files), Some(6000));

        // A stale file (no line data) never contributes to either total.
        let mixed = vec![
            covered(true, 4, 4),
            CoveredFile {
                path: "stale".into(),
                fresh: false,
                instrumented_lines: 0,
                covered_lines: 0,
            },
        ];
        assert_eq!(
            overall_coverage_bp(&mixed),
            Some(10_000),
            "4/4 fresh = 100%, the stale row is ignored"
        );

        // 2/3 = 66.666…% → 6667 bp (nearest).
        assert_eq!(overall_coverage_bp(&[covered(true, 3, 2)]), Some(6667));
    }

    /// `n/a` (`None`) when no fresh covered lines exist: an empty snapshot, an
    /// all-stale snapshot, or fresh-but-uncovered files ([FR-CV-06]).
    #[test]
    fn overall_aggregate_is_na_without_fresh_covered_lines() {
        assert_eq!(overall_coverage_bp(&[]), None, "no files at all");
        assert_eq!(
            overall_coverage_bp(&[covered(false, 4, 3)]),
            None,
            "all files stale — nothing fresh to aggregate"
        );
        assert_eq!(
            overall_coverage_bp(&[covered(true, 10, 0)]),
            None,
            "fresh but zero covered lines — no fresh covered lines exist"
        );
    }
}
