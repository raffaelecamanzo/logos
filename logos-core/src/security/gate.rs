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
//! [`SecurityBless`] is `logos security bless` ([FR-SC-25]): the same summary's
//! baseline findings, written to the checked-in `.logos/security-baseline.json`
//! that the summary then reads as the ratchet's baseline.
//!
//! Freshness is the one the existing read model decides
//! ([`status`](super::status)): a scanner's scan is fresh exactly when that
//! readout says `fresh`. The summary carries every stored finding with its
//! scanner's state; which ones count is governance's rule ([NFR-RA-05]).
//!
//! [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
//! [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
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
    baseline_entries, check_security, missing_scans, read_checked_in, write_checked_in, BaselineSource, ScanFreshness,
    ScannerFreshness, SecurityEvidence, SecuritySummary, SummaryFinding, SummaryReporter, CHECKED_IN_FILE_REL,
};
use crate::history::security::{
    classify, label, project_location, read_baseline, read_latest, IgnoreFile, OwaspEdition,
};
use crate::models::quality::{MissingScan, Violation, EXIT_MISSING_SCAN};

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
    /// shows that readout probes the scanners once — scanners and findings
    /// only, with no baseline read.
    fn findings_over(&self, status: &SecurityStatus) -> Result<SecuritySummary> {
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

        let labelled = label(&view.findings, &IgnoreFile::load(root)?, root, today);
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
                let unbound_location = unbound_location(root, &f);
                SummaryFinding {
                    owasp: editions
                        .iter()
                        .flat_map(|e| e.findings[idx].categories.iter().map(|c| (*c).to_string()))
                        .collect(),
                    historical: f.binding == crate::history::FindingBinding::Historical,
                    short_commit: f.anchor.map(|a| a.short_commit),
                    path: f.path.or(f.report_path),
                    line: f.start_line,
                    reporters: f
                        .reporters
                        .into_iter()
                        .map(|r| SummaryReporter {
                            scanner: r.scanner_name,
                            fingerprint: r.fingerprint,
                            legacy_fingerprints: r.legacy_fingerprints,
                            rule_id: r.rule_id,
                        })
                        .collect(),
                    scanner: f.scanner_name,
                    fingerprint: f.fingerprint,
                    legacy_fingerprints: f.legacy_fingerprints,
                    unbound_location,
                    rule_id: f.rule_id,
                    severity: f.severity,
                    suppressed,
                }
            })
            .collect();

        Ok(SecuritySummary { scanners, findings, ..SecuritySummary::default() })
    }

    /// [`findings_over`](Self::findings_over) with the ratchet's baseline from
    /// its one source ([FR-SC-25]): the checked-in
    /// `.logos/security-baseline.json` when it exists, else the set `gate
    /// --save` recorded in `history.db`, which is then not read.
    ///
    /// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
    fn summary_over(&self, status: &SecurityStatus) -> Result<SecuritySummary> {
        let root = self.0.root();
        let mut summary = self.findings_over(status)?;
        match read_checked_in(root)? {
            Some(entries) => {
                if stopgap_on() {
                    summary.baseline_locations =
                        entries.iter().filter_map(|e| Some((e.rule_id.clone(), e.path.clone()?))).collect();
                }
                summary.baseline = Some(entries.into_iter().map(|e| e.fingerprint).collect());
                summary.baseline_source = BaselineSource::CheckedIn;
            }
            None => {
                summary.baseline = read_baseline(root)?.map(|b| b.fingerprints);
                summary.baseline_source = BaselineSource::Store;
            }
        }
        Ok(summary)
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

/// The sprint 107 review's D3 stopgap: a checked-in baseline also knows a
/// finding that binds to no indexed file by its rule id and project-relative
/// path ([`SummaryFinding::unbound_location`]). Since history.db migration 10
/// such a finding's fingerprint is root-independent, so the key alone holds it
/// in any clone; the stopgap keeps green a file blessed before migration 10,
/// whose fingerprints carry the blessing checkout's directory. It is kept for
/// one release after migration 10 ships, then removed (CR-241, owner decision
/// 4). A test switches it off to prove the key alone ([`STOPGAP_OFF`]).
fn stopgap_on() -> bool {
    #[cfg(test)]
    if STOPGAP_OFF.with(std::cell::Cell::get) {
        return false;
    }
    true
}

#[cfg(test)]
thread_local! {
    /// Set by a test, on its own thread, to switch [`stopgap_on`] off.
    pub(crate) static STOPGAP_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The project-relative path of a finding that binds to no indexed file and is
/// reported by an absolute path under `root`: what a checked-in baseline also
/// matches it by ([`SummaryFinding::unbound_location`]). `None` for any other.
fn unbound_location(root: &std::path::Path, f: &crate::history::SecurityFinding) -> Option<String> {
    use crate::history::FindingBinding;
    if !matches!(f.binding, FindingBinding::Unmatched | FindingBinding::Ambiguous) {
        return None;
    }
    let report = f.report_path.as_deref()?;
    std::path::Path::new(report).is_absolute().then(|| project_location(root, report)).flatten()
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
        ScanState::NotApplicable { .. } => ScanFreshness::NotApplicable,
        ScanState::NotInstalled => ScanFreshness::NotInstalled,
        ScanState::Disabled => ScanFreshness::Disabled,
    }
}

impl SecurityGateEvidence<'_> {
    /// Each scanner's state as the gate reads it: never its version, since
    /// `check` runs on the Dashboard's reads as well as the CLI's, and never
    /// its counts, so no finding's code scope is asked of the graph: the gate
    /// keeps test code as any other finding ([BR-98]). The status read still
    /// classifies each finding by its path and, for a lockfile finding, by the
    /// lockfile ([FR-SC-30]), which parses each lockfile once; the verdict never
    /// reads that, and a lockfile it cannot read classifies as production.
    ///
    /// [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
    ///
    /// [BR-98]: ../../../docs/specs/software-spec.md#328-security-findings
    fn gate_status(&self) -> Result<SecurityStatus> {
        super::status(
            self.0.root(),
            &self.0.scan_environment(),
            self.0.tools_manifest(),
            today_utc(),
            super::VersionProbes::Never,
            false,
            super::scope::no_test_symbols,
        )
    }

    /// `logos security bless` ([FR-SC-25]): write the findings the ratchet's
    /// baseline records — every unsuppressed finding of every stored scan, as
    /// `gate --save` records them — to the checked-in
    /// `.logos/security-baseline.json`, as fingerprint, rule id and
    /// project-relative path only. Sorted, so blessing an unchanged state
    /// writes the same bytes, and leaves the file untouched. Under a
    /// `[security]` table with a required scan missing it writes nothing, as
    /// `gate --save` records nothing then. The file being rewritten is never
    /// read, so a malformed one is replaced rather than refused.
    ///
    /// # Errors
    /// An invalid `.logos/rules.toml` or ignore file (exit 2), a failed write
    /// (exit 2), or a store failure.
    ///
    /// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
    pub(crate) fn bless(&self) -> Result<SecurityBless> {
        let root = self.0.root();
        let table = load_rules_from_root(root)?.security;
        let summary = self.findings_over(&self.gate_status()?)?;
        let missing_scans = table.as_ref().map(|table| missing_scans(table, &summary)).unwrap_or_default();
        if !missing_scans.is_empty() {
            let named: Vec<&str> = missing_scans.iter().map(|m| m.scanner.as_str()).collect();
            return Ok(SecurityBless {
                file: CHECKED_IN_FILE_REL.to_string(),
                recorded: false,
                changed: false,
                findings: 0,
                message: format!(
                    "security baseline not blessed: required scan missing or stale at HEAD: {} — \
                     {CHECKED_IN_FILE_REL} is left as it was",
                    named.join(", ")
                ),
                missing_scans,
            });
        }
        let entries = baseline_entries(&summary, |path| project_location(root, path));
        let changed = write_checked_in(root, &entries)?;
        let outcome = if changed { "written" } else { "unchanged" };
        let reader = if table.is_some() {
            "the ratchet reads it in every checkout"
        } else {
            "the ratchet reads it once .logos/rules.toml has a [security] table"
        };
        Ok(SecurityBless {
            file: CHECKED_IN_FILE_REL.to_string(),
            recorded: true,
            changed,
            findings: entries.len(),
            message: format!(
                "security baseline blessed: {} findings in {CHECKED_IN_FILE_REL} ({outcome}) — commit it: \
                 {reader}",
                entries.len()
            ),
            missing_scans,
        })
    }
}

/// What `logos security bless` did ([FR-SC-25]).
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityBless {
    /// The file it writes: `.logos/security-baseline.json`.
    pub file: String,
    /// Whether the file now records the current findings — `false` only when
    /// a required scan is missing, and the file was left as it was.
    pub recorded: bool,
    /// Whether the file's bytes changed: blessing an unchanged state does not
    /// touch it.
    pub changed: bool,
    /// How many findings the file records (0 when nothing was recorded).
    pub findings: usize,
    /// Required scanners with no fresh scan at HEAD — exit 5, nothing written.
    pub missing_scans: Vec<MissingScan>,
    pub message: String,
}

impl SecurityBless {
    /// `5` when a required scan was missing and nothing was written, else `0`.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.missing_scans.is_empty() {
            0
        } else {
            EXIT_MISSING_SCAN
        }
    }
}

impl SecurityEvidence for SecurityGateEvidence<'_> {
    fn summary(&self) -> Result<SecuritySummary> {
        self.summary_over(&self.gate_status()?)
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
