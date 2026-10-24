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
//!   registered scanner. One scanner failing never stops the others. A
//!   [`ScanRequest`] can name the scanners and the paths ([FR-SC-17]): see
//!   [below](#a-selected-or-scoped-scan-fr-sc-17).
//! - [`status`] states one condition per registered scanner, the copy in use
//!   (S-744) and the consent state. *Never run* and *fresh, 0 findings* are
//!   different states.
//! - [`findings`] lists the stored findings, classified ([S-664]'s
//!   [`classify`]) and labelled ([S-666]'s [`label`]), with filters that
//!   intersect, each in the graph context the engine computes for it
//!   ([`super::context`], S-669) and with its code scope ([`super::scope`],
//!   S-795).
//!
//! # Test code ([FR-SC-28])
//! Every finding is `test` or `production`, classified when it is read
//! ([`super::scope`]). A list keeps production code unless its scope says
//! otherwise and states how many findings the scope hid
//! ([`SecurityFindings::hidden_by_scope`]); `status` counts a report's fresh,
//! unsuppressed, production findings by [`counts`](crate::history::security::counts)
//! and states the rest by why ([`ScanState::Fresh`]). The gate reads each
//! scanner's state alone, never these counts ([BR-98]).
//!
//! # Consent ([FR-SC-07])
//! Every installed, enabled scanner may reach the network to refresh its rules
//! or its vulnerability database, so each must be named in the project's
//! recorded consent before a scan starts it. A scan without `consent` whose
//! scanners are not all named refuses with exit 2, naming the missing ones and
//! the flag, and starts nothing. With `consent` it records the missing names
//! and runs. A scanner installed or enabled later is missing from the record,
//! so it alone is asked for. Only the binary lookup runs before [`scan`]'s
//! check: no scanner process, not even a version probe, starts without
//! consent. `security scan --install` and `security install` are different:
//! deciding what to install probes the versions of the copies already there
//! ([`crate::tools::surface`], S-744) — a version command with its update
//! check off, never a scan — before either consent is checked.
//!
//! # Following and cancelling a scan (S-673)
//! [`scan`] reports each scanner's start and its settled outcome to a
//! `progress` callback as it happens; the web UI's scan job replays them as
//! per-scanner status. Run on a [job](crate::jobs) thread, a scan stops at a
//! cancel: the scanner running is killed with its process group, nothing after
//! it starts, no failure log is written for it and nothing it left is
//! ingested, and the scan returns [`Cancelled`](crate::jobs::Cancelled).
//! [`consent_pending`] answers, without starting a process, whether a scan
//! without consent would refuse — what the UI asks before it starts one.
//!
//! # A selected or scoped scan ([FR-SC-17])
//! A request naming scanners runs only those; every other scanner reads
//! [`RunOutcome::NotSelected`]. A request naming paths confines each to the
//! project root first ([`super::resolve_scope`]) — a refused path stops the
//! scan with exit 2 before consent is read and before any process starts. A
//! path-capable scanner then runs on those paths only, and its report is
//! ingested as a scoped one: its findings within the paths are replaced, and
//! every other file's findings and freshness are carried over unchanged
//! ([`crate::history::security::ingest_scoped`]). Gitleaks, whose scoped run
//! reads the files and whose full scan reads their history, keeps the full
//! scan's findings in the paths, and a secret found again keeps its fingerprint
//! ([`super::registry::PathSlot::keeps_full_findings`]). A scanner with no path
//! position reports [`RunScope::Unscoped`] — *whole-manifest only* — and runs
//! unscoped when named, or is left out when not. Consent is asked for the
//! scanners the request will start, exactly as a full scan asks for all.
//!
//! # A failed scanner's output ([CR-217])
//! A failed scanner's whole bounded output is written to
//! `.logos/security/<scanner>.failed.log`. The payload names that log and
//! carries the exit status, a short [masked](super::mask) excerpt, so no
//! secret the scanner echoed reaches a payload, and the [cause](failure_cause)
//! in one masked line taken from it (S-769). The log is removed when the
//! scanner next runs cleanly or is found to have nothing to scan, so its presence is what [`status`] reports as
//! the last run having failed, with the same cause read back from it.
//!
//! # A scanner with nothing to scan (S-769)
//! A scanner that reads one language's files and finds none the project
//! keeps ([`Orchestrator::not_applicable`]) is [`RunOutcome::NotApplicable`]:
//! the scan reports it without a start and spawns nothing for it, and
//! [`status`] reads it [`ScanState::NotApplicable`] while no report of it is
//! stored — a state the `[security]` gate and the workspace roll-up never
//! count against the project.
//!
//! [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
//! [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
//! [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
//! [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
//! [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [BR-98]: ../../../docs/specs/software-spec.md#328-security-findings
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

use super::context::FindingContext;
use super::mask;
use super::registry::{Scanner, ScopedPath, REGISTRY};
use super::scope::{scopes, ScopeFilter, ScopeLabel};
use super::Excluded;
use super::{
    is_symlink, project_ignores, remove_file_if_present, resolve_scope, write_new, Availability, Coverage,
    InstalledCopy, Orchestrator, Outcome, Probe, ScanEnvironment, ScanRequest, Selection, Unprobed, OUTPUT_DIR,
};
use crate::config::{load_install_consent, load_scan_consent, record_scan_consent, ConfigError};
use crate::history::security::owasp::{count_production, CategoryCoverage, Unclassified};
use crate::history::security::{
    classify, counts, label, left_out, read_latest, CodeScope, EntryStatus, IgnoreFile, IngestScope, LeftOut, OwaspEdition,
    Suppression,
};
use crate::history::{SecurityFinding, SecurityIngestSummary, Severity, FRESHNESS_STALE};
use crate::tools::Manifest;

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

/// One step of a scan as it happens ([`scan`]'s `progress`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ScanProgress {
    /// An installed, enabled scanner is about to run.
    Started { scanner: &'static str },
    /// A scanner's part is settled — it ran, failed or was skipped — exactly
    /// as the summary will list it.
    Finished(ScannerRun),
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
    /// A scoped scan only ([FR-SC-17]): the paths it ran on, or why it could
    /// not be scoped. Absent from a whole-project scan's payload.
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<RunScope>,
    /// What its whole-project run did with the project's ignored directories
    /// ([FR-SC-26]): `ignored_paths` and, when it read them, `coverage_kind` and
    /// `coverage_note`.
    /// Absent when it did not run, or ran on the paths a scoped scan named.
    ///
    /// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
    #[serde(flatten)]
    pub coverage: Option<Coverage>,
}

/// How a scoped scan treated one scanner ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunScope {
    /// It runs on these project-relative paths only.
    Paths { paths: Vec<String> },
    /// It has no path position: it runs on the whole project when the request
    /// names it, and is left out when it does not.
    Unscoped { reason: &'static str },
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
    /// Installed and enabled, but the request did not name it, or it cannot
    /// be scoped and a scoped scan that named no scanner left it out
    /// ([FR-SC-17]). Nothing started.
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    NotSelected,
    /// CodeQL only: every language present needs a build; nothing ran.
    RequiresBuild {
        languages: Vec<String>,
        reason: &'static str,
    },
    /// Nothing to scan, so nothing started: CodeQL with no file in a language
    /// it analyses, or a single-language scanner with none of its files
    /// (S-769).
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
        /// Why it failed, in one masked line of that output
        /// ([`failure_cause`], S-769).
        cause: String,
    },
}

/// Run the installed, enabled scanners `request` selects, on the paths it
/// names, and ingest what each wrote ([FR-SC-06], [FR-SC-17]), after the
/// consent check the [module docs](self) describe. `ingest` is the engine's
/// security ingest, binding against the graph, handed a scoped run's
/// project-relative paths; `manifest` names the managed copies and minimum
/// versions (the builtin one in every shipped surface); `progress` hears each
/// scanner's start and outcome as it happens.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid repository or user config, a
/// scanner name that is not registered, a path refused by
/// [`super::resolve_scope`], or a consent that is required and not given —
/// each before any process starts; an I/O error recording consent, or a
/// refusal to write through a symlinked `.logos/security`;
/// [`Cancelled`](crate::jobs::Cancelled) when the job it runs on was
/// cancelled. One scanner's failure is its [`RunOutcome::Failed`], never an
/// error.
///
/// [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
pub fn scan(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    consent: bool,
    request: &ScanRequest,
    progress: &mut dyn FnMut(&ScanProgress),
    mut ingest: impl FnMut(&Path, Option<IngestScope<'_>>) -> Result<SecurityIngestSummary>,
) -> Result<SecurityScanSummary> {
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    request.scanners.check_registered()?;
    let scope = if request.paths.is_empty() { None } else { Some(resolve_scope(root, &request.paths)?) };
    let plan: Vec<_> = orchestrator
        .availability()
        .into_iter()
        .map(|(s, availability)| (s, availability, plan(request, scope.as_deref(), s)))
        .collect();
    // Consent covers what this scan will start, as a full scan's covers all.
    let starting: Vec<_> =
        plan.iter().filter(|(_, _, p)| p.runs).map(|(s, availability, _)| (*s, *availability)).collect();
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    let missing = pending_consent(&orchestrator, &starting, &consented);
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
    // At the scan's start, whatever it then starts: a scan whose scanners are
    // all absent still removes a key file a crash left (FR-SC-27). A refused
    // `.logos` is the first scanner's failure to report.
    if let Ok(out_dir) = super::output_dir(root) {
        super::sweep_stale_key_file(&out_dir);
    }

    let mut scanners = Vec::with_capacity(plan.len());
    let mut total = 0;
    for (s, availability, plan) in plan {
        if crate::jobs::cancelled() {
            return Err(crate::jobs::Cancelled.into());
        }
        let mut run = match availability {
            Availability::Disabled => unrun(s, RunOutcome::Disabled),
            Availability::NotInstalled => unrun(s, RunOutcome::NotInstalled),
            Availability::Installed if !plan.runs => unrun(s, RunOutcome::NotSelected),
            Availability::Installed => match orchestrator.not_applicable(s) {
                Some(reason) => {
                    clear_failure_log(&failure_log(root, s.name));
                    unrun(s, RunOutcome::NotApplicable { reason })
                }
                None => {
                    progress(&ScanProgress::Started { scanner: s.name });
                    run_one(root, &orchestrator, s, plan.scope, &mut ingest)?
                }
            },
        };
        run.scope = plan.note;
        if let RunOutcome::Ran { findings, .. } = &run.outcome {
            total += findings;
        }
        progress(&ScanProgress::Finished(run.clone()));
        scanners.push(run);
    }
    Ok(SecurityScanSummary {
        consent_recorded: missing.into_iter().map(str::to_string).collect(),
        scanners,
        findings: total,
    })
}

/// What a request does with one scanner.
struct Plan<'a> {
    /// Whether it is started (when installed and enabled).
    runs: bool,
    /// The paths it runs on; `None` runs it on the whole project.
    scope: Option<&'a [ScopedPath]>,
    /// What its payload says about the scope.
    note: Option<RunScope>,
}

/// The [`Plan`] for `s` under `request`, `scope` being its resolved paths.
fn plan<'a>(request: &ScanRequest, scope: Option<&'a [ScopedPath]>, s: &Scanner) -> Plan<'a> {
    let selected = request.scanners == Selection::All || request.names(s);
    let unselected = Plan { runs: false, scope: None, note: None };
    match scope {
        None => Plan { runs: selected, scope: None, note: None },
        Some(_) if !selected => unselected,
        Some(scope) => match s.unscoped_reason() {
            None => Plan {
                runs: true,
                scope: Some(scope),
                note: Some(RunScope::Paths { paths: scope.iter().map(|p| p.rel.clone()).collect() }),
            },
            // Run on the whole project only when asked for by name.
            Some(reason) => Plan { runs: request.names(s), scope: None, note: Some(RunScope::Unscoped { reason }) },
        },
    }
}

/// The installed, enabled scanners the project's consent does not name yet:
/// what [`scan`] without `consent` would refuse over. Starts no process.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid config or consent record.
pub fn consent_pending(root: &Path, env: &ScanEnvironment, manifest: &Manifest) -> Result<Vec<&'static str>> {
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    Ok(pending_consent(&orchestrator, &orchestrator.availability(), &consented))
}

/// The installed, enabled scanners `consented` does not name and that have
/// something to scan: what a scan asks consent for, and what `status` reports
/// as pending — one rule for both. A `not_applicable` one starts nothing, so
/// it is asked for once the project gives it something to scan (S-769).
fn pending_consent(
    orchestrator: &Orchestrator,
    availability: &[(&'static Scanner, Availability)],
    consented: &BTreeSet<String>,
) -> Vec<&'static str> {
    availability
        .iter()
        .filter(|(s, a)| *a == Availability::Installed && !consented.contains(s.name))
        .filter(|(s, _)| orchestrator.not_applicable(s).is_none())
        .map(|(s, _)| s.name)
        .collect()
}

fn unrun(s: &'static Scanner, outcome: RunOutcome) -> ScannerRun {
    ScannerRun {
        scanner: s.name,
        driver: s.driver,
        version: None,
        outcome,
        scope: None,
        coverage: None,
    }
}

/// Run one scanner — on `scope` when given — and ingest its reports, emitting
/// its run event. Each scanner is its own [`Orchestrator::scan`], so its
/// reports are ingested the moment it finishes rather than after every other
/// scanner.
fn run_one(
    root: &Path,
    orchestrator: &Orchestrator,
    s: &'static Scanner,
    scope: Option<&[ScopedPath]>,
    ingest: &mut impl FnMut(&Path, Option<IngestScope<'_>>) -> Result<SecurityIngestSummary>,
) -> Result<ScannerRun> {
    let started = Instant::now();
    let report = orchestrator.scan_in(&Selection::Only(vec![s.name.to_string()]), scope)?;
    let paths: Option<Vec<String>> = scope.map(|scope| scope.iter().map(|p| p.rel.clone()).collect());
    let keeps_full_findings = s.path_slot.is_some_and(|slot| slot.keeps_full_findings);
    let scope = paths.as_deref().map(|paths| IngestScope { paths, keeps_full_findings });
    let Some(ran) = report.outcomes.into_iter().next() else {
        anyhow::bail!("the scan of {} reported no outcome", s.name);
    };
    let log = failure_log(root, s.name);
    // Killed by a cancel: what it left is no failure of the scanner's, so no
    // log is written and the previous snapshot stays as it was.
    if matches!(ran.outcome, Outcome::Failed { .. }) && crate::jobs::cancelled() {
        return Err(crate::jobs::Cancelled.into());
    }
    let outcome = match ran.outcome {
        Outcome::Ran { sarif, requires_build } => match ingest_all(&sarif, scope, ingest) {
            Ok(ingests) => {
                clear_failure_log(&log);
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
        Outcome::NotApplicable { reason } => {
            clear_failure_log(&log);
            RunOutcome::NotApplicable { reason }
        }
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
        scope: None,
        coverage: ran.coverage,
    })
}

/// Ingest each report, as a scoped one over `scope` when given.
fn ingest_all(
    sarif: &[PathBuf],
    scope: Option<IngestScope<'_>>,
    ingest: &mut impl FnMut(&Path, Option<IngestScope<'_>>) -> Result<SecurityIngestSummary>,
) -> Result<Vec<SecurityIngestSummary>> {
    sarif.iter().map(|path| ingest(path, scope)).collect()
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
        cause: failure_cause(output),
    }
}

/// The cause a failed scanner's outcome names when its output is empty.
pub(crate) const NO_OUTPUT: &str = "the scanner printed nothing on stderr or stdout";

/// The longest cause, in characters: one line a table cell can hold.
pub(crate) const CAUSE_CHARS: usize = 240;

/// Why a scanner failed, in one line of its `output` — what its log holds —
/// with secrets [masked](mask::mask_secrets) (S-769): the first line holding
/// the word `error`, `fatal` or `panic` (any case, a whole word, so `0 errors`
/// is none), else the last line that is not blank, else [`NO_OUTPUT`].
/// Trimmed, and cut to [`CAUSE_CHARS`] with a closing `…`. Two failures are
/// named by a sentence of Logos's instead: the NVD API rejecting a missing or
/// invalid key ([`NVD_KEY_CAUSE`], returned whole), whose first error line says
/// only that the update failed, and gosec unable to type the standard library
/// ([`untyped_stdlib_cause`], S-782, the line the orchestrator adds, preferred
/// to gosec's own, which names neither version nor remedy).
pub(crate) fn failure_cause(output: &str) -> String {
    let masked = mask::mask_secrets(output);
    if masked.lines().any(rejects_nvd_api_key) {
        return NVD_KEY_CAUSE.to_string();
    }
    let lines = || masked.lines().map(str::trim).filter(|l| !l.is_empty());
    let reports_error = |line: &&str| {
        line.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| ["error", "fatal", "panic"].iter().any(|w| word.eq_ignore_ascii_case(w)))
    };
    let has_marker = masked.contains(UNTYPED_STDLIB);
    let names_untyped_stdlib = |line: &&str| has_marker && line.starts_with(UNTYPED_STDLIB_CAUSE);
    let Some(line) = lines()
        .find(names_untyped_stdlib)
        .or_else(|| lines().find(reports_error))
        .or_else(|| lines().next_back())
    else {
        return NO_OUTPUT.to_string();
    };
    if line.chars().count() <= CAUSE_CHARS {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(CAUSE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The cause of a Dependency-Check run the NVD API refused for want of a key:
/// Dependency-Check 13 needs one and its own first error line does not say so.
/// It names [`NVD_API_KEY_ENV`](super::NVD_API_KEY_ENV) first ([FR-SC-27]),
/// then the user-level `config.toml`'s `extra_args`, the only tier whose
/// `[security_scan.<scanner>]` may set them; within [`CAUSE_CHARS`]. A fixed
/// sentence: the value of a rejected key is never echoed.
///
/// [FR-SC-27]: ../../../docs/specs/requirements/FR-SC-27.md
pub(crate) const NVD_KEY_CAUSE: &str = "NVD API key missing or rejected: set NVD_API_KEY where logos \
     or `logos serve` runs, or [security_scan.dependency-check] extra_args = [\"--nvdApiKey\", \"<key>\"] \
     in the user config; free key: https://nvd.nist.gov/developers/request-an-api-key";

/// What gosec prints when the Go toolchain it runs under cannot give it the
/// export data of a standard-library package it imports: it exits 1 and
/// writes no report, so the run is `failed`, never a project with nothing to
/// scan (S-782).
pub(crate) const UNTYPED_STDLIB: &str = "without types was imported";

/// How the line the orchestrator adds after gosec's own begins
/// ([`untyped_stdlib_cause`]); [`failure_cause`] reads the cause from it.
const UNTYPED_STDLIB_CAUSE: &str = "gosec cannot type the standard library:";

/// The cause of a gosec run that printed [`UNTYPED_STDLIB`]: the gosec version
/// and the local Go it ran under (each as read, or `unknown`) and the remedy.
/// A fixed sentence around two version tokens: nothing else of gosec's output
/// is repeated.
pub(crate) fn untyped_stdlib_cause(gosec: Option<&str>, go: Option<&str>) -> String {
    format!(
        "{UNTYPED_STDLIB_CAUSE} gosec {} with the local {} ({UNTYPED_STDLIB}); use a gosec release built for that Go, or a Go toolchain gosec supports",
        gosec.unwrap_or("unknown"),
        go.unwrap_or("go unknown"),
    )
}

/// Whether `line` is the NVD client's exception for a key it will not take:
/// an `NvdApiException` that names the API key.
fn rejects_nvd_api_key(line: &str) -> bool {
    line.contains("NvdApiException") && line.to_ascii_lowercase().contains("api key")
}

/// Where `scanner`'s failure log lives: `<OUTPUT_DIR>/<scanner>.failed.log`.
fn failure_log(root: &Path, scanner: &str) -> PathBuf {
    root.join(OUTPUT_DIR).join(format!("{scanner}{FAILURE_LOG_SUFFIX}"))
}

/// Remove a failure log that no longer describes the scanner's last run: it
/// ran cleanly, or it had nothing to scan (S-769), so `status` reports no
/// failure for it. A log that cannot be removed is a warning, never an error.
fn clear_failure_log(log: &Path) {
    if let Err(e) = remove_file_if_present(log) {
        warn_log_io("clear", log, &e);
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
    /// Every entry of `.logos/security-ignore.toml` with the findings it
    /// suppresses counted, expired and orphaned flagged ([S-770]).
    ///
    /// [S-770]: ../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path
    pub suppressions: Vec<EntryStatus>,
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
    /// The copy that runs — `override`, `path` or `managed` — with its probed
    /// version and, for a managed copy older than the manifest's pin, the
    /// version `update_available` ([FR-SC-22]). `None` when none is installed
    /// here, when the scanner is disabled, and for an unregistered tool.
    ///
    /// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
    pub installed: Option<InstalledCopy>,
    /// For a scanner Logos can install that is not installed, or whose copy
    /// on `PATH` is below the minimum tested version, the command that
    /// installs it — never while a user-level `path` override is set, since
    /// nothing else would run. An MCP client cannot install: this names what
    /// the user runs in a terminal — with the licence flag, for CodeQL while
    /// its pinned version's licence is not accepted.
    pub install: Option<String>,
    /// For a scanner that installs only once its licence is accepted
    /// (CodeQL, [FR-SC-23]), the user's recorded acknowledgement; absent for
    /// every other scanner.
    ///
    /// [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub licence: Option<LicenceAcceptance>,
    /// Whether the scanner is installed only when named — never by `install
    /// --all` or `scan --install` ([FR-SC-23]): serialised only when true, so
    /// a page does not offer `scan --install` for it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub opt_in: bool,
    /// For Dependency-Check alone, whether it has an NVD API key
    /// ([FR-SC-27]): `set` when the process that runs the scan has
    /// [`NVD_API_KEY_ENV`](super::NVD_API_KEY_ENV) or the user's own
    /// `extra_args` hand it one, else `not_set` — never the key; absent from
    /// every other row.
    ///
    /// [FR-SC-27]: ../../../docs/specs/requirements/FR-SC-27.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvd_api_key: Option<NvdKeyState>,
    /// The failure log, when its last scan failed.
    pub last_failure: Option<String>,
    /// Why that scan failed, in one masked line read back from the log
    /// ([`failure_cause`], S-769); absent when there is no failure log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_cause: Option<String>,
    /// What a whole-project scan of the tree as it is now would do with the
    /// project's ignored directories ([FR-SC-26]) — a forecast, not the stored
    /// report's — for an installed, enabled scanner: `ignored_paths` and, when
    /// it would read them, `coverage_kind` and `coverage_note`. Absent for any other row, for a
    /// scanner the repository's config refuses or that has nothing to scan
    /// ([`ScanState::NotApplicable`]), and from a readout that did
    /// not ask for it ([`status`]'s `coverage`). CodeQL's row states it even
    /// when the scan would find no language to analyse.
    ///
    /// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
    #[serde(flatten)]
    pub coverage: Option<Coverage>,
}

/// Whether Dependency-Check has an NVD API key ([`ScannerStatus::nvd_api_key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NvdKeyState {
    Set,
    NotSet,
}

impl NvdKeyState {
    fn of(set: bool) -> Self {
        if set {
            NvdKeyState::Set
        } else {
            NvdKeyState::NotSet
        }
    }
}

/// A licensed scanner's acknowledgement, as recorded per user with the install
/// consent (S-746).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LicenceAcceptance {
    /// The versions whose licence the user accepted; empty until one is.
    pub accepted: Vec<String>,
    /// Whether the version this release pins is among them.
    pub pinned_accepted: bool,
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
    /// Installed, enabled, no report is stored, and it has nothing to scan
    /// here: none of the files it reads is one the project keeps (S-769). No
    /// scan is missing, so no surface counts it as one.
    NotApplicable { reason: String },
    /// A report is stored, but HEAD moved since the full scan behind it, a
    /// file it anchors changed, or no full scan stands behind it. Its
    /// `snapshot_head` is that full scan's HEAD, when there is one.
    Stale {
        because: StaleBecause,
        snapshot_head: String,
        version: String,
        /// Its counted findings, as [`Fresh`](Self::Fresh)'s.
        findings: usize,
        suppressed: usize,
        not_fresh: usize,
        test: usize,
    },
    /// A report is stored at this HEAD over unchanged files.
    Fresh {
        snapshot_head: String,
        version: String,
        /// Its counted findings — fresh, unsuppressed and in production code
        /// ([`counts`]); `0` with nothing left out is a clean scan.
        findings: usize,
        /// Findings left out because a current entry suppresses them.
        suppressed: usize,
        /// Unsuppressed findings left out because their file changed since
        /// the scan.
        not_fresh: usize,
        /// Fresh, unsuppressed findings left out because they sit in test code
        /// ([FR-SC-28]): "N more in test code". With the three above, every
        /// finding of the report, each once.
        ///
        /// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
        test: usize,
    },
}

/// One report's findings counted and, for those left out, by the first
/// reason ([`left_out`]): what [`ScanState::Fresh`] and [`ScanState::Stale`]
/// state.
#[derive(Debug, Clone, Copy, Default)]
struct ReportTally {
    counted: usize,
    excluded: Excluded,
    /// Whether a file it anchors moved since the scan, suppressed or not.
    moved: bool,
}

impl ReportTally {
    fn add(&mut self, finding: &SecurityFinding, suppression: Option<&Suppression>, scope: CodeScope) {
        self.moved |= finding.freshness == FRESHNESS_STALE;
        match left_out(finding, suppression, scope) {
            None => self.counted += 1,
            Some(why) => self.excluded.add(why),
        }
    }
}

/// Why a stored report is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleBecause {
    /// HEAD moved since the full scan behind it.
    HeadMoved,
    FilesChanged,
    /// Only scoped runs stand behind it: no full scan looked at the files
    /// outside their paths ([FR-SC-17]).
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    ScopedOnly,
}

/// Which installed scanners [`status`] probes for their version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionProbes {
    /// Every installed, enabled scanner, on every call: the CLI and MCP
    /// readout.
    Every,
    /// Only those the project's scan consent names, each binary once per
    /// process ([`probes`](super::probes)): a page read, which a cross-site
    /// page can trigger ([ADR-38], [ADR-80] §4). Every other installed copy
    /// reads [`Unprobed::NotConsented`].
    ///
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    /// [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
    Consented,
    /// None: the caller reads each scanner's state alone, which no version
    /// changes ([`Unprobed::StateOnly`]). `check` runs on page reads too.
    Never,
}

/// Per-scanner state, the copy in use, and consent ([FR-SC-07], [FR-SC-08],
/// [FR-SC-22]). Starts no process but `git` for HEAD and the version probes
/// `probes` allows — which is what the copy's version, and the choice between
/// a `PATH` copy and a managed one, need. No scanner runs. With `coverage`,
/// each installed scanner's row also states what a whole-project scan of the
/// tree as it is now does with the ignored directories ([FR-SC-26]), which
/// walks the tree once: the CLI, MCP and the Security page ask for it; the
/// workspace roll-up and `check`, which never read it, do not. Every readout,
/// those two included, walks it for an installed single-language scanner with
/// no stored report or a stale one, stopping at the first file it reads:
/// whether it is `not_applicable` decides what they count as missing (S-769).
/// Each stored report's findings are counted by [`counts`], each with its code
/// scope: `in_test_symbols` is the graph's answer for every stored finding at
/// once ([`super::scope::scopes`]).
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid config, consent record or
/// `.logos/security-ignore.toml`, a store failure, or `in_test_symbols`' own
/// error.
///
/// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
/// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
/// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
pub fn status(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
    probes: VersionProbes,
    coverage: bool,
    in_test_symbols: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<SecurityStatus> {
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    let availability = orchestrator.availability();
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    let install_consent = load_install_consent(env.user_dir.as_deref())?;
    let ignore = IgnoreFile::load(root)?;
    let view = read_latest(root)?;
    let labelled = label(&view.findings, &ignore, root, today);
    let scoped = scopes(root, &view.findings.iter().collect::<Vec<_>>(), in_test_symbols)?;
    let head_sha = crate::history::head_sha(root);

    // Per driver: each finding counted or left out, and whether a file moved.
    let mut tally: BTreeMap<&str, ReportTally> = BTreeMap::new();
    for (item, scope) in labelled.findings.iter().zip(&scoped) {
        let t = tally.entry(item.finding.scanner_name.as_str()).or_default();
        t.add(&item.finding, item.suppression.as_ref(), scope.code_scope);
    }
    let state_of = |driver: &str| {
        view.snapshots.iter().find(|s| s.scanner_name == driver).map(|snap| {
            let ReportTally { counted: findings, excluded, moved } = tally.get(driver).copied().unwrap_or_default();
            let Excluded { suppressed, not_fresh, test } = excluded;
            // A scoped run carries rows a full scan wrote, at its own HEAD or an
            // earlier one: freshness is that full scan's (S-685).
            let because = match snap.full_head_sha.as_deref() {
                None => Some(StaleBecause::ScopedOnly),
                Some(full) if head_sha.as_deref() != Some(full) => Some(StaleBecause::HeadMoved),
                Some(_) => moved.then_some(StaleBecause::FilesChanged),
            };
            let snapshot_head = snap.full_head_sha.clone().unwrap_or_else(|| snap.head_sha.clone());
            let version = snap.scanner_version.clone();
            match because {
                Some(because) => {
                    ScanState::Stale { because, snapshot_head, version, findings, suppressed, not_fresh, test }
                }
                None => ScanState::Fresh { snapshot_head, version, findings, suppressed, not_fresh, test },
            }
        })
    };

    // Walked once, and only for a readout that shows the coverage of a scanner
    // that would run.
    let ignores = (coverage && availability.iter().any(|(_, a)| *a == Availability::Installed))
        .then(|| project_ignores(root));
    let coverage_of = |s: &Scanner| {
        let ignores = ignores.as_ref()?;
        // A scan does not start one the repository's config refuses.
        if orchestrator.refusal(s).is_some() {
            return None;
        }
        Some(s.coverage(ignores))
    };

    let log_of = |name: &str| {
        let log = failure_log(root, name);
        (log.is_file() && !is_symlink(&log)).then_some(log)
    };
    let scanners = availability
        .iter()
        .map(|(s, availability)| {
            let stored = state_of(s.driver);
            // Decided before any probe: a scanner with nothing to scan here
            // is started by no readout, its version command included (S-769).
            // A fresh report outranks it; a stale one does not, since no scan
            // here can refresh it.
            let nothing_to_scan = match (availability, &stored) {
                (Availability::Installed, None | Some(ScanState::Stale { .. })) => orchestrator.not_applicable(s),
                _ => None,
            };
            let probe = match probes {
                _ if nothing_to_scan.is_some() => Probe::Skip(Unprobed::NothingToScan),
                VersionProbes::Every => Probe::Run,
                VersionProbes::Consented if consented.contains(s.name) => Probe::Cached,
                VersionProbes::Consented => Probe::Skip(Unprobed::NotConsented),
                VersionProbes::Never => Probe::Skip(Unprobed::StateOnly),
            };
            let installed = match availability {
                Availability::Installed => orchestrator.copy_in_use_with(s.name, probe),
                _ => None,
            };
            // Named exactly when a plain `install` would install it: missing
            // with no override set, or a `PATH` copy below the minimum. An
            // unprobed version is unknown, not below it.
            let missing = *availability == Availability::NotInstalled
                && orchestrator.override_of(s.name).is_none();
            let below_minimum = installed.as_ref().is_some_and(|c| {
                c.copy == super::CopyKind::Path
                    && c.unprobed.is_none()
                    && !manifest.meets_minimum(s.name, c.version.as_deref())
            });
            let entry = manifest.tool(s.name);
            let licence = entry.filter(|e| e.licence().is_some()).map(|e| LicenceAcceptance {
                accepted: install_consent.licences(s.name),
                pinned_accepted: install_consent.licence_accepted(s.name, &e.version),
            });
            let licence_flag = entry
                .and_then(|e| e.licence())
                .filter(|_| !licence.as_ref().is_some_and(|l| l.pinned_accepted))
                .map(|l| format!(" {}", l.flag))
                .unwrap_or_default();
            let install = ((missing || below_minimum) && entry.is_some())
                .then(|| format!("logos security install {}{licence_flag}", s.name));
            let state = match (availability, stored, nothing_to_scan) {
                (Availability::Disabled, _, _) => ScanState::Disabled,
                (_, _, Some(reason)) => ScanState::NotApplicable { reason },
                (_, Some(stored), None) => stored,
                (Availability::NotInstalled, None, None) => ScanState::NotInstalled,
                (Availability::Installed, None, None) => ScanState::NeverRun,
            };
            let log = log_of(s.name);
            ScannerStatus {
                scanner: s.name.to_string(),
                driver: s.driver.to_string(),
                installed,
                install,
                licence,
                opt_in: entry.is_some_and(|e| !e.is_default()),
                nvd_api_key: orchestrator.nvd_api_key_set(s).map(NvdKeyState::of),
                last_failure: log.as_deref().map(|log| relative(root, log)),
                last_failure_cause: log.as_deref().and_then(read_cause),
                // A scan would start nothing for a scanner with nothing to scan.
                coverage: match (availability, &state) {
                    (Availability::Installed, ScanState::NotApplicable { .. }) => None,
                    (Availability::Installed, _) => coverage_of(s),
                    _ => None,
                },
                state,
            }
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
                installed: None,
                install: None,
                licence: None,
                opt_in: false,
                nvd_api_key: None,
                last_failure: None,
                last_failure_cause: None,
                coverage: None,
            })
        })
        .collect();
    let pending = pending_consent(&orchestrator, &availability, &consented)
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
        suppressions: labelled.entries,
    })
}

/// The [`failure_cause`] of the failure log at `log`, read up to both
/// captured streams' caps — more than a scan ever writes there, but bounded
/// whatever else put a file in its place; `None` when it cannot be read.
fn read_cause(log: &Path) -> Option<String> {
    use std::io::Read;
    let cap = crate::jobs::spawn::STDOUT_CAP + crate::jobs::spawn::STDERR_CAP;
    let mut bytes = Vec::new();
    std::fs::File::open(log).ok()?.take(cap as u64).read_to_end(&mut bytes).ok()?;
    Some(failure_cause(&String::from_utf8_lossy(&bytes)))
}

// ── findings ─────────────────────────────────────────────────────────────────

/// How a findings list is narrowed ([FR-SC-08]). Every filter that is set
/// must hold, so combined filters intersect.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindingsFilter {
    /// The least severity listed (`high` lists high and critical), compared
    /// as a gate compares it: an `unrated` finding as `medium`. `unrated`
    /// itself lists the unrated findings alone.
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
    /// The code scope listed ([FR-SC-28]): `production` when a surface names
    /// none ([`parse`](Self::parse)), every scope in this type's default.
    ///
    /// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
    pub scope: ScopeFilter,
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
    /// [`ConfigError::InvalidValue`] (exit 2) naming a severity, edition,
    /// suppression or scope token that is not one Logos knows.
    pub fn parse(
        severity: Option<&str>,
        scanner: Option<&str>,
        category: Option<&str>,
        path: Option<&str>,
        suppression: Option<&str>,
        edition: Option<&str>,
        scope: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let bad = |key: &str, token: &str, expected: &str| ConfigError::InvalidValue {
            key: key.to_string(),
            message: format!("{token:?} is not one of {expected}"),
        };
        let severity = severity
            .map(|t| {
                Severity::from_str_stored(&t.to_ascii_lowercase())
                    .ok_or_else(|| bad("severity", t, "critical, high, medium, low, info, unrated"))
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
            scope: ScopeFilter::parse(scope)?,
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
    /// Installed, enabled scanners with no stored report and something to
    /// scan — `status`'s `never_run`, never a disabled, absent or
    /// `not_applicable` scanner.
    pub never_run: Vec<&'static str>,
    /// Said when nothing was ever scanned or ingested.
    pub notice: Option<&'static str>,
    /// Stored findings before filtering.
    pub total: usize,
    /// The code scope listed: `production` unless the caller named one.
    pub scope: ScopeFilter,
    /// Findings every other filter kept and the scope left out: what the
    /// listing states it hid, so a hidden finding is never silent.
    pub hidden_by_scope: usize,
    /// The findings the filters kept, in store order.
    pub findings: Vec<ListedFinding>,
    /// Every category of the edition over all stored findings: covered with a
    /// count when an enabled scanner's stored report could detect it, else not
    /// covered. A not-covered category can still hold findings, listed per
    /// finding above; it never reads as a count. A covered category's
    /// `state.findings` counts every code scope; its `production_findings`
    /// counts the production-code findings alone (suppressed ones in both),
    /// the figure the Security page states under its default scope.
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

/// One listed finding: the stored finding, its OWASP categories, its
/// suppression and its graph context.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListedFinding {
    #[serde(flatten)]
    pub finding: SecurityFinding,
    /// Why its severity reads as it does, when that is not plain: `unrated`'s
    /// "the scanner gave none" ([`Severity::reason`]); absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity_reason: Option<&'static str>,
    /// The categories it falls under, in OWASP's order.
    pub owasp: Vec<&'static str>,
    /// Why it falls under none, when it does not.
    pub unclassified: Option<Unclassified>,
    /// Its suppression, current or expired; `None` when no entry names it.
    pub suppression: Option<Suppression>,
    /// Its `code_scope`, `test` or `production`, and the reason it reads
    /// `test` ([FR-SC-28]).
    ///
    /// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
    #[serde(flatten)]
    pub scope: ScopeLabel,
    /// Its enclosing symbol, route reachability and blast radius, each with a
    /// confidence label ([FR-SC-13]).
    ///
    /// [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
    pub context: FindingContext,
}

impl ListedFinding {
    /// Whether an unexpired entry suppresses it.
    pub fn is_suppressed(&self) -> bool {
        matches!(self.suppression, Some(Suppression::Suppressed { .. }))
    }

    /// Whether it counts, by [`counts`]: the one definition every security
    /// surface shares.
    pub fn is_counted(&self) -> bool {
        counts(&self.finding, self.suppression.as_ref(), self.scope.code_scope)
    }

    /// Why it is left out of every count, `None` when it counts ([`left_out`]).
    pub fn left_out(&self) -> Option<LeftOut> {
        left_out(&self.finding, self.suppression.as_ref(), self.scope.code_scope)
    }
}

/// The notice a findings list carries when nothing is stored at all.
const NOTHING_SCANNED: &str =
    "no security report is stored: run `logos security scan`, or `logos security ingest <report>`";

/// The stored findings, classified, labelled, scoped and filtered
/// ([FR-SC-08], [FR-SC-28]), each with the graph context `context` returns for
/// it ([FR-SC-13]). `in_test_symbols` is called once, with every stored
/// finding, and answers whether each sits in a test symbol
/// ([`super::scope::scopes`]); `context` is called once, with the findings the
/// filters kept, and answers one context per finding in the same order.
/// Starts no process.
///
/// # Errors
/// A [`ConfigError`] (exit 2) for an invalid config or ignore file, a store
/// failure, or `in_test_symbols`' or `context`'s own error.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
/// [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
/// [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
pub fn findings(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    filter: &FindingsFilter,
    today: NaiveDate,
    in_test_symbols: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<bool>>,
    context: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<FindingContext>>,
) -> Result<SecurityFindings> {
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
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
    let mut owasp = classify(&view.findings, filter.edition, &looking);
    let labelled = label(&view.findings, &ignore, root, today);
    let scoped = scopes(root, &view.findings.iter().collect::<Vec<_>>(), in_test_symbols)?;
    // Each category's production count, over every stored finding: the scope
    // classifier's own verdict, suppression ignored, as the page's OWASP list
    // reads it.
    let production: Vec<bool> = scoped.iter().map(|s| ScopeFilter::Production.keeps(s.code_scope)).collect();
    count_production(&mut owasp.categories, &owasp.findings, &production);

    let total = labelled.findings.len();
    let mut hidden_by_scope = 0;
    let kept: Vec<_> = labelled
        .findings
        .into_iter()
        .zip(owasp.findings)
        .zip(scoped)
        .filter(|((item, class), _)| keeps(filter, &item.finding, &class.categories, item.suppression.as_ref()))
        // Every other filter first, so the hidden count is of what they kept.
        .filter(|(_, scope)| {
            let kept = filter.scope.keeps(scope.code_scope);
            hidden_by_scope += usize::from(!kept);
            kept
        })
        .collect();
    // The graph is asked only about what is listed.
    let contexts = context(&kept.iter().map(|((item, _), _)| &item.finding).collect::<Vec<_>>())?;
    anyhow::ensure!(
        contexts.len() == kept.len(),
        "the graph context answered {} findings for {} listed",
        contexts.len(),
        kept.len()
    );
    let findings = kept
        .into_iter()
        .zip(contexts)
        .map(|(((item, class), scope), context)| ListedFinding {
            severity_reason: item.finding.severity.reason(),
            finding: item.finding,
            owasp: class.categories,
            unclassified: class.unclassified,
            suppression: item.suppression,
            scope,
            context,
        })
        .collect();
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
        // A scanner with nothing to scan here is `status`'s `not_applicable`,
        // never `never_run` (S-769).
        never_run: availability
            .iter()
            .filter(|(s, a)| {
                *a == Availability::Installed && !stored.contains(s.driver) && orchestrator.not_applicable(s).is_none()
            })
            .map(|(s, _)| s.name)
            .collect(),
        notice: view.snapshots.is_empty().then_some(NOTHING_SCANNED),
        total,
        scope: filter.scope,
        hidden_by_scope,
        findings,
        categories: owasp.categories,
        suppressions: labelled.entries,
    })
}

/// `true` when a finding with these categories and this suppression passes
/// every filter that is set.
fn keeps(
    filter: &FindingsFilter,
    finding: &SecurityFinding,
    owasp: &[&'static str],
    suppression: Option<&Suppression>,
) -> bool {
    let severity = filter.severity.is_none_or(|least| match least {
        Severity::Unrated => finding.severity == Severity::Unrated,
        rated => finding.severity.gated_as() >= rated,
    });
    let scanner = filter.scanner.as_deref().is_none_or(|want| {
        finding.scanner_name.eq_ignore_ascii_case(want)
            || super::scanner(&want.to_ascii_lowercase()).is_some_and(|s| s.driver == finding.scanner_name)
    });
    let category = filter.category.as_deref().is_none_or(|want| {
        owasp.iter().any(|id| {
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
            .is_some_and(|p| crate::history::security::path_within(p, want))
    });
    let suppression = filter.suppression.is_none_or(|want| {
        let suppressed = matches!(suppression, Some(Suppression::Suppressed { .. }));
        suppressed == (want == SuppressionFilter::Suppressed)
    });
    severity && scanner && category && path && suppression
}
