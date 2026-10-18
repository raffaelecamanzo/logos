//! [`Severity`] — a security finding's normalised severity ([FR-SC-02],
//! [FR-SC-10]).
//!
//! Shared vocabulary, so it lives in the leaf: the evidence store writes and
//! reads it, and `rules.toml [security]` names a threshold in it, without
//! `config` depending on `history` ([S-668]). How a SARIF result or a stored
//! token maps onto it stays with the evidence store that parses them.
//!
//! [FR-SC-02]: ../../../docs/specs/requirements/FR-SC-02.md
//! [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
//! [S-668]: ../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5

/// A finding's severity, normalised across scanners. Ordered `Info < Low <
/// Medium < High < Critical`, so a threshold is a comparison. Deserialisable
/// from the same lowercase token, so `rules.toml [security]` names a threshold
/// in the spelling every surface prints ([FR-SC-10]).
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// SARIF level `none`, or a CVSS score of 0.
    Info,
    /// SARIF level `note`, or a CVSS score below 4.0.
    Low,
    /// SARIF level `warning` (the SARIF default), or a CVSS score below 7.0.
    Medium,
    /// SARIF level `error`, or a CVSS score below 9.0.
    High,
    /// A CVSS score of 9.0 or more.
    Critical,
}

impl Severity {
    /// The lowercase wire token (the stored `severity`).
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}
