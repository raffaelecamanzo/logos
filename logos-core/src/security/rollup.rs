//! One member's security roll-up for the workspace page ([FR-SC-16], [S-674]):
//! its scanner states, its fresh unsuppressed findings by severity and by OWASP
//! category, and what was left out of those counts and why.
//!
//! **A member never scanned is `never_run`, never zero** ([BR-66]). Its
//! [`findings`](MemberSecurity::findings) is `None` — not a count of zero — and
//! so is every category's, because no report is there to count from.
//!
//! **Only fresh, unsuppressed findings are counted.** A finding counts when the
//! report it comes from is `fresh` — stored at this HEAD over unchanged files,
//! [`ScanState::Fresh`], the same state `security status` shows — and no
//! current entry suppresses it. Everything else is tallied in
//! [`excluded`](MemberSecurity::excluded), so the counted findings and the
//! excluded ones add up to every stored finding.
//!
//! A category is covered here only by a **fresh** report from a scanner that
//! can detect it: the member page's coverage ([`CategoryCoverage`], [BR-66])
//! narrowed to the fresh scanners, so a covered category's count of zero means
//! a current scan found nothing, never that an old one did.
//!
//! [FR-SC-16]: ../../../docs/specs/requirements/FR-SC-16.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-674]: ../../../docs/planning/journal.md#s-674-the-workspace-rolls-security-up-per-member
//! [`CategoryCoverage`]: crate::history::security::owasp::CategoryCoverage

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{ListedFinding, ScanState, ScannerStatus, SecurityFindings, SecurityStatus};
use crate::history::security::owasp::CoverageState;
use crate::history::security::{OwaspEdition, Suppression};
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

/// Findings by severity, and their sum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCounts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub info: usize,
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
        self.total += other.total;
    }
}

/// The stored findings the counts leave out, by why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Excluded {
    /// Findings in a report that is not fresh: stale, or from a scanner the
    /// member's config now disables.
    pub not_fresh: usize,
    /// Findings in a fresh report that a current entry suppresses.
    pub suppressed: usize,
}

impl Excluded {
    /// Add `other`'s counts to these.
    pub fn absorb(&mut self, other: &Self) {
        self.not_fresh += other.not_fresh;
        self.suppressed += other.suppressed;
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
    /// Registered scanners the member's config does not disable, plus every
    /// unregistered tool whose report is stored: the denominator of
    /// [`scanners_fresh`](Self::scanners_fresh).
    pub scanners_enabled: usize,
    /// Each scanner's one state, as `security status` reports it: the
    /// registered ones in registry order, then the unregistered tools.
    pub scanners: Vec<ScannerStatus>,
    /// Fresh, unsuppressed findings by severity; `None` unless
    /// [`scan`](Self::scan) is `fresh` — nothing current to count.
    pub findings: Option<SeverityCounts>,
    /// The stored findings left out of [`findings`](Self::findings).
    pub excluded: Excluded,
    /// The counted findings by category, one entry per edition `listings`
    /// gave, in its order.
    pub owasp: Vec<MemberEdition>,
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
        let counted = |item: &ListedFinding| {
            fresh_drivers.contains(item.finding.scanner_name.as_str())
                && !matches!(item.suppression, Some(Suppression::Suppressed { .. }))
        };

        let stored = listings.first().is_some_and(|l| !l.scanned.is_empty());
        let scan = match (fresh_drivers.is_empty(), stored) {
            (false, _) => MemberScan::Fresh,
            (true, true) => MemberScan::Stale,
            (true, false) => MemberScan::NeverRun,
        };

        // Every edition lists the same stored findings, so the first one's
        // tally is the member's.
        let mut counts = SeverityCounts::default();
        let mut excluded = Excluded::default();
        for item in listings.first().map_or(&[][..], |l| &l.findings) {
            if counted(item) {
                counts.add(item.finding.severity);
            } else if fresh_drivers.contains(item.finding.scanner_name.as_str()) {
                excluded.suppressed += 1;
            } else {
                excluded.not_fresh += 1;
            }
        }

        let owasp = listings
            .iter()
            .map(|listed| {
                let mut per_category: BTreeMap<&str, usize> = BTreeMap::new();
                let mut unclassified = 0;
                for item in listed.findings.iter().filter(|item| counted(item)) {
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

        let enabled = |s: &&ScannerStatus| !matches!(s.state, ScanState::Disabled);
        Self {
            scan,
            scanners_fresh: scanners.iter().filter(|s| is_fresh(s)).count(),
            scanners_enabled: scanners.iter().filter(enabled).count(),
            findings: (scan == MemberScan::Fresh).then_some(counts),
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
        ] {
            for _ in 0..times {
                counts.add(severity);
            }
        }
        assert_eq!(counts, SeverityCounts { critical: 1, high: 2, medium: 3, low: 4, info: 5, total: 15 });
    }

    /// Absorbing sums every field, the total included.
    #[test]
    fn absorbing_sums_every_field() {
        let mut sum = SeverityCounts { critical: 1, high: 2, medium: 3, low: 4, info: 5, total: 15 };
        sum.absorb(&SeverityCounts { critical: 10, high: 20, medium: 30, low: 40, info: 50, total: 150 });
        assert_eq!(sum, SeverityCounts { critical: 11, high: 22, medium: 33, low: 44, info: 55, total: 165 });
        let mut excluded = Excluded { not_fresh: 1, suppressed: 2 };
        excluded.absorb(&Excluded { not_fresh: 10, suppressed: 20 });
        assert_eq!(excluded, Excluded { not_fresh: 11, suppressed: 22 });
    }
}
