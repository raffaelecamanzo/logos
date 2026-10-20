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
/// Unrated < Medium < High < Critical`, which is the order a list sorts by. A
/// threshold compares [`gated_as`](Self::gated_as), so an `unrated` finding is
/// gated as `medium`. Deserialisable from the same lowercase token, so
/// `rules.toml [security]` names a threshold in the spelling every surface
/// prints ([FR-SC-10]); that table refuses `unrated` as a threshold.
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// SARIF level `none`, or a CVSS score of 0.
    Info,
    /// SARIF level `note`, or a CVSS score below 4.0.
    Low,
    /// The report gave no rating: no CVSS score and only a default level,
    /// one the report left out (SARIF's default, `warning`) or one its
    /// scanner writes on every result (OSV-Scanner's `warning`). It never
    /// reads `medium` ([S-768]); [`reason`](Self::reason) says why.
    ///
    /// [S-768]: ../../../docs/planning/journal.md#s-768-osv-scanner-severities-are-never-made-up
    Unrated,
    /// A SARIF level `warning` the report states (OSV-Scanner's on every
    /// result does not count), or a CVSS score below 7.0.
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
            Severity::Unrated => "unrated",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// The severity a threshold compares: `unrated` is gated as `medium`,
    /// the rank its default level `warning` gives, so a cap, ratchet or
    /// filter decides on it exactly as it did before it read `unrated`.
    pub fn gated_as(self) -> Self {
        match self {
            Severity::Unrated => Severity::Medium,
            rated => rated,
        }
    }

    /// Why a finding reads this severity, when that is not plain: `unrated`'s
    /// [`UNRATED_REASON`]; `None` for a rating.
    pub fn reason(self) -> Option<&'static str> {
        (self == Severity::Unrated).then_some(UNRATED_REASON)
    }
}

/// Why a finding reads `unrated`, on every surface that shows one.
pub const UNRATED_REASON: &str = "the scanner gave none";

#[cfg(test)]
mod tests {
    use super::Severity::{self, *};

    /// The list order is `info < low < unrated < medium < high < critical`,
    /// and a threshold sees `unrated` as `medium` and every rating as itself.
    #[test]
    fn unrated_sorts_between_low_and_medium_and_is_gated_as_medium() {
        let mut shuffled = [Critical, Unrated, Info, High, Low, Medium];
        shuffled.sort();
        assert_eq!(shuffled, [Info, Low, Unrated, Medium, High, Critical]);
        let gated: Vec<Severity> = shuffled.iter().map(|s| s.gated_as()).collect();
        assert_eq!(gated, [Info, Low, Medium, Medium, High, Critical]);
        assert_eq!(Unrated.reason(), Some("the scanner gave none"));
        assert!(shuffled.iter().filter(|&&s| s != Unrated).all(|s| s.reason().is_none()));
    }
}
