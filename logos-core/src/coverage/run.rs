//! `logos coverage run` and `coverage refresh` ([FR-CV-14], S-737): every
//! enabled, installed tool in registry order — or the one named — then the
//! custom entry, each report ingested into the HEAD snapshot as it lands, so a
//! polyglot project ends the run with one merged snapshot ([FR-CV-04]).
//!
//! - **Consent first.** The project's execution consent ([`super::consent`])
//!   is checked for exactly the tools the run will start, and for the custom
//!   command's text, before any process starts.
//! - **A missing coverage dev dependency** — vitest without its provider, the
//!   project's pytest without pytest-cov — is added first by the project's own
//!   package manager, under the per-project dependency consent, and the tool
//!   then runs ([`super::deps`], [FR-CV-18]). The execution consent counts
//!   such a tool as one the run starts only when its addition will run —
//!   Logos would add it, and the dependency consent is given or names its
//!   command. Otherwise the tool reads `not_installed` and the run carries the
//!   consent statement, or the refusal.
//! - **One tool never stops another.** Each tool's end is its
//!   [`ToolOutcome`]; a report that does not ingest makes that tool `failed`.
//!   A cancel kills the tool running, and every tool after it reads
//!   `cancelled` without starting.
//! - **The custom entry** is the configured `[coverage_ingest].refresh_cmd`,
//!   run through `sh -c` — the one shell a run starts — because the consent
//!   pins its exact text ([FR-CV-13]). Its report is the newest conventional
//!   or configured artifact it wrote or rewrote: one an earlier run left is
//!   never credited.
//! - **What a run leaves.** Each started tool's end is recorded in
//!   [`LAST_RUN_FILE`] under `.logos/coverage/`, which `coverage status` reads
//!   back ([`runner_status`]). A failed tool's whole bounded output goes to
//!   `<tool>.failed.log` beside it, and the payload carries only a
//!   [masked](crate::history::security::mask) excerpt, so no secret a build or
//!   a test printed reaches a payload.
//!
//! Nothing here is on the watcher path: a run starts only when a user or an
//! agent asks for one ([ADR-87] §4).
//!
//! [FR-CV-04]: ../../../docs/specs/requirements/FR-CV-04.md
//! [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
//! [FR-CV-14]: ../../../docs/specs/requirements/FR-CV-14.md
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use super::consent::{self, CUSTOM_TOOL};
use super::deps::{self, DependencyOutcome, DependencyRun, Need};
use super::registry::{self, CoverageTool, DEFAULT_TIMEOUT, OUTPUT_DIR, REGISTRY};
use super::{spawn_and_settle, CoverageRunner, Detection, Origin, Probes, RunOutcome};
use crate::config::{load_coverage_consent, ConfigError, EffectiveCoverageIngest, COVERAGE_TOOLS};
use crate::history::coverage::{artifact_candidates, CoverageFormat};
use crate::history::security::mask;
use crate::history::{CoverageConsentStatus, CoverageLastRun, CoverageRunnerStatus, CoverageToolStatus, IngestSummary};
use crate::security::{real_output_dir, remove_file_if_present, write_new, ScanEnvironment};

/// The command a terminal user runs a coverage run with, which a refused run
/// names with `--consent`.
pub const RUN_ENTRY: &str = "logos coverage run";

/// The record of each tool's last run, under [`OUTPUT_DIR`].
pub const LAST_RUN_FILE: &str = "last-run.json";

/// The suffix of a failed tool's log under [`OUTPUT_DIR`]: `<tool>.failed.log`.
pub(super) const FAILURE_LOG_SUFFIX: &str = ".failed.log";

/// The most of [`LAST_RUN_FILE`] that is read: a few lines per tool.
const RECORD_CAP: u64 = 64 * 1024;

/// The shell the custom entry runs through.
const SHELL: &str = "/bin/sh";

/// What a run starts, and how it was asked for.
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    /// The one tool to run — a registry name or `custom` — or `None` for all.
    pub tool: Option<&'a str>,
    /// Record what the consent does not cover yet, then run.
    pub consent: bool,
    /// The command a terminal user runs with `--consent` to give it.
    pub entry: &'a str,
    /// Record the dependency consent for each missing coverage dev
    /// dependency's command, then add it ([`super::deps`]).
    pub dependency_consent: bool,
}

/// What `coverage run` returns: one outcome per tool it was asked to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageRunSummary {
    /// The HEAD the run's reports were ingested at, when the project has one.
    pub head_sha: Option<String>,
    /// What this run added to the project's execution consent.
    pub consent_recorded: Vec<String>,
    /// Each missing coverage dev dependency the run found, and whether it was
    /// added, is awaiting the dependency consent, or was refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DependencyRun>,
    /// One per registered tool in registry order, then the custom entry when
    /// one is configured — or only the tool named.
    pub tools: Vec<ToolRun>,
    /// The snapshot every report this run ingested went into; `None` when
    /// none was ingested.
    pub snapshot_id: Option<i64>,
    /// Whether one tool was named: then it not starting is a failure.
    #[serde(skip)]
    named: bool,
    /// Whether the dependency consent was given: then a refused addition is
    /// a failure.
    #[serde(skip)]
    dependency_consent: bool,
}

impl CoverageRunSummary {
    /// 0 when every tool the run started ran cleanly — and, for a named tool,
    /// when it started — and every dependency it started was added; 1
    /// otherwise, a failing suite whose report was ingested included, and a
    /// dependency the consent asked for that was refused.
    pub fn exit_code(&self) -> i32 {
        let clean = self.tools.iter().all(|t| match t.outcome {
            ToolOutcome::Ran { .. } => true,
            ToolOutcome::NotApplicable | ToolOutcome::Disabled | ToolOutcome::NotInstalled { .. } => !self.named,
            _ => false,
        });
        let added = self.dependencies.iter().all(|d| match d.outcome {
            DependencyOutcome::Added | DependencyOutcome::Pending { .. } => true,
            DependencyOutcome::Refused { .. } => !self.dependency_consent,
            _ => false,
        });
        i32::from(!(clean && added))
    }
}

/// One tool's part in a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolRun {
    /// The registry name, or `custom`.
    pub tool: &'static str,
    /// The version detection read, for a tool that is installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub outcome: ToolOutcome,
}

/// How one tool's part in a run ended ([FR-CV-11]'s states).
///
/// [FR-CV-11]: ../../../docs/specs/requirements/FR-CV-11.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolOutcome {
    /// It exited 0, and each report it wrote was ingested.
    Ran { reports: Vec<String>, ingests: Vec<IngestSummary> },
    /// *Ran — tests failed*: it exited non-zero but wrote reports, and each
    /// was ingested (CR-234 CRA-01).
    TestsFailed { exit_code: Option<i32>, reports: Vec<String>, ingests: Vec<IngestSummary> },
    /// It wrote no report, could not start, or a report did not ingest.
    /// `stderr` is a masked excerpt of the end of the output; `log` holds the
    /// whole bounded output, each stream's head and tail.
    /// `ingests` are the reports that did ingest before one did not: each is
    /// in the store, so the payload says so.
    Failed {
        exit_code: Option<i32>,
        stderr: String,
        log: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        ingests: Vec<IngestSummary>,
    },
    /// Its timeout passed and its whole process group was killed.
    TimedOut { after_secs: u64 },
    /// The run was cancelled before it ended; nothing it wrote is credited.
    Cancelled,
    NotApplicable,
    Disabled,
    NotInstalled {
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<&'static str>,
    },
}

impl ToolOutcome {
    /// The token a started tool's end is recorded under; `None` for a tool
    /// that did not start.
    fn recorded(&self) -> Option<&'static str> {
        Some(match self {
            ToolOutcome::Ran { .. } => "ran",
            ToolOutcome::TestsFailed { .. } => "tests_failed",
            ToolOutcome::Failed { .. } => "failed",
            ToolOutcome::TimedOut { .. } => "timed_out",
            ToolOutcome::Cancelled => "cancelled",
            ToolOutcome::NotApplicable | ToolOutcome::Disabled | ToolOutcome::NotInstalled { .. } => return None,
        })
    }

    fn exit_code(&self) -> Option<i32> {
        match self {
            ToolOutcome::TestsFailed { exit_code, .. } | ToolOutcome::Failed { exit_code, .. } => *exit_code,
            ToolOutcome::Ran { .. } => Some(0),
            _ => None,
        }
    }

    fn ingests(&self) -> &[IngestSummary] {
        match self {
            ToolOutcome::Ran { ingests, .. }
            | ToolOutcome::TestsFailed { ingests, .. }
            | ToolOutcome::Failed { ingests, .. } => ingests,
            _ => &[],
        }
    }

    fn log(&self) -> Option<&str> {
        match self {
            ToolOutcome::Failed { log, .. } => log.as_deref(),
            _ => None,
        }
    }
}

/// One thing a run can start.
#[derive(Debug, Clone, Copy)]
enum Entry {
    Tool(&'static CoverageTool),
    /// The configured `refresh_cmd`.
    Custom,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Entry::Tool(tool) => tool.name,
            Entry::Custom => CUSTOM_TOOL,
        }
    }
}

/// How a report is ingested: its path, the format to read it as (`None`
/// detects it), and the `[coverage_ingest]` table the run loaded.
pub trait Ingest: FnMut(&Path, Option<CoverageFormat>, &EffectiveCoverageIngest) -> Result<IngestSummary> {}
impl<F: FnMut(&Path, Option<CoverageFormat>, &EffectiveCoverageIngest) -> Result<IngestSummary>> Ingest for F {}

/// One step of a run as it happens ([`run`]'s `progress`): what the
/// `coverage-run` job replays (S-738), as the security scan's job replays its
/// [`ScanProgress`](crate::security::ScanProgress).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum RunProgress {
    /// An installed tool, or the custom command, is about to start.
    Started { tool: &'static str },
    /// A tool's part is settled — it ran, failed or was not run — exactly as
    /// the summary will list it.
    Finished(ToolRun),
    /// The project's package manager is about to add `tool`'s missing
    /// coverage dev dependency with `command` (S-750).
    DependencyStarted { tool: &'static str, command: String },
    /// That addition settled, exactly as the summary will list it.
    DependencyFinished(DependencyRun),
}

/// What a start of the `coverage-run` job resolved to ([`super::job::start`]).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CoverageJobStart {
    /// The job started, or the running one was joined.
    Started(crate::jobs::Started),
    /// The consent dialog's answer does not cover the run: what is pending,
    /// with nothing recorded and nothing started.
    Ask(consent::PendingConsent),
}

/// What a run would do, found before anything starts: the runner, the
/// `[coverage_ingest]` table, the entries asked for, the tools and custom
/// command the execution consent must cover, and the coverage dev
/// dependencies missing for its tools.
struct Plan {
    runner: CoverageRunner,
    ingest_cfg: EffectiveCoverageIngest,
    entries: Vec<Entry>,
    starts: Vec<&'static str>,
    custom: Option<String>,
    additions: Vec<deps::Plan>,
}

impl Plan {
    /// `dependency_consent` is whether the run records the dependency
    /// consent: with it, every addition Logos would make counts toward
    /// `starts`; without it, only those the recorded consent names.
    ///
    /// # Errors
    /// As [`run`], the consent aside.
    fn of(root: &Path, env: &ScanEnvironment, tool: Option<&str>, dependency_consent: bool) -> Result<Self> {
        let runner = CoverageRunner::open_with(root, env.user_dir.as_deref(), env.search_path.clone())?
            .with_tools_dir(env.tools_dir.clone());
        let ingest_cfg = runner.switches.effective();
        let command = ingest_cfg.refresh_cmd.clone();
        let entries = select(tool, command.as_deref())?;
        // What the consent must cover is found without a version probe, so a
        // refused run starts no process at all ([FR-CV-13]); once it is given,
        // each tool is detected again, probed, as it runs.
        let additions: Vec<deps::Plan> =
            runner.needs(&entries).into_iter().map(|(tool, need)| runner.dependency_plan(tool, need)).collect();
        let adding = deps::would_add(env.user_dir.as_deref(), root, &additions, dependency_consent)?;
        let starts = runner.would_start(&entries, &adding);
        let custom = entries.iter().any(|entry| matches!(entry, Entry::Custom)).then_some(command).flatten();
        Ok(Self { runner, ingest_cfg, entries, starts, custom, additions })
    }
}

/// What the project's execution consent does not cover yet of a run of
/// `tool` — every tool when `None` — in the project at `root`: what the web
/// UI's consent dialog lists, the custom command's text verbatim among it
/// ([FR-CV-13]). Starts no process.
///
/// # Errors
/// As [`run`], the consent aside: an invalid config or consent record, an
/// unregistered tool, `custom` with no `refresh_cmd`.
pub fn consent_pending(root: &Path, env: &ScanEnvironment, tool: Option<&str>) -> Result<consent::PendingConsent> {
    let plan = Plan::of(root, env, tool, false)?;
    let record = load_coverage_consent(env.user_dir.as_deref())?;
    Ok(consent::pending(&record, root, &plan.starts, plan.custom.as_deref()))
}

/// [`consent_pending`] of a full run that also starts each registered tool of
/// `enabled`, which the run is about to make runnable — installed, or its
/// missing dev dependency added (S-750) — in registry order. Starts no
/// process.
///
/// # Errors
/// As [`consent_pending`].
pub(crate) fn consent_pending_enabling(
    root: &Path,
    env: &ScanEnvironment,
    enabled: &[&str],
) -> Result<consent::PendingConsent> {
    let plan = Plan::of(root, env, None, false)?;
    let starts: Vec<&str> = plan
        .entries
        .iter()
        .map(|entry| entry.name())
        .filter(|name| plan.starts.contains(name) || enabled.contains(name))
        .collect();
    let record = load_coverage_consent(env.user_dir.as_deref())?;
    Ok(consent::pending(&record, root, &starts, plan.custom.as_deref()))
}

/// The plan for adding the missing coverage dev dependency of registered
/// tool `tool` in the project at `root` (S-750): its command and files, or
/// why Logos will not add it; `None` when the tool is not missing one. Found
/// with no version probe; starts no process.
///
/// # Errors
/// As [`run`], the consent aside: an invalid config, an unregistered tool.
pub(crate) fn dependency_plan(root: &Path, env: &ScanEnvironment, tool: &str) -> Result<Option<deps::Plan>> {
    if tool == CUSTOM_TOOL {
        return Ok(None);
    }
    Ok(Plan::of(root, env, Some(tool), false)?.additions.into_iter().next())
}

/// Run what `request` asks for in the project at `root`, ingesting each
/// report through `ingest` as its tool ends and handing each tool's start and
/// settled part to `progress` as it happens. Blocks until the last tool ends;
/// runs as a [foreground run](crate::jobs::foreground) — within the job, on a
/// job's thread — so a stop signal or a cancel kills the tool running.
///
/// Before the first tool, each missing coverage dev dependency is added under
/// the project's dependency consent, recorded first with
/// `request.dependency_consent` ([`super::deps`]); one the consent does not
/// cover, or that Logos will not add, is reported in the summary and never
/// fails the run.
///
/// # Errors
/// Nothing is started on any error. Exit 2: an invalid repository or
/// user-level config, or consent record; a tool name that is not registered;
/// the consent missing while `request.consent` is false (naming `<entry>
/// --consent`). An ordinary error (exit 3, as `coverage refresh` always gave):
/// `custom` with no `refresh_cmd` configured. A tool's own failure is its
/// [`ToolOutcome`], a dependency's its [`DependencyOutcome`].
pub fn run(
    root: &Path,
    env: &ScanEnvironment,
    request: &RunRequest<'_>,
    mut progress: impl FnMut(&RunProgress),
    mut ingest: impl Ingest,
) -> Result<CoverageRunSummary> {
    let Plan { runner, ingest_cfg, entries, starts, custom, additions } =
        Plan::of(root, env, request.tool, request.dependency_consent)?;
    // A registered tool the consent was not checked for keeps the state it was
    // planned in, even when an addition or an earlier tool installs it
    // meanwhile: it is never started, and the next run asks for it.
    let unconsented: Vec<Option<Detection>> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Tool(_) if !starts.contains(&entry.name()) => runner.detection(*entry, Probes::Never),
            _ => None,
        })
        .collect();
    let consent_recorded =
        consent::require(env.user_dir.as_deref(), root, &starts, custom.as_deref(), request.consent, request.entry)?;
    let additions = deps::admit(env.user_dir.as_deref(), root, additions, request.dependency_consent, request.entry)?;
    let head_sha = crate::history::head_sha(root);
    let (dependencies, tools): (Vec<DependencyRun>, Vec<ToolRun>) = crate::jobs::foreground(|| {
        let dependencies = additions
            .into_iter()
            .map(|step| {
                if !step.starts() {
                    return step.finish(&runner);
                }
                let (tool, command) = (step.run().tool, step.run().command.clone().unwrap_or_default());
                progress(&RunProgress::DependencyStarted { tool, command });
                let added = step.finish(&runner);
                progress(&RunProgress::DependencyFinished(added.clone()));
                added
            })
            .collect();
        let tools = entries
            .into_iter()
            .zip(unconsented)
            .map(|(entry, planned)| {
                let detection = planned.or_else(|| runner.detection(entry, Probes::Every));
                if matches!(detection, None | Some(Detection::Installed { .. })) {
                    progress(&RunProgress::Started { tool: entry.name() });
                }
                let ran = runner.run_entry(entry, detection, &ingest_cfg, &mut ingest);
                progress(&RunProgress::Finished(ran.clone()));
                ran
            })
            .collect();
        (dependencies, tools)
    });
    record(root, head_sha.as_deref(), &tools);
    let snapshot_id = tools.iter().flat_map(|t| t.outcome.ingests()).map(|i| i.snapshot_id).next_back();
    Ok(CoverageRunSummary {
        head_sha,
        consent_recorded,
        dependencies,
        tools,
        snapshot_id,
        named: request.tool.is_some(),
        dependency_consent: request.dependency_consent,
    })
}

/// The entries `tool` names: every registered tool then the custom entry
/// when `refresh_cmd` is configured, or the one named.
fn select(tool: Option<&str>, refresh_cmd: Option<&str>) -> Result<Vec<Entry>> {
    match tool {
        None => Ok(REGISTRY.iter().map(Entry::Tool).chain(refresh_cmd.map(|_| Entry::Custom)).collect()),
        Some(CUSTOM_TOOL) if refresh_cmd.is_some() => Ok(vec![Entry::Custom]),
        Some(CUSTOM_TOOL) => Err(anyhow::anyhow!(
            "no [coverage_ingest].refresh_cmd is configured in .logos/config.toml, so there is no custom entry \
             to run — set it, or run `logos coverage run` for the detected tools"
        )),
        Some(name) => registry::tool(name).map(|t| vec![Entry::Tool(t)]).ok_or_else(|| ConfigError::InvalidValue {
            key: "tool".to_string(),
            message: format!(
                "{name:?} is not a registered coverage tool (expected one of: {}, {CUSTOM_TOOL})",
                COVERAGE_TOOLS.join(", ")
            ),
        }.into()),
    }
}

impl CoverageRunner {
    /// `entry`'s detection; `None` for the custom entry, which is always
    /// there to run once it is configured.
    fn detection(&self, entry: Entry, probes: Probes) -> Option<Detection> {
        match entry {
            Entry::Tool(tool) => Some(self.detect_one(tool, probes)),
            Entry::Custom => None,
        }
    }

    /// The registered tools among `entries` that a run would start, found with
    /// no version probe: what the execution consent must cover. A pytest on
    /// `PATH` counts even when, probed, its pytest-cov plugin turns out to be
    /// missing — the consent then covers one tool more, never one fewer. So
    /// does each tool of `adding`, whose missing dependency the run will add
    /// before it: the tool then runs in the same run.
    fn would_start(&self, entries: &[Entry], adding: &[&'static str]) -> Vec<&'static str> {
        entries
            .iter()
            .filter(|entry| {
                adding.contains(&entry.name())
                    || matches!(self.detection(**entry, Probes::Never), Some(Detection::Installed { .. }))
            })
            .map(|entry| entry.name())
            .collect()
    }

    /// The registered tools among `entries` that a coverage dev dependency is
    /// missing for, found with no version probe ([`super::deps`]).
    fn needs(&self, entries: &[Entry]) -> Vec<(&'static str, Need)> {
        entries
            .iter()
            .filter_map(|entry| Some((entry.name(), Need::of(&self.detection(*entry, Probes::Never)?, &self.root)?)))
            .collect()
    }

    /// Run `entry` if it can start, ingest what it wrote, and settle its part.
    fn run_entry(
        &self,
        entry: Entry,
        detection: Option<Detection>,
        ingest_cfg: &EffectiveCoverageIngest,
        ingest: &mut impl Ingest,
    ) -> ToolRun {
        let started = Instant::now();
        let (version, ran, format) = match (entry, detection) {
            (Entry::Tool(tool), Some(Detection::Installed { binary, version, .. })) => {
                (version, self.run_installed(tool, &binary), Some(tool.format))
            }
            (_, Some(detection)) => (None, RunOutcome::NotRun { detection }, None),
            (_, None) => {
                let format = ingest_cfg.format_override().and_then(CoverageFormat::from_flag);
                (None, self.run_custom(ingest_cfg), format)
            }
        };
        let outcome = settle(&self.root, entry.name(), ran, &mut |report: &Path| ingest(report, format, ingest_cfg));
        trace_run(entry.name(), version.as_deref(), started, &outcome);
        ToolRun { tool: entry.name(), version, outcome }
    }

    /// Run the configured `refresh_cmd` through `sh -c` in the project root,
    /// under the default timeout, and collect the artifact it wrote or
    /// rewrote. Reached only for a configured command.
    fn run_custom(&self, ingest_cfg: &EffectiveCoverageIngest) -> RunOutcome {
        let command = ingest_cfg.refresh_cmd.as_deref().unwrap_or_default();
        let before: BTreeSet<(PathBuf, SystemTime)> = artifact_candidates(&self.root, ingest_cfg).into_iter().collect();
        let argv = [OsString::from("-c"), OsString::from(command)];
        // Only an artifact the command wrote or rewrote is its report: one an
        // earlier run left — the newest, or one discovery falls back to once
        // the newest is gone — is never credited.
        let fresh = || {
            artifact_candidates(&self.root, ingest_cfg)
                .into_iter()
                .filter(|candidate| !before.contains(candidate))
                .max_by_key(|(_, written)| *written)
                .map(|(artifact, _)| artifact)
                .into_iter()
                .collect()
        };
        let no_report = "the custom command exited 0 but wrote no coverage artifact in a conventional or \
                         configured location — check its output path or set [coverage_ingest].artifact_glob";
        spawn_and_settle(Path::new(SHELL), &argv, &self.launch(DEFAULT_TIMEOUT, &[]), fresh, no_report)
    }
}

/// Turn how a tool ended into its part in the run: ingest each report it
/// wrote, and keep a failure's whole output in its log.
fn settle(root: &Path, name: &str, ran: RunOutcome, ingest: &mut impl FnMut(&Path) -> Result<IngestSummary>) -> ToolOutcome {
    let log = root.join(OUTPUT_DIR).join(format!("{name}{FAILURE_LOG_SUFFIX}"));
    let outcome = match ran {
        RunOutcome::Ran { reports } => match ingest_all(&reports, ingest) {
            (ingests, None) => ToolOutcome::Ran { reports: relative_all(root, &reports), ingests },
            (ingests, Some(e)) => failed(root, &log, Some(0), &format!("{e:#}"), None, ingests),
        },
        RunOutcome::TestsFailed { exit_code, reports } => match ingest_all(&reports, ingest) {
            (ingests, None) => ToolOutcome::TestsFailed { exit_code, reports: relative_all(root, &reports), ingests },
            (ingests, Some(e)) => failed(root, &log, exit_code, &format!("{e:#}"), None, ingests),
        },
        RunOutcome::Failed { exit_code, stderr, cause } => {
            failed(root, &log, exit_code, &stderr, cause.as_deref(), Vec::new())
        }
        RunOutcome::TimedOut { after_secs } => ToolOutcome::TimedOut { after_secs },
        RunOutcome::Cancelled => ToolOutcome::Cancelled,
        RunOutcome::NotRun { detection } => not_run(detection),
    };
    // Cleared only inside a real `.logos/coverage/`: through a committed
    // symlink the removal would reach outside the project.
    if matches!(outcome, ToolOutcome::Ran { .. } | ToolOutcome::TestsFailed { .. }) && real_output(root) {
        if let Err(e) = remove_file_if_present(&log) {
            tracing::warn!(target: "logos::coverage", log = %log.display(), "could not clear the failure log: {e}");
        }
    }
    outcome
}

/// Ingest each report in order, stopping at the first that does not ingest:
/// the summaries of those that did — each already in the store — and that
/// error.
fn ingest_all(
    reports: &[PathBuf],
    ingest: &mut impl FnMut(&Path) -> Result<IngestSummary>,
) -> (Vec<IngestSummary>, Option<anyhow::Error>) {
    let mut ingests = Vec::with_capacity(reports.len());
    for report in reports {
        match ingest(report) {
            Ok(summary) => ingests.push(summary),
            Err(e) => return (ingests, Some(e)),
        }
    }
    (ingests, None)
}

/// A failed tool: its whole bounded `output` written to `log`, the payload's
/// excerpt masked and made from `cause` — the end of each stream — when there
/// is one, else from `output`.
fn failed(
    root: &Path,
    log: &Path,
    exit_code: Option<i32>,
    output: &str,
    cause: Option<&str>,
    ingests: Vec<IngestSummary>,
) -> ToolOutcome {
    ToolOutcome::Failed {
        exit_code,
        stderr: mask::excerpt(cause.unwrap_or(output)),
        log: write_failure_log(root, log, output),
        ingests,
    }
}

/// Write a failure's whole `output` to `log` under a real [`OUTPUT_DIR`], and
/// return its path relative to `root`; `None`, with a warning, when it cannot
/// be written. A tool's failure and a dependency addition's alike.
pub(super) fn write_failure_log(root: &Path, log: &Path, output: &str) -> Option<String> {
    let written = real_output_dir(root, OUTPUT_DIR).and_then(|_| write_new(log, output.as_bytes()));
    if let Err(e) = &written {
        tracing::warn!(target: "logos::coverage", log = %log.display(), "could not write the failure log: {e}");
    }
    written.ok().map(|()| relative(root, log))
}

fn not_run(detection: Detection) -> ToolOutcome {
    match detection {
        Detection::NotApplicable => ToolOutcome::NotApplicable,
        Detection::Disabled => ToolOutcome::Disabled,
        Detection::NotInstalled { hint } => ToolOutcome::NotInstalled { hint },
        // Never reached: an installed tool runs.
        Detection::Installed { .. } => ToolOutcome::NotInstalled { hint: None },
    }
}

/// The per-tool run event ([coverage-runner] observability): a log line, not
/// a telemetry row — the `coverage_run` span already counts the call. It
/// names counts and states, never what a tool printed.
///
/// [coverage-runner]: ../../../docs/specs/architecture/components/coverage-runner.md
fn trace_run(tool: &str, version: Option<&str>, started: Instant, outcome: &ToolOutcome) {
    let ingests = outcome.ingests();
    tracing::info!(
        target: "logos::coverage",
        tool,
        version = version.unwrap_or(""),
        duration_ms = started.elapsed().as_millis() as u64,
        outcome = outcome.recorded().unwrap_or("not_run"),
        exit_code = outcome.exit_code().map_or_else(String::new, |c| c.to_string()),
        matched_files = ingests.iter().map(|i| i.matched_files).sum::<usize>(),
        unmatched_files = ingests.iter().map(|i| i.unmatched.len()).sum::<usize>(),
        "coverage tool run",
    );
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

fn relative_all(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| relative(root, p)).collect()
}

/// Whether `.logos` and [`OUTPUT_DIR`] under `root` both exist and neither is
/// a symbolic link: the only place a run reads or removes what it left.
pub(super) fn real_output(root: &Path) -> bool {
    let real = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    real(&root.join(".logos")) && real(&root.join(OUTPUT_DIR))
}

// ── the last-run record ─────────────────────────────────────────────────────

/// Record how each tool that started ended, over what the record held: a tool
/// this run did not start keeps its last run, and a run that started nothing
/// writes nothing. A record that cannot be written is a warning; the run's own
/// outcomes stand.
fn record(root: &Path, head_sha: Option<&str>, tools: &[ToolRun]) {
    if tools.iter().all(|t| t.outcome.recorded().is_none()) {
        return;
    }
    let finished_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut runs = read_record(root);
    for t in tools {
        let Some(outcome) = t.outcome.recorded() else { continue };
        runs.insert(
            t.tool.to_string(),
            CoverageLastRun {
                outcome: outcome.to_string(),
                exit_code: t.outcome.exit_code(),
                head_sha: head_sha.map(str::to_string),
                finished_at,
                log: t.outcome.log().map(str::to_string),
            },
        );
    }
    let written = serde_json::to_vec_pretty(&runs)
        .map_err(std::io::Error::other)
        .and_then(|bytes| Ok((real_output_dir(root, OUTPUT_DIR)?, bytes)))
        .and_then(|(dir, bytes)| write_new(&dir.join(LAST_RUN_FILE), &bytes));
    if let Err(e) = written {
        tracing::warn!(target: "logos::coverage", "could not record the coverage run: {e}");
    }
}

/// Each tool's last run, as [`record`] wrote it. Read only from a regular
/// file directly under a real `.logos/coverage/`, to [`RECORD_CAP`]; anything
/// else — or anything unreadable — reads as no run at all.
fn read_record(root: &Path) -> BTreeMap<String, CoverageLastRun> {
    let path = root.join(OUTPUT_DIR).join(LAST_RUN_FILE);
    if !real_output(root) || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        return BTreeMap::new();
    }
    let mut text = String::new();
    let read = fs::File::open(&path).and_then(|f| f.take(RECORD_CAP).read_to_string(&mut text));
    read.ok().and_then(|_| serde_json::from_str(&text).ok()).unwrap_or_default()
}

// ── status ──────────────────────────────────────────────────────────────────

/// The runner's state for `coverage status` (S-737): each registered tool's
/// one state — its detection, else its last run, else `never_run` — then the
/// custom entry's when one is configured, and the project's consent as the
/// next full run would find it. Starts no process beyond the version probes
/// of a copy outside the project that `probes` allows.
///
/// # Errors
/// Exit 2 for an invalid repository or user-level config or consent record.
pub fn runner_status(root: &Path, env: &ScanEnvironment, probes: Probes) -> Result<CoverageRunnerStatus> {
    // What the next full run would ask for, planned as the run plans it.
    let Plan { runner, starts, custom, .. } = Plan::of(root, env, None, false)?;
    let states = runner.detect_probing(probes);
    let consent_record = load_coverage_consent(env.user_dir.as_deref())?;
    let pending = consent::pending(&consent_record, root, &starts, custom.as_deref());
    let consent = CoverageConsentStatus {
        consented: consent_record.tools(root).into_iter().collect(),
        pending: pending.tools,
        command_changed: pending.command_changed,
    };
    let mut last = read_record(root);
    let mut tools: Vec<CoverageToolStatus> = states
        .into_iter()
        .map(|s| {
            let offer = offer(&s.detection, root);
            CoverageToolStatus { offer, ..tool_status(s.tool, Some(s.detection), last.remove(s.tool)) }
        })
        .collect();
    if custom.is_some() {
        tools.push(tool_status(CUSTOM_TOOL, None, last.remove(CUSTOM_TOOL)));
    }
    Ok(CoverageRunnerStatus { consent, tools })
}

/// What Logos can do for a tool that is not installed (S-750): `install` a
/// managed tool missing from the machine ([FR-CV-17]), or `add_dependency`
/// for one missing its coverage dev dependency ([FR-CV-18]); `None` for every
/// other state.
///
/// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
/// [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
fn offer(detection: &Detection, root: &Path) -> Option<&'static str> {
    match detection {
        Detection::NotInstalled { hint: Some(hint) } if *hint == super::detect::HINT_COVERAGE_INSTALL => Some("install"),
        _ => Need::of(detection, root).map(|_| "add_dependency"),
    }
}

fn tool_status(tool: &str, detection: Option<Detection>, last_run: Option<CoverageLastRun>) -> CoverageToolStatus {
    let ran = last_state(last_run.as_ref());
    let (state, version, origin, hint) = match detection {
        Some(Detection::NotApplicable) => ("not_applicable", None, None, None),
        Some(Detection::Disabled) => ("disabled", None, None, None),
        Some(Detection::NotInstalled { hint }) => ("not_installed", None, None, hint),
        Some(Detection::Installed { version, origin, .. }) => (ran, version, Some(origin_token(origin)), None),
        None => (ran, None, None, None),
    };
    CoverageToolStatus { tool: tool.to_string(), state, version, origin, hint, offer: None, last_run }
}

/// The state a last run gives an installed tool; `never_run` without one, or
/// with an outcome this version does not know.
fn last_state(last_run: Option<&CoverageLastRun>) -> &'static str {
    const RECORDED: [&str; 5] = ["ran", "tests_failed", "failed", "timed_out", "cancelled"];
    last_run
        .and_then(|run| RECORDED.into_iter().find(|token| *token == run.outcome))
        .unwrap_or("never_run")
}

fn origin_token(origin: Origin) -> &'static str {
    match origin {
        Origin::Override => "override",
        Origin::Wrapper => "wrapper",
        Origin::Project => "project",
        Origin::Path => "path",
        Origin::Managed => "managed",
    }
}
