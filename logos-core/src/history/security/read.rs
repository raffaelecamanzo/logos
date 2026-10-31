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
//!   unmatched, ambiguous or historical finding is `n/a` — there is no file to
//!   compare.
//!
//! - **One advisory, one finding** ([FR-SC-29]): the findings several dependency
//!   scanners — or one, under two ids — report for one advisory on one
//!   lockfile, package and version are read as one, naming every reporter
//!   ([`super::merge`]). The rows stay stored per scanner.
//!
//! Every read is ordered, so the result is byte-identical across runs for the
//! same store and tree ([NFR-RA-06]).
//!
//! [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-CV-05]: ../../../../docs/specs/requirements/FR-CV-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};

use rusqlite::OptionalExtension;

use super::{CommitAnchor, ScannerSnapshot, SecurityBaseline, SecurityFinding, SecurityView};
use crate::history::{FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE};

/// Read every scanner's latest snapshot under `root`, findings freshness-resolved.
///
/// With no `history.db` at all nothing was ever stored, so the view is empty
/// and no store is created: a read writes nothing ([ADR-28]). The workspace
/// security roll-up reads every member this way, never-scanned ones included.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
///
/// [ADR-28]: ../../../../docs/specs/architecture/decisions/ADR-28.md
pub fn read_latest(root: &Path) -> Result<SecurityView> {
    read_where(root, LATEST, None, true)
}

/// The finding in the latest snapshots whose fingerprint is `fingerprint`, or
/// that had it before a re-key (its legacy fingerprints), or a merged one with
/// a reporter known by it ([`SecurityFinding::known_by`]); the first in store
/// order. `None` when no stored finding has it.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub fn stored_finding(root: &Path, fingerprint: &str) -> Result<Option<SecurityFinding>> {
    Ok(read_latest(root)?
        .findings
        .into_iter()
        .find(|f| f.known_by(fingerprint)))
}

/// Every scanner's latest snapshot as [`read_latest`] reads it, without its
/// findings: no file is hashed.
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub fn latest_snapshots(root: &Path) -> Result<Vec<ScannerSnapshot>> {
    Ok(read_where(root, LATEST, None, false)?.snapshots)
}

/// The condition selecting each scanner's most recently written snapshot.
const LATEST: &str = "s.id = (SELECT MAX(id) FROM security_snapshots WHERE scanner_name = s.scanner_name)";

/// Read every scanner's snapshot anchored to `commit` — at most one per scanner
/// (a re-ingest at one HEAD replaces it) — findings freshness-resolved against
/// the working tree as [`read_latest`] resolves them. The merge-base half of
/// the branch security delta ([FR-SC-18]): the snapshot *recorded* at that
/// commit, never a scan of its tree. An empty view when none was recorded,
/// and no store is created ([ADR-28]).
///
/// # Errors
/// Returns an error only on an unexpected store failure.
///
/// [FR-SC-18]: ../../../../docs/specs/requirements/FR-SC-18.md
/// [ADR-28]: ../../../../docs/specs/architecture/decisions/ADR-28.md
pub fn read_at(root: &Path, commit: &str) -> Result<SecurityView> {
    read_where(root, "s.head_sha = ?1", Some(commit), true)
}

/// The snapshots `filter` (a condition on `security_snapshots s`, with `param`
/// bound as `?1` when given) selects, and — `with_findings` — their findings.
fn read_where(root: &Path, filter: &str, param: Option<&str>, with_findings: bool) -> Result<SecurityView> {
    if !crate::history::db_path(root).exists() {
        return Ok(SecurityView { snapshots: Vec::new(), findings: Vec::new() });
    }
    let conn = crate::history::open(root)?;
    let snapshots = snapshots_where(&conn, filter, param)?;

    if !with_findings {
        return Ok(SecurityView { snapshots, findings: Vec::new() });
    }
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

        let rows = super::store::finding_rows(
            &conn,
            snap.snapshot_id,
            "path IS NULL, path, start_line, start_column, rule_id, fingerprint",
        )?;
        for row in rows {
            let freshness = match row.path.as_deref() {
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
            let package = row.package;
            findings.push((SecurityFinding {
                scanner_name: snap.scanner_name.clone(),
                scanner_version: snap.scanner_version.clone(),
                head_sha: snap.head_sha.clone(),
                fingerprint: row.fingerprint,
                severity: row.severity,
                confidence: row.confidence,
                cwes: row.cwes,
                rule_id: row.rule_id,
                message: row.message,
                redacted: row.redacted,
                binding: row.binding,
                path: row.path,
                start_line: row.region.start_line,
                start_column: row.region.start_column,
                end_line: row.region.end_line,
                end_column: row.region.end_column,
                anchor: row.anchor_commit.zip(row.report_path.as_deref()).map(|(commit, path)| CommitAnchor::new(commit, path)),
                report_path: row.report_path,
                commits: row.commits,
                legacy_fingerprints: row.legacy_fingerprints,
                freshness,
                reporters: Vec::new(),
            }, package));
        }
    }

    Ok(SecurityView { snapshots, findings: super::merge::merge(findings, root) })
}

/// The snapshots `filter` selects (see [`read_where`]), by scanner name, each
/// with its stored scope decoded.
fn snapshots_where(conn: &rusqlite::Connection, filter: &str, param: Option<&str>) -> Result<Vec<ScannerSnapshot>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT s.id, s.scanner_name, s.scanner_version, s.head_sha, s.report_hash,
                    (SELECT COUNT(*) FROM security_findings f WHERE f.snapshot_id = s.id),
                    s.created_at, s.full_head_sha, s.scope
               FROM security_snapshots s
              WHERE {filter}
              ORDER BY s.scanner_name"
        ))
        .context("preparing the security snapshot read")?;
    let params: Vec<&dyn rusqlite::ToSql> = param.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
    let rows = stmt
        .query_map(params.as_slice(), |row| {
            Ok((
                ScannerSnapshot {
                    snapshot_id: row.get(0)?,
                    scanner_name: row.get(1)?,
                    scanner_version: row.get(2)?,
                    head_sha: row.get(3)?,
                    report_hash: row.get(4)?,
                    finding_count: row.get::<_, i64>(5)? as usize,
                    created_at: row.get(6)?,
                    full_head_sha: row.get(7)?,
                    scope: Vec::new(),
                },
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .context("reading the security snapshots")?;
    let mut snapshots = Vec::new();
    for row in rows {
        let (mut snapshot, scope) = row.context("collecting the security snapshots")?;
        if let Some(scope) = scope {
            snapshot.scope = serde_json::from_str(&scope).context("reading a stored scan scope")?;
        }
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}

/// Read the security ratchet's baseline under `root` — the fingerprints the
/// last `gate --save` recorded under a `[security]` table ([FR-SC-10]) — or
/// `None` when none was ever saved, which is not an empty baseline. With no
/// `history.db` at all none was, and no store is created ([ADR-28]), as
/// [`read_latest`] does.
///
/// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
/// [ADR-28]: ../../../../docs/specs/architecture/decisions/ADR-28.md
///
/// # Errors
/// Returns an error only on an unexpected store failure.
pub fn read_baseline(root: &Path) -> Result<Option<SecurityBaseline>> {
    if !crate::history::db_path(root).exists() {
        return Ok(None);
    }
    let conn = crate::history::open(root)?;
    let Some((head_sha, config_hash)) = conn
        .query_row("SELECT head_sha, config_hash FROM security_baseline WHERE id = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .context("reading the security baseline")?
    else {
        return Ok(None);
    };
    let mut stmt = conn
        .prepare("SELECT fingerprint FROM security_baseline_findings ORDER BY fingerprint")
        .context("preparing the security baseline read")?;
    let fingerprints = stmt
        .query_map([], |row| row.get(0))
        .context("reading the security baseline fingerprints")?
        .collect::<rusqlite::Result<BTreeSet<String>>>()
        .context("collecting the security baseline fingerprints")?;
    Ok(Some(SecurityBaseline { head_sha, config_hash, fingerprints }))
}
