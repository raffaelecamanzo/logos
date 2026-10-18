//! The `rules.toml [security]` gate ([FR-SC-10], [ADR-79] §4–7, [S-668]).
//!
//! Security evidence is gate input **only** when the contract carries a
//! `[security]` table: without one nothing here runs, nothing reads the evidence
//! store, and `check` / `gate` output is byte-identical whatever evidence exists
//! ([BR-65]).
//!
//! **Dependency direction** ([ADR-79] §5). This module never opens `history.db`.
//! The engine — the api-facade — reads the evidence (through the adapter it
//! builds, `security::SecurityGateEvidence`) and hands it over as a plain
//! [`SecuritySummary`] through [`SecurityEvidence`], exactly as it injects
//! `indexed_paths` the other way for an ingest. [`check_security`] is a pure
//! function of the table and that summary.
//!
//! **The rules** ([ADR-79] §6), evaluated over unsuppressed findings of fresh
//! scans only — a suppressed finding is excluded from every rule ([FR-SC-11]):
//! - `security:ratchet` — a finding at or above `ratchet_min_severity` whose
//!   fingerprint is not in the set `gate --save` recorded. With no recorded set
//!   every such finding is new, and the message names the command that records
//!   one, as the constraint ratchet does with no blessed set ([FR-GV-23]).
//! - `security:max_severity` — any finding at or above the cap.
//! - `security:fail_on_owasp:<id>` — any finding in the named category.
//!
//! **Missing evidence** ([ADR-79] §7) is not a violation: a required scanner
//! with no fresh scan at HEAD is a [`MissingScan`], which the surfaces map to
//! exit 5 ([FR-CL-03]). The gate never runs a scanner.
//!
//! [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
//! [FR-SC-11]: ../../../docs/specs/requirements/FR-SC-11.md
//! [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
//! [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-65]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-668]: ../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5

use std::collections::BTreeSet;

use anyhow::Result;

use crate::config::SecurityRules;
use crate::model::Severity;
use crate::models::quality::{MissingScan, Violation};

/// The rule key of the ratchet.
pub(crate) const RULE_RATCHET: &str = "security:ratchet";
/// The rule key of the severity cap.
pub(crate) const RULE_MAX_SEVERITY: &str = "security:max_severity";
/// The rule-key prefix of an OWASP cap; the category id follows.
pub(crate) const RULE_FAIL_ON_OWASP: &str = "security:fail_on_owasp:";
/// The command that records the ratchet's baseline.
pub(crate) const SAVE_COMMAND: &str = "logos gate --save";

/// Where the gate's security evidence comes from — implemented by
/// `security::SecurityGateEvidence`, which the engine builds over itself and
/// which reads `history.db`; governance only calls it, and only when the
/// contract carries a `[security]` table ([ADR-79] §5).
///
/// [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
pub(crate) trait SecurityEvidence {
    /// The read-only summary [`check_security`] evaluates.
    ///
    /// # Errors
    /// A store failure, or a `ConfigError` (exit 2) for an invalid ignore file.
    fn summary(&self) -> Result<SecuritySummary>;

    /// Replace the ratchet's baseline with `fingerprints` — `(scanner,
    /// fingerprint)` pairs — recorded under `rules` (`gate --save`). Returns how
    /// many were recorded.
    ///
    /// # Errors
    /// A store failure.
    fn save_baseline(&self, rules: &SecurityRules, fingerprints: &[(String, String)]) -> Result<usize>;
}

/// The plain value the engine injects ([ADR-79] §5): per-scanner freshness at
/// HEAD, every stored finding with its severity, OWASP categories and
/// suppression state, and the recorded baseline. It holds no rule: which
/// scanners are required, and which findings count, is decided here.
///
/// [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SecuritySummary {
    /// One row per scanner the evidence knows: every registered one, then each
    /// unregistered tool with a stored report.
    pub scanners: Vec<ScannerFreshness>,
    /// Every finding of every scanner's latest stored scan, fresh or not, in the
    /// reader's order.
    pub findings: Vec<SummaryFinding>,
    /// The fingerprints `gate --save` recorded; `None` when it never ran under
    /// a `[security]` table — not the same as an empty set.
    pub baseline: Option<BTreeSet<String>>,
}

/// One scanner's state at HEAD, as `logos security status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScannerFreshness {
    /// The registry name, or the driver name of an unregistered tool.
    pub scanner: String,
    /// The driver name its findings carry.
    pub driver: String,
    pub state: ScanFreshness,
    /// Whether a report of it is stored — a disabled scanner may have one.
    pub stored: bool,
}

/// The states of [`ScannerFreshness`]; only `Fresh` is evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanFreshness {
    Fresh,
    /// Stored, but HEAD moved or a scanned file changed; the detail says which.
    Stale(String),
    NeverRun,
    NotInstalled,
    Disabled,
}

/// One finding as the gate sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryFinding {
    /// The driver that reported it.
    pub scanner: String,
    /// Its fingerprint — the ratchet's and the ignore file's key.
    pub fingerprint: String,
    pub rule_id: String,
    pub severity: Severity,
    /// The bound repo-relative path, else the report's own, else `None`.
    pub path: Option<String>,
    pub line: Option<i64>,
    /// Full OWASP category ids in both editions (`A05:2025`, `A03:2021`).
    pub owasp: Vec<String>,
    /// Whether an unexpired ignore entry suppresses it.
    pub suppressed: bool,
}

impl SummaryFinding {
    /// `path:line`, `path`, or `(no location)` — never the finding's message,
    /// which a secret-class finding has redacted anyway.
    fn location(&self) -> String {
        match (&self.path, self.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => "(no location)".to_string(),
        }
    }

    /// The sentence naming the finding: severity, rule, scanner and location.
    fn named(&self) -> String {
        format!(
            "{} finding `{}` ({}) at {}",
            self.severity.as_str(),
            self.rule_id,
            self.scanner,
            self.location()
        )
    }
}

/// What [`check_security`] decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SecurityVerdict {
    /// One `error` per finding a rule fails on: the ratchet's, then the
    /// severity cap's, then each OWASP cap's in declared order.
    pub violations: Vec<Violation>,
    /// The rules evaluated: the ratchet, plus each cap that is set.
    pub checked: u32,
    /// Required scanners with no fresh scan at HEAD — exit 5 ([`missing_scans`]).
    pub missing: Vec<MissingScan>,
    /// Advisory notes: no recorded baseline, evidence left unevaluated.
    pub notes: Vec<String>,
}

/// Whether `row` is the scanner `name` (a registry or driver name, either case).
fn names(name: &str, row: &ScannerFreshness) -> bool {
    let name = name.trim();
    row.scanner.eq_ignore_ascii_case(name) || row.driver.eq_ignore_ascii_case(name)
}

/// The required scanners with no fresh scan at HEAD ([FR-SC-10], [ADR-79] §7),
/// in `required_scanners` order, one per scanner however often or in whatever
/// case it is named. Each message names the state and what produces the scan:
/// the gate never runs a scanner.
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
/// [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
pub(crate) fn missing_scans(rules: &SecurityRules, summary: &SecuritySummary) -> Vec<MissingScan> {
    const RUN: &str = "run `logos security scan`, or `logos security ingest <report>` for a report produced elsewhere";
    let mut seen = BTreeSet::new();
    let mut missing = Vec::new();
    for name in &rules.required_scanners {
        let row = summary.scanners.iter().find(|row| names(name, row));
        let key = row.map_or_else(|| name.trim().to_ascii_lowercase(), |row| row.driver.clone());
        if !seen.insert(key) {
            continue;
        }
        let (state, why) = match row.map(|row| &row.state) {
            Some(ScanFreshness::Fresh) => continue,
            Some(ScanFreshness::Stale(detail)) => ("stale", format!("its scan is stale: {detail} — {RUN}")),
            Some(ScanFreshness::NeverRun) => ("never_run", format!("it is installed but has never run here — {RUN}")),
            Some(ScanFreshness::NotInstalled) => (
                "not_installed",
                "it is not installed and no report of it is stored — install it, or ingest a report \
                 produced elsewhere with `logos security ingest <report>`"
                    .to_string(),
            ),
            Some(ScanFreshness::Disabled) => (
                "disabled",
                "it is disabled in .logos/config.toml — enable it, or remove it from \
                 `required_scanners`"
                    .to_string(),
            ),
            None => (
                "unknown",
                "no registered scanner or stored report has that name — check its spelling in \
                 `required_scanners`"
                    .to_string(),
            ),
        };
        let scanner = name.trim().to_string();
        let message = format!("required scanner {scanner} has no fresh scan at HEAD: {why}");
        missing.push(MissingScan { scanner, state, message });
    }
    missing
}

/// Evaluate the `[security]` table over `summary` ([FR-SC-10]). Pure and
/// deterministic: the violations follow the summary's order ([NFR-RA-06]).
///
/// Only findings of fresh scans are evaluated, so a stale scan fabricates no
/// location ([NFR-RA-05]); a stored scan that is not fresh and not required is
/// named in a note instead.
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn check_security(rules: &SecurityRules, summary: &SecuritySummary) -> SecurityVerdict {
    let fresh: BTreeSet<&str> = summary
        .scanners
        .iter()
        .filter(|row| row.state == ScanFreshness::Fresh)
        .map(|row| row.driver.as_str())
        .collect();
    let active: Vec<&SummaryFinding> = summary
        .findings
        .iter()
        .filter(|f| fresh.contains(f.scanner.as_str()) && !f.suppressed)
        .collect();
    let mut not_evaluated: Vec<&str> = Vec::new();
    for row in &summary.scanners {
        let required = rules.required_scanners.iter().any(|name| names(name, row));
        if row.stored && row.state != ScanFreshness::Fresh && !required && !not_evaluated.contains(&row.driver.as_str()) {
            not_evaluated.push(&row.driver);
        }
    }
    let mut violations = Vec::new();

    let suppress_hint = "fix it, or suppress it with a reason in .logos/security-ignore.toml";
    for f in active.iter().filter(|f| f.severity >= rules.ratchet_min_severity) {
        let message = match &summary.baseline {
            Some(recorded) if recorded.contains(&f.fingerprint) => continue,
            Some(_) => format!(
                "new {}: not in the findings `{SAVE_COMMAND}` recorded — {suppress_hint} \
                 (fingerprint {})",
                f.named(),
                f.fingerprint
            ),
            None => format!(
                "new {}: no security baseline is recorded — run `{SAVE_COMMAND}` to accept the \
                 current findings, or {suppress_hint} (fingerprint {})",
                f.named(),
                f.fingerprint
            ),
        };
        violations.push(violation(RULE_RATCHET.to_string(), f, message));
    }

    if let Some(cap) = rules.max_severity {
        for f in active.iter().filter(|f| f.severity >= cap) {
            let message = format!(
                "{} is at or above max_severity = \"{}\" — {suppress_hint} (fingerprint {})",
                f.named(),
                cap.as_str(),
                f.fingerprint
            );
            violations.push(violation(RULE_MAX_SEVERITY.to_string(), f, message));
        }
    }

    let categories = rules.owasp_categories();
    for category in &categories {
        for f in active.iter().filter(|f| f.owasp.iter().any(|c| c == category)) {
            let message = format!(
                "{} is in OWASP {category}, which fail_on_owasp names — {suppress_hint} \
                 (fingerprint {})",
                f.named(),
                f.fingerprint
            );
            violations.push(violation(format!("{RULE_FAIL_ON_OWASP}{category}"), f, message));
        }
    }

    let mut notes = Vec::new();
    if summary.baseline.is_none() {
        notes.push(format!(
            "no security baseline is recorded: every unsuppressed {} finding or worse is new — run \
             `{SAVE_COMMAND}` to record the current findings",
            rules.ratchet_min_severity.as_str()
        ));
    }
    if !not_evaluated.is_empty() {
        notes.push(format!(
            "security evidence not evaluated (its scan is not fresh at HEAD, and \
             `required_scanners` does not name it): {}",
            not_evaluated.join(", ")
        ));
    }

    SecurityVerdict {
        violations,
        checked: 1 + u32::from(rules.max_severity.is_some()) + u32::try_from(categories.len()).unwrap_or(u32::MAX),
        missing: missing_scans(rules, summary),
        notes,
    }
}

/// The `(scanner, fingerprint)` pairs `gate --save` records as the ratchet's
/// baseline: every unsuppressed finding of every stored scan, at any severity.
///
/// - A **stale** scan's findings are recorded too: they exist at save time, and
///   leaving them out would read each one as new once the scan is refreshed.
/// - A **suppressed** finding is not: its acceptance is the ignore entry, and
///   when that expires or is removed the finding returns to the ratchet
///   ([FR-SC-11]) rather than staying accepted for good.
///
/// [FR-SC-11]: ../../../docs/specs/requirements/FR-SC-11.md
pub(crate) fn baseline_fingerprints(summary: &SecuritySummary) -> Vec<(String, String)> {
    summary
        .findings
        .iter()
        .filter(|f| !f.suppressed)
        .map(|f| (f.scanner.clone(), f.fingerprint.clone()))
        .collect()
}

/// A security violation: always an `error`, a `constraint` row (the
/// `violations` table's discriminator, no migration), on the finding's file.
fn violation(rule: String, f: &SummaryFinding, message: String) -> Violation {
    Violation {
        rule,
        rule_type: "constraint".to_string(),
        severity: super::SEVERITY_ERROR.to_string(),
        file: f.path.clone().unwrap_or_default(),
        node_id: None,
        message,
        ratchet: None,
    }
}
