//! The `coverage-run` job ([FR-CV-15], [FR-CV-14], S-738): the web UI's Run
//! coverage and MCP `coverage_run` start the project's coverage run here, on
//! the [analysis-jobs] runner the security scan runs on — one runner for both
//! surfaces under `serve`, so either one joins a run the other started.
//!
//! The run is [`Engine::coverage_run_reporting`]: each tool's start and
//! outcome is a job event, and a cancel kills the tool running and starts
//! nothing after it, ending the job cancelled. A start that does not join is
//! admitted against the project's execution consent first
//! ([`Engine::coverage_run_admit`], [FR-CV-13]). The kind takes no project
//! lock and excludes no other kind: a coverage run and a security scan run
//! side by side ([ADR-87] §5).
//!
//! **Prepared runs** (S-750, [FR-CV-17], [FR-CV-18]). The Coverage page's
//! **Install** and **Add dependency** start the same job through
//! [`start_prepared`], which readies a tool before the run: the missing
//! managed coverage tools installed, with `llvm-tools-preview` — or one
//! tool's missing coverage dev dependency added by the project's own package
//! manager. One dialog asks for every consent the run needs — the install
//! lines, the dependency command and the files it changes, the execution
//! consent for the tools the run will start — and nothing is recorded or
//! started until it has answered all of them ([`admit_prepared`]). The
//! install then runs first on the job's thread; the addition runs as the
//! run's own first step, under the dependency consent just recorded. A cancel
//! kills whichever is going: an install leaves no partial tool, an addition
//! puts back the files it changed, and no tool runs after it.
//!
//! [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [FR-CV-15]: ../../../docs/specs/requirements/FR-CV-15.md
//! [FR-CV-14]: ../../../docs/specs/requirements/FR-CV-14.md
//! [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md
//! [analysis-jobs]: ../../../docs/specs/architecture/components/analysis-jobs.md

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use super::consent::{self, ConsentAnswer, PendingConsent};
use super::deps::DependencyOutcome;
use super::CoverageJobStart;
use crate::config::{load_coverage_dependency_consent, record_coverage_dependency_consent, ConfigError};
use crate::jobs::lock::{peek, Busy};
use crate::jobs::{AnalysisJobs, Cancelled, JobContext, JobKind, Started};
use crate::observability::Surface;
use crate::tools::surface::{coverage_install_pending, record_coverage_install_lines, CoverageInstallLine};
use crate::tools::{ComponentOutcome, InstallOutcome};
use crate::Engine;

/// Start `engine`'s project's coverage run of `tool` — every tool when
/// `None` — as a job on `jobs`, or join the one running there; a run that was
/// cancelled is waited out, not joined. `answer` is the
/// web UI's consent dialog, `None` on a surface that cannot give the consent;
/// a dialog that does not accept everything pending is
/// [`Ask`](CoverageJobStart::Ask)ed again with nothing recorded. The job
/// answers for `surface`.
///
/// # Errors
/// As [`Engine::coverage_run_admit`], and the job thread's creation. Nothing
/// is started on any error.
pub fn start(
    engine: &Arc<Engine>,
    jobs: &AnalysisJobs,
    tool: Option<&str>,
    answer: Option<&ConsentAnswer<'_>>,
    surface: Surface,
) -> Result<CoverageJobStart> {
    let root = engine.root();
    // A Run right after a Cancel starts a new run once the cancelled one has
    // ended, never joins the run being cancelled.
    jobs.await_cancelled(JobKind::CoverageRun, root, crate::jobs::SHUTDOWN_GRACE);
    if !jobs.is_running(JobKind::CoverageRun, root) {
        if let Some(pending) = engine.coverage_run_admit(tool, answer)? {
            return Ok(CoverageJobStart::Ask(pending));
        }
    }
    let (engine, tool) = (Arc::clone(engine), tool.map(str::to_string));
    let started = jobs.start(JobKind::CoverageRun, root, move |job| {
        crate::observability::in_surface(surface, || run_reported(&engine, tool.as_deref(), job))
    })?;
    Ok(CoverageJobStart::Started(started))
}

/// The run of `tool` — every tool when `None` — on `job`'s thread, each step
/// a job event; the summary is the job's result.
fn run_reported(engine: &Engine, tool: Option<&str>, job: &JobContext) -> Result<serde_json::Value> {
    let summary = engine.coverage_run_reporting(tool, |step| {
        job.emit(serde_json::to_value(step).unwrap_or_default());
    })?;
    // Each tool's part is already in the events and the last-run record; the
    // job itself reads cancelled, not completed.
    if job.is_cancelled() {
        return Err(Cancelled.into());
    }
    Ok(serde_json::to_value(summary)?)
}

/// What the Coverage page readies before a run (S-750).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepare {
    /// **Install**: the missing managed coverage tools — `cargo-llvm-cov` —
    /// and `llvm-tools-preview` on the user's toolchain ([FR-CV-17]).
    ///
    /// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
    Install,
    /// **Add dependency**: the missing coverage dev dependency of the
    /// registered tool named, by the project's own package manager
    /// ([FR-CV-18]).
    ///
    /// [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
    Dependency(String),
}

/// The consent dialog's answers to a prepared start.
#[derive(Debug, Clone, Copy)]
pub struct PrepareAnswer<'a> {
    /// The execution consent: the tools, and the custom command's exact text.
    pub run: ConsentAnswer<'a>,
    /// The install lines accepted, each as [`CoverageInstallLine::accept`].
    pub install: &'a [String],
    /// The dependency commands accepted, each exactly as shown.
    pub dependency: &'a [String],
}

/// A coverage dev dependency the dialog asks to add: the exact command and
/// the files it changes ([FR-CV-18]).
///
/// [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencyAsk {
    /// The registry tool it is for.
    pub tool: &'static str,
    /// `@vitest/coverage-v8@<vitest major>` or `pytest-cov`.
    pub package: String,
    /// The package manager the lockfile names.
    pub manager: &'static str,
    /// The exact command; the answer echoes it to accept it.
    pub command: String,
    /// The files it changes, relative to the project root.
    pub files: Vec<String>,
}

/// What a prepared start still needs consent for: the dialog's question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreparePending {
    /// The execution consent the run lacks, the readied tool counted.
    pub run: PendingConsent,
    /// The install lines the consent does not cover yet.
    pub install: Vec<CoverageInstallLine>,
    /// The dependency whose command the consent does not name yet.
    pub dependencies: Vec<DependencyAsk>,
}

impl PreparePending {
    fn is_empty(&self) -> bool {
        self.run.is_empty() && self.install.is_empty() && self.dependencies.is_empty()
    }

    /// Whether `answer` accepts every line: the execution consent, each
    /// install line, each dependency command — exactly as shown.
    fn accepted_by(&self, answer: &PrepareAnswer<'_>) -> bool {
        (self.run.is_empty() || self.run.accepted_by(&answer.run))
            && self.install.iter().all(|line| answer.install.contains(&line.accept()))
            && self.dependencies.iter().all(|ask| answer.dependency.contains(&ask.command))
    }
}

/// What [`admit_prepared`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareAdmission {
    /// Every consent the prepared run needs is recorded: it may start.
    Ready,
    /// A line is unanswered: the dialog shows this, and nothing was recorded.
    Ask(PreparePending),
    /// Logos will not add this dependency here — no lockfile, an unknown or a
    /// second manager, an unreadable vitest major: the reason and the command
    /// to run by hand. Nothing was recorded or changed.
    Refused { reason: String, hand_command: String },
}

/// Decide whether `prepare` may run in `engine`'s project with `answer`
/// (S-750). Every line it needs must be accepted at once — the install lines
/// ([`coverage_install_pending`]), the dependency command, and the execution
/// consent for every tool the run will start, the one being readied included.
/// Then exactly those lines are recorded, each in its own user-level record,
/// never in the repository. Otherwise nothing is recorded. Starts no process:
/// detection here probes no version.
///
/// # Errors
/// Exit 2: an unregistered tool, a tool with no missing dev dependency to
/// add, an invalid config or consent record; and, for an install, what
/// [`coverage_install_pending`] refuses (no tools directory, a platform
/// without managed installs). An I/O error recording.
pub fn admit_prepared(engine: &Engine, prepare: &Prepare, answer: &PrepareAnswer<'_>) -> Result<PrepareAdmission> {
    let root = engine.root();
    let env = engine.scan_environment();
    let user_dir = env.user_dir.as_deref();
    let (install, dependencies, enabled): (Vec<CoverageInstallLine>, Vec<DependencyAsk>, Vec<String>) = match prepare {
        Prepare::Install => {
            let manifest = engine.tools_manifest();
            let enabled = manifest.coverage.iter().map(|entry| entry.tool.clone()).collect();
            (coverage_install_pending(root, &env, manifest)?, Vec::new(), enabled)
        }
        Prepare::Dependency(tool) => {
            let plan = super::run::dependency_plan(root, &env, tool)?.ok_or_else(|| ConfigError::InvalidValue {
                key: "tool".to_string(),
                message: format!(
                    "{tool:?} is not missing a coverage dev dependency Logos adds: only a vitest without its \
                     coverage provider, or the project's pytest without pytest-cov, is"
                ),
            })?;
            let run = plan.run();
            let (Some(command), Some(manager)) = (run.command.clone(), run.manager) else {
                let DependencyOutcome::Refused { reason, hand_command } = &run.outcome else {
                    unreachable!("a plan with no command is refused");
                };
                return Ok(PrepareAdmission::Refused { reason: reason.clone(), hand_command: hand_command.clone() });
            };
            let covered = load_coverage_dependency_consent(user_dir)?.covers(root, &command);
            let ask = DependencyAsk { tool: run.tool, package: run.package.clone(), manager, command, files: run.files.clone() };
            (Vec::new(), if covered { Vec::new() } else { vec![ask] }, vec![tool.clone()])
        }
    };
    let enabled: Vec<&str> = enabled.iter().map(String::as_str).collect();
    let run = super::run::consent_pending_enabling(root, &env, &enabled)?;
    let pending = PreparePending { run, install, dependencies };
    if pending.is_empty() {
        return Ok(PrepareAdmission::Ready);
    }
    if !pending.accepted_by(answer) {
        return Ok(PrepareAdmission::Ask(pending));
    }
    record_coverage_install_lines(user_dir, &pending.install)?;
    let commands: Vec<&str> = pending.dependencies.iter().map(|ask| ask.command.as_str()).collect();
    if !commands.is_empty() {
        record_coverage_dependency_consent(user_dir, root, &commands)?;
    }
    if !pending.run.is_empty() {
        consent::record(user_dir, root, &pending.run)?;
    }
    Ok(PrepareAdmission::Ready)
}

/// What [`start_prepared`] resolved to.
#[derive(Debug)]
pub enum PreparedStart {
    /// The job started.
    Started(Started),
    /// A line is unanswered: the dialog's question; nothing recorded or started.
    Ask(PreparePending),
    /// The dependency is one Logos will not add here.
    Refused { reason: String, hand_command: String },
    /// A coverage run is going for the project: a prepared run never joins
    /// it, since it would not ready the tool.
    Running,
    /// A scan or install of the project, started anywhere, holds its lock,
    /// which an install takes.
    Busy(Busy),
}

/// Start `engine`'s project's prepared coverage run as a `coverage-run` job
/// on `jobs` (S-750): admitted first ([`admit_prepared`]), then — on the
/// job's thread — the install, then the run of every tool, each step a job
/// event. An install that does not complete ends the job failed with what
/// went wrong, and no tool runs. The job answers for `surface`.
///
/// # Errors
/// As [`admit_prepared`], the lock file's, and the job thread's creation.
/// Nothing is started on any error.
pub fn start_prepared(
    engine: &Arc<Engine>,
    jobs: &AnalysisJobs,
    prepare: &Prepare,
    answer: &PrepareAnswer<'_>,
    surface: Surface,
) -> Result<PreparedStart> {
    let root = engine.root();
    jobs.await_cancelled(JobKind::CoverageRun, root, crate::jobs::SHUTDOWN_GRACE);
    if jobs.is_running(JobKind::CoverageRun, root) {
        return Ok(PreparedStart::Running);
    }
    let install = *prepare == Prepare::Install;
    if install {
        if let Some(busy) = peek(&engine.security_lock_file()?)? {
            return Ok(PreparedStart::Busy(busy));
        }
    }
    match admit_prepared(engine, prepare, answer)? {
        PrepareAdmission::Ready => {}
        PrepareAdmission::Ask(pending) => return Ok(PreparedStart::Ask(pending)),
        PrepareAdmission::Refused { reason, hand_command } => return Ok(PreparedStart::Refused { reason, hand_command }),
    }
    let engine = Arc::clone(engine);
    let started = jobs.start(JobKind::CoverageRun, root, move |job| {
        crate::observability::in_surface(surface, || {
            if install {
                let done = engine.coverage_install_job(|step| {
                    job.emit(serde_json::to_value(step).unwrap_or_default());
                })?;
                if job.is_cancelled() {
                    return Err(Cancelled.into());
                }
                if let Some(why) = install_failure(&done) {
                    anyhow::bail!("the install did not complete, so no coverage tool ran: {why}");
                }
            }
            run_reported(&engine, None, job)
        })
    })?;
    Ok(PreparedStart::Started(started))
}

/// What went wrong in an install that did not put everything in place: each
/// tool not installed and a failed `llvm-tools-preview` step, as its outcome
/// says; `None` when everything is in place.
fn install_failure(done: &crate::tools::surface::CoverageInstall) -> Option<String> {
    if done.exit_code() == 0 {
        return None;
    }
    let tools = done.tools.iter().filter_map(|t| {
        let why = match &t.outcome {
            InstallOutcome::Installed { .. } | InstallOutcome::AlreadyPresent { .. } => return None,
            InstallOutcome::Unavailable { reason } => (*reason).to_string(),
            InstallOutcome::Failed { stage, reason } => format!("{} failed: {reason}", serde_json::to_value(stage).ok()?.as_str()?),
            InstallOutcome::Cancelled => "cancelled".to_string(),
        };
        Some(format!("{} {}: {why}", t.tool, t.version))
    });
    let component = match &done.llvm_tools {
        ComponentOutcome::Failed { reason } => Some(format!("{}: {reason}", crate::tools::LLVM_TOOLS_ADD)),
        ComponentOutcome::Cancelled => Some(format!("{}: cancelled", crate::tools::LLVM_TOOLS_ADD)),
        _ => None,
    };
    Some(tools.chain(component).collect::<Vec<_>>().join("; "))
}
