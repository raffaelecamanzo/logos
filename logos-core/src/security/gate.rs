//! The `rules.toml [security]` gate's evidence ([FR-SC-10], [ADR-79] §5, [S-668]).
//!
//! [`SecurityGateEvidence`] is the engine's side of the gate: it reads the
//! evidence store, which governance never opens, and hands governance a plain
//! summary. It is built by the engine over itself and called only when the
//! contract carries the table.
//!
//! It lives here, beside the read model it projects, rather than in
//! `engine.rs` — the engine names the adapter and nothing it is made of, which
//! keeps the engine's module fan-out (`max_fan_out`) off `governance::security`
//! and `history::security::suppress`, which only the adapter touches. The move is
//! budget-driven; the dependency direction of [ADR-79] §5 is unchanged.
//!
//! [`SecurityGateBand`] is the same verdict for the Security page ([S-670]):
//! `check_security` over the same summary, so the page and `check` agree.
//!
//! Freshness is the one the existing read model decides
//! ([`status`](super::status)): a scanner's scan is fresh exactly when that
//! readout says `fresh`. The summary carries every stored finding with its
//! scanner's state; which ones count is governance's rule ([NFR-RA-05]).
//!
//! [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [S-668]: ../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5
//! [S-670]: ../../../docs/planning/journal.md#s-670-the-security-page-shows-scanners-findings-and-the-owasp-view
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use anyhow::{Context, Result};
use serde::Serialize;

use super::{ScanState, SecurityStatus, StaleBecause};
use crate::config::load_rules_from_root;
use crate::engine::{today_utc, Engine};
use crate::governance::security::{
    check_security, ScanFreshness, ScannerFreshness, SecurityEvidence, SecuritySummary, SummaryFinding,
};
use crate::history::security::{classify, label, read_baseline, read_latest, IgnoreFile, OwaspEdition};
use crate::models::quality::{MissingScan, Violation};

/// The evidence the `[security]` gate reads, over one engine.
pub(crate) struct SecurityGateEvidence<'a>(pub(crate) &'a Engine);

/// The security half of `check` over the `[security]` table, as the Security
/// page's gate band shows it ([FR-SC-09], [FR-SC-10]): the same
/// `check_security` verdict, so the band and `check` cannot disagree.
///
/// [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityGateBand {
    /// `false` when a required scan is missing or a rule fails on a finding.
    pub passed: bool,
    /// The rules evaluated: the ratchet, plus each cap that is set.
    pub checked_rules: u32,
    /// The table's `required_scanners`, as written.
    pub required_scanners: Vec<String>,
    /// Required scanners with no fresh scan at HEAD — `check` exits 5 on these.
    pub missing_scans: Vec<MissingScan>,
    /// One `error` per finding a rule fails on (`security:ratchet`,
    /// `security:max_severity`, `security:fail_on_owasp:<id>`).
    pub violations: Vec<Violation>,
    /// No recorded baseline; evidence left unevaluated.
    pub notes: Vec<String>,
}

impl SecurityGateEvidence<'_> {
    /// The summary over a status readout already taken, so a caller that also
    /// shows that readout probes the scanners once.
    fn summary_over(&self, status: &SecurityStatus) -> Result<SecuritySummary> {
        let root = self.0.root();
        let today = today_utc();
        let view = read_latest(root)?;
        let stored: std::collections::BTreeSet<&str> =
            view.snapshots.iter().map(|snap| snap.scanner_name.as_str()).collect();
        let scanners = status
            .scanners
            .iter()
            .chain(&status.other_reports)
            .map(|row| ScannerFreshness {
                scanner: row.scanner.clone(),
                driver: row.driver.clone(),
                state: freshness_of(&row.state),
                stored: stored.contains(row.driver.as_str()),
            })
            .collect();

        let labelled = label(&view.findings, &IgnoreFile::load(root)?, today);
        let none: &[&str] = &[];
        let editions = [
            classify(&view.findings, OwaspEdition::V2025, none),
            classify(&view.findings, OwaspEdition::V2021, none),
        ];
        let findings = labelled
            .findings
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                let suppressed = item.is_suppressed();
                let f = item.finding;
                SummaryFinding {
                    owasp: editions
                        .iter()
                        .flat_map(|e| e.findings[idx].categories.iter().map(|c| (*c).to_string()))
                        .collect(),
                    path: f.path.or(f.report_path),
                    line: f.start_line,
                    scanner: f.scanner_name,
                    fingerprint: f.fingerprint,
                    rule_id: f.rule_id,
                    severity: f.severity,
                    suppressed,
                }
            })
            .collect();

        Ok(SecuritySummary {
            scanners,
            findings,
            baseline: read_baseline(root)?.map(|b| b.fingerprints),
        })
    }

    /// The `[security]` gate as the Security page shows it ([FR-SC-09]): the
    /// verdict `check` reaches on the table alone, over `status` — `None`
    /// without a table, so nothing is evaluated then, as for `check`.
    ///
    /// # Errors
    /// An invalid `.logos/rules.toml` (exit 2), or a store failure.
    ///
    /// [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
    pub(crate) fn band(&self, status: &SecurityStatus) -> Result<Option<SecurityGateBand>> {
        let Some(table) = load_rules_from_root(self.0.root())?.security else {
            return Ok(None);
        };
        let verdict = check_security(&table, &self.summary_over(status)?);
        Ok(Some(SecurityGateBand {
            passed: verdict.missing.is_empty() && verdict.violations.is_empty(),
            checked_rules: verdict.checked,
            required_scanners: table.required_scanners,
            missing_scans: verdict.missing,
            violations: verdict.violations,
            notes: verdict.notes,
        }))
    }
}

/// The gate's freshness for one scanner state: `fresh` exactly when `status`
/// says so, every stale state with the reason `check` names.
pub(super) fn freshness_of(state: &ScanState) -> ScanFreshness {
    match state {
        ScanState::Fresh { .. } => ScanFreshness::Fresh,
        ScanState::Stale { because: StaleBecause::HeadMoved, snapshot_head, .. } => {
            ScanFreshness::Stale(format!("HEAD moved since {}", snapshot_head.get(..12).unwrap_or(snapshot_head)))
        }
        ScanState::Stale { because: StaleBecause::FilesChanged, .. } => {
            ScanFreshness::Stale("a file it scanned changed".to_string())
        }
        ScanState::Stale { because: StaleBecause::ScopedOnly, .. } => {
            ScanFreshness::Stale("only a scoped scan is stored".to_string())
        }
        ScanState::NeverRun => ScanFreshness::NeverRun,
        ScanState::NotInstalled => ScanFreshness::NotInstalled,
        ScanState::Disabled => ScanFreshness::Disabled,
    }
}

impl SecurityEvidence for SecurityGateEvidence<'_> {
    fn summary(&self) -> Result<SecuritySummary> {
        let status = super::status(
            self.0.root(),
            &self.0.scan_environment(),
            self.0.tools_manifest(),
            today_utc(),
            // The summary reads each scanner's state, never its version, and
            // `check` runs on the Dashboard's reads as well as the CLI's.
            super::VersionProbes::Never,
        )?;
        self.summary_over(&status)
    }

    fn save_baseline(&self, rules: &crate::config::SecurityRules, fingerprints: &[(String, String)]) -> Result<usize> {
        let root = self.0.root();
        // ADR-79 Neutral: the table and the ignore file it was saved under,
        // recorded with the baseline for provenance.
        let ignore = IgnoreFile::load(root)?.to_toml();
        let table = serde_json::to_string(rules).context("serialising the [security] table")?;
        let config_hash = blake3::hash(format!("{table}\u{1f}{ignore}").as_bytes()).to_hex().to_string();
        crate::history::security::save_baseline(
            root,
            crate::history::head_sha(root).as_deref(),
            &config_hash,
            fingerprints,
        )
    }
}
