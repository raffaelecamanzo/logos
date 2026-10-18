//! The risk overlay — unsuppressed findings per path ([FR-SC-14], [S-671]).
//!
//! The hotspot board and Files & Risk each carry a findings cell per file: how
//! many unsuppressed findings are *fresh* on it and the highest of their
//! severities. The count is composed here from the read model ([`read_latest`])
//! and the suppression labeller ([`label`], [S-666]):
//!
//! - **Suppressed** findings (an unexpired entry names them) are not counted; an
//!   *expired* entry no longer suppresses, so its finding counts.
//! - **Stale** findings (the file moved since ingest) are never counted as fresh
//!   — they are tallied apart as [`PathFindings::stale`], so a surface can label
//!   them and the count stays true ([NFR-CC-04]).
//! - **A project nothing was scanned in** has no overlay at all ([`None`]),
//!   which is distinct from a scan that found nothing on a file (count `0`).
//!   So has one whose every stored report comes from `--path` scans alone
//!   ([S-685]): those read only their paths, so a `0` elsewhere would claim a
//!   scan that never ran.
//!
//! Pure over its input apart from the two reads in [`path_findings`]; ordered
//! by path, so byte-identical across runs ([NFR-RA-06]).
//!
//! [FR-SC-14]: ../../../../docs/specs/requirements/FR-SC-14.md
//! [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [S-666]: ../../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry
//! [S-671]: ../../../../docs/planning/journal.md#s-671-hotspots-and-files--risk-show-findings
//! [S-685]: ../../../../docs/planning/journal.md#s-685-a-security-re-scan-can-target-one-scanner-and-a-set-of-paths

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

use super::{label, read_latest, IgnoreFile, LabelledFinding, Severity};
use crate::history::{FRESHNESS_FRESH, FRESHNESS_STALE};

/// One path's findings cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PathFindings {
    /// Unsuppressed findings on the path whose file still matches its anchor.
    pub count: usize,
    /// The highest severity among the counted findings; `None` when `count` is `0`.
    pub highest: Option<Severity>,
    /// Unsuppressed findings the file's later change made stale — labelled by
    /// the surface, never part of `count`.
    pub stale: usize,
}

/// The per-path findings of the latest stored reports, or `None` when no
/// scanner has a stored report with a full scan behind it (never scanned, or
/// scanned only on `--path`s: no column, not a column of zeros). A path with no
/// finding is absent from the map — its cell is [`PathFindings::default`].
///
/// The ignore file is read only once a report is stored: with nothing scanned
/// there is nothing to label, so a project that never scanned never depends on it.
///
/// # Errors
/// With a stored report, an invalid `.logos/security-ignore.toml` (exit 2);
/// otherwise only a store failure.
pub fn path_findings(root: &Path, today: NaiveDate) -> Result<Option<BTreeMap<String, PathFindings>>> {
    let view = read_latest(root)?;
    if !view.snapshots.iter().any(|s| s.full_head_sha.is_some()) {
        return Ok(None);
    }
    let ignore = IgnoreFile::load(root)?;
    Ok(Some(tally(&label(&view.findings, &ignore, today).findings)))
}

/// Fold labelled findings into per-path cells. A finding bound to no file has
/// no path to sit on and joins no cell.
fn tally(findings: &[LabelledFinding]) -> BTreeMap<String, PathFindings> {
    let mut by_path: BTreeMap<String, PathFindings> = BTreeMap::new();
    for item in findings.iter().filter(|item| !item.is_suppressed()) {
        let Some(path) = item.finding.path.as_deref() else { continue };
        let cell = by_path.entry(path.to_string()).or_default();
        match item.finding.freshness {
            FRESHNESS_FRESH => {
                cell.count += 1;
                cell.highest = cell.highest.max(Some(item.finding.severity));
            }
            FRESHNESS_STALE => cell.stale += 1,
            _ => {}
        }
    }
    by_path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::security::{FindingBinding, SecurityFinding, Suppression};
    use crate::history::FRESHNESS_NA;

    fn item(path: Option<&str>, severity: Severity, freshness: &'static str) -> LabelledFinding {
        LabelledFinding {
            finding: SecurityFinding {
                scanner_name: "Semgrep OSS".into(),
                scanner_version: "1.85.0".into(),
                head_sha: "0".repeat(40),
                fingerprint: "f".repeat(64),
                rule_id: "sql-injection".into(),
                severity,
                cwes: Vec::new(),
                message: "tainted query".into(),
                redacted: false,
                binding: FindingBinding::Bound,
                report_path: path.map(str::to_string),
                path: path.map(str::to_string),
                start_line: Some(1),
                start_column: None,
                end_line: None,
                end_column: None,
                freshness,
            },
            suppression: None,
        }
    }

    #[test]
    fn a_cell_counts_fresh_findings_and_names_the_highest_severity() {
        let cells = tally(&[
            item(Some("a.rs"), Severity::Low, FRESHNESS_FRESH),
            item(Some("a.rs"), Severity::High, FRESHNESS_FRESH),
            item(Some("a.rs"), Severity::Medium, FRESHNESS_FRESH),
        ]);
        assert_eq!(
            cells["a.rs"],
            PathFindings { count: 3, highest: Some(Severity::High), stale: 0 }
        );
    }

    #[test]
    fn a_stale_finding_is_tallied_apart_and_never_counted_or_ranked() {
        let cells = tally(&[
            item(Some("a.rs"), Severity::Critical, FRESHNESS_STALE),
            item(Some("a.rs"), Severity::Low, FRESHNESS_FRESH),
        ]);
        assert_eq!(
            cells["a.rs"],
            PathFindings { count: 1, highest: Some(Severity::Low), stale: 1 },
            "the stale critical must not lift the highest severity or the count"
        );
    }

    #[test]
    fn only_stale_findings_leave_a_zero_count_with_no_severity() {
        let cells = tally(&[item(Some("a.rs"), Severity::High, FRESHNESS_STALE)]);
        assert_eq!(cells["a.rs"], PathFindings { count: 0, highest: None, stale: 1 });
    }

    #[test]
    fn a_suppressed_finding_is_excluded_but_an_expired_suppression_counts() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 1);
        let mut suppressed = item(Some("a.rs"), Severity::High, FRESHNESS_FRESH);
        suppressed.suppression =
            Some(Suppression::Suppressed { reason: "accepted".into(), expires: day });
        let mut expired = item(Some("b.rs"), Severity::High, FRESHNESS_FRESH);
        expired.suppression = Some(Suppression::Expired { reason: "accepted".into(), expires: day });
        let cells = tally(&[suppressed, expired]);
        assert!(!cells.contains_key("a.rs"), "a suppressed finding joins no cell");
        assert_eq!(cells["b.rs"].count, 1, "an expired suppression no longer suppresses");
    }

    #[test]
    fn a_finding_bound_to_no_file_joins_no_cell() {
        assert!(tally(&[item(None, Severity::High, FRESHNESS_NA)]).is_empty());
    }
}
