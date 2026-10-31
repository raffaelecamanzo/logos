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
//! Both thresholds compare [`Severity::gated_as`]: an `unrated` finding is
//! gated as `medium`, the rank its default level gave it before it read
//! `unrated` (S-768), so no verdict changed with the spelling.
//!
//! **One advisory is one finding** ([FR-SC-29]). The summary carries the
//! merged view the reader gives: a dependency advisory several scanners report
//! on one lockfile, package and version is one [`SummaryFinding`] naming every
//! reporter, so each rule counts it once. It is evaluated when any reporter's
//! scan is fresh, is known to a baseline by any reporter's key, and is recorded
//! by `gate --save` and `security bless` under every reporter's key, so a
//! reporter that drops out later leaves it known.
//!
//! **Missing evidence** ([ADR-79] §7) is not a violation: a required scanner
//! with no fresh scan at HEAD is a [`MissingScan`], which the surfaces map to
//! exit 5 ([FR-CL-03]). The gate never runs a scanner.
//!
//! **The ratchet's baseline has one source per checkout** ([FR-SC-25]). When
//! the checked-in `.logos/security-baseline.json` exists it is the baseline,
//! and the set `gate --save` records in the gitignored `history.db` is not
//! read; otherwise that set is. So a fresh CI clone, which has no `history.db`,
//! still reads a blessed finding as known. Only `logos security bless` writes
//! the file — fingerprint, rule id and project-relative path per finding, no
//! message, snippet or secret — sorted and rendered byte-identically for an
//! unchanged set ([NFR-RA-06]), as `check --bless` writes the constraint
//! ratchet's `rules-baseline.json` ([FR-GV-23]).
//!
//! [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
//! [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
//! [FR-SC-29]: ../../../docs/specs/requirements/FR-SC-29.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [FR-SC-11]: ../../../docs/specs/requirements/FR-SC-11.md
//! [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
//! [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-65]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-668]: ../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, SecurityRules};
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
/// The checked-in baseline's root-relative path, as every message names it
/// ([FR-SC-25]).
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
pub(crate) const CHECKED_IN_FILE_REL: &str = ".logos/security-baseline.json";
/// The command that writes the checked-in baseline — its only writer.
pub(crate) const BLESS_COMMAND: &str = "logos security bless";
/// The checked-in baseline's format version. A file carrying any other is
/// refused.
const CHECKED_IN_VERSION: u32 = 1;

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
    /// The ratchet's baseline fingerprints, from [`baseline_source`](Self::baseline_source);
    /// `None` when no file is checked in and `gate --save` never ran under a
    /// `[security]` table — not the same as an empty set.
    pub baseline: Option<BTreeSet<String>>,
    /// Where [`baseline`](Self::baseline) was read from.
    pub baseline_source: BaselineSource,
    /// The checked-in baseline's `(rule id, project-relative path)` pairs —
    /// empty for the store's — matched for a finding with an
    /// [`unbound_location`](SummaryFinding::unbound_location) only: the
    /// stopgap for a file blessed before history.db migration 10.
    pub baseline_locations: BTreeSet<(String, String)>,
}

/// Where the ratchet's baseline comes from: one source per checkout
/// ([FR-SC-25]).
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum BaselineSource {
    /// The set `gate --save` recorded in `history.db` — read only when no
    /// file is checked in.
    #[default]
    Store,
    /// The checked-in [`CHECKED_IN_FILE_REL`].
    CheckedIn,
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
    /// Installed, with nothing to scan in this project (S-769): never a
    /// missing scan.
    NotApplicable,
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
    /// The keys it was stored under before a re-key ([FR-SC-17]): a baseline
    /// that recorded one of them recorded this finding.
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    pub legacy_fingerprints: Vec<String>,
    /// The project-relative path of a finding that binds to no indexed file
    /// and is reported by an absolute path under the root (OSV-Scanner on a
    /// lockfile). Before history.db migration 10 its fingerprint carried the
    /// checkout's directory, so a checked-in baseline blessed then also knows
    /// it by rule id and this path — a stopgap kept for one release after
    /// migration 10, then removed (CR-241).
    pub unbound_location: Option<String>,
    pub rule_id: String,
    pub severity: Severity,
    /// The bound repo-relative path, else the report's own, else `None`.
    pub path: Option<String>,
    pub line: Option<i64>,
    /// A secret found only in history (S-767): its place is its commit's, not
    /// today's code, and is named so.
    pub historical: bool,
    /// The short commit a history-mode finding is anchored to, when known.
    pub short_commit: Option<String>,
    /// Full OWASP category ids in both editions (`A05:2025`, `A03:2021`).
    pub owasp: Vec<String>,
    /// Whether an unexpired ignore entry suppresses it.
    pub suppressed: bool,
    /// Every stored finding a merged one stands for, its own first
    /// ([FR-SC-29]); empty for a finding one result stands behind, which is
    /// its own one reporter.
    ///
    /// [FR-SC-29]: ../../../docs/specs/requirements/FR-SC-29.md
    pub reporters: Vec<SummaryReporter>,
}

/// One scanner's stored finding within a merged [`SummaryFinding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryReporter {
    /// The driver that reported it.
    pub scanner: String,
    /// Its fingerprint in that driver's snapshot.
    pub fingerprint: String,
    /// The keys it was stored under before a re-key.
    pub legacy_fingerprints: Vec<String>,
    /// The advisory id that driver reports it under.
    pub rule_id: String,
}

impl SummaryFinding {
    /// Its reporters: each of [`reporters`](Self::reporters), or the finding
    /// itself when one result stands behind it.
    fn members(&self) -> Vec<SummaryReporter> {
        if !self.reporters.is_empty() {
            return self.reporters.clone();
        }
        vec![SummaryReporter {
            scanner: self.scanner.clone(),
            fingerprint: self.fingerprint.clone(),
            legacy_fingerprints: self.legacy_fingerprints.clone(),
            rule_id: self.rule_id.clone(),
        }]
    }

    /// Whether `recorded` holds this finding, under its key or one it had
    /// before a re-key — a checked-in baseline is never re-keyed (S-764 ×
    /// S-765), so a secret blessed under its old key stays known: before
    /// history.db migration 7, or, for a history-only secret, before migration
    /// 11 keyed it by its own path instead of the file it bound to (S-780).
    /// A merged finding is recorded under any reporter's key ([FR-SC-29]): a
    /// baseline written before the merge recorded each of them.
    ///
    /// [FR-SC-29]: ../../../docs/specs/requirements/FR-SC-29.md
    fn recorded_in(&self, recorded: &BTreeSet<String>) -> bool {
        let known = |fingerprint: &String, legacy: &[String]| {
            recorded.contains(fingerprint) || legacy.iter().any(|old| recorded.contains(old))
        };
        known(&self.fingerprint, &self.legacy_fingerprints)
            || self.reporters.iter().any(|r| known(&r.fingerprint, &r.legacy_fingerprints))
    }

    /// Whether `locations` (a checked-in baseline's) records this finding by
    /// rule id and project-relative path: only one with an
    /// [`unbound_location`](Self::unbound_location); a bound finding is known by
    /// its fingerprint alone.
    fn located_in(&self, locations: &BTreeSet<(String, String)>) -> bool {
        self.unbound_location.as_ref().is_some_and(|path| {
            self.members().into_iter().any(|member| locations.contains(&(member.rule_id, path.clone())))
        })
    }

    /// The drivers that reported it, each once, in reporter order.
    fn scanners(&self) -> Vec<String> {
        let mut scanners: Vec<String> = Vec::new();
        for member in self.members() {
            if !scanners.contains(&member.scanner) {
                scanners.push(member.scanner);
            }
        }
        scanners
    }

    /// `path:line`, `path`, or `(no location)` — never the finding's message,
    /// which a secret-class finding has redacted anyway. A historical one says
    /// so, with its short commit when known: that place is history, never
    /// today's code.
    fn location(&self) -> String {
        let place = match (&self.path, self.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => "(no location)".to_string(),
        };
        match (self.historical, &self.short_commit) {
            (false, _) => place,
            (true, Some(commit)) => format!("{place} (historical, commit {commit})"),
            (true, None) => format!("{place} (historical)"),
        }
    }

    /// The sentence naming the finding: severity, rule, scanner and location.
    /// An `unrated` one says what it is gated as.
    fn named(&self) -> String {
        let severity = match self.severity {
            Severity::Unrated => format!("unrated (gated as {})", Severity::Unrated.gated_as().as_str()),
            rated => rated.as_str().to_string(),
        };
        format!(
            "{severity} finding `{}` ({}) at {}",
            self.rule_id,
            self.scanners().join(", "),
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
/// the gate never runs a scanner. A scanner with nothing to scan here
/// ([`ScanFreshness::NotApplicable`], S-769) is not missing: no scan of it
/// could exist.
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
            Some(ScanFreshness::Fresh | ScanFreshness::NotApplicable) => continue,
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
        .filter(|f| f.scanners().iter().any(|s| fresh.contains(s.as_str())) && !f.suppressed)
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
    let recorder = match summary.baseline_source {
        BaselineSource::Store => format!("the findings `{SAVE_COMMAND}` recorded"),
        BaselineSource::CheckedIn => format!("the findings {CHECKED_IN_FILE_REL} records"),
    };
    for f in active.iter().filter(|f| f.severity.gated_as() >= rules.ratchet_min_severity) {
        let message = match &summary.baseline {
            Some(recorded) if f.recorded_in(recorded) || f.located_in(&summary.baseline_locations) => continue,
            Some(_) => format!(
                "new {}: not in {recorder} — {suppress_hint} (fingerprint {})",
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
        for f in active.iter().filter(|f| f.severity.gated_as() >= cap) {
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
/// - A **merged** finding is recorded under each reporter's key ([FR-SC-29]).
///
/// [FR-SC-11]: ../../../docs/specs/requirements/FR-SC-11.md
/// [FR-SC-29]: ../../../docs/specs/requirements/FR-SC-29.md
pub(crate) fn baseline_fingerprints(summary: &SecuritySummary) -> Vec<(String, String)> {
    baselined(summary)
        .flat_map(SummaryFinding::members)
        .map(|member| (member.scanner, member.fingerprint))
        .collect()
}

/// The findings a baseline records, whichever its source: every unsuppressed
/// finding of every stored scan ([`baseline_fingerprints`]).
fn baselined(summary: &SecuritySummary) -> impl Iterator<Item = &SummaryFinding> {
    summary.findings.iter().filter(|f| !f.suppressed)
}

/// One finding the checked-in baseline records ([FR-SC-25]): its fingerprint,
/// rule id and project-relative path, and nothing else — no message, snippet,
/// line or secret value. The path is `None` for a finding with no location, or
/// one reported outside the project.
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaselineEntry {
    pub fingerprint: String,
    pub rule_id: String,
    pub path: Option<String>,
}

impl BaselineEntry {
    /// The file's order: by path, then rule id, then fingerprint, so a review
    /// reads the blessed findings file by file.
    fn key(&self) -> (&Option<String>, &str, &str) {
        (&self.path, &self.rule_id, &self.fingerprint)
    }
}

/// The on-disk document. `deny_unknown_fields`, so a hand-added field — a
/// message, say — fails loud rather than being carried along.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckedInFile {
    version: u32,
    findings: Vec<BaselineEntry>,
}

/// The checked-in baseline's entries for `summary` — the findings
/// [`baseline_fingerprints`] records, a merged one once per reporter — sorted
/// and without repeats, each path
/// made project-relative by `relative` ([FR-SC-25]).
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
pub(crate) fn baseline_entries(
    summary: &SecuritySummary,
    relative: impl Fn(&str) -> Option<String>,
) -> Vec<BaselineEntry> {
    let mut entries: Vec<BaselineEntry> = baselined(summary)
        .flat_map(|f| {
            let path = f.path.as_deref().and_then(&relative);
            f.members().into_iter().map(move |member| BaselineEntry {
                fingerprint: member.fingerprint,
                rule_id: member.rule_id,
                path: path.clone(),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.key().cmp(&b.key()));
    entries.dedup();
    entries
}

/// Where the checked-in baseline lives under `root`.
pub(crate) fn checked_in_path(root: &Path) -> PathBuf {
    root.join(CHECKED_IN_FILE_REL)
}

/// The canonical rendering of `entries` (already in [`baseline_entries`]'
/// order): two-space indent, trailing newline — byte-identical for an equal
/// set ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn render_checked_in(entries: &[BaselineEntry]) -> String {
    let file = CheckedInFile { version: CHECKED_IN_VERSION, findings: entries.to_vec() };
    let mut text = serde_json::to_string_pretty(&file).expect("a document of strings serialises");
    text.push('\n');
    text
}

/// Write `entries` to the checked-in baseline under `root` atomically. An
/// unchanged file is left untouched, mtime included; returns whether the bytes
/// changed. Called by [`BLESS_COMMAND`] only.
///
/// # Errors
/// A failed write is a [`ConfigError`] (exit 2).
pub(crate) fn write_checked_in(root: &Path, entries: &[BaselineEntry]) -> Result<bool> {
    let path = checked_in_path(root);
    let text = render_checked_in(entries);
    if std::fs::read(&path).is_ok_and(|current| current == text.as_bytes()) {
        return Ok(false);
    }
    let write_err = |source: std::io::Error| ConfigError::Write { path: path.clone(), source };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(write_err)?;
    }
    crate::config::atomic::publish(&path, text.as_bytes(), None).map_err(write_err)?;
    Ok(true)
}

/// The entries of the checked-in baseline under `root`, or `None` when no file
/// is checked in ([FR-SC-25]).
///
/// # Errors
/// An exit-2 [`ConfigError`] when the file is unreadable, malformed, carries a
/// field it does not define, or is of another format version.
///
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
pub(crate) fn read_checked_in(root: &Path) -> Result<Option<Vec<BaselineEntry>>> {
    let path = checked_in_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ConfigError::Io { path, source }.into()),
    };
    let invalid = |message: String| ConfigError::InvalidValue { key: CHECKED_IN_FILE_REL.to_string(), message };
    let file: CheckedInFile = serde_json::from_str(&text)
        .map_err(|e| invalid(format!("{e} — re-run `{BLESS_COMMAND}` to rewrite it")))?;
    if file.version != CHECKED_IN_VERSION {
        return Err(invalid(format!(
            "format version {} is not the supported version {CHECKED_IN_VERSION}",
            file.version
        ))
        .into());
    }
    Ok(Some(file.findings))
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
