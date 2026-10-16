//! Security-finding persistence on the `history.db` track — snapshot semantics
//! ([FR-SC-01], [FR-SC-02], [ADR-79]).
//!
//! The whole ingest write is **one transaction** ([NFR-RA-07]): a crash mid-ingest
//! leaves the store at its prior coherent state. The tables are created by
//! migration 4 in [`super::super::db`]; this module owns only their DML.
//!
//! The rules ([FR-SC-02]):
//! - **New HEAD** → a new snapshot per scanner; prior snapshots are kept.
//! - **Same HEAD, same scanner** → that scanner's snapshot is **replaced**; other
//!   scanners' snapshots at the same HEAD are untouched.
//! - **Same HEAD, byte-identical report** → a no-op: every scanner in the report
//!   already holds a snapshot of this exact report, so nothing is written and the
//!   ingest reports `already_ingested` ([FR-SC-01]).
//!
//! [FR-SC-01]: ../../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

use super::parse::{Region, SecurityFormat};
use super::{FindingBinding, Severity};

/// One scanner's share of a report, ready to persist.
pub(crate) struct ScannerBatch {
    /// `tool.driver.name`.
    pub(crate) name: String,
    /// The recorded scanner version.
    pub(crate) version: String,
    /// `(path, content_hash)` for every file a finding bound to, path-sorted.
    pub(crate) files: Vec<(String, String)>,
    /// The findings, fingerprint-unique within the batch.
    pub(crate) findings: Vec<FindingRow>,
}

/// One finding row. There is no snippet column to fill ([FR-SC-03]).
///
/// [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
pub(crate) struct FindingRow {
    pub(crate) fingerprint: String,
    pub(crate) rule_id: String,
    pub(crate) severity: Severity,
    pub(crate) cwes: Vec<String>,
    pub(crate) message: String,
    pub(crate) redacted: bool,
    pub(crate) binding: FindingBinding,
    pub(crate) report_path: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) region: Region,
}

/// What [`persist_ingest`] did.
pub(crate) struct PersistOutcome {
    /// `true` when the report was already stored at this HEAD — nothing written.
    pub(crate) already_ingested: bool,
    /// Per batch, in batch order: the snapshot id it is stored under, and whether
    /// an earlier snapshot for that scanner at this HEAD was replaced.
    pub(crate) snapshots: Vec<(i64, bool)>,
}

/// Persist one ingest under the snapshot rules, atomically.
///
/// # Errors
/// Returns an error only if a statement fails or the transaction cannot commit.
pub(crate) fn persist_ingest(
    conn: &mut Connection,
    head_sha: &str,
    report_hash: &str,
    format: SecurityFormat,
    batches: &[ScannerBatch],
) -> Result<PersistOutcome> {
    let tx = conn
        .transaction()
        .context("opening the security ingest transaction")?;

    // Per batch: the scanner's snapshot at this HEAD (id, report hash), and
    // whether it is that scanner's latest snapshot — the one the reader serves.
    let mut existing = Vec::with_capacity(batches.len());
    let mut current = Vec::with_capacity(batches.len());
    for batch in batches {
        let row: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, report_hash FROM security_snapshots
                 WHERE head_sha = ?1 AND scanner_name = ?2",
                rusqlite::params![head_sha, batch.name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("looking up the security snapshot for HEAD and scanner")?;
        let latest: Option<i64> = tx
            .query_row(
                "SELECT MAX(id) FROM security_snapshots WHERE scanner_name = ?1",
                [&batch.name],
                |row| row.get(0),
            )
            .context("looking up the scanner's latest security snapshot")?;
        current.push(row.as_ref().is_some_and(|(id, _)| Some(*id) == latest));
        existing.push(row);
    }

    // Idempotency: every scanner in this report already holds a snapshot of this
    // very report at this HEAD — the store stays byte-identical ([FR-SC-01]). It
    // must also be each scanner's latest: re-ingesting an older HEAD's report
    // after a newer HEAD's makes it current again, so that is a rewrite (which
    // re-inserts it as the latest), never a no-op that leaves the newer HEAD's
    // snapshot served for the tree now checked out.
    let already = !batches.is_empty()
        && existing
            .iter()
            .zip(&current)
            .all(|(row, &is_latest)| is_latest && row.as_ref().is_some_and(|(_, hash)| hash == report_hash));
    if already {
        tx.commit()
            .context("committing the no-op security ingest")?;
        return Ok(PersistOutcome {
            already_ingested: true,
            snapshots: existing
                .into_iter()
                .flatten()
                .map(|(id, _)| (id, false))
                .collect(),
        });
    }

    let mut snapshots = Vec::with_capacity(batches.len());
    for (batch, prior) in batches.iter().zip(existing) {
        let replaced = prior.is_some();
        if let Some((id, _)) = prior {
            // Same HEAD, same scanner: replace. The cascade drops its files and
            // findings with it.
            tx.execute("DELETE FROM security_snapshots WHERE id = ?1", [id])
                .context("replacing the scanner's snapshot at this HEAD")?;
        }
        tx.execute(
            "INSERT INTO security_snapshots
                 (head_sha, scanner_name, scanner_version, report_hash, format, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch())",
            rusqlite::params![head_sha, batch.name, batch.version, report_hash, format.as_str()],
        )
        .with_context(|| format!("creating the security snapshot for {}", batch.name))?;
        let snapshot_id = tx.last_insert_rowid();

        {
            let mut file_stmt = tx
                .prepare_cached(
                    "INSERT INTO security_files (snapshot_id, path, content_hash)
                     VALUES (?1, ?2, ?3)",
                )
                .context("preparing the security file insert")?;
            for (path, hash) in &batch.files {
                file_stmt
                    .execute(rusqlite::params![snapshot_id, path, hash])
                    .with_context(|| format!("anchoring {path}"))?;
            }

            let mut finding_stmt = tx
                .prepare_cached(
                    "INSERT INTO security_findings (
                         snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted,
                         binding, report_path, path,
                         start_line, start_column, end_line, end_column
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                )
                .context("preparing the security finding insert")?;
            for f in &batch.findings {
                finding_stmt
                    .execute(rusqlite::params![
                        snapshot_id,
                        f.fingerprint,
                        f.rule_id,
                        f.severity.as_str(),
                        f.cwes.join(","),
                        f.message,
                        f.redacted,
                        f.binding.as_str(),
                        f.report_path,
                        f.path,
                        f.region.start_line,
                        f.region.start_column,
                        f.region.end_line,
                        f.region.end_column,
                    ])
                    .with_context(|| format!("inserting finding {} ({})", f.fingerprint, f.rule_id))?;
            }
        }
        snapshots.push((snapshot_id, replaced));
    }

    tx.commit().context("committing the security ingest")?;
    Ok(PersistOutcome {
        already_ingested: false,
        snapshots,
    })
}
