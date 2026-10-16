//! What `logos security scan|status|findings` and their MCP twins return
//! ([FR-SC-06], [FR-SC-07], [FR-SC-08], S-667).
//!
//! Each surface is one function here, over values the engine supplies — the
//! [`ScanEnvironment`], the ingest closure that binds against the graph, and
//! the date — so the CLI and MCP twins share one payload ([NFR-CC-01]) and
//! nothing below reads the clock or the graph itself.
//!
//! - [`scan`] asks for consent, runs each installed, enabled scanner through
//!   the [`Orchestrator`], ingests what it wrote, and reports one outcome per
//!   registered scanner. One scanner failing never stops the others.
//! - [`status`] states one condition per registered scanner and the consent
//!   state. *Never run* and *fresh, 0 findings* are different states.
//! - [`findings`] lists the stored findings, classified ([S-664]'s
//!   [`classify`]) and labelled ([S-666]'s [`label`]), with filters that
//!   intersect.
//!
//! # Consent ([FR-SC-07])
//! Every installed, enabled scanner may reach the network to refresh its rules
//! or its vulnerability database, so each must be named in the project's
//! recorded consent before a scan starts it. A scan without `consent` whose
//! scanners are not all named refuses with exit 2, naming the missing ones and
//! the flag, and starts nothing. With `consent` it records the missing names
//! and runs. A scanner installed or enabled later is missing from the record,
//! so it alone is asked for. Only the binary lookup runs before the check: no
//! scanner process, not even a version probe, starts without consent.
//!
//! # A failed scanner's output ([CR-217])
//! A failed scanner's whole bounded output is written to
//! `.logos/security/<scanner>.failed.log`. The payload names that log and
//! carries the exit status and a short [masked](super::mask) excerpt, so no
//! secret the scanner echoed reaches a payload. The log is removed when the
//! scanner next runs cleanly, so its presence is what [`status`] reports as
//! the last run having failed.
//!
//! [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
//! [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
//! [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
//! [CR-217]: ../../../docs/requests/CR-217-security-findings-in-graph-context.md
//! [S-664]: ../../../docs/planning/journal.md#s-664-findings-are-classified-by-owasp-top-10-2025-and-2021
//! [S-666]: ../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

use super::mask;
use super::registry::{Scanner, REGISTRY};
use super::{
    is_symlink, remove_file_if_present, write_new, Availability, Orchestrator, Outcome,
    ScanEnvironment, Selection, OUTPUT_DIR,
};
use crate::config::{load_scan_consent, record_scan_consent, ConfigError};
use crate::history::security::owasp::{CategoryCoverage, Unclassified};
use crate::history::security::{
    classify, label, read_latest, EntryStatus, IgnoreFile, OwaspEdition, Suppression,
};
use crate::history::{SecurityFinding, SecurityIngestSummary, Severity, FRESHNESS_STALE};

/// The suffix of a failed scanner's log in [`OUTPUT_DIR`]:
/// `<scanner>.failed.log`.
pub const FAILURE_LOG_SUFFIX: &str = ".failed.log";

// ── scan ─────────────────────────────────────────────────────────────────────

/// What `security scan` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityScanSummary {
    /// The scanners this run added to the project's consent; empty when the
    /// record already named every scanner that ran.
    pub consent_recorded: Vec<String>,
    /// One entry per registered scanner, in registry order.
    pub scanners: Vec<ScannerRun>,
    /// Findings in every report this run ingested.
    pub findings: usize,
}

/// One scanner's part of a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannerRun {
    /// The registry name.
    pub scanner: &'static str,
    /// The SARIF driver name its reports carry.
    pub driver: &'static str,
    /// The detected version, when it was probed.
    pub version: Option<String>,
    #[serde(flatten)]
    pub outcome: RunOutcome,
}

/// What happened to one scanner, ingest included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// It ran and every report it wrote was ingested.
    Ran {
        /// The reports, relative to the project root.
        sarif: Vec<String>,
        /// CodeQL only: the languages present but skipped as needing a build.
        requires_build: Vec<String>,
        /// Findings across its reports.
        findings: usize,
        /// One ingest summary per report.
        ingests: Vec<SecurityIngestSummary>,
    },
    NotInstalled,
    Disabled,
    /// CodeQL only: every language present needs a build; nothing ran.
    RequiresBuild {
        languages: Vec<String>,
        reason: &'static str,
    },
    /// CodeQL only: no file in a language it analyses; nothing ran.
    NotApplicable { reason: String },
    /// It produced no report, or a report that could not be ingested. Its
    /// previous snapshot is kept.
    Failed {
        /// The scanner's own exit code, when it exited by itself.
        exit_code: Option<i32>,
        /// The log holding the whole bounded output, relative to the root;
        /// `None` when the log could not be written.
        log: Option<String>,
        /// The output's last lines, secrets masked.
        stderr: String,
    },
}

/// Run every installed, enabled scanner and ingest what each wrote
/// ([FR-SC-06]), after the consent check the [module docs](self) describe.
/// `ingest` is the engine's security ingest, binding against the graph.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid repository or user config, or a
/// consent that is required and not given; an I/O error recording consent, or
/// a refusal to write through a symlinked `.logos/security`. One scanner's
/// failure is its [`RunOutcome::Failed`], never an error.
///
/// [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
pub fn scan(
    root: &Path,
    env: &ScanEnvironment,
    consent: bool,
    mut ingest: impl FnMut(&Path) -> Result<SecurityIngestSummary>,
) -> Result<SecurityScanSummary> {
    let orchestrator = Orchestrator::open_in(root, env)?;
    let availability = orchestrator.availability();
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    let missing = pending_consent(&availability, &consented);
    if !missing.is_empty() && !consent {
        return Err(ConfigError::InvalidValue {
            key: "consent".to_string(),
            message: format!(
                "none is recorded in this project for {} — a scanner may reach the network to \
                 refresh its rules or vulnerability database. Run `logos security scan \
                 --consent` in a terminal to record it (an MCP client cannot); later scans will \
                 not ask again",
                missing.join(", ")
            ),
        }
        .into());
    }
    if !missing.is_empty() {
        record_scan_consent(env.user_dir.as_deref(), root, &missing)?;
    }

    let mut scanners = Vec::with_capacity(availability.len());
    let mut total = 0;
    for (s, availability) in availability {
        let run = match availability {
            Availability::Disabled => unrun(s, RunOutcome::Disabled),
            Availability::NotInstalled => unrun(s, RunOutcome::NotInstalled),
            Availability::Installed => run_one(root, &orchestrator, s, &mut ingest)?,
        };
        if let RunOutcome::Ran { findings, .. } = &run.outcome {
            total += findings;
        }
        scanners.push(run);
    }
    Ok(SecurityScanSummary {
        consent_recorded: missing.into_iter().map(str::to_string).collect(),
        scanners,
        findings: total,
    })
}

/// The installed, enabled scanners `consented` does not name: what a scan asks
/// consent for, and what `status` reports as pending — one rule for both.
fn pending_consent(
    availability: &[(&'static Scanner, Availability)],
    consented: &BTreeSet<String>,
) -> Vec<&'static str> {
    availability
        .iter()
        .filter(|(s, a)| *a == Availability::Installed && !consented.contains(s.name))
        .map(|(s, _)| s.name)
        .collect()
}

fn unrun(s: &'static Scanner, outcome: RunOutcome) -> ScannerRun {
    ScannerRun {
        scanner: s.name,
        driver: s.driver,
        version: None,
        outcome,
    }
}

/// Run one scanner and ingest its reports, emitting its run event. Each
/// scanner is its own [`Orchestrator::scan`], so its reports are ingested the
/// moment it finishes rather than after every other scanner.
fn run_one(
    root: &Path,
    orchestrator: &Orchestrator,
    s: &'static Scanner,
    ingest: &mut impl FnMut(&Path) -> Result<SecurityIngestSummary>,
) -> Result<ScannerRun> {
    let started = Instant::now();
    let report = orchestrator.scan(&Selection::Only(vec![s.name.to_string()]))?;
    let Some(ran) = report.outcomes.into_iter().next() else {
        anyhow::bail!("the scan of {} reported no outcome", s.name);
    };
    let log = root.join(OUTPUT_DIR).join(format!("{}{FAILURE_LOG_SUFFIX}", s.name));
    let outcome = match ran.outcome {
        Outcome::Ran { sarif, requires_build } => match ingest_all(&sarif, ingest) {
            Ok(ingests) => {
                if let Err(e) = remove_file_if_present(&log) {
                    warn_log_io("clear", &log, &e);
                }
                RunOutcome::Ran {
                    sarif: sarif.iter().map(|p| relative(root, p)).collect(),
                    requires_build,
                    findings: ingests.iter().map(|i| i.findings).sum(),
                    ingests,
                }
            }
            Err(e) => failed(root, &log, None, &format!("{e:#}")),
        },
        Outcome::Failed { exit_code, stderr } => failed(root, &log, exit_code, &stderr),
        Outcome::NotInstalled => RunOutcome::NotInstalled,
        Outcome::Disabled => RunOutcome::Disabled,
        Outcome::RequiresBuild { languages, reason } => RunOutcome::RequiresBuild { languages, reason },
        Outcome::NotApplicable { reason } => RunOutcome::NotApplicable { reason },
    };
    let (token, exit_code, findings) = match &outcome {
        RunOutcome::Ran { findings, .. } => ("ran", None, *findings),
        RunOutcome::Failed { exit_code, .. } => ("failed", *exit_code, 0),
        _ => ("skipped", None, 0),
    };
    // The per-scanner run event ([ADR-80]): a log line, not a telemetry row —
    // the `security_scan` span already counts the call.
    tracing::info!(
        target: "logos::security",
        scanner = s.name,
        version = ran.version.as_deref().unwrap_or(""),
        duration_ms = started.elapsed().as_millis() as u64,
        outcome = token,
        exit_code = exit_code.map_or_else(String::new, |c| c.to_string()),
        findings,
        "security scanner run",
    );
    Ok(ScannerRun {
        scanner: s.name,
        driver: s.driver,
        version: ran.version,
        outcome,
    })
}

fn ingest_all(
    sarif: &[PathBuf],
    ingest: &mut impl FnMut(&Path) -> Result<SecurityIngestSummary>,
) -> Result<Vec<SecurityIngestSummary>> {
    sarif.iter().map(|path| ingest(path)).collect()
}

/// Write `output` to `log` and return the failed outcome naming it, with a
/// masked excerpt. A log that cannot be written is left out of the outcome,
/// never allowed to stop the scan.
fn failed(root: &Path, log: &Path, exit_code: Option<i32>, output: &str) -> RunOutcome {
    let written = match write_new(log, output.as_bytes()) {
        Ok(()) => Some(relative(root, log)),
        Err(e) => {
            warn_log_io("write", log, &e);
            None
        }
    };
    RunOutcome::Failed {
        exit_code,
        log: written,
        stderr: mask::excerpt(output),
    }
}

/// A failure log Logos could not write or clear: a warning naming the path,
/// never scanner output.
fn warn_log_io(what: &str, log: &Path, e: &std::io::Error) {
    tracing::warn!(target: "logos::security", log = %log.display(), "could not {what} the failure log: {e}");
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ── status ───────────────────────────────────────────────────────────────────

/// What `security status` returns ([FR-SC-08]).
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityStatus {
    /// The working tree's HEAD, when it has one.
    pub head_sha: Option<String>,
    /// The project's recorded consent.
    pub consent: ConsentState,
    /// One entry per registered scanner, in registry order.
    pub scanners: Vec<ScannerStatus>,
    /// Stored reports from tools that are not registered scanners (a SARIF
    /// ingested by hand), by driver name.
    pub other_reports: Vec<ScannerStatus>,
}

/// The project's scan consent ([FR-SC-07]).
///
/// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsentState {
    /// The scanners named in the record.
    pub consented: Vec<String>,
    /// Installed, enabled scanners the record does not name: the next scan
    /// asks for these, and only these.
    pub pending: Vec<String>,
}

/// One scanner's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannerStatus {
    /// The registry name, or the driver name of an unregistered tool.
    pub scanner: String,
    /// The SARIF driver name its reports carry.
    pub driver: String,
    #[serde(flatten)]
    pub state: ScanState,
    /// The failure log, when its last scan failed.
    pub last_failure: Option<String>,
}

/// Exactly one of the states [FR-SC-08] names. A stored snapshot outranks
/// detection: a report ingested from CI reads `fresh` or `stale` whether or not
/// the scanner is installed here. `disabled` outranks everything.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScanState {
    NotInstalled,
    Disabled,
    /// Installed, enabled, and no report is stored.
    NeverRun,
    /// A report is stored, but HEAD moved since, or a file it anchors changed.
    Stale {
        because: StaleBecause,
        snapshot_head: String,
        version: String,
        /// Unsuppressed findings in it.
        findings: usize,
        suppressed: usize,
    },
    /// A report is stored at this HEAD over unchanged files.
    Fresh {
        snapshot_head: String,
        version: String,
        /// Unsuppressed findings in it; `0` is a clean scan.
        findings: usize,
        suppressed: usize,
    },
}

/// Why a stored report is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleBecause {
    HeadMoved,
    FilesChanged,
}

/// Per-scanner state and consent ([FR-SC-07], [FR-SC-08]). Starts no process
/// but `git` for HEAD.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid config, consent record or
/// `.logos/security-ignore.toml`, or a store failure.
///
/// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
pub fn status(root: &Path, env: &ScanEnvironment, today: NaiveDate) -> Result<SecurityStatus> {
    let orchestrator = Orchestrator::open_in(root, env)?;
    let availability = orchestrator.availability();
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    let ignore = IgnoreFile::load(root)?;
    let view = read_latest(root)?;
    let labelled = label(&view.findings, &ignore, today);
    let head_sha = crate::history::head_sha(root);

    // Per driver: unsuppressed and suppressed counts, and whether a file moved.
    let mut tally: BTreeMap<&str, (usize, usize, bool)> = BTreeMap::new();
    for item in &labelled.findings {
        let t = tally.entry(item.finding.scanner_name.as_str()).or_default();
        if item.is_suppressed() {
            t.1 += 1;
        } else {
            t.0 += 1;
        }
        t.2 |= item.finding.freshness == FRESHNESS_STALE;
    }
    let state_of = |driver: &str| {
        view.snapshots.iter().find(|s| s.scanner_name == driver).map(|snap| {
            let (findings, suppressed, moved) = tally.get(driver).copied().unwrap_or_default();
            let because = if head_sha.as_deref() != Some(snap.head_sha.as_str()) {
                Some(StaleBecause::HeadMoved)
            } else {
                moved.then_some(StaleBecause::FilesChanged)
            };
            let (snapshot_head, version) = (snap.head_sha.clone(), snap.scanner_version.clone());
            match because {
                Some(because) => ScanState::Stale { because, snapshot_head, version, findings, suppressed },
                None => ScanState::Fresh { snapshot_head, version, findings, suppressed },
            }
        })
    };

    let log_of = |name: &str| {
        let log = root.join(OUTPUT_DIR).join(format!("{name}{FAILURE_LOG_SUFFIX}"));
        (log.is_file() && !is_symlink(&log)).then(|| relative(root, &log))
    };
    let scanners = availability
        .iter()
        .map(|(s, availability)| ScannerStatus {
            scanner: s.name.to_string(),
            driver: s.driver.to_string(),
            state: match (availability, state_of(s.driver)) {
                (Availability::Disabled, _) => ScanState::Disabled,
                (_, Some(stored)) => stored,
                (Availability::NotInstalled, None) => ScanState::NotInstalled,
                (Availability::Installed, None) => ScanState::NeverRun,
            },
            last_failure: log_of(s.name),
        })
        .collect();
    let other_reports = view
        .snapshots
        .iter()
        .filter(|snap| REGISTRY.iter().all(|s| s.driver != snap.scanner_name))
        .filter_map(|snap| {
            Some(ScannerStatus {
                scanner: snap.scanner_name.clone(),
                driver: snap.scanner_name.clone(),
                state: state_of(&snap.scanner_name)?,
                last_failure: None,
            })
        })
        .collect();
    let pending = pending_consent(&availability, &consented)
        .into_iter()
        .map(str::to_string)
        .collect();
    Ok(SecurityStatus {
        head_sha,
        consent: ConsentState {
            consented: consented.into_iter().collect(),
            pending,
        },
        scanners,
        other_reports,
    })
}

// ── findings ─────────────────────────────────────────────────────────────────

/// How a findings list is narrowed ([FR-SC-08]). Every filter that is set
/// must hold, so combined filters intersect.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindingsFilter {
    /// The least severity listed (`high` lists high and critical).
    pub severity: Option<Severity>,
    /// A registry name (`semgrep`) or a driver name (`Semgrep OSS`), either
    /// case.
    pub scanner: Option<String>,
    /// An OWASP category of the edition: its full id (`A05:2025`) or its
    /// number (`A05`).
    pub category: Option<String>,
    /// A project-relative file, or a directory holding the file.
    pub path: Option<String>,
    pub suppression: Option<SuppressionFilter>,
    /// The OWASP edition findings are classified under.
    pub edition: OwaspEdition,
}

/// Which findings the suppression filter keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionFilter {
    /// Not suppressed — an expired suppression no longer suppresses.
    Active,
    /// Suppressed by a current entry.
    Suppressed,
}

impl FindingsFilter {
    /// A filter from the tokens a surface was given.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] (exit 2) naming a severity, edition or
    /// suppression token that is not one Logos knows.
    pub fn parse(
        severity: Option<&str>,
        scanner: Option<&str>,
        category: Option<&str>,
        path: Option<&str>,
        suppression: Option<&str>,
        edition: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let bad = |key: &str, token: &str, expected: &str| ConfigError::InvalidValue {
            key: key.to_string(),
            message: format!("{token:?} is not one of {expected}"),
        };
        let severity = severity
            .map(|t| {
                Severity::from_str_stored(&t.to_ascii_lowercase())
                    .ok_or_else(|| bad("severity", t, "critical, high, medium, low, info"))
            })
            .transpose()?;
        let suppression = suppression
            .map(|t| match t.to_ascii_lowercase().as_str() {
                "active" => Ok(SuppressionFilter::Active),
                "suppressed" => Ok(SuppressionFilter::Suppressed),
                _ => Err(bad("suppression", t, "active, suppressed")),
            })
            .transpose()?;
        let edition = edition
            .map(|t| OwaspEdition::from_year(t).ok_or_else(|| bad("edition", t, "2025, 2021")))
            .transpose()?
            .unwrap_or_default();
        let path = path
            .map(|p| p.trim_start_matches("./").trim_end_matches('/').replace('\\', "/"))
            .filter(|p| !p.is_empty());
        Ok(Self {
            severity,
            scanner: scanner.map(str::to_string),
            category: category.map(str::to_string),
            path,
            suppression,
            edition,
        })
    }
}

/// What `security findings` returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecurityFindings {
    /// The OWASP edition the findings are classified under.
    pub edition: OwaspEdition,
    /// The date suppression expiry was judged on.
    pub as_of: String,
    /// Every stored report the findings come from, by driver name. Empty when
    /// nothing was ever scanned or ingested — distinct from a report with no
    /// findings, which is listed here with `findings: 0`.
    pub scanned: Vec<ScannedReport>,
    /// Installed, enabled scanners with no stored report — `status`'s
    /// `never_run`, never a disabled or absent scanner.
    pub never_run: Vec<&'static str>,
    /// Said when nothing was ever scanned or ingested.
    pub notice: Option<&'static str>,
    /// Stored findings before filtering.
    pub total: usize,
    /// The findings the filters kept, in store order.
    pub findings: Vec<ListedFinding>,
    /// Every category of the edition over all stored findings: covered with a
    /// count when an enabled scanner's stored report could detect it, else not
    /// covered. A not-covered category can still hold findings, listed per
    /// finding above; it never reads as a count.
    pub categories: Vec<CategoryCoverage>,
    /// Every suppression entry, expired and orphaned flagged.
    pub suppressions: Vec<EntryStatus>,
}

/// One stored report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannedReport {
    pub driver: String,
    pub version: String,
    pub head_sha: String,
    pub findings: usize,
}

/// One listed finding: the stored finding, its OWASP categories and its
/// suppression.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListedFinding {
    #[serde(flatten)]
    pub finding: SecurityFinding,
    /// The categories it falls under, in OWASP's order.
    pub owasp: Vec<&'static str>,
    /// Why it falls under none, when it does not.
    pub unclassified: Option<Unclassified>,
    /// Its suppression, current or expired; `None` when no entry names it.
    pub suppression: Option<Suppression>,
}

/// The notice a findings list carries when nothing is stored at all.
const NOTHING_SCANNED: &str =
    "no security report is stored: run `logos security scan`, or `logos security ingest <report>`";

/// The stored findings, classified, labelled and filtered ([FR-SC-08]).
/// Starts no process.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid config or ignore file, or a store
/// failure.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
pub fn findings(
    root: &Path,
    env: &ScanEnvironment,
    filter: &FindingsFilter,
    today: NaiveDate,
) -> Result<SecurityFindings> {
    let orchestrator = Orchestrator::open_in(root, env)?;
    let ignore = IgnoreFile::load(root)?;
    let view = read_latest(root)?;
    let stored: BTreeSet<&str> = view.snapshots.iter().map(|s| s.scanner_name.as_str()).collect();
    // What has looked at a category: an enabled scanner whose report is stored,
    // run here or ingested from CI. One that is installed but never ran has
    // looked at nothing, so it covers nothing ([BR-66]). Named by registry id.
    let availability = orchestrator.availability();
    let looking: Vec<&str> = availability
        .iter()
        .filter(|(s, a)| *a != Availability::Disabled && stored.contains(s.driver))
        .map(|(s, _)| s.name)
        .collect();
    let owasp = classify(&view.findings, filter.edition, &looking);
    let labelled = label(&view.findings, &ignore, today);

    let listed: Vec<ListedFinding> = labelled
        .findings
        .into_iter()
        .zip(owasp.findings)
        .map(|(item, class)| ListedFinding {
            finding: item.finding,
            owasp: class.categories,
            unclassified: class.unclassified,
            suppression: item.suppression,
        })
        .collect();
    let total = listed.len();
    let findings = listed.into_iter().filter(|f| keeps(filter, f)).collect();
    Ok(SecurityFindings {
        edition: filter.edition,
        as_of: today.format("%Y-%m-%d").to_string(),
        scanned: view
            .snapshots
            .iter()
            .map(|s| ScannedReport {
                driver: s.scanner_name.clone(),
                version: s.scanner_version.clone(),
                head_sha: s.head_sha.clone(),
                findings: s.finding_count,
            })
            .collect(),
        never_run: availability
            .iter()
            .filter(|(s, a)| *a == Availability::Installed && !stored.contains(s.driver))
            .map(|(s, _)| s.name)
            .collect(),
        notice: view.snapshots.is_empty().then_some(NOTHING_SCANNED),
        total,
        findings,
        categories: owasp.categories,
        suppressions: labelled.entries,
    })
}

/// `true` when `f` passes every filter that is set.
fn keeps(filter: &FindingsFilter, f: &ListedFinding) -> bool {
    let finding = &f.finding;
    let severity = filter.severity.is_none_or(|least| finding.severity >= least);
    let scanner = filter.scanner.as_deref().is_none_or(|want| {
        finding.scanner_name.eq_ignore_ascii_case(want)
            || super::scanner(&want.to_ascii_lowercase()).is_some_and(|s| s.driver == finding.scanner_name)
    });
    let category = filter.category.as_deref().is_none_or(|want| {
        f.owasp.iter().any(|id| {
            id.eq_ignore_ascii_case(want)
                || id
                    .split_once(':')
                    .is_some_and(|(number, _)| number.eq_ignore_ascii_case(want))
        })
    });
    let path = filter.path.as_deref().is_none_or(|want| {
        finding
            .path
            .as_deref()
            .or(finding.report_path.as_deref())
            .is_some_and(|p| p == want || p.strip_prefix(want).is_some_and(|rest| rest.starts_with('/')))
    });
    let suppression = filter.suppression.is_none_or(|want| {
        let suppressed = matches!(f.suppression, Some(Suppression::Suppressed { .. }));
        suppressed == (want == SuppressionFilter::Suppressed)
    });
    severity && scanner && category && path && suppression
}
