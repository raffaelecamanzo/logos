//! OWASP Top 10 classification of stored findings ([S-664], [FR-SC-04],
//! [ADR-79]).
//!
//! Two parts, both pure — no store, no clock, no filesystem:
//!
//! - **A versioned table.** For each edition ([`OwaspEdition`]: 2025, the
//!   default, and 2021) the ten categories, the CWE identifiers OWASP lists under
//!   each, and the registered scanners that can detect the category. The table is
//!   compiled in; nothing about it is ingested, so an edition is a read-time
//!   choice.
//! - **A classifier** ([`classify`]) over [`SecurityFinding`]s as
//!   [`read_latest`](super::read_latest) returns them, taking the edition and the
//!   enabled-scanner set. It returns each finding's categories and, per category,
//!   whether the enabled scanners can see it at all.
//!
//! # Three states, never a bare zero
//! A finding's CWE tags decide its categories. A finding with no CWE tag is
//! [`Unclassified::NoCwe`]; one whose CWEs the edition does not list is
//! [`Unclassified::Unmapped`]. Neither is dropped. A category no enabled scanner
//! can detect is [`CoverageState::NotCovered`]; only a category some enabled
//! scanner *can* detect reports a finding count, and then `0` means the scanners
//! looked and found nothing ([BR-66]).
//!
//! # Scanner identity
//! The table names scanners by their registry id (`semgrep`, `trivy`, …; see
//! [`SCANNER_IDS`]). The enabled set is whatever the caller resolved from the
//! registry and the user's configuration — installed and not disabled. This
//! module does not read the registry: a finding's `scanner_name` is the driver's
//! own spelling (`Semgrep OSS`) and plays no part in classification, which is
//! decided by CWE alone.
//!
//! # Provenance of the table
//! The CWE lists are a transcription of the lists OWASP publishes for each
//! edition, one category per CWE (a CWE OWASP lists under two categories is
//! kept under its primary one). Scanner capability is a statement about what each
//! scanner's rule packs target, not a promise about a given rule set: it decides
//! *not covered* versus *covered*, never a finding's category.
//!
//! [BR-66]: ../../../../docs/specs/software-spec.md#328-security-findings
//! [FR-SC-04]: ../../../../docs/specs/requirements/FR-SC-04.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [S-664]: ../../../../docs/planning/journal.md#s-664-findings-are-classified-by-owasp-top-10-2025-and-2021

use std::collections::BTreeSet;

use super::SecurityFinding;

/// The registry ids of the nine registered scanners, as the table names them.
pub const SCANNER_IDS: [&str; 9] = [
    "semgrep",
    "trivy",
    "gitleaks",
    "osv-scanner",
    "bandit",
    "gosec",
    "checkov",
    "codeql",
    "dependency-check",
];

/// An OWASP Top 10 edition. Classification is computed on read for whichever
/// edition is asked for, so switching one never re-ingests anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize)]
pub enum OwaspEdition {
    /// OWASP Top 10:2025 — the default.
    #[default]
    #[serde(rename = "2025")]
    V2025,
    /// OWASP Top 10:2021.
    #[serde(rename = "2021")]
    V2021,
}

impl OwaspEdition {
    /// The year token (`"2025"`, `"2021"`).
    pub fn as_str(self) -> &'static str {
        match self {
            OwaspEdition::V2025 => "2025",
            OwaspEdition::V2021 => "2021",
        }
    }

    /// The edition a year token names, or `None` for an edition Logos has no
    /// table for.
    pub fn from_year(token: &str) -> Option<Self> {
        match token.trim() {
            "2025" => Some(OwaspEdition::V2025),
            "2021" => Some(OwaspEdition::V2021),
            _ => None,
        }
    }
}

/// One OWASP category of one edition, as the compiled-in table holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwaspCategory {
    /// The category id, edition-qualified (`A05:2025`).
    pub id: &'static str,
    /// The category name (`Injection`).
    pub name: &'static str,
    /// The CWE numbers OWASP lists under it.
    pub cwes: &'static [u32],
    /// The registered scanners ([`SCANNER_IDS`]) that can detect it.
    pub scanners: &'static [&'static str],
}

// Scanner capability, shared by the categories that several rows of the table
// name alike.
const CODE_ANALYSIS: &[&str] = &["semgrep", "codeql", "bandit", "gosec"];
const ACCESS_CONTROL: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "checkov"];
const MISCONFIGURATION: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "checkov", "trivy"];
const CRYPTOGRAPHY: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "checkov", "trivy"];
const DESIGN: &[&str] = &["semgrep", "codeql"];
const AUTHENTICATION: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "gitleaks", "trivy"];
const INTEGRITY: &[&str] = &["semgrep", "codeql", "bandit"];
const LOGGING: &[&str] = &["semgrep", "codeql", "checkov"];
const COMPONENTS: &[&str] = &["trivy", "osv-scanner", "dependency-check"];

/// The 2025 edition.
static TABLE_2025: [OwaspCategory; 10] = [
    OwaspCategory {
        id: "A01:2025",
        name: "Broken Access Control",
        cwes: &[
            22, 23, 36, 59, 61, 65, 66, 200, 201, 219, 276, 284, 285, 352, 359, 377, 379, 425, 441,
            497, 538, 540, 548, 552, 566, 601, 615, 639, 668, 732, 749, 862, 863, 918, 922, 1275,
        ],
        scanners: ACCESS_CONTROL,
    },
    OwaspCategory {
        id: "A02:2025",
        name: "Security Misconfiguration",
        cwes: &[
            5, 11, 13, 15, 16, 260, 315, 489, 526, 547, 611, 614, 942, 1004, 1032, 1174,
        ],
        scanners: MISCONFIGURATION,
    },
    OwaspCategory {
        id: "A03:2025",
        name: "Software Supply Chain Failures",
        cwes: &[447, 477, 937, 1035, 1104, 1329, 1357],
        scanners: &["trivy", "osv-scanner", "dependency-check", "checkov"],
    },
    OwaspCategory {
        id: "A04:2025",
        name: "Cryptographic Failures",
        cwes: &[
            261, 296, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 335, 336,
            337, 338, 340, 347, 523, 757, 759, 760, 780, 916, 1240,
        ],
        scanners: CRYPTOGRAPHY,
    },
    OwaspCategory {
        id: "A05:2025",
        name: "Injection",
        cwes: &[
            20, 74, 76, 77, 78, 79, 80, 83, 86, 87, 88, 89, 90, 91, 93, 94, 95, 96, 97, 98, 99,
            100, 113, 116, 138, 184, 470, 471, 564, 610, 643, 917,
        ],
        scanners: CODE_ANALYSIS,
    },
    OwaspCategory {
        id: "A06:2025",
        name: "Insecure Design",
        cwes: &[
            73, 183, 256, 266, 269, 311, 312, 313, 316, 419, 430, 434, 444, 451, 472, 501, 522,
            525, 539, 579, 598, 602, 642, 646, 653, 656, 657, 799, 807, 841, 1021, 1173,
        ],
        scanners: DESIGN,
    },
    OwaspCategory {
        id: "A07:2025",
        name: "Authentication Failures",
        cwes: &[
            255, 259, 287, 288, 289, 290, 291, 293, 294, 295, 297, 300, 302, 303, 304, 305, 306,
            307, 308, 309, 346, 384, 521, 613, 620, 640, 798, 940, 1216, 1390, 1391, 1392, 1393,
        ],
        scanners: AUTHENTICATION,
    },
    OwaspCategory {
        id: "A08:2025",
        name: "Software or Data Integrity Failures",
        cwes: &[345, 353, 426, 494, 502, 565, 784, 829, 830, 915],
        scanners: INTEGRITY,
    },
    OwaspCategory {
        id: "A09:2025",
        name: "Security Logging and Alerting Failures",
        cwes: &[117, 221, 223, 532, 778],
        scanners: LOGGING,
    },
    OwaspCategory {
        id: "A10:2025",
        name: "Mishandling of Exceptional Conditions",
        cwes: &[
            209, 215, 234, 235, 248, 252, 274, 280, 369, 390, 391, 394, 396, 397, 431, 456, 476,
            478, 479, 484, 550, 636, 703, 754, 755,
        ],
        scanners: &["semgrep", "codeql", "gosec"],
    },
];

/// The 2021 edition.
static TABLE_2021: [OwaspCategory; 10] = [
    OwaspCategory {
        id: "A01:2021",
        name: "Broken Access Control",
        cwes: &[
            22, 23, 35, 59, 200, 201, 219, 264, 275, 276, 284, 285, 352, 359, 377, 402, 425, 441,
            497, 538, 540, 548, 552, 566, 601, 639, 651, 668, 706, 862, 863, 913, 922, 1275,
        ],
        scanners: ACCESS_CONTROL,
    },
    OwaspCategory {
        id: "A02:2021",
        name: "Cryptographic Failures",
        cwes: &[
            261, 296, 310, 319, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 335, 336,
            337, 338, 340, 347, 523, 720, 757, 759, 760, 780, 818, 916,
        ],
        scanners: CRYPTOGRAPHY,
    },
    OwaspCategory {
        id: "A03:2021",
        name: "Injection",
        cwes: &[
            20, 74, 75, 77, 78, 79, 80, 83, 87, 88, 89, 90, 91, 93, 94, 95, 96, 97, 98, 99, 100,
            113, 116, 138, 184, 470, 471, 564, 610, 643, 644, 652, 917,
        ],
        scanners: CODE_ANALYSIS,
    },
    OwaspCategory {
        id: "A04:2021",
        name: "Insecure Design",
        cwes: &[
            73, 183, 209, 213, 235, 256, 257, 266, 269, 280, 311, 312, 313, 316, 419, 430, 434,
            444, 451, 472, 501, 522, 525, 539, 579, 598, 602, 642, 646, 650, 653, 656, 657, 799,
            807, 840, 841, 927, 1021, 1173,
        ],
        scanners: DESIGN,
    },
    OwaspCategory {
        id: "A05:2021",
        name: "Security Misconfiguration",
        cwes: &[
            2, 11, 13, 15, 16, 260, 315, 520, 526, 537, 541, 547, 611, 614, 756, 776, 942, 1004,
            1032, 1174,
        ],
        scanners: MISCONFIGURATION,
    },
    OwaspCategory {
        id: "A06:2021",
        name: "Vulnerable and Outdated Components",
        cwes: &[937, 1035, 1104],
        scanners: COMPONENTS,
    },
    OwaspCategory {
        id: "A07:2021",
        name: "Identification and Authentication Failures",
        cwes: &[
            255, 259, 287, 288, 290, 294, 295, 297, 300, 302, 304, 306, 307, 346, 384, 521, 613,
            620, 640, 798, 940, 1216,
        ],
        scanners: AUTHENTICATION,
    },
    OwaspCategory {
        id: "A08:2021",
        name: "Software and Data Integrity Failures",
        cwes: &[345, 353, 426, 494, 502, 565, 784, 829, 830, 915],
        scanners: INTEGRITY,
    },
    OwaspCategory {
        id: "A09:2021",
        name: "Security Logging and Monitoring Failures",
        cwes: &[117, 223, 532, 778],
        scanners: LOGGING,
    },
    OwaspCategory {
        id: "A10:2021",
        name: "Server-Side Request Forgery",
        cwes: &[918],
        scanners: &["semgrep", "codeql", "bandit", "gosec"],
    },
];

/// The ten categories of `edition`, in OWASP's order.
pub fn categories(edition: OwaspEdition) -> &'static [OwaspCategory] {
    match edition {
        OwaspEdition::V2025 => &TABLE_2025,
        OwaspEdition::V2021 => &TABLE_2021,
    }
}

/// The category of `edition` that lists CWE number `cwe`, if any.
pub fn category_of_cwe(edition: OwaspEdition, cwe: u32) -> Option<&'static OwaspCategory> {
    categories(edition).iter().find(|c| c.cwes.contains(&cwe))
}

/// The number in a stored `CWE-<n>` identifier; `None` for any other spelling.
fn cwe_number(tag: &str) -> Option<u32> {
    let digits = tag.strip_prefix("CWE-")?;
    // `u32::parse` also accepts a leading `+`; a stored tag never has one.
    digits
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| digits.parse().ok())?
}

/// Why a finding sits under no category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unclassified {
    /// The scanner gave the finding no CWE tag.
    NoCwe,
    /// The finding has CWE tags, none of which the edition lists.
    Unmapped,
}

/// One finding's classification, keyed by the pair that identifies it: its
/// scanner and its fingerprint (unique within the scanner's snapshot).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FindingClassification {
    /// The scanner that reported the finding.
    pub scanner_name: String,
    /// The finding's fingerprint.
    pub fingerprint: String,
    /// The category ids it falls under, in OWASP's order; empty exactly when
    /// [`unclassified`](Self::unclassified) is set.
    pub categories: Vec<&'static str>,
    /// Why it has no category, when it has none.
    pub unclassified: Option<Unclassified>,
}

/// Whether the enabled scanners can see a category at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CoverageState {
    /// No enabled scanner can detect the category. This is not a count of zero:
    /// nothing was looking ([BR-66](../../../../docs/specs/software-spec.md#328-security-findings)).
    NotCovered,
    /// At least one enabled scanner can detect it. `findings` is the number of
    /// classified findings under it; `0` here means the scanners found nothing.
    Covered {
        /// Findings classified under the category.
        findings: usize,
    },
}

/// One category's coverage under an enabled-scanner set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CategoryCoverage {
    /// The category id (`A05:2025`).
    pub id: &'static str,
    /// The category name.
    pub name: &'static str,
    /// Every registered scanner that can detect it.
    pub scanners: Vec<&'static str>,
    /// The subset of [`scanners`](Self::scanners) that is enabled.
    pub enabled_scanners: Vec<&'static str>,
    /// Covered with a count, or not covered.
    pub state: CoverageState,
    /// The part of a covered category's [`findings`](CoverageState::Covered)
    /// that sits in production code, suppressed ones included: the figure the
    /// Security page's OWASP list states under its default code scope. `None`
    /// for a not-covered category, and until [`count_production`] has been given
    /// the findings' code scopes ([`classify`] alone knows none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_findings: Option<usize>,
    /// The production-code part of [`findings`](CoverageState::Covered) whose
    /// scanner marks it low-confidence, "needs review" ([FR-SC-31]), apart from
    /// [`production_findings`](Self::production_findings): production, needs
    /// review and test code partition `findings`' every code scope. `None` as
    /// `production_findings` is.
    ///
    /// [FR-SC-31]: ../../../../docs/specs/requirements/FR-SC-31.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_review_findings: Option<usize>,
    /// The part of [`findings`](CoverageState::Covered) on a dev-only
    /// dependency ([FR-SC-30]), apart from
    /// [`production_findings`](Self::production_findings): production, dev,
    /// needs review and test code partition `findings`' every code scope. `None`
    /// as `production_findings` is.
    ///
    /// [FR-SC-30]: ../../../../docs/specs/requirements/FR-SC-30.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev_findings: Option<usize>,
}

/// The result of [`classify`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OwaspReport {
    /// The edition the findings were classified under.
    pub edition: OwaspEdition,
    /// One entry per input finding, in input order.
    pub findings: Vec<FindingClassification>,
    /// One entry per category of the edition, in OWASP's order.
    pub categories: Vec<CategoryCoverage>,
    /// Findings under no category.
    pub unclassified: usize,
}

/// Classify `findings` under `edition`, with `enabled` naming the scanners (by
/// registry id, [`SCANNER_IDS`]) that are enabled and installed.
///
/// Pure: the same findings, edition and set give the same report, and nothing is
/// written — the stored findings carry CWE tags, never a category, so the same
/// findings classify under either edition without a re-ingest. Scanner ids match
/// ignoring ASCII case; an id the table does not know enables nothing.
pub fn classify<S: AsRef<str>>(
    findings: &[SecurityFinding],
    edition: OwaspEdition,
    enabled: &[S],
) -> OwaspReport {
    let table = categories(edition);
    let enabled: BTreeSet<String> = enabled
        .iter()
        .map(|s| s.as_ref().to_ascii_lowercase())
        .collect();

    let mut per_category = vec![0usize; table.len()];
    let mut classified = Vec::with_capacity(findings.len());
    let mut unclassified = 0;
    for finding in findings {
        let mut hit = vec![false; table.len()];
        for number in finding.cwes.iter().filter_map(|t| cwe_number(t)) {
            if let Some(idx) = table.iter().position(|c| c.cwes.contains(&number)) {
                hit[idx] = true;
            }
        }
        let ids: Vec<&'static str> = table
            .iter()
            .zip(&hit)
            .filter(|(_, &h)| h)
            .map(|(c, _)| c.id)
            .collect();
        for (count, _) in per_category.iter_mut().zip(&hit).filter(|(_, &h)| h) {
            *count += 1;
        }
        let reason = if !ids.is_empty() {
            None
        } else if finding.cwes.is_empty() {
            Some(Unclassified::NoCwe)
        } else {
            Some(Unclassified::Unmapped)
        };
        unclassified += usize::from(reason.is_some());
        classified.push(FindingClassification {
            scanner_name: finding.scanner_name.clone(),
            fingerprint: finding.fingerprint.clone(),
            categories: ids,
            unclassified: reason,
        });
    }

    let coverage = table
        .iter()
        .zip(per_category)
        .map(|(category, count)| {
            let enabled_scanners: Vec<&'static str> = category
                .scanners
                .iter()
                .copied()
                .filter(|s| enabled.contains(*s))
                .collect();
            let state = if enabled_scanners.is_empty() {
                CoverageState::NotCovered
            } else {
                CoverageState::Covered { findings: count }
            };
            CategoryCoverage {
                id: category.id,
                name: category.name,
                scanners: category.scanners.to_vec(),
                enabled_scanners,
                state,
                production_findings: None,
                needs_review_findings: None,
                dev_findings: None,
            }
        })
        .collect();

    OwaspReport {
        edition,
        findings: classified,
        categories: coverage,
        unclassified,
    }
}

/// Set each covered category's [`production_findings`](CategoryCoverage::production_findings)
/// [`needs_review_findings`](CategoryCoverage::needs_review_findings) and
/// [`dev_findings`](CategoryCoverage::dev_findings): the classified findings
/// under it whose entry in `production`, `needs_review` or `dev` is `true`. Each
/// is one flag per entry of `classified`, in the same order, and no finding has
/// two; a not-covered category keeps
/// `None`, never a count of zero.
pub fn count_production(
    categories: &mut [CategoryCoverage],
    classified: &[FindingClassification],
    production: &[bool],
    needs_review: &[bool],
    dev: &[bool],
) {
    debug_assert_eq!(classified.len(), production.len());
    debug_assert_eq!(classified.len(), needs_review.len());
    debug_assert_eq!(classified.len(), dev.len());
    for category in categories
        .iter_mut()
        .filter(|c| matches!(c.state, CoverageState::Covered { .. }))
    {
        category.production_findings = Some(
            classified
                .iter()
                .zip(production)
                .filter(|(class, &production)| production && class.categories.contains(&category.id))
                .count(),
        );
        category.needs_review_findings = Some(
            classified
                .iter()
                .zip(needs_review)
                .filter(|(class, &review)| review && class.categories.contains(&category.id))
                .count(),
        );
        category.dev_findings = Some(
            classified
                .iter()
                .zip(dev)
                .filter(|(class, &dev)| dev && class.categories.contains(&category.id))
                .count(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::security::{FindingBinding, ScannerConfidence, Severity};

    fn finding(scanner: &str, fingerprint: &str, cwes: &[&str]) -> SecurityFinding {
        SecurityFinding {
            scanner_name: scanner.into(),
            scanner_version: "1".into(),
            head_sha: "h".repeat(40),
            fingerprint: fingerprint.into(),
            rule_id: "rule".into(),
            severity: Severity::High,
            confidence: ScannerConfidence::Unstated,
            cwes: cwes.iter().map(|s| s.to_string()).collect(),
            message: "m".into(),
            redacted: false,
            binding: FindingBinding::Unbound,
            report_path: None,
            path: None,
            start_line: None,
            start_column: None,
            end_line: None,
            end_column: None,
            commits: Vec::new(),
            legacy_fingerprints: Vec::new(),
            anchor: None,
            freshness: crate::history::FRESHNESS_NA,
            reporters: Vec::new(),
        }
    }

    fn only_category(report: &OwaspReport, index: usize) -> &'static str {
        assert_eq!(
            report.findings[index].categories.len(),
            1,
            "{:?}",
            report.findings[index]
        );
        report.findings[index].categories[0]
    }

    fn coverage<'a>(report: &'a OwaspReport, id: &str) -> &'a CategoryCoverage {
        report
            .categories
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no {id}"))
    }

    const NONE: &[&str] = &[];

    #[test]
    fn cwe_89_is_injection_under_both_editions() {
        let f = [finding("Semgrep OSS", "a", &["CWE-89"])];
        let r25 = classify(&f, OwaspEdition::V2025, NONE);
        let r21 = classify(&f, OwaspEdition::V2021, NONE);
        assert_eq!(only_category(&r25, 0), "A05:2025");
        assert_eq!(only_category(&r21, 0), "A03:2021");
        for edition in [OwaspEdition::V2025, OwaspEdition::V2021] {
            assert_eq!(category_of_cwe(edition, 89).unwrap().name, "Injection");
        }
    }

    #[test]
    fn a_cwe_mapped_differently_between_editions_follows_the_edition() {
        // SSRF moved into access control; vulnerable components became supply
        // chain; error-message exposure moved from design to exceptional conditions.
        for (cwe, name25, name21) in [
            (918, "Broken Access Control", "Server-Side Request Forgery"),
            (
                1104,
                "Software Supply Chain Failures",
                "Vulnerable and Outdated Components",
            ),
            (
                209,
                "Mishandling of Exceptional Conditions",
                "Insecure Design",
            ),
        ] {
            assert_eq!(
                category_of_cwe(OwaspEdition::V2025, cwe).unwrap().name,
                name25,
                "CWE-{cwe}"
            );
            assert_eq!(
                category_of_cwe(OwaspEdition::V2021, cwe).unwrap().name,
                name21,
                "CWE-{cwe}"
            );
        }
        let f = [finding("s", "a", &["CWE-918"])];
        assert_eq!(
            only_category(&classify(&f, OwaspEdition::V2025, NONE), 0),
            "A01:2025"
        );
        assert_eq!(
            only_category(&classify(&f, OwaspEdition::V2021, NONE), 0),
            "A10:2021"
        );
    }

    #[test]
    fn a_finding_without_a_cwe_is_unclassified_and_one_with_an_unlisted_cwe_says_so() {
        let f = [
            finding("s", "none", &[]),
            finding("s", "odd", &["CWE-99999"]),
            finding("s", "garbled", &["not-a-cwe", "CWE-"]),
            finding("s", "ok", &["CWE-79"]),
        ];
        let r = classify(&f, OwaspEdition::V2025, NONE);
        assert_eq!(r.findings[0].unclassified, Some(Unclassified::NoCwe));
        assert!(r.findings[0].categories.is_empty());
        assert_eq!(r.findings[1].unclassified, Some(Unclassified::Unmapped));
        assert_eq!(r.findings[2].unclassified, Some(Unclassified::Unmapped));
        assert_eq!(r.findings[3].unclassified, None);
        assert_eq!(r.unclassified, 3);
    }

    #[test]
    fn several_cwes_give_several_categories_each_counted_once() {
        // CWE-89 and CWE-79 are both injection; CWE-798 is authentication.
        let f = [finding("s", "a", &["CWE-79", "CWE-798", "CWE-89"])];
        let r = classify(&f, OwaspEdition::V2025, &["semgrep"]);
        assert_eq!(r.findings[0].categories, ["A05:2025", "A07:2025"]);
        assert_eq!(
            coverage(&r, "A05:2025").state,
            CoverageState::Covered { findings: 1 }
        );
        assert_eq!(
            coverage(&r, "A07:2025").state,
            CoverageState::Covered { findings: 1 }
        );
    }

    #[test]
    fn semgrep_alone_leaves_the_categories_outside_its_capability_not_covered() {
        let f = [finding("Semgrep OSS", "a", &["CWE-89"])];
        for edition in [OwaspEdition::V2025, OwaspEdition::V2021] {
            let r = classify(&f, edition, &["semgrep"]);
            assert_eq!(r.categories.len(), 10);
            for c in categories(edition) {
                let cov = coverage(&r, c.id);
                if c.scanners.contains(&"semgrep") {
                    assert!(
                        matches!(cov.state, CoverageState::Covered { .. }),
                        "{}",
                        c.id
                    );
                    assert_eq!(cov.enabled_scanners, ["semgrep"]);
                } else {
                    assert_eq!(cov.state, CoverageState::NotCovered, "{}", c.id);
                    assert!(cov.enabled_scanners.is_empty());
                }
            }
        }
        let r25 = classify(&f, OwaspEdition::V2025, &["semgrep"]);
        assert_eq!(coverage(&r25, "A03:2025").state, CoverageState::NotCovered);
        assert_eq!(
            coverage(&r25, "A05:2025").state,
            CoverageState::Covered { findings: 1 }
        );
        // Covered with nothing found is a real zero, distinct from not covered.
        assert_eq!(
            coverage(&r25, "A09:2025").state,
            CoverageState::Covered { findings: 0 }
        );
    }

    #[test]
    fn production_counts_cover_a_covered_category_alone_and_leave_the_total_unchanged() {
        // A different category set at each position, so a flag applied to the wrong
        // finding (shifted, reversed) changes a count; `a` falls under two categories.
        let f = [
            finding("semgrep", "a", &["CWE-89", "CWE-798"]),
            finding("semgrep", "b", &["CWE-89"]),
            finding("semgrep", "c", &["CWE-798"]),
            finding("semgrep", "d", &[]),
        ];
        let mut with = classify(&f, OwaspEdition::V2025, &["semgrep"]);
        assert!(with.categories.iter().all(|c| c.production_findings.is_none()), "classify knows no scope");
        count_production(
            &mut with.categories,
            &with.findings,
            &[true, false, true, false],
            &[false, false, false, true],
            &[false, true, false, false],
        );
        // `a` counts once under each of its two categories; `b` is not production.
        assert_eq!(coverage(&with, "A05:2025").production_findings, Some(1));
        assert_eq!(coverage(&with, "A05:2025").state, CoverageState::Covered { findings: 2 });
        assert_eq!(coverage(&with, "A07:2025").production_findings, Some(2));
        assert_eq!(coverage(&with, "A07:2025").state, CoverageState::Covered { findings: 2 });
        // Covered with no production finding is a real zero; not covered states none.
        assert_eq!(coverage(&with, "A09:2025").production_findings, Some(0));
        // The needs-review flag is `d`'s alone, which falls under no category.
        assert_eq!(coverage(&with, "A05:2025").needs_review_findings, Some(0));
        // The dev flag is `b`'s alone (under CWE-89 only): a fourth figure, apart from production.
        assert_eq!(coverage(&with, "A05:2025").dev_findings, Some(1));
        assert_eq!(coverage(&with, "A07:2025").dev_findings, Some(0));
        assert_eq!(coverage(&with, "A03:2025").dev_findings, None);
        assert_eq!(coverage(&with, "A03:2025").needs_review_findings, None);
        assert_eq!(coverage(&with, "A03:2025").state, CoverageState::NotCovered);
        assert_eq!(coverage(&with, "A03:2025").production_findings, None);
        let json = serde_json::to_value(coverage(&with, "A03:2025")).unwrap();
        assert!(json.get("production_findings").is_none(), "{json}");
    }

    #[test]
    fn no_enabled_scanner_means_every_category_is_not_covered() {
        let r = classify(&[finding("s", "a", &["CWE-89"])], OwaspEdition::V2025, NONE);
        assert!(r
            .categories
            .iter()
            .all(|c| c.state == CoverageState::NotCovered));
        // The finding is still classified; coverage is about the scanner set.
        assert_eq!(r.findings[0].categories, ["A05:2025"]);
    }

    #[test]
    fn scanner_ids_match_ignoring_case_and_unknown_ids_enable_nothing() {
        let f: [SecurityFinding; 0] = [];
        let r = classify(&f, OwaspEdition::V2021, &["SemGrep"]);
        assert!(matches!(
            coverage(&r, "A03:2021").state,
            CoverageState::Covered { .. }
        ));
        // "semgrep-oss" is one character-run from "semgrep" and names no scanner.
        let r = classify(
            &f,
            OwaspEdition::V2021,
            &["semgrep-oss", "semgre", "Semgrep OSS"],
        );
        assert!(r
            .categories
            .iter()
            .all(|c| c.state == CoverageState::NotCovered));
    }

    #[test]
    fn the_table_is_well_formed_for_every_edition() {
        for edition in [OwaspEdition::V2025, OwaspEdition::V2021] {
            let table = categories(edition);
            assert_eq!(table.len(), 10);
            let mut seen = BTreeSet::new();
            for (i, c) in table.iter().enumerate() {
                assert_eq!(c.id, format!("A{:02}:{}", i + 1, edition.as_str()));
                assert!(!c.scanners.is_empty(), "{} has no scanner", c.id);
                for s in c.scanners {
                    assert!(
                        SCANNER_IDS.contains(s),
                        "{} names unregistered scanner {s}",
                        c.id
                    );
                }
                for cwe in c.cwes {
                    assert!(
                        seen.insert(*cwe),
                        "CWE-{cwe} is listed under two categories of {edition:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn edition_defaults_to_2025_and_parses_its_year() {
        assert_eq!(OwaspEdition::default(), OwaspEdition::V2025);
        assert_eq!(OwaspEdition::from_year("2021"), Some(OwaspEdition::V2021));
        assert_eq!(OwaspEdition::from_year(" 2025 "), Some(OwaspEdition::V2025));
        assert_eq!(OwaspEdition::from_year("2017"), None);
        assert_eq!(OwaspEdition::from_year("20251"), None);
    }

    /// The capability matrix, restated literally: a wrong list silently flips a
    /// category between `not covered` and `covered, 0 findings` ([BR-66]), so it
    /// is pinned independently of the table it checks.
    #[test]
    fn the_capability_matrix_is_pinned_per_edition() {
        const AC: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "checkov"];
        const MISCONF_CRYPTO: &[&str] =
            &["semgrep", "codeql", "bandit", "gosec", "checkov", "trivy"];
        const CODE: &[&str] = &["semgrep", "codeql", "bandit", "gosec"];
        const DESIGN: &[&str] = &["semgrep", "codeql"];
        const AUTH: &[&str] = &["semgrep", "codeql", "bandit", "gosec", "gitleaks", "trivy"];
        const INTEGRITY: &[&str] = &["semgrep", "codeql", "bandit"];
        const LOGGING: &[&str] = &["semgrep", "codeql", "checkov"];
        const DEPS: &[&str] = &["trivy", "osv-scanner", "dependency-check"];
        let expected: [(OwaspEdition, [&[&str]; 10]); 2] = [
            (
                OwaspEdition::V2025,
                [
                    AC,
                    MISCONF_CRYPTO,
                    &["trivy", "osv-scanner", "dependency-check", "checkov"],
                    MISCONF_CRYPTO,
                    CODE,
                    DESIGN,
                    AUTH,
                    INTEGRITY,
                    LOGGING,
                    &["semgrep", "codeql", "gosec"],
                ],
            ),
            (
                OwaspEdition::V2021,
                [
                    AC,
                    MISCONF_CRYPTO,
                    CODE,
                    DESIGN,
                    MISCONF_CRYPTO,
                    DEPS,
                    AUTH,
                    INTEGRITY,
                    LOGGING,
                    CODE,
                ],
            ),
        ];
        for (edition, lists) in expected {
            for (category, want) in categories(edition).iter().zip(lists) {
                assert_eq!(category.scanners, want, "{}", category.id);
            }
        }
    }

    #[test]
    fn trivy_alone_covers_exactly_what_it_can_detect() {
        let covered = |edition| -> Vec<&'static str> {
            let none: [SecurityFinding; 0] = [];
            let r = classify(&none, edition, &["trivy"]);
            r.categories
                .iter()
                .filter(|c| matches!(c.state, CoverageState::Covered { findings: 0 }))
                .map(|c| c.id)
                .collect()
        };
        assert_eq!(
            covered(OwaspEdition::V2025),
            ["A02:2025", "A03:2025", "A04:2025", "A07:2025"]
        );
        assert_eq!(
            covered(OwaspEdition::V2021),
            ["A02:2021", "A05:2021", "A06:2021", "A07:2021"]
        );
    }

    #[test]
    fn one_anchor_cwe_per_category_pins_the_transcription() {
        let anchors: [(OwaspEdition, &[(u32, &str)]); 2] = [
            (
                OwaspEdition::V2025,
                &[
                    (22, "A01:2025"),
                    (16, "A02:2025"),
                    (1104, "A03:2025"),
                    (327, "A04:2025"),
                    (89, "A05:2025"),
                    (434, "A06:2025"),
                    (798, "A07:2025"),
                    (502, "A08:2025"),
                    (532, "A09:2025"),
                    (703, "A10:2025"),
                ],
            ),
            (
                OwaspEdition::V2021,
                &[
                    (22, "A01:2021"),
                    (327, "A02:2021"),
                    (89, "A03:2021"),
                    (434, "A04:2021"),
                    (16, "A05:2021"),
                    (1104, "A06:2021"),
                    (798, "A07:2021"),
                    (502, "A08:2021"),
                    (532, "A09:2021"),
                    (918, "A10:2021"),
                ],
            ),
        ];
        for (edition, rows) in anchors {
            for &(cwe, id) in rows {
                assert_eq!(
                    category_of_cwe(edition, cwe).map(|c| c.id),
                    Some(id),
                    "CWE-{cwe}"
                );
            }
        }
    }

    #[test]
    fn a_finding_is_keyed_by_scanner_and_fingerprint_and_names_its_categories() {
        let f = [
            finding("scanner-a", "fp", &["CWE-89"]),
            finding("scanner-b", "fp", &["CWE-79"]),
        ];
        let r = classify(&f, OwaspEdition::V2025, &["semgrep"]);
        let keys: Vec<(&str, &str)> = r
            .findings
            .iter()
            .map(|c| (c.scanner_name.as_str(), c.fingerprint.as_str()))
            .collect();
        assert_eq!(keys, [("scanner-a", "fp"), ("scanner-b", "fp")]);
        let injection = coverage(&r, "A05:2025");
        assert_eq!(injection.name, "Injection");
        assert_eq!(injection.scanners, ["semgrep", "codeql", "bandit", "gosec"]);
        assert_eq!(injection.enabled_scanners, ["semgrep"]);
        assert_eq!(injection.state, CoverageState::Covered { findings: 2 });
    }

    #[test]
    fn no_findings_is_an_empty_report_not_a_failure() {
        let none: [SecurityFinding; 0] = [];
        let r = classify(&none, OwaspEdition::V2021, &["semgrep"]);
        assert!(r.findings.is_empty());
        assert_eq!(r.unclassified, 0);
        assert!(r
            .categories
            .iter()
            .any(|c| c.state == CoverageState::Covered { findings: 0 }));
    }

    #[test]
    fn a_cwe_spelling_the_parser_never_stores_is_unmapped_not_guessed() {
        for spelling in ["cwe-89", "89", "CWE-89x", " CWE-89", "CWE-+89"] {
            let r = classify(&[finding("s", "a", &[spelling])], OwaspEdition::V2025, NONE);
            assert_eq!(
                r.findings[0].unclassified,
                Some(Unclassified::Unmapped),
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn the_wire_shape_is_pinned() {
        let f = [
            finding("s", "fp1", &["CWE-89"]),
            finding("s", "fp2", &[]),
            finding("s", "fp3", &["CWE-99999"]),
        ];
        let v = serde_json::to_value(classify(&f, OwaspEdition::V2021, &["semgrep"])).unwrap();
        assert_eq!(v["edition"], "2021");
        assert_eq!(v["unclassified"], 2);
        assert_eq!(
            v["findings"][0]["categories"],
            serde_json::json!(["A03:2021"])
        );
        assert_eq!(v["findings"][0]["unclassified"], serde_json::Value::Null);
        assert_eq!(v["findings"][1]["unclassified"], "no_cwe");
        assert_eq!(v["findings"][2]["unclassified"], "unmapped");
        let cats = v["categories"].as_array().unwrap();
        let state = |id: &str| cats.iter().find(|c| c["id"] == id).unwrap()["state"].clone();
        assert_eq!(
            state("A03:2021"),
            serde_json::json!({"state": "covered", "findings": 1})
        );
        assert_eq!(
            state("A06:2021"),
            serde_json::json!({"state": "not_covered"})
        );
        let v25 = serde_json::to_value(classify(&f, OwaspEdition::default(), NONE)).unwrap();
        assert_eq!(v25["edition"], "2025");
    }

    /// The acceptance check: findings are ingested from the committed reports
    /// ONCE, read back, and re-classified under the other edition with the store
    /// untouched.
    mod over_ingested_fixtures {
        use std::path::{Path, PathBuf};
        use std::process::Command;

        use super::*;
        use crate::history::security::{ingest, read_latest};

        fn git(cwd: &Path, args: &[&str]) {
            let ceiling = cwd.parent().unwrap_or(cwd);
            let out = Command::new("git")
                .arg("-C")
                .arg(cwd)
                .env("GIT_CEILING_DIRECTORIES", ceiling)
                .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
                .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
                .args(args)
                .output()
                .expect("git is on PATH");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn fixture(name: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/sarif")
                .join(name)
        }

        #[test]
        fn switching_edition_reclassifies_stored_findings_without_a_re_ingest() {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = tmp.path().join("repo");
            std::fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            std::fs::write(root.join("README.md"), "fixture\n").unwrap();
            git(&root, &["add", "-A"]);
            git(&root, &["commit", "-q", "-m", "fixture"]);

            for name in [
                "semgrep.sarif",
                "trivy.sarif",
                "gitleaks.sarif",
                "osv-scanner.sarif",
                "bandit.sarif",
                "gosec.sarif",
                "checkov.sarif",
                "codeql.sarif",
                "dependency-check.sarif",
            ] {
                ingest(&root, &fixture(name), &[]).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            }

            let stored = read_latest(&root).unwrap().findings;
            assert!(stored.len() > 50, "ingested findings: {}", stored.len());
            let store = crate::history::db_path(&root);
            let before = std::fs::read(&store).unwrap();

            let enabled = ["semgrep", "trivy", "bandit"];
            let r25 = classify(&stored, OwaspEdition::default(), &enabled);
            let r21 = classify(&stored, OwaspEdition::V2021, &enabled);
            // Reading again and classifying again is the same answer.
            assert_eq!(
                classify(
                    &read_latest(&root).unwrap().findings,
                    OwaspEdition::V2025,
                    &enabled
                ),
                r25
            );
            assert_eq!(r25.edition, OwaspEdition::V2025);
            assert_eq!(r21.edition, OwaspEdition::V2021);
            assert_eq!(r25.findings.len(), stored.len());
            assert_eq!(r21.findings.len(), stored.len());

            // The same stored findings land under different ids per edition.
            assert!(r25
                .findings
                .iter()
                .flat_map(|f| &f.categories)
                .all(|id| id.ends_with(":2025")));
            assert!(r21
                .findings
                .iter()
                .flat_map(|f| &f.categories)
                .all(|id| id.ends_with(":2021")));
            assert!(
                r25.findings.iter().any(|f| !f.categories.is_empty()),
                "nothing classified"
            );
            assert_ne!(r25.categories, r21.categories);

            // An injection finding from the SQL-injection fixtures is Injection in both.
            let injected: Vec<usize> = stored
                .iter()
                .enumerate()
                .filter(|(_, f)| f.cwes.iter().any(|c| c == "CWE-89"))
                .map(|(i, _)| i)
                .collect();
            assert!(!injected.is_empty(), "no CWE-89 finding among the fixtures");
            for i in injected {
                assert!(r25.findings[i].categories.contains(&"A05:2025"));
                assert!(r21.findings[i].categories.contains(&"A03:2021"));
            }

            // Classification wrote nothing: the store file is byte-identical.
            assert_eq!(
                std::fs::read(&store).unwrap(),
                before,
                "classifying touched the store"
            );
        }
    }
}
