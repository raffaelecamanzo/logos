//! One member's security roll-up for the workspace page ([FR-SC-16], [S-674]):
//! its scanner states, its fresh unsuppressed findings by severity and by OWASP
//! category, and what was left out of those counts and why.
//!
//! **A member never scanned is `never_run`, never zero** ([BR-66]). Its
//! [`findings`](MemberSecurity::findings) is `None` — not a count of zero — and
//! so is every category's, because no report is there to count from.
//!
//! **Only fresh, unsuppressed findings in production code are counted — fresh
//! per finding.** A finding counts by [`counts`](crate::history::security::counts),
//! the one definition every security surface shares ([S-760]): no current
//! entry suppresses it, its own file is unchanged since the scan, and it does
//! not sit in test code ([FR-SC-28]) — by this member's own graph. A report's own
//! freshness does not decide it, so a member whose only report is a `--path`
//! scan still counts the findings in files nothing has touched. Everything
//! else is tallied in [`excluded`](MemberSecurity::excluded), so the counted
//! findings and the excluded ones add up to every stored finding. How many
//! reports are fresh is its own figure ([`reports_fresh`](MemberSecurity::reports_fresh)
//! of [`reports_stored`](MemberSecurity::reports_stored)).
//!
//! A category is covered here only by a **fresh** report from a scanner that
//! can detect it: the member page's coverage ([`CategoryCoverage`], [BR-66])
//! narrowed to the fresh scanners, so a covered category's count of zero means
//! a current scan found nothing, never that an old one did.
//!
//! [FR-SC-16]: ../../../docs/specs/requirements/FR-SC-16.md
//! [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-674]: ../../../docs/planning/journal.md#s-674-the-workspace-rolls-security-up-per-member
//! [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
//! [`CategoryCoverage`]: crate::history::security::owasp::CategoryCoverage

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{ListedFinding, ScanState, ScannerStatus, SecurityFindings, SecurityStatus};
use crate::history::security::owasp::CoverageState;
use crate::history::security::{LeftOut, OwaspEdition};
use crate::history::Severity;

/// Whether a member has a current scan at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberScan {
    /// No report is stored: nothing has looked ([BR-66]).
    ///
    /// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
    NeverRun,
    /// Reports are stored, and none is fresh at HEAD.
    Stale,
    /// At least one stored report is fresh at HEAD.
    Fresh,
}

/// Findings by severity, and their sum. `unrated` findings have their own
/// count, never folded into `medium` (S-768).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCounts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub info: usize,
    pub unrated: usize,
    pub total: usize,
}

impl SeverityCounts {
    fn add(&mut self, severity: Severity) {
        *match severity {
            Severity::Critical => &mut self.critical,
            Severity::High => &mut self.high,
            Severity::Medium => &mut self.medium,
            Severity::Low => &mut self.low,
            Severity::Info => &mut self.info,
            Severity::Unrated => &mut self.unrated,
        } += 1;
        self.total += 1;
    }

    /// Add `other`'s counts to these.
    pub fn absorb(&mut self, other: &Self) {
        self.critical += other.critical;
        self.high += other.high;
        self.medium += other.medium;
        self.low += other.low;
        self.info += other.info;
        self.unrated += other.unrated;
        self.total += other.total;
    }
}

/// The stored findings the counts leave out, by why. Each is left out for the
/// first reason that holds, in this order — suppressed, not fresh, test code —
/// so the buckets never share a finding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Excluded {
    /// Unsuppressed findings whose own file changed since the scan.
    pub not_fresh: usize,
    /// Findings a current entry suppresses, whatever their file's state.
    pub suppressed: usize,
    /// Fresh, unsuppressed findings in test code ([FR-SC-28]): "N more in
    /// test code".
    ///
    /// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
    pub test: usize,
}

impl Excluded {
    /// Count one finding left out for `why`.
    pub(crate) fn add(&mut self, why: LeftOut) {
        *match why {
            LeftOut::Suppressed => &mut self.suppressed,
            LeftOut::NotFresh => &mut self.not_fresh,
            LeftOut::Test => &mut self.test,
        } += 1;
    }

    /// Add `other`'s counts to these.
    pub fn absorb(&mut self, other: &Self) {
        self.not_fresh += other.not_fresh;
        self.suppressed += other.suppressed;
        self.test += other.test;
    }
}

/// Every stored finding of one project, counted or left out by
/// [`counts`](crate::history::security::counts): what the Security page's
/// figure and a member's roll-up row both state, so they cannot differ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FindingTally {
    /// The counted findings by severity.
    pub counted: SeverityCounts,
    /// The stored findings left out of `counted`, and why.
    pub excluded: Excluded,
}

impl FindingTally {
    /// The tally of `findings` — every stored finding, not a filtered view.
    pub fn of(findings: &[ListedFinding]) -> Self {
        let mut tally = Self::default();
        for item in findings {
            match item.left_out() {
                None => tally.counted.add(item.finding.severity),
                Some(why) => tally.excluded.add(why),
            }
        }
        tally
    }
}

/// One OWASP category in one member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberCategory {
    /// The category id (`A05:2025`).
    pub id: &'static str,
    /// The category name.
    pub name: &'static str,
    /// Fresh, unsuppressed findings under it; `None` when no fresh report
    /// from a scanner that can detect it is stored — not covered, never zero.
    pub findings: Option<usize>,
}

/// One member's categories under one OWASP edition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberEdition {
    pub edition: OwaspEdition,
    /// Every category of the edition, in OWASP's order.
    pub categories: Vec<MemberCategory>,
    /// Counted findings under no category of this edition (no CWE, or one the
    /// edition does not map): in no category's count, so stated apart, as the
    /// member's own page does.
    pub unclassified: usize,
}

/// One member's security roll-up ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberSecurity {
    pub scan: MemberScan,
    /// Scanners whose stored report is fresh, registered or not.
    pub scanners_fresh: usize,
    /// Registered scanners the member's config does not disable and that have
    /// something to scan there (a `not_applicable` one is no gap, S-769), plus
    /// every unregistered tool whose report is stored: the denominator of
    /// [`scanners_fresh`](Self::scanners_fresh).
    pub scanners_enabled: usize,
    /// Each scanner's one state, as `security status` reports it: the
    /// registered ones in registry order, then the unregistered tools.
    pub scanners: Vec<ScannerStatus>,
    /// Stored reports — a disabled scanner's included: the denominator of
    /// [`reports_fresh`](Self::reports_fresh).
    pub reports_stored: usize,
    /// Stored reports that are fresh at HEAD ([`ScanState::Fresh`]). A disabled
    /// scanner's report is in [`reports_stored`](Self::reports_stored) and never
    /// here: `status` reports it `disabled`, not fresh.
    pub reports_fresh: usize,
    /// Fresh, unsuppressed findings by severity, counted per finding; `None`
    /// only when no report is stored ([`MemberScan::NeverRun`]) — nothing to
    /// count from, which is not a count of zero.
    pub findings: Option<SeverityCounts>,
    /// The stored findings left out of [`findings`](Self::findings).
    pub excluded: Excluded,
    /// The counted findings by category, one entry per edition `listings`
    /// gave, in its order.
    pub owasp: Vec<MemberEdition>,
}

/// Whether a member has a current scan at all: `fresh` when any stored report
/// is fresh at HEAD, else `stale` when one is stored, else `never_run`. The one
/// definition every security roll-up shares, the Dashboard widget's
/// ([`SecuritySummary`](super::SecuritySummary)) included.
pub(super) fn scan_state(fresh: bool, stored: bool) -> MemberScan {
    match (fresh, stored) {
        (true, _) => MemberScan::Fresh,
        (false, true) => MemberScan::Stale,
        (false, false) => MemberScan::NeverRun,
    }
}

/// Whether a scanner could have run here: not disabled, and with something to
/// scan ([`ScanState::NotApplicable`] is no gap, S-769). The denominator of
/// "N of M scanners", in a member's row and in the Dashboard widget alike.
pub(super) fn could_run(scanner: &ScannerStatus) -> bool {
    !matches!(scanner.state, ScanState::Disabled | ScanState::NotApplicable { .. })
}

impl MemberSecurity {
    /// The roll-up of one member's `security status` and its stored findings
    /// listed under each edition ([`listings`](super::page::listings)). Pure:
    /// it reads nothing, so every case is assertable without a store.
    pub fn from_reads(status: SecurityStatus, listings: &[SecurityFindings]) -> Self {
        let scanners: Vec<ScannerStatus> = status.scanners.into_iter().chain(status.other_reports).collect();
        let is_fresh = |s: &ScannerStatus| matches!(s.state, ScanState::Fresh { .. });
        let fresh_drivers: BTreeSet<&str> = scanners.iter().filter(|s| is_fresh(s)).map(|s| s.driver.as_str()).collect();
        let fresh_names: BTreeSet<&str> = scanners.iter().filter(|s| is_fresh(s)).map(|s| s.scanner.as_str()).collect();
        let stored = listings.first().is_some_and(|l| !l.scanned.is_empty());
        let scan = scan_state(!fresh_drivers.is_empty(), stored);

        // Every edition lists the same stored findings, so the first one's
        // tally is the member's.
        let FindingTally { counted, excluded } = FindingTally::of(listings.first().map_or(&[][..], |l| &l.findings));

        let owasp = listings
            .iter()
            .map(|listed| {
                let mut per_category: BTreeMap<&str, usize> = BTreeMap::new();
                let mut unclassified = 0;
                for item in listed.findings.iter().filter(|item| item.is_counted()) {
                    unclassified += usize::from(item.owasp.is_empty());
                    for id in &item.owasp {
                        *per_category.entry(id).or_default() += 1;
                    }
                }
                MemberEdition {
                    unclassified,
                    edition: listed.edition,
                    categories: listed
                        .categories
                        .iter()
                        .map(|c| MemberCategory {
                            id: c.id,
                            name: c.name,
                            findings: (matches!(c.state, CoverageState::Covered { .. })
                                && c.enabled_scanners.iter().any(|s| fresh_names.contains(s)))
                            .then(|| per_category.get(c.id).copied().unwrap_or(0)),
                        })
                        .collect(),
                }
            })
            .collect();

        let fresh = scanners.iter().filter(|s| is_fresh(s)).count();
        Self {
            scan,
            scanners_fresh: fresh,
            scanners_enabled: scanners.iter().filter(|s| could_run(s)).count(),
            reports_stored: listings.first().map_or(0, |l| l.scanned.len()),
            reports_fresh: fresh,
            findings: (scan != MemberScan::NeverRun).then_some(counted),
            excluded,
            owasp,
            scanners,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each severity lands in its own field and in the total.
    #[test]
    fn each_severity_is_counted_in_its_own_field() {
        let mut counts = SeverityCounts::default();
        for (severity, times) in [
            (Severity::Critical, 1),
            (Severity::High, 2),
            (Severity::Medium, 3),
            (Severity::Low, 4),
            (Severity::Info, 5),
            (Severity::Unrated, 6),
        ] {
            for _ in 0..times {
                counts.add(severity);
            }
        }
        assert_eq!(counts, SeverityCounts { critical: 1, high: 2, medium: 3, low: 4, info: 5, unrated: 6, total: 21 });
    }

    /// Absorbing sums every field, the total included.
    #[test]
    fn absorbing_sums_every_field() {
        let mut sum = SeverityCounts { critical: 1, high: 2, medium: 3, low: 4, info: 5, unrated: 6, total: 21 };
        sum.absorb(&SeverityCounts { critical: 10, high: 20, medium: 30, low: 40, info: 50, unrated: 60, total: 210 });
        assert_eq!(sum, SeverityCounts { critical: 11, high: 22, medium: 33, low: 44, info: 55, unrated: 66, total: 231 });
        let mut excluded = Excluded { not_fresh: 1, suppressed: 2, test: 3 };
        excluded.absorb(&Excluded { not_fresh: 10, suppressed: 20, test: 30 });
        assert_eq!(excluded, Excluded { not_fresh: 11, suppressed: 22, test: 33 });
    }
}
