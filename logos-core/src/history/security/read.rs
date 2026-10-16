//! The security **read** path — each scanner's latest snapshot with per-file
//! freshness resolved against the working tree ([FR-SC-02], [ADR-79]).
//!
//! The read twin of [`super::store`], and the one reader the later stories
//! consume: the OWASP classifier and the suppression labeller take its
//! [`SecurityFinding`]s as plain values, and the surfaces render them.
//!
//! - **Latest per scanner.** A scanner's current evidence is its most recently
//!   written snapshot (highest id — a replace or a new HEAD always inserts a new
//!   row). A scanner with no snapshot is absent: *never run*, which is distinct
//!   from a snapshot with zero findings (*fresh, 0 findings*).
//! - **Freshness per file, by content hash** ([FR-CV-05] reapplied): a bound
//!   finding is `fresh` while its file's current blake3 equals the anchor written
//!   at ingest and `stale` once the file moved (or vanished); an unbound,
//!   unmatched or ambiguous finding is `n/a` — there is no file to compare.
//!
//! Every read is ordered, so the result is byte-identical across runs for the
//! same store and tree ([NFR-RA-06]).
//!
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-CV-05]: ../../../../docs/specs/requirements/FR-CV-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use super::{FindingBinding, ScannerSnapshot, SecurityFinding, SecurityView, Severity};
use crate::history::{FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE};

/// Read every scanner's latest snapshot under `root`, findings freshness-resolved.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub fn read_latest(root: &Path) -> Result<SecurityView> {
    let conn = crate::history::open(root)?;

    let snapshots: Vec<ScannerSnapshot> = {
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.scanner_name, s.scanner_version, s.head_sha, s.report_hash,
                        (SELECT COUNT(*) FROM security_findings f WHERE f.snapshot_id = s.id)
                   FROM security_snapshots s
                  WHERE s.id = (SELECT MAX(id) FROM security_snapshots
                                 WHERE scanner_name = s.scanner_name)
                  ORDER BY s.scanner_name",
            )
            .context("preparing the latest security snapshot read")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ScannerSnapshot {
                    snapshot_id: row.get(0)?,
                    scanner_name: row.get(1)?,
                    scanner_version: row.get(2)?,
                    head_sha: row.get(3)?,
                    report_hash: row.get(4)?,
                    finding_count: row.get::<_, i64>(5)? as usize,
                })
            })
            .context("reading the latest security snapshots")?;
        rows.collect::<rusqlite::Result<_>>()
            .context("collecting the latest security snapshots")?
    };

    // Each file's current hash is computed once, however many snapshots anchor it.
    let mut current: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut findings = Vec::new();
    for snap in &snapshots {
        let anchors: BTreeMap<String, String> = {
            let mut stmt = conn
                .prepare_cached("SELECT path, content_hash FROM security_files WHERE snapshot_id = ?1")
                .context("preparing the security anchor read")?;
            let rows = stmt
                .query_map([snap.snapshot_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .context("reading the security file anchors")?;
            rows.collect::<rusqlite::Result<_>>()
                .context("collecting the security file anchors")?
        };

        let mut stmt = conn
            .prepare_cached(
                "SELECT fingerprint, rule_id, severity, cwes, message, redacted, binding,
                        report_path, path, start_line, start_column, end_line, end_column
                   FROM security_findings
                  WHERE snapshot_id = ?1
                  ORDER BY path IS NULL, path, start_line, start_column, rule_id, fingerprint",
            )
            .context("preparing the security finding read")?;
        type Row = (
            String,
            String,
            String,
            String,
            String,
            bool,
            String,
            Option<String>,
            Option<String>,
            [Option<i64>; 4],
        );
        let rows: Vec<Row> = stmt
            .query_map([snap.snapshot_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    [row.get(9)?, row.get(10)?, row.get(11)?, row.get(12)?],
                ))
            })
            .context("reading security findings")?
            .collect::<rusqlite::Result<_>>()
            .context("collecting security findings")?;

        for (fingerprint, rule_id, severity, cwes, message, redacted, binding, report_path, path, region) in rows {
            let freshness = match path.as_deref() {
                Some(p) => {
                    let now = current
                        .entry(p.to_string())
                        .or_insert_with(|| crate::history::coverage::content_hash(&root.join(p)));
                    // A file that cannot be read can no longer match its anchor:
                    // stale, never fabricated as fresh.
                    if now.is_some() && now.as_deref() == anchors.get(p).map(String::as_str) {
                        FRESHNESS_FRESH
                    } else {
                        FRESHNESS_STALE
                    }
                }
                None => FRESHNESS_NA,
            };
            findings.push(SecurityFinding {
                scanner_name: snap.scanner_name.clone(),
                scanner_version: snap.scanner_version.clone(),
                head_sha: snap.head_sha.clone(),
                fingerprint,
                severity: Severity::from_str_stored(&severity)
                    .ok_or_else(|| anyhow!("stored severity {severity:?} is not one Logos writes"))?,
                cwes: if cwes.is_empty() {
                    Vec::new()
                } else {
                    cwes.split(',').map(str::to_string).collect()
                },
                rule_id,
                message,
                redacted,
                binding: FindingBinding::from_str_stored(&binding)
                    .ok_or_else(|| anyhow!("stored binding {binding:?} is not one Logos writes"))?,
                report_path,
                path,
                start_line: region[0],
                start_column: region[1],
                end_line: region[2],
                end_column: region[3],
                freshness,
            });
        }
    }

    Ok(SecurityView { snapshots, findings })
}
