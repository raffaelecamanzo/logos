//! Security-finding persistence on the `history.db` track — snapshot semantics
//! ([FR-SC-01], [FR-SC-02], [ADR-79]).
//!
//! The whole ingest write is **one transaction** ([NFR-RA-07]): a crash mid-ingest
//! leaves the store at its prior coherent state. The tables are created by
//! migration 4 in [`super::super::db`] (`security_findings` rebuilt by
//! migration 8); this module owns only their DML.
//!
//! The rules ([FR-SC-02]):
//! - **New HEAD** → a new snapshot per scanner; prior snapshots are kept.
//! - **Same HEAD, same scanner** → that scanner's snapshot is **replaced**; other
//!   scanners' snapshots at the same HEAD are untouched.
//! - **Same HEAD, byte-identical report** → a no-op: every scanner in the report
//!   already holds a snapshot of this exact report, so nothing is written and the
//!   ingest reports `already_ingested` ([FR-SC-01]).
//! - **A scoped report** ([FR-SC-17]) → each scanner's new snapshot holds the
//!   report's findings within the scope, plus every finding and file anchor of
//!   the scanner's latest snapshot outside it, copied unchanged. It records the
//!   HEAD of the full scan behind the copied rows (`full_head_sha`, `NULL` when
//!   none is) and the paths scoped runs re-scanned since (`scope`) — migration 6.
//!   The rules above still decide whether it replaces or adds a snapshot, and
//!   whether it is a no-op: its report hash keys its scope with its bytes.
//! - **A scoped report that reads other evidence** ([`IngestScope::keeps_full_findings`]:
//!   Gitleaks, whose scoped run reads the files and whose full scan reads their
//!   git history) → the full scan's findings within the scope are kept too,
//!   since a secret removed from a file is still in its history. A scoped finding
//!   that is one of them keeps that finding's row, so the `[security]` baseline
//!   and suppressions still know it: it has the same fingerprint, both being
//!   keyed by the secret (migration 7). Only an unmatched one is stored, marked
//!   `scoped`, and reads as new; the next scoped run replaces it. A kept row
//!   stored before migration 7 is still keyed by its report, and is matched by
//!   position instead ([`matches_kept`]): it keeps its row and its key, as it
//!   did before, since a position match is no proof that it is the same secret.
//! - **Re-keying** ([`rekey`], migration 7) → a Gitleaks ingest maps the key
//!   each of its results had before migration 7 — its report key, computed
//!   exactly as it was then, never a position match — onto the secret-keyed one, and
//!   re-keys every stored row of the scanner and every baseline fingerprint
//!   under an old key, so a store written before the migration reads, and
//!   ratchets, under the new identity. An OSV-Scanner ingest does the same
//!   from the key each result had before migration 10 — its absolute path and
//!   its report's identity — onto its location key, and so does any ingest
//!   whose unbound absolute path under the root moved its key; a Gitleaks
//!   ingest, too, from the key each history-mode result had before migration
//!   11 — keyed by the file its report path bound to by suffix — onto the one
//!   its own project-relative path gives; a row stored before migration 11
//!   whose old binding today's index no longer gives is matched by its place
//!   instead ([`pre11_places`]). An OSV-Scanner, Trivy or Dependency-Check
//!   ingest — that scanner's first since migration 13 ([`recovers_pre13`]) —
//!   maps the key each dependency result had before the migration — its
//!   location or report key with its occurrence suffix — onto its package
//!   key, so an exact repeat's rows merge into one. The committed
//!   ignore file is never rewritten: an entry under an old key still applies,
//!   and [`super::label`] names the fingerprint it should name.
//! - **A history-mode row** records the commit whose place it is
//!   (`anchor_commit`, migration 8), and is bound to its file only where the
//!   secret is still on its line; otherwise it is stored `historical`, with no
//!   path and no file anchor ([S-767]).
//!
//! [FR-SC-01]: ../../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-SC-17]: ../../../../docs/specs/requirements/FR-SC-17.md
//! [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

use super::parse::{PackageCoordinate, Region, SecurityFormat};
use super::{in_scope, FindingBinding, IngestScope, ScannerConfidence, Severity};

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
#[derive(Clone)]
pub(crate) struct FindingRow {
    pub(crate) fingerprint: String,
    pub(crate) rule_id: String,
    pub(crate) severity: Severity,
    /// What the scanner states of its confidence (migration 12).
    pub(crate) confidence: ScannerConfidence,
    pub(crate) cwes: Vec<String>,
    pub(crate) message: String,
    pub(crate) redacted: bool,
    pub(crate) binding: FindingBinding,
    pub(crate) report_path: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) region: Region,
    /// Written by a scoped run rather than a full scan (migration 6).
    pub(crate) scoped: bool,
    /// The commit whose place the row's region and report path are, for a
    /// history-mode result; `None` otherwise (migration 8).
    pub(crate) anchor_commit: Option<String>,
    /// The commits the secret was found in, newest first (migration 7).
    pub(crate) commits: Vec<String>,
    /// The keys its results had before a re-key, sorted: the report keys of a
    /// secret-keyed row (migration 7) and, for a history-mode one, its secret
    /// key before migration 11 where it differs, or a report-keyed row's keys
    /// before migrations 10, 11 and 13 where they differ — a dependency row's
    /// before 13 one per result it merges; empty for a row keyed as it always
    /// was, as every row stored before migration 7 is.
    pub(crate) legacy_fingerprints: Vec<String>,
    /// A dependency row's keys before migrations 10 and 13, of every result
    /// it merges, each with its occurrence suffix: legacy
    /// fingerprints only on the scanner's first ingest since that migration
    /// ([`recovers_pre13`]), and carried forward from it on their finding
    /// ([`latest_legacy`]). Never stored apart.
    pub(crate) pre13_legacy: Vec<String>,
    /// Every `(commit, region)` a history-mode finding's results report at its
    /// report path, which is how a row stored before migration 11 is matched
    /// to it ([`pre11_places`]); empty for any other row. Never stored.
    pub(crate) places: Vec<(String, Region)>,
    /// The package a dependency row names, as its scanner states it (migration
    /// 14): what merges one advisory across scanners when it is read
    /// ([`super::merge`]). `None` for every other row, and for a row stored
    /// before migration 14.
    pub(crate) package: Option<PackageCoordinate>,
}

impl FindingRow {
    /// Keyed by its report rather than by its secret, and never re-keyed:
    /// every row of a scanner other than Gitleaks, and a Gitleaks row stored
    /// before migration 7 and not re-keyed since. Asked only of a Gitleaks
    /// row, where a legacy key marks a row keyed under an identity Logos has
    /// moved to — by its secret, or by its report where its secret could not
    /// be read and its key still moved: an unbound absolute path (migration
    /// 10), or any history-mode result, whose report key has a namespace of
    /// its own since migration 11. Such a row is matched by its key alone,
    /// never by position ([`matches_kept`]): a scoped finding of a secret its
    /// full scan could not read (a commit missing from this clone) is stored
    /// beside it, marked `scoped`, and reads as new.
    pub(crate) fn report_keyed(&self) -> bool {
        self.legacy_fingerprints.is_empty()
    }
}

/// What [`persist_ingest`] did.
pub(crate) struct PersistOutcome {
    /// `true` when the report was already stored at this HEAD — nothing written.
    pub(crate) already_ingested: bool,
    /// Per batch, in batch order: the snapshot id it is stored under, and whether
    /// an earlier snapshot for that scanner at this HEAD was replaced.
    pub(crate) snapshots: Vec<(i64, bool)>,
}

/// Persist one ingest under the snapshot rules, atomically. `scope` is a
/// scoped report's scope; each batch must then hold only findings within its
/// paths.
///
/// # Errors
/// Returns an error only if a statement fails, a carried row holds a token
/// Logos never writes, or the transaction cannot commit.
pub(crate) fn persist_ingest(
    conn: &mut Connection,
    head_sha: &str,
    report_hash: &str,
    format: SecurityFormat,
    batches: &[ScannerBatch],
    scope: Option<IngestScope<'_>>,
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
    // A scoped report's hash keys its scope too, so it is a no-op only when the
    // latest snapshot is this very report over this very scope.
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
        // Read before the delete below, which may drop that very snapshot.
        let carried = scope.map(|scope| carried_outside(&tx, &batch.name, scope)).transpose()?;
        let placed = pre11_places(&tx, &batch.name, &batch.findings)?;
        let known = latest_legacy(&tx, &batch.name)?;
        let recovers = recovers_pre13(&tx, &batch.name)?;
        let pre13 = |f: &FindingRow| if recovers { f.pre13_legacy.clone() } else { Vec::new() };
        let replaced = prior.is_some();
        if let Some((id, _)) = prior {
            // Same HEAD, same scanner: replace. The cascade drops its files and
            // findings with it.
            tx.execute("DELETE FROM security_snapshots WHERE id = ?1", [id])
                .context("replacing the scanner's snapshot at this HEAD")?;
        }
        let (full_head_sha, stored_scope) = match &carried {
            None => (Some(head_sha.to_string()), None),
            Some(c) => (c.full_head_sha.clone(), Some(serde_json::to_string(&c.scope)?)),
        };
        tx.execute(
            "INSERT INTO security_snapshots
                 (head_sha, scanner_name, scanner_version, report_hash, format, created_at,
                  full_head_sha, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), ?6, ?7)",
            rusqlite::params![
                head_sha,
                batch.name,
                batch.version,
                report_hash,
                format.as_str(),
                full_head_sha,
                stored_scope
            ],
        )
        .with_context(|| format!("creating the security snapshot for {}", batch.name))?;
        let snapshot_id = tx.last_insert_rowid();
        let (carried_files, carried_findings) = match &carried {
            Some(c) => (c.files.as_slice(), c.findings.as_slice()),
            None => (&[][..], &[][..]),
        };
        // A scoped finding that is a kept full-scan finding is that finding:
        // its row stays, and the scoped copy is not stored a second time. A
        // kept row keyed before migration 7 is matched by position and keeps
        // its key: only a full scan's report key re-keys it.
        let matched: Vec<usize> = carried.as_ref().map_or_else(Vec::new, |c| matches_kept(&batch.findings, &c.kept));
        let fresh: Vec<&FindingRow> = batch
            .findings
            .iter()
            .enumerate()
            .filter(|(i, f)| !matched.contains(i) && carried_findings.iter().all(|c| c.fingerprint != f.fingerprint))
            .map(|(_, f)| f)
            .collect();
        let mut renamed: std::collections::BTreeMap<String, String> = batch
            .findings
            .iter()
            .flat_map(|f| f.legacy_fingerprints.iter().cloned().chain(pre13(f)).map(|old| (old, f.fingerprint.clone())))
            .collect();
        renamed.extend(placed.iter().map(|(old, new)| (old.clone(), new.clone())));
        let anchored: Vec<&str> = batch.files.iter().map(|(path, _)| path.as_str()).collect();

        {
            let mut file_stmt = tx
                .prepare_cached(
                    "INSERT INTO security_files (snapshot_id, path, content_hash)
                     VALUES (?1, ?2, ?3)",
                )
                .context("preparing the security file insert")?;
            // A kept finding's file is anchored anew when the scoped run read it.
            let carried_files = carried_files.iter().filter(|(path, _)| !anchored.contains(&path.as_str()));
            for (path, hash) in batch.files.iter().chain(carried_files) {
                file_stmt
                    .execute(rusqlite::params![snapshot_id, path, hash])
                    .with_context(|| format!("anchoring {path}"))?;
            }

            let mut finding_stmt = tx
                .prepare_cached(
                    "INSERT INTO security_findings (
                         snapshot_id, fingerprint, rule_id, severity, cwes, message, redacted,
                         binding, report_path, path,
                         start_line, start_column, end_line, end_column, scoped,
                         commits, legacy_fingerprints, anchor_commit, confidence,
                         package_name, package_version
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                )
                .context("preparing the security finding insert")?;
            let keys: std::collections::HashSet<&str> = batch.findings.iter().map(|f| f.fingerprint.as_str()).collect();
            for f in fresh.into_iter().chain(carried_findings) {
                let recovered = pre13(f);
                let mut legacy: std::collections::BTreeSet<&str> = f.legacy_fingerprints.iter().map(String::as_str).collect();
                legacy.extend(recovered.iter().map(String::as_str));
                legacy.extend(placed.iter().filter(|(_, new)| **new == f.fingerprint).map(|(old, _)| old.as_str()));
                legacy.extend(known.get(&f.fingerprint).into_iter().flatten().map(String::as_str).filter(|old| !keys.contains(old)));
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
                        f.scoped,
                        f.commits.join(","),
                        legacy.into_iter().collect::<Vec<_>>().join(","),
                        f.anchor_commit,
                        f.confidence.as_str(),
                        f.package.as_ref().map(|p| p.name.as_str()),
                        f.package.as_ref().map(|p| p.version.as_str()),
                    ])
                    .with_context(|| format!("inserting finding {} ({})", f.fingerprint, f.rule_id))?;
            }
        }
        rekey(&tx, &batch.name, &renamed)?;
        snapshots.push((snapshot_id, replaced));
    }

    tx.commit().context("committing the security ingest")?;
    Ok(PersistOutcome {
        already_ingested: false,
        snapshots,
    })
}

/// Every finding row of snapshot `snapshot_id`, in `order` (an `ORDER BY`
/// clause over `security_findings`): the one decoder of the rows
/// [`persist_ingest`] encodes, shared by the read path and the scoped ingest's
/// carry-over, so the two cannot drift apart.
///
/// # Errors
/// A statement failure, or a stored severity or binding Logos never writes.
pub(crate) fn finding_rows(conn: &Connection, snapshot_id: i64, order: &str) -> Result<Vec<FindingRow>> {
    let mut stmt = conn
        .prepare_cached(&format!(
            "SELECT fingerprint, rule_id, severity, cwes, message, redacted, binding,
                    report_path, path, start_line, start_column, end_line, end_column, scoped,
                    commits, legacy_fingerprints, anchor_commit, confidence,
                    package_name, package_version
               FROM security_findings WHERE snapshot_id = ?1 ORDER BY {order}"
        ))
        .context("preparing the security finding read")?;
    // The tokens are decoded after the query: a token Logos never writes is
    // this read's error, not a column-type one.
    let raw: Vec<(String, String, String, String, String, String, FindingRow)> = stmt
        .query_map([snapshot_id], |row| {
            Ok((
                row.get(2)?,
                row.get(3)?,
                row.get(6)?,
                row.get(14)?,
                row.get(15)?,
                row.get(17)?,
                FindingRow {
                    fingerprint: row.get(0)?,
                    rule_id: row.get(1)?,
                    severity: Severity::Info,
                    confidence: ScannerConfidence::Unstated,
                    cwes: Vec::new(),
                    message: row.get(4)?,
                    redacted: row.get(5)?,
                    binding: FindingBinding::Bound,
                    report_path: row.get(7)?,
                    path: row.get(8)?,
                    region: Region {
                        start_line: row.get(9)?,
                        start_column: row.get(10)?,
                        end_line: row.get(11)?,
                        end_column: row.get(12)?,
                    },
                    scoped: row.get(13)?,
                    anchor_commit: row.get(16)?,
                    commits: Vec::new(),
                    legacy_fingerprints: Vec::new(),
                    pre13_legacy: Vec::new(),
                    places: Vec::new(),
                    package: row
                        .get::<_, Option<String>>(18)?
                        .zip(row.get::<_, Option<String>>(19)?)
                        .map(|(name, version)| PackageCoordinate { name, version }),
                },
            ))
        })
        .context("reading security findings")?
        .collect::<rusqlite::Result<_>>()
        .context("collecting security findings")?;
    raw.into_iter()
        .map(|(severity, cwes, binding, commits, legacy, confidence, mut row)| {
            row.severity = Severity::from_str_stored(&severity)
                .ok_or_else(|| anyhow::anyhow!("stored severity {severity:?} is not one Logos writes"))?;
            row.confidence = ScannerConfidence::from_str_stored(&confidence)
                .ok_or_else(|| anyhow::anyhow!("stored confidence {confidence:?} is not one Logos writes"))?;
            row.cwes = split_list(&cwes);
            row.commits = split_list(&commits);
            row.legacy_fingerprints = split_list(&legacy);
            row.binding = FindingBinding::from_str_stored(&binding)
                .ok_or_else(|| anyhow::anyhow!("stored binding {binding:?} is not one Logos writes"))?;
            Ok(row)
        })
        .collect()
}

/// A stored comma-separated list: `''` is none.
fn split_list(stored: &str) -> Vec<String> {
    if stored.is_empty() {
        Vec::new()
    } else {
        stored.split(',').map(str::to_string).collect()
    }
}

/// Re-key every stored row of `scanner`, in every snapshot, and every baseline
/// fingerprint, whose key is an old one `renamed` maps (migrations 7, 10, 11 and 13). A row
/// re-keyed onto a key its snapshot already holds is merged into that row: the
/// per-commit repeats of one secret stored before the migration become one, as
/// do a dependency result's repeats stored before migration 13.
/// Each re-keyed row records the key it had as a legacy fingerprint.
///
/// # Errors
/// A statement failure.
fn rekey(tx: &rusqlite::Transaction<'_>, scanner: &str, renamed: &std::collections::BTreeMap<String, String>) -> Result<()> {
    if renamed.is_empty() {
        return Ok(());
    }
    let stored: Vec<(i64, String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT f.snapshot_id, f.fingerprint, f.legacy_fingerprints
                   FROM security_findings f JOIN security_snapshots s ON s.id = f.snapshot_id
                  WHERE s.scanner_name = ?1 ORDER BY f.snapshot_id, f.fingerprint",
            )
            .context("preparing the re-key read")?;
        let rows = stmt
            .query_map([scanner], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .context("reading the rows to re-key")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the rows to re-key")?;
        rows.into_iter().filter(|(_, fingerprint, _)| renamed.contains_key(fingerprint)).collect()
    };
    for (snapshot_id, old, legacy) in stored {
        let new = &renamed[&old];
        let target: Option<String> = tx
            .query_row(
                "SELECT legacy_fingerprints FROM security_findings WHERE snapshot_id = ?1 AND fingerprint = ?2",
                rusqlite::params![snapshot_id, new],
                |row| row.get(0),
            )
            .optional()
            .context("looking up the re-keyed row")?;
        let mut merged: std::collections::BTreeSet<String> = split_list(&legacy).into_iter().collect();
        merged.extend(split_list(target.as_deref().unwrap_or_default()));
        merged.insert(old.clone());
        let merged = merged.into_iter().collect::<Vec<_>>().join(",");
        if target.is_some() {
            tx.execute(
                "DELETE FROM security_findings WHERE snapshot_id = ?1 AND fingerprint = ?2",
                rusqlite::params![snapshot_id, old],
            )
            .context("merging a re-keyed row")?;
            tx.execute(
                "UPDATE security_findings SET legacy_fingerprints = ?3 WHERE snapshot_id = ?1 AND fingerprint = ?2",
                rusqlite::params![snapshot_id, new, merged],
            )
            .context("recording a merged row's old key")?;
        } else {
            tx.execute(
                "UPDATE security_findings SET fingerprint = ?3, legacy_fingerprints = ?4
                  WHERE snapshot_id = ?1 AND fingerprint = ?2",
                rusqlite::params![snapshot_id, old, new, merged],
            )
            .context("re-keying a stored row")?;
        }
    }
    for (old, new) in renamed {
        tx.execute(
            "INSERT INTO security_baseline_findings (fingerprint, scanner_name)
             SELECT ?2, scanner_name FROM security_baseline_findings WHERE fingerprint = ?1
             ON CONFLICT (fingerprint) DO NOTHING",
            rusqlite::params![old, new],
        )
        .context("re-keying a baseline fingerprint")?;
        tx.execute("DELETE FROM security_baseline_findings WHERE fingerprint = ?1", [old])
            .context("dropping a re-keyed baseline fingerprint")?;
    }
    Ok(())
}

/// The old keys each finding of `scanner`'s latest snapshot is also known by,
/// by its key. An ingest keeps them on the same finding, so a key recovered
/// once — by its place, from a row stored before migration 11 that no later
/// report recomputes — still matches a suppression or a checked-in baseline
/// at the next HEAD. One that is a key of the ingest itself is not kept.
///
/// # Errors
/// A statement failure.
fn latest_legacy(
    tx: &rusqlite::Transaction<'_>,
    scanner: &str,
) -> Result<std::collections::HashMap<String, Vec<String>>> {
    let mut stmt = tx
        .prepare(
            "SELECT fingerprint, legacy_fingerprints FROM security_findings
              WHERE legacy_fingerprints != ''
                AND snapshot_id = (SELECT MAX(id) FROM security_snapshots WHERE scanner_name = ?1)",
        )
        .context("preparing the latest snapshot's old-key read")?;
    let rows = stmt
        .query_map([scanner], |row| Ok((row.get::<_, String>(0)?, split_list(&row.get::<_, String>(1)?))))
        .context("reading the latest snapshot's old keys")?
        .collect::<rusqlite::Result<_>>()
        .context("collecting the latest snapshot's old keys")?;
    Ok(rows)
}

/// Whether this ingest is `scanner`'s first since history.db migration 13:
/// its latest snapshot carries the mark of a migration up to 13, or it has
/// none — a fresh store, whose checked-in baseline and ignore file may still
/// name keys from before the migration. Only then are a dependency row's
/// [`FindingRow::pre13_legacy`] keys its legacy fingerprints: they were
/// suffixed in report order, and a later report's order — a package added
/// ahead of another — would hand one package's old key to another. Once
/// recorded, an old key stays on its finding ([`latest_legacy`]).
///
/// # Errors
/// A statement failure.
fn recovers_pre13(tx: &rusqlite::Transaction<'_>, scanner: &str) -> Result<bool> {
    let latest: Option<String> = tx
        .query_row(
            "SELECT report_hash FROM security_snapshots WHERE scanner_name = ?1 ORDER BY id DESC LIMIT 1",
            [scanner],
            |row| row.get(0),
        )
        .optional()
        .context("looking up the scanner's latest report hash")?;
    let marked_by = |hash: &str| hash.strip_prefix("pre-")?.split_once(':')?.0.parse::<u32>().ok();
    Ok(latest.is_none_or(|hash| marked_by(&hash).is_some_and(|migration| migration <= 13)))
}

/// The rows of `scanner`'s snapshots stored before history.db migration 11
/// (report hash marked `pre-11:`) that are history-mode findings of this
/// ingest under another key, as `old key → new key`. Such a row was keyed by
/// the file its report path bound to by suffix then, a binding never stored
/// and not always given again by today's index; but its rule, report path,
/// commit and exact region name one Gitleaks match in an immutable blob, so
/// the finding with a result at that very place is the row's finding ([S-780],
/// CR-241). A row already under a key of this ingest is left alone.
///
/// [S-780]: ../../../../docs/planning/journal.md#s-780-a-secret-found-only-in-history-is-known-by-its-own-path-not-by-todays-files
///
/// # Errors
/// A statement failure.
fn pre11_places(
    tx: &rusqlite::Transaction<'_>,
    scanner: &str,
    findings: &[FindingRow],
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut placed = std::collections::BTreeMap::new();
    if findings.iter().all(|f| f.places.is_empty()) {
        return Ok(placed);
    }
    let current: std::collections::HashSet<&str> = findings.iter().map(|f| f.fingerprint.as_str()).collect();
    let mut stmt = tx
        .prepare(
            "SELECT f.fingerprint, f.rule_id, f.report_path, f.anchor_commit,
                    f.start_line, f.start_column, f.end_line, f.end_column
               FROM security_findings f JOIN security_snapshots s ON s.id = f.snapshot_id
              WHERE s.scanner_name = ?1 AND s.report_hash LIKE 'pre-11:%'
                AND f.anchor_commit IS NOT NULL AND f.report_path IS NOT NULL",
        )
        .context("preparing the pre-migration-11 place read")?;
    let rows = stmt
        .query_map([scanner], |row| {
            let region = Region {
                start_line: row.get(4)?,
                start_column: row.get(5)?,
                end_line: row.get(6)?,
                end_column: row.get(7)?,
            };
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, region))
        })
        .context("reading the pre-migration-11 places")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting the pre-migration-11 places")?;
    for (old, rule_id, report_path, commit, region) in rows {
        if current.contains(old.as_str()) {
            continue;
        }
        let report_path = super::normalise_report_path(&report_path);
        let found = findings.iter().find(|f| {
            f.rule_id == rule_id
                && f.report_path.as_deref().map(super::normalise_report_path).as_deref() == Some(report_path.as_str())
                && f.places.iter().any(|(c, r)| *c == commit && *r == region)
        });
        if let Some(f) = found {
            placed.insert(old, f.fingerprint.clone());
        }
    }
    Ok(placed)
}

/// What a scoped ingest copies from a scanner's latest snapshot: the rows
/// outside the scope, the full-scan rows inside it that the scope keeps, and
/// the snapshot's provenance merged with the new scope.
struct Carried {
    /// The full scan behind the copied rows; `None` when there is none — the
    /// scanner has no snapshot, or only scoped ones.
    full_head_sha: Option<String>,
    /// The latest snapshot's scope with the new one added, sorted, once each.
    scope: Vec<String>,
    files: Vec<(String, String)>,
    /// Every copied finding: those outside the scope, then [`kept`](Self::kept).
    findings: Vec<FindingRow>,
    /// Copies of the full-scan findings inside the scope that a
    /// [`keeps_full_findings`](IngestScope::keeps_full_findings) scope keeps.
    kept: Vec<FindingRow>,
}

/// The rows of `scanner`'s latest snapshot the scoped ingest keeps, unchanged,
/// with its provenance ([FR-SC-17]): every row outside the scope and, when the
/// scope keeps full-scan findings, every full-scan row inside it with its
/// file's anchor. A finding is inside the scope when its bound path — or,
/// unbound to an indexed file, the path its report named — is a scoped path or
/// lies under one; a finding with no location never is.
fn carried_outside(tx: &rusqlite::Transaction<'_>, scanner: &str, scope: IngestScope<'_>) -> Result<Carried> {
    let latest: Option<(i64, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT id, full_head_sha, scope FROM security_snapshots
              WHERE scanner_name = ?1 ORDER BY id DESC LIMIT 1",
            [scanner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .context("looking up the scanner's latest security snapshot")?;
    let mut merged: std::collections::BTreeSet<String> = scope.paths.iter().cloned().collect();
    let Some((id, full_head_sha, prior_scope)) = latest else {
        return Ok(Carried {
            full_head_sha: None,
            scope: merged.into_iter().collect(),
            files: Vec::new(),
            findings: Vec::new(),
            kept: Vec::new(),
        });
    };
    if let Some(prior) = prior_scope {
        let prior: Vec<String> = serde_json::from_str(&prior).context("reading a stored scan scope")?;
        merged.extend(prior);
    }

    let mut findings = Vec::new();
    let mut kept = Vec::new();
    for row in finding_rows(tx, id, "fingerprint").context("reading the carried findings")? {
        let inside = in_scope(location(row.path.as_deref(), row.report_path.as_deref()).as_deref(), scope.paths);
        if inside && !(scope.keeps_full_findings && !row.scoped) {
            continue;
        }
        if inside {
            kept.push(row.clone());
        }
        findings.push(row);
    }

    // An anchor goes with what is copied: every file outside the scope, and a
    // kept finding's file inside it.
    let files = {
        let mut stmt = tx
            .prepare("SELECT path, content_hash FROM security_files WHERE snapshot_id = ?1 ORDER BY path")
            .context("preparing the carried anchor read")?;
        let rows = stmt
            .query_map([id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .context("reading the carried anchors")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting the carried anchors")?;
        rows.into_iter()
            .filter(|(path, _)| !in_scope(Some(path), scope.paths) || kept.iter().any(|k| k.path.as_deref() == Some(path)))
            .collect()
    };
    Ok(Carried { full_head_sha, scope: merged.into_iter().collect(), files, findings, kept })
}

/// A finding's location for scope matching: its bound path, else the path its
/// report named, separator-normalised.
fn location(path: Option<&str>, report_path: Option<&str>) -> Option<String> {
    path.map(str::to_string).or_else(|| report_path.map(super::normalise_report_path))
}

/// Which of a scoped run's `fresh` findings are one of the `kept` full-scan
/// findings keyed by their report — rows stored before migration 7 — by index
/// into `fresh`: the same file and rule, matched first on the whole match
/// position (start and end line and column), then — for a secret whose lines
/// moved — on its column span alone, when exactly one unmatched finding on
/// each side shares it. Each kept finding matches at most one fresh one; ties
/// go to the lowest fingerprint, so the answer is the same on every run. A row
/// keyed by its secret needs none of this: a scoped finding of the same secret
/// has its very fingerprint.
pub(crate) fn matches_kept(fresh: &[FindingRow], kept: &[FindingRow]) -> Vec<usize> {
    let file = |f: &FindingRow| location(f.path.as_deref(), f.report_path.as_deref());
    let mut order: Vec<&FindingRow> = kept.iter().filter(|k| k.report_keyed()).collect();
    order.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    let mut taken = vec![false; order.len()];
    let mut matched = Vec::new();
    // Pass 1: the same file, rule and whole position.
    for (i, f) in fresh.iter().enumerate() {
        let hit = order.iter().enumerate().position(|(k, h)| {
            !taken[k] && h.rule_id == f.rule_id && file(h) == file(f) && h.region == f.region
        });
        if let Some(k) = hit {
            taken[k] = true;
            matched.push(i);
        }
    }
    // Pass 2: the same file, rule and column span, unique on both sides.
    let span = |f: &FindingRow| (file(f), f.rule_id.clone(), f.region.start_column, f.region.end_column);
    for (i, f) in fresh.iter().enumerate() {
        if matched.contains(&i) || f.region.start_column.is_none() {
            continue;
        }
        let candidates: Vec<usize> = (0..order.len()).filter(|&k| !taken[k] && span(order[k]) == span(f)).collect();
        let rivals = fresh.iter().enumerate().filter(|(j, g)| !matched.contains(j) && span(g) == span(f)).count();
        if let ([k], 1) = (candidates.as_slice(), rivals) {
            taken[*k] = true;
            matched.push(i);
        }
    }
    matched.sort_unstable();
    matched
}

/// Replace the security ratchet's baseline with `findings` — `(scanner,
/// fingerprint)` pairs — atomically ([FR-SC-10], migration 5). A fingerprint
/// two scanners share is recorded once, under the first.
///
/// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
///
/// # Errors
/// Returns an error only if a statement fails or the transaction cannot commit.
pub(crate) fn persist_baseline(
    conn: &mut Connection,
    head_sha: Option<&str>,
    config_hash: &str,
    findings: &[(String, String)],
) -> Result<usize> {
    let tx = conn
        .transaction()
        .context("opening the security baseline transaction")?;
    tx.execute("DELETE FROM security_baseline_findings", [])
        .context("clearing the security baseline")?;
    tx.execute(
        "INSERT INTO security_baseline (id, head_sha, config_hash, created_at)
         VALUES (1, ?1, ?2, unixepoch())
         ON CONFLICT (id) DO UPDATE SET head_sha = excluded.head_sha,
             config_hash = excluded.config_hash, created_at = excluded.created_at",
        rusqlite::params![head_sha, config_hash],
    )
    .context("recording the security baseline")?;
    let mut recorded = 0;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO security_baseline_findings (fingerprint, scanner_name) VALUES (?1, ?2)
                 ON CONFLICT (fingerprint) DO NOTHING",
            )
            .context("preparing the security baseline insert")?;
        for (scanner, fingerprint) in findings {
            recorded += stmt
                .execute(rusqlite::params![fingerprint, scanner])
                .context("recording a security baseline fingerprint")?;
        }
    }
    tx.commit().context("committing the security baseline")?;
    Ok(recorded)
}
