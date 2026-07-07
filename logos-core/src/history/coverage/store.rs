//! Coverage persistence on the `history.db` track — merge / snapshot semantics
//! ([FR-CV-02], [FR-CV-04], [ADR-23]).
//!
//! The whole ingest write is **one transaction** ([NFR-RA-07] atomic write): a
//! crash mid-ingest leaves the store at its prior coherent state. The migration
//! ledger for these tables lives in [`super::super::db`] (migration 2); this
//! module owns only their DML.
//!
//! The rules ([FR-CV-04]):
//! - **New HEAD** → a new snapshot; prior snapshots are retained as provenance.
//! - **Same HEAD, new report** → merge into the open snapshot: a file already
//!   present whose content hash still matches has its line hits **summed**; a
//!   content-hash mismatch **rejects** that file's entries (the anchor moved —
//!   never apply old line data to a changed file, [FR-CV-05] never-fabricate).
//! - **Same HEAD, same report** (identical `report_hash`) → a no-op: the store is
//!   byte-identical, so a re-ingest is idempotent ([UAT-CV-01]).
//!
//! [FR-CV-02]: ../../../../docs/specs/requirements/FR-CV-02.md
//! [FR-CV-04]: ../../../../docs/specs/requirements/FR-CV-04.md
//! [FR-CV-05]: ../../../../docs/specs/requirements/FR-CV-05.md
//! [UAT-CV-01]: ../../../../docs/specs/requirements/UAT-CV-01.md
//! [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

use super::parse::CoverageFormat;

/// One matched file ready to persist: the repo-relative indexed path it bound to,
/// the content hash anchoring it at ingest, and its deduped line hits (ascending
/// by line number, so the store is byte-identical across runs, [NFR-RA-06]).
pub(crate) struct MatchedFile {
    /// Repo-relative indexed path.
    pub(crate) path: String,
    /// blake3 of the file content at ingest — the freshness anchor ([FR-CV-02]).
    pub(crate) content_hash: String,
    /// `(line_no, hits)` pairs, deduped and ascending.
    pub(crate) lines: Vec<(i64, i64)>,
}

/// What [`persist_ingest`] did, for the ingest summary.
pub(crate) struct PersistOutcome {
    /// The snapshot the ingest wrote into (new or pre-existing).
    pub(crate) snapshot_id: i64,
    /// `true` when a snapshot already existed at this HEAD (a merge, not a fresh
    /// snapshot).
    pub(crate) merged_into_existing: bool,
    /// `true` when this exact report was already in the snapshot — nothing was
    /// written ([UAT-CV-01] idempotency).
    pub(crate) already_ingested: bool,
    /// Files whose stored anchor no longer matched their current content and were
    /// rejected from the merge, sorted ([FR-CV-04] mismatch notice).
    pub(crate) rejected_stale: Vec<String>,
}

/// Persist one ingest under the merge / snapshot rules, atomically.
///
/// # Errors
/// Returns an error only if the transaction cannot be opened or committed; the
/// expected outcomes (new snapshot, merge, idempotent no-op, per-file rejection)
/// are all carried in [`PersistOutcome`].
pub(crate) fn persist_ingest(
    conn: &mut Connection,
    head_sha: &str,
    config_hash: &str,
    report_hash: &str,
    format: CoverageFormat,
    files: &[MatchedFile],
) -> Result<PersistOutcome> {
    let tx = conn
        .transaction()
        .context("opening the coverage ingest transaction")?;

    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM coverage_snapshots WHERE head_sha = ?1",
            [head_sha],
            |row| row.get(0),
        )
        .optional()
        .context("looking up the coverage snapshot for HEAD")?;

    let mut rejected_stale = Vec::new();
    let merged_into_existing = existing.is_some();

    let snapshot_id = match existing {
        Some(id) => id,
        None => {
            tx.execute(
                "INSERT INTO coverage_snapshots (head_sha, config_hash, created_at)
                 VALUES (?1, ?2, unixepoch())",
                rusqlite::params![head_sha, config_hash],
            )
            .context("creating the coverage snapshot")?;
            tx.last_insert_rowid()
        }
    };

    // Idempotency: an identical report already merged into this snapshot is a
    // no-op — the store stays byte-identical ([UAT-CV-01]).
    if merged_into_existing {
        let seen: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM coverage_reports WHERE snapshot_id = ?1 AND report_hash = ?2",
                rusqlite::params![snapshot_id, report_hash],
                |row| row.get(0),
            )
            .optional()
            .context("checking for a duplicate coverage report")?;
        if seen.is_some() {
            tx.commit()
                .context("committing the no-op coverage ingest")?;
            return Ok(PersistOutcome {
                snapshot_id,
                merged_into_existing,
                already_ingested: true,
                rejected_stale,
            });
        }
    }

    tx.execute(
        "INSERT INTO coverage_reports (snapshot_id, report_hash, format, ingested_at)
         VALUES (?1, ?2, ?3, unixepoch())",
        rusqlite::params![snapshot_id, report_hash, format.as_str()],
    )
    .context("recording the coverage report")?;

    for file in files {
        let prior: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, content_hash FROM coverage_files WHERE snapshot_id = ?1 AND path = ?2",
                rusqlite::params![snapshot_id, file.path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("looking up the file anchor in the snapshot")?;

        let file_id = match prior {
            Some((id, anchor)) => {
                if anchor != file.content_hash {
                    // The anchored content changed between ingests — reject this
                    // file's entries rather than apply stale line data to it.
                    rejected_stale.push(file.path.clone());
                    continue;
                }
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO coverage_files (snapshot_id, path, content_hash)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![snapshot_id, file.path, file.content_hash],
                )
                .with_context(|| format!("inserting coverage file {}", file.path))?;
                tx.last_insert_rowid()
            }
        };

        let mut line_stmt = tx
            .prepare_cached(
                "INSERT INTO coverage_lines (file_id, line_no, hits)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (file_id, line_no) DO UPDATE SET hits = hits + excluded.hits",
            )
            .context("preparing the coverage line upsert")?;
        for &(line_no, hits) in &file.lines {
            line_stmt
                .execute(rusqlite::params![file_id, line_no, hits])
                .with_context(|| format!("inserting coverage line {line_no} of {}", file.path))?;
        }
    }

    rejected_stale.sort();
    tx.commit().context("committing the coverage ingest")?;

    Ok(PersistOutcome {
        snapshot_id,
        merged_into_existing,
        already_ingested: false,
        rejected_stale,
    })
}
