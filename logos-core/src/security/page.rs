//! The Security page's read-model ([FR-SC-09], [S-670]): the scanner matrix,
//! the findings in graph context, both OWASP editions and the `[security]` gate
//! band, composed by [`Engine::security_page`](crate::Engine::security_page)
//! for `GET /api/v1/security`.
//!
//! **Both editions ride on one payload**, so the page switches between them
//! without a refetch ([FR-SC-04]): [`editions`] classifies the same stored
//! findings under 2025 and 2021, each with its category coverage. It reuses
//! [`findings`](super::findings) — the one place that decides which scanners
//! have looked at a category ([BR-66]) — and keeps only the classification and
//! each category's counts, the production one scoped by the graph's own
//! test-symbol answer as the page's findings list is, so "not covered" cannot
//! be decided two ways. The graph context is asked for once, by the listing the
//! page shows; here it is not computed at all.
//!
//! No source snippet is read or carried: a stored finding has none
//! ([ADR-79] §2), and a secret-class message is stored redacted ([FR-SC-03]).
//!
//! [FR-SC-03]: ../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-04]: ../../../docs/specs/requirements/FR-SC-04.md
//! [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-670]: ../../../docs/planning/journal.md#s-670-the-security-page-shows-scanners-findings-and-the-owasp-view

use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

use super::context::{FindingContext, UnboundBecause};
use super::rollup::{could_run, scan_state};
use super::{
    ConsentState, FindingTally, FindingsFilter, MemberScan, ScanEnvironment, ScanState, ScannerStatus, SecurityFindings,
    SecurityGateBand, SecurityStatus,
};
use crate::history::security::owasp::{CategoryCoverage, FindingClassification};
use crate::history::security::OwaspEdition;
use crate::history::SecurityFinding;
use crate::tools::Manifest;

/// What `GET /api/v1/security` serves ([FR-SC-09]).
///
/// [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecurityPage {
    /// Each scanner's one state, with the install command for a missing one.
    pub status: SecurityStatus,
    /// Every stored finding (unfiltered, every code scope: the page filters),
    /// classified under 2025, each in graph context and with its code scope.
    pub findings: SecurityFindings,
    /// The stored findings the page counts — fresh per finding, unsuppressed,
    /// in production code — and those it leaves out, test code among them: the
    /// figure the workspace roll-up row and Files & Risk read too ([S-760],
    /// [FR-SC-28]).
    ///
    /// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
    /// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
    pub counts: FindingTally,
    /// The same findings under each edition, 2025 then 2021.
    pub editions: Vec<EditionClassification>,
    /// The `[security]` gate's verdict; `None` without the table.
    pub gate: Option<SecurityGateBand>,
}

/// The stored findings under one OWASP edition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditionClassification {
    pub edition: OwaspEdition,
    /// Every category of the edition: covered with a count, or not covered.
    pub categories: Vec<CategoryCoverage>,
    /// One per stored finding, keyed by scanner and fingerprint.
    pub findings: Vec<FindingClassification>,
}

/// The editions the page offers, in the order it lists them.
const EDITIONS: [OwaspEdition; 2] = [OwaspEdition::V2025, OwaspEdition::V2021];

/// The stored findings classified under each edition ([module docs](self)).
/// Each category's `production_findings` is counted over the graph's own
/// answer to `in_test_symbols` — the one the page's findings list is scoped by —
/// so the page's categories and its findings cannot disagree on which finding
/// is test code. Starts no process, as [`findings`](super::findings) starts none.
///
/// # Errors
/// As [`listings`]: an invalid config or ignore file (exit 2), a store
/// failure, or `in_test_symbols`' own error.
pub(crate) fn editions(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
    in_test_symbols: impl Fn(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<Vec<EditionClassification>> {
    Ok(listings(root, env, manifest, today, in_test_symbols)?
        .into_iter()
        .map(|listed| EditionClassification {
            edition: listed.edition,
            categories: listed.categories,
            findings: listed
                .findings
                .into_iter()
                .map(|item| FindingClassification {
                    scanner_name: item.finding.scanner_name,
                    fingerprint: item.finding.fingerprint,
                    categories: item.owasp,
                    unclassified: item.unclassified,
                })
                .collect(),
        })
        .collect())
}

/// The stored findings listed under each edition, 2025 then 2021, with **no**
/// graph context: each finding carries a placeholder (unbound, no location)
/// where [`findings`](super::findings) would compute one. For the read-models
/// that keep only the classification, the counts and the suppression —
/// [`editions`] here and the workspace roll-up's
/// [`MemberSecurity`](super::MemberSecurity) — so neither pays for the graph
/// walk, and neither re-derives which scanners have looked at a category
/// ([BR-66]). Every code scope is listed, each finding with its scope:
/// `in_test_symbols` is the graph's answer for every stored finding, asked
/// once per edition, each over that edition's own read. Starts no process.
///
/// # Errors
/// As [`findings`](super::findings): an invalid config or ignore file
/// (exit 2), a store failure, or `in_test_symbols`' own error.
///
/// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
pub(crate) fn listings(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
    in_test_symbols: impl Fn(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<Vec<SecurityFindings>> {
    EDITIONS
        .into_iter()
        .map(|edition| {
            let filter = FindingsFilter { edition, ..FindingsFilter::default() };
            super::findings(root, env, manifest, &filter, today, &in_test_symbols, |kept| {
                Ok(kept
                    .iter()
                    .map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoLocation))
                    .collect())
            })
        })
        .collect()
}

// ── The Dashboard's Security widget read-model (S-789, FR-UI-09) ──────────────
//
// The Dashboard's Security widget read-model ([FR-UI-09], [S-789]): the
// stored security state of one project in a handful of figures, from stored
// data alone.
//
// **Stored-only.** It asks no scanner for anything — not even a version:
// [`Engine::security_summary`](crate::Engine::security_summary) reads the
// status with [`VersionProbes::Never`](super::VersionProbes::Never), so a
// Dashboard load, which a cross-site page can trigger, starts no scanner
// process at all ([ADR-80] §4). It writes nothing ([ADR-28]).
//
// **One definition of every figure.** The counts are
// [`FindingTally::of`] over the stored findings — the figure the Security
// page and a workspace member's row state — so a finding in test code, a
// suppressed one or one whose file moved is left out here exactly as there
// ([S-760], [S-795]). The scanners' failures are read off the same status rows
// `security status` prints, and the scanner denominator is the roll-up's
// ([`MemberSecurity::scanners_enabled`](super::MemberSecurity::scanners_enabled)),
// so the widget and a member's row cannot disagree.
//
// **A project never scanned is `never_run`, never zero** ([BR-66]):
// [`tally`](SecuritySummary::tally) is `None` when no report is stored.
//
// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
// [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
// [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md
// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
// [S-789]: ../../../docs/planning/journal.md#s-789-the-dashboard-leads-with-quality-security-and-coverage
// [S-795]: ../../../docs/planning/journal.md#s-795-a-finding-in-test-code-is-classified-labelled-and-counted-apart-and-the-gate-still-sees-it

/// The Security widget's figures ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecuritySummary {
    /// Whether a report is stored, and whether any is fresh at HEAD.
    pub scan: MemberScan,
    /// The stored findings counted and left out, as the Security page states
    /// them; `None` when no report is stored — nothing to count from, which is
    /// not a count of zero.
    pub tally: Option<FindingTally>,
    /// When the newest stored report was ingested, in Unix seconds; `None`
    /// with no report stored. The widget states it as a relative age.
    pub newest_scan_at: Option<i64>,
    /// Scanners whose last scan failed (a failure log is on disk for them) among
    /// those that could have run: always within [`scanners_total`](Self::scanners_total).
    pub scanners_failed: usize,
    /// Scanners that could have run: the registered ones the config does not
    /// disable and that have something to scan, plus every unregistered tool
    /// whose report is stored ([`MemberSecurity::scanners_enabled`](super::MemberSecurity::scanners_enabled)).
    pub scanners_total: usize,
    /// The project's recorded scan consent: what tells a never-scanned project
    /// from one whose scan was never allowed.
    pub consent: ConsentState,
    /// The `[security]` gate's verdict; `None` without the table.
    pub gate: Option<SecurityGateBand>,
}

impl SecuritySummary {
    /// The summary of one project's `status`, its stored `findings` (any one
    /// edition's listing: every edition lists the same stored findings), the
    /// Unix seconds each stored report was ingested at, and the `gate` band.
    /// Pure: it reads nothing, so every state is assertable without a store.
    pub fn from_reads(
        status: SecurityStatus,
        findings: &SecurityFindings,
        ingested_at: &[i64],
        gate: Option<SecurityGateBand>,
    ) -> Self {
        let scanners: Vec<&ScannerStatus> = status.scanners.iter().chain(&status.other_reports).collect();
        let fresh = scanners.iter().any(|s| matches!(s.state, ScanState::Fresh { .. }));
        let stored = !findings.scanned.is_empty();
        let scan = scan_state(fresh, stored);
        // Only a scanner that could have run counts, failed or not: a failure
        // log left behind by one since disabled is no gap, so the numerator is
        // always within the denominator.
        let runnable = scanners.iter().filter(|s| could_run(s));
        Self {
            scan,
            tally: stored.then(|| FindingTally::of(&findings.findings)),
            newest_scan_at: ingested_at.iter().copied().max(),
            scanners_failed: runnable.clone().filter(|s| s.last_failure.is_some()).count(),
            scanners_total: runnable.count(),
            consent: status.consent,
            gate,
        }
    }
}

/// The Dashboard widget's [`SecuritySummary`] over one project: its `status`
/// and `gate` band, already read, with the stored findings
/// ([`summary_findings`]) and the ingest time of each stored report. The one
/// call [`Engine::security_summary`](crate::Engine::security_summary) makes, so
/// the engine depends on this module through `security`'s re-export alone.
///
/// # Errors
/// As [`summary_findings`], or a store failure reading the snapshots.
pub(crate) fn summary(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
    status: SecurityStatus,
    gate: Option<SecurityGateBand>,
    in_test_symbols: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<SecuritySummary> {
    let findings = summary_findings(root, env, manifest, today, in_test_symbols)?;
    let ingested: Vec<i64> =
        crate::history::security::latest_snapshots(root)?.iter().map(|s| s.created_at).collect();
    Ok(SecuritySummary::from_reads(status, &findings, &ingested, gate))
}

/// Every stored finding, in every code scope, under the default edition and
/// with **no** graph context (each carries the unbound placeholder, as
/// [`listings`](super::listings) leaves it): what [`FindingTally::of`] needs and
/// nothing the widget does not show. `in_test_symbols` is the graph's answer
/// for every stored finding. Starts no process.
///
/// # Errors
/// As [`findings`](super::findings): an invalid config or ignore file
/// (exit 2), a store failure, or `in_test_symbols`' own error.
fn summary_findings(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
    in_test_symbols: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<SecurityFindings> {
    super::findings(root, env, manifest, &FindingsFilter::default(), today, in_test_symbols, |kept| {
        Ok(kept.iter().map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoLocation)).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{ScopeFilter, StaleBecause};

    /// A status row in `state`, its failure log present when `failed`.
    fn row(name: &str, state: ScanState, failed: bool) -> ScannerStatus {
        ScannerStatus {
            scanner: name.to_string(),
            driver: name.to_string(),
            state,
            installed: None,
            install: None,
            licence: None,
            opt_in: false,
            nvd_api_key: None,
            last_failure: failed.then(|| format!(".logos/security/{name}.failed.log")),
            last_failure_cause: None,
            coverage: None,
        }
    }

    fn fresh() -> ScanState {
        ScanState::Fresh { snapshot_head: "h".into(), version: "1".into(), findings: 0, suppressed: 0, not_fresh: 0, test: 0, dev: 0, needs_review: 0, merged: 0 }
    }

    fn stale() -> ScanState {
        ScanState::Stale {
            because: StaleBecause::HeadMoved,
            snapshot_head: "h".into(),
            version: "1".into(),
            findings: 0,
            suppressed: 0,
            not_fresh: 0,
            test: 0,
            dev: 0,
            needs_review: 0,
            merged: 0,
        }
    }

    fn status(scanners: Vec<ScannerStatus>, other_reports: Vec<ScannerStatus>) -> SecurityStatus {
        SecurityStatus {
            head_sha: None,
            consent: ConsentState { consented: vec!["semgrep".into()], pending: Vec::new() },
            scanners,
            other_reports,
            suppressions: Vec::new(),
        }
    }

    /// The stored findings: `reports` stored reports, no finding listed.
    fn stored(reports: usize) -> SecurityFindings {
        SecurityFindings {
            edition: OwaspEdition::V2025,
            as_of: "2026-10-22".into(),
            scanned: (0..reports)
                .map(|n| crate::security::ScannedReport {
                    driver: format!("d{n}"),
                    version: "1".into(),
                    head_sha: "h".into(),
                    findings: 0,
                })
                .collect(),
            never_run: Vec::new(),
            notice: None,
            total: 0,
            scope: ScopeFilter::default(),
            hidden_by_scope: 0,
            hidden_needs_review: 0,
            hidden_dev: 0,
            findings: Vec::new(),
            categories: Vec::new(),
            suppressions: Vec::new(),
        }
    }

    /// A fresh report, a stale one and none are three states; a stored report
    /// with no finding is a clean scan — a count of zero — never a project
    /// nothing has looked at (BR-66).
    #[test]
    fn the_scan_state_follows_the_stored_reports_and_a_clean_scan_is_not_never_run() {
        let summary = |scanners, reports| SecuritySummary::from_reads(status(scanners, Vec::new()), &stored(reports), &[], None);

        let none = summary(vec![row("semgrep", ScanState::NeverRun, false)], 0);
        assert_eq!((none.scan, none.tally), (MemberScan::NeverRun, None), "nothing stored, nothing counted");

        let old = summary(vec![row("semgrep", stale(), false)], 1);
        assert_eq!(old.scan, MemberScan::Stale);
        assert_eq!(old.tally, Some(FindingTally::default()), "a stored report counts, even a stale one");

        let clean = summary(vec![row("semgrep", fresh(), false)], 1);
        assert_eq!(clean.scan, MemberScan::Fresh);
        assert_eq!(clean.tally, Some(FindingTally::default()), "a clean scan is zero findings, not never run");
    }

    /// "N of M scanners" counts only scanners that could have run: a disabled
    /// one and one with nothing to scan are in neither figure, even with a
    /// failure log left behind; a scanner not installed is in M; an
    /// unregistered tool with a stored report is in both.
    #[test]
    fn the_failed_scanners_are_counted_within_those_that_could_run() {
        let rows = vec![
            row("semgrep", fresh(), true),
            row("bandit", ScanState::NeverRun, false),
            row("gosec", ScanState::Disabled, true),
            row("trivy", ScanState::NotApplicable { reason: "no files".into() }, true),
            row("codeql", ScanState::NotInstalled, false),
        ];
        let other = vec![row("custom", fresh(), true)];
        let summary = SecuritySummary::from_reads(status(rows, other), &stored(2), &[], None);
        assert_eq!(
            (summary.scanners_failed, summary.scanners_total),
            (2, 4),
            "semgrep and the unregistered tool failed; semgrep, bandit, codeql and it could run"
        );
        assert!(summary.scanners_failed <= summary.scanners_total);
    }

    /// The newest stored scan is the greatest ingest time; none stored is none.
    #[test]
    fn the_newest_scan_is_the_latest_ingest_time() {
        let at = |times: &[i64]| {
            SecuritySummary::from_reads(status(Vec::new(), Vec::new()), &stored(times.len()), times, None).newest_scan_at
        };
        assert_eq!(at(&[300, 900, 100]), Some(900));
        assert_eq!(at(&[]), None);
    }
}
