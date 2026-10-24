//! The coverage runner ([ADR-87]): what Logos needs before it starts the
//! project's own coverage tools, which build the project and run its tests,
//! and how it starts them.
//!
//! - [`consent`] — the per-project **execution consent** every run checks
//!   before any process starts ([FR-CV-13]).
//! - [`registry`] — the seven built-in tools, each a marker, a probe, a fixed
//!   argv template and its report ([FR-CV-11]).
//! - [`deps`] — a missing vitest coverage provider or pytest-cov, added as a
//!   dev dependency by the project's own package manager under the
//!   per-project **dependency consent** ([FR-CV-18]).
//! - [`CoverageRunner`] — detection ([`CoverageRunner::detect`]) and the run
//!   of one tool: its argv rendered from the template, spawned as an argv
//!   vector in a process group of its own, never through a shell, and the
//!   whole group killed when the tool's timeout passes ([NFR-SE-08]).
//!
//! Configuration comes from two places, and only one of them can choose what
//! runs. The repository's `[coverage_ingest].tools` may only switch a tool on
//! or off; the binary path, the extra arguments and the timeout come from the
//! user-level `[coverage_ingest.tools.<tool>]` alone. Both are loaded, and a
//! committed command, path or flag key fails the load, before anything is
//! spawned.
//!
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md
//! [FR-CV-11]: ../../../docs/specs/requirements/FR-CV-11.md
//! [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

pub mod consent;
pub mod deps;
mod detect;
pub mod job;
pub mod registry;
mod run;

#[cfg(all(test, unix))]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::config::{
    load_config_from_root, load_user_config, user_config_dir, user_tools_dir, ConfigError, CoverageIngest,
    UserCoverageIngest,
};
use crate::history::security::mask;
use crate::jobs::spawn::{self, inside_root, usable_path_entries, Launch};
use crate::security::{real_dir, real_output_dir, remove_file_if_present, write_new};

pub use deps::{DependencyOutcome, DependencyRun, DEPENDENCY_CONSENT_FLAG};
pub use detect::{Detection, Origin, Probes, ToolState, HINT_COVERAGE_INSTALL};
pub use run::{
    consent_pending, run, runner_status, CoverageJobStart, CoverageRunSummary, RunProgress, RunRequest, ToolOutcome,
    ToolRun, LAST_RUN_FILE, RUN_ENTRY,
};
use registry::{
    CoverageTool, Fill, Report, DEFAULT_TIMEOUT, GRADLE_INIT_FILE, GRADLE_INIT_SCRIPT, OUTPUT_DIR,
    REGISTRY,
};

/// How deep the walk for per-module reports descends from the root.
const MODULE_DEPTH: usize = 12;

/// Directories the walk for per-module reports never enters: no module lives
/// in one.
const NOT_MODULES: &[&str] = &[".git", ".logos", "node_modules", "target", "build", ".gradle", ".venv"];

/// The coverage runner for one project root, its repository switches and the
/// user's overrides resolved.
#[derive(Debug, Clone)]
pub struct CoverageRunner {
    root: PathBuf,
    switches: CoverageIngest,
    overrides: UserCoverageIngest,
    /// The usable `PATH` entries: absolute, and outside the project root.
    search_dirs: Vec<PathBuf>,
    /// [`search_dirs`](Self::search_dirs) joined: the `PATH` every tool
    /// process runs with (`None` when no entry is usable).
    child_path: Option<OsString>,
    /// The user-level tools directory a managed `cargo-llvm-cov` is found in
    /// (S-748); `None` when there is none, or when it is inside the project.
    tools_dir: Option<PathBuf>,
}

/// How one tool's run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// It exited 0 and wrote its reports.
    Ran { reports: Vec<PathBuf> },
    /// It exited non-zero but wrote reports: its tests failed, and the
    /// reports are still evidence (CR-234 CRA-01).
    TestsFailed { exit_code: Option<i32>, reports: Vec<PathBuf> },
    /// It wrote no report, or could not be started; `stderr` is what it
    /// printed — each stream's head and tail ([`spawn`]) — or why it did not
    /// start. `cause` is the part of that the payload's excerpt is made from
    /// when it is not all of `stderr`: the end of each stream
    /// ([`failure_cause`]).
    Failed { exit_code: Option<i32>, stderr: String, cause: Option<String> },
    /// Its timeout passed, and its whole process group was killed.
    TimedOut { after_secs: u64 },
    /// The job or foreground run running it was cancelled, before the tool
    /// started or while it ran: its process group was killed, and no report
    /// it wrote is credited.
    Cancelled,
    /// Not started: it does not apply, is switched off, or is not installed.
    NotRun { detection: Detection },
}

impl CoverageRunner {
    /// Load the repository's `.logos/config.toml`, the user-level config from
    /// [`user_config_dir`], the process `PATH`, and the managed copies in
    /// [`user_tools_dir`].
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2) when either config file is invalid —
    /// notably a repository `[coverage_ingest]` carrying a command, a path or
    /// a flag key. Nothing is spawned before this returns.
    pub fn open(root: &Path) -> Result<Self, ConfigError> {
        Ok(Self::open_with(root, user_config_dir().as_deref(), std::env::var_os("PATH"))?.with_tools_dir(user_tools_dir()))
    }

    /// [`open`](Self::open) with the user-level config directory and the
    /// executable search path given explicitly, and no managed copies.
    ///
    /// # Errors
    /// As [`open`](Self::open).
    pub fn open_with(root: &Path, user_dir: Option<&Path>, search_path: Option<OsString>) -> Result<Self, ConfigError> {
        let switches = load_config_from_root(root)?.coverage_ingest;
        let overrides = load_user_config(user_dir)?.coverage_ingest;
        let search_dirs = usable_path_entries(root, search_path.as_deref());
        let child_path = (!search_dirs.is_empty()).then(|| std::env::join_paths(&search_dirs).ok()).flatten();
        Ok(Self { root: root.to_path_buf(), switches, overrides, search_dirs, child_path, tools_dir: None })
    }

    /// Find managed copies in `tools_dir` — ignored when it is inside the
    /// project, which would let the repository plant a "managed" tool, as the
    /// [scan](crate::security::Orchestrator::open_in) ignores one.
    #[must_use]
    pub fn with_tools_dir(mut self, tools_dir: Option<PathBuf>) -> Self {
        let canonical_root = self.root.canonicalize().ok();
        self.tools_dir = tools_dir.filter(|dir| !inside_root(&self.root, canonical_root.as_deref(), dir));
        self
    }

    /// Detect every registered tool, in registry order. Runs no binary the
    /// project supplies, and none at all for a tool that does not apply or is
    /// switched off.
    pub fn detect(&self) -> Vec<ToolState> {
        self.detect_probing(Probes::Every)
    }

    /// [`detect`](Self::detect), running a version command only as `probes`
    /// allows.
    pub fn detect_probing(&self, probes: Probes) -> Vec<ToolState> {
        REGISTRY.iter().map(|t| ToolState { tool: t.name, detection: self.detect_one(t, probes) }).collect()
    }

    fn detect_one(&self, tool: &CoverageTool, probes: Probes) -> Detection {
        if !detect::applies(tool, &self.root) {
            return Detection::NotApplicable;
        }
        if !self.switches.is_enabled(tool.name) {
            return Detection::Disabled;
        }
        self.locate(tool, probes)
    }

    /// The copy of the registered tool `name` that would run, whatever the
    /// project's markers and switches say — the install surface asks per
    /// user, not per project — or why none would; `None` for a name that is
    /// not registered.
    pub fn copy_in_use(&self, name: &str, probes: Probes) -> Option<Detection> {
        registry::tool(name).map(|tool| self.locate(tool, probes))
    }

    /// The user-level `path` override for `name`, when one is set — whether
    /// or not it can run.
    pub fn override_of(&self, name: &str) -> Option<&Path> {
        self.overrides.get(name).and_then(|o| o.path.as_deref())
    }

    fn locate(&self, tool: &CoverageTool, probes: Probes) -> Detection {
        let lookup = detect::Lookup {
            root: &self.root,
            override_path: self.overrides.get(tool.name).and_then(|o| o.path.as_deref()),
            search_dirs: &self.search_dirs,
            child_path: self.child_path.as_ref(),
            probes,
            tools_dir: self.tools_dir.as_deref(),
        };
        detect::locate(tool, &lookup)
    }

    /// The timeout `tool` runs under: the user-level `timeout_secs`, else
    /// [`DEFAULT_TIMEOUT`].
    pub fn timeout(&self, tool: &CoverageTool) -> Duration {
        self.overrides.get(tool.name).and_then(|o| o.timeout_secs).map_or(DEFAULT_TIMEOUT, Duration::from_secs)
    }

    /// Run the registered tool `name` once, blocking until it ends, and
    /// collect the reports it wrote. Starts nothing for a tool that is not
    /// [installed](Detection::Installed). This runs the project's own build
    /// and tests: every entry that calls it checks the project's execution
    /// [consent] first ([FR-CV-13]), as every scan entry checks the scan
    /// consent before [`Orchestrator::scan`](crate::security::Orchestrator::scan).
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] for a name that is not registered, before
    /// anything is spawned. A tool's own failure is its [`RunOutcome`].
    ///
    /// [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
    pub fn run_tool(&self, name: &str) -> Result<RunOutcome, ConfigError> {
        let tool = registry::tool(name).ok_or_else(|| ConfigError::InvalidValue {
            key: "tool".to_string(),
            message: format!("{name:?} is not a registered coverage tool"),
        })?;
        let binary = match self.detect_one(tool, Probes::Every) {
            Detection::Installed { binary, .. } => binary,
            detection => return Ok(RunOutcome::NotRun { detection }),
        };
        Ok(self.run_installed(tool, &binary))
    }

    fn run_installed(&self, tool: &CoverageTool, binary: &Path) -> RunOutcome {
        // A symlink committed in place of `.logos` or of the output directory
        // would send every report and the init script outside the project.
        let out_dir = match real_output_dir(&self.root, OUTPUT_DIR) {
            Ok(dir) => dir,
            Err(e) => return failed(format!("could not prepare {OUTPUT_DIR}: {e}")),
        };
        // A report an earlier run left must never be credited to this one.
        let (out, expected) = match self.prepare_report(tool, &out_dir) {
            Ok(prepared) => prepared,
            Err(e) => return failed(format!("could not clear the previous report: {e}")),
        };
        let init_script = out_dir.join(GRADLE_INIT_FILE);
        if tool.run.contains(&registry::Arg::InitScript) {
            if let Err(e) = write_new(&init_script, GRADLE_INIT_SCRIPT.as_bytes()) {
                return failed(format!("could not write {}: {e}", init_script.display()));
            }
        }
        let extra = self.overrides.get(tool.name).map_or(&[][..], |o| o.extra_args.as_slice());
        let argv = tool.argv(&Fill { out: &out, extra, init_script: &init_script });
        let launch = self.launch(self.timeout(tool), tool.run_env);
        let reports = || expected.collect(&self.root);
        spawn_and_settle(binary, &argv, &launch, reports, &format!("{} exited 0 but wrote no report", tool.name))
    }

    /// How every tool process starts: in the project root, on the usable
    /// `PATH`, with `env` added, killed with its whole group after `timeout`.
    fn launch<'a>(&'a self, timeout: Duration, env: &'a [(&'a str, &'a str)]) -> Launch<'a> {
        Launch {
            cwd: &self.root,
            env,
            vars: &[],
            isolated: None,
            path: self.child_path.as_ref(),
            deadline: Some(timeout),
        }
    }
}

/// Run `program` with `argv` as `launch` says and settle how it ended — the
/// one judgement behind every tool and the custom entry. `reports` collects
/// what it wrote once it exited; `no_report` is the failure an exit 0 that
/// wrote none reads as.
fn spawn_and_settle(
    program: &Path,
    argv: &[OsString],
    launch: &Launch<'_>,
    reports: impl FnOnce() -> Vec<PathBuf>,
    no_report: &str,
) -> RunOutcome {
    let finished = match spawn::run(program, argv, launch) {
        Ok(finished) => finished,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => return RunOutcome::Cancelled,
        Err(e) => return failed(format!("could not start {}: {e}", program.display())),
    };
    // A cancel during the run killed the group: whatever exit status that
    // left, and whatever reports some modules wrote before it, the run was
    // cancelled, and none of it is credited.
    if crate::jobs::cancelled() {
        return RunOutcome::Cancelled;
    }
    let Some(status) = finished.status else {
        return RunOutcome::TimedOut { after_secs: launch.deadline.unwrap_or_default().as_secs() };
    };
    let reports = reports();
    let exit_code = status.code();
    match (status.success(), reports.is_empty()) {
        (true, false) => RunOutcome::Ran { reports },
        (false, false) => RunOutcome::TestsFailed { exit_code, reports },
        (true, true) => RunOutcome::Failed { exit_code, stderr: no_report.to_string(), cause: None },
        (false, true) => {
            RunOutcome::Failed { exit_code, stderr: failure_output(&finished), cause: failure_cause(&finished) }
        }
    }
}

/// What a failed tool printed. Many coverage tools print their error to
/// stdout and only warnings to stderr, so neither stream alone is the cause:
/// with text on both, both are kept, labelled, stderr last.
fn failure_output(finished: &spawn::Finished) -> String {
    match (finished.stdout.trim().is_empty(), finished.stderr.trim().is_empty()) {
        (_, true) => finished.stdout.clone(),
        (true, false) => finished.stderr.clone(),
        (false, false) => format!(
            "stdout:\n{}\nstderr:\n{}",
            finished.stdout.trim_end(),
            finished.stderr.trim_end()
        ),
    }
}

/// Why a failed tool failed, in the end of what it printed, when both streams
/// have text — one alone is [`failure_output`] whole, and its excerpt is
/// already its last lines: the last [`CAUSE_STDOUT_LINES`] lines of stdout (a
/// test runner's `failures:` list and `test result: FAILED` line) and the last
/// [`CAUSE_STDERR_LINES`] of stderr, labelled as [`failure_output`] is, each
/// within its own byte budget so that a long line of one stream cannot push
/// the other's out of the payload's excerpt. Each stream is masked whole
/// before it is cut, so a secret that straddles the cut is masked first.
fn failure_cause(finished: &spawn::Finished) -> Option<String> {
    if finished.stdout.trim().is_empty() || finished.stderr.trim().is_empty() {
        return None;
    }
    Some(format!(
        "stdout:\n{}\nstderr:\n{}",
        tail(&finished.stdout, CAUSE_STDOUT_LINES, CAUSE_STDOUT_BYTES),
        tail(&finished.stderr, CAUSE_STDERR_LINES, CAUSE_STDERR_BYTES)
    ))
}

/// The last `lines` lines of `text`, masked, and of those the last `bytes`
/// bytes at most, an ellipsis standing for what was cut.
fn tail(text: &str, lines: usize, bytes: usize) -> String {
    let masked = mask::mask_secrets(text.trim_end());
    let all: Vec<&str> = masked.lines().collect();
    let kept = all[all.len().saturating_sub(lines)..].join("\n");
    if kept.len() <= bytes {
        return kept;
    }
    let mut start = kept.len() - (bytes - '…'.len_utf8());
    while !kept.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &kept[start..])
}

/// How many trailing lines of stdout and of stderr a failure's cause keeps when
/// both have text — with their two labels, the payload's twelve — and how many
/// bytes of each: with the labels, the payload's 1 KiB.
const CAUSE_STDOUT_LINES: usize = 6;
const CAUSE_STDERR_LINES: usize = 4;
const CAUSE_STDOUT_BYTES: usize = 640;
const CAUSE_STDERR_BYTES: usize = 360;

impl CoverageRunner {
    /// The [`Arg::Out`](registry::Arg::Out) path for `tool` under `out_dir`,
    /// and where its reports will be once it ends — with the report an
    /// earlier run left there cleared, or, for per-module reports, those
    /// already there remembered.
    fn prepare_report(&self, tool: &CoverageTool, out_dir: &Path) -> io::Result<(PathBuf, Expected)> {
        Ok(match tool.report {
            Report::File(file) => {
                let out = out_dir.join(file);
                remove_file_if_present(&out)?;
                (out.clone(), Expected::One(out))
            }
            Report::Dir { dir, produced } => {
                let out = out_dir.join(dir);
                real_dir(&out)?;
                let report = out.join(produced);
                remove_file_if_present(&report)?;
                (out, Expected::One(report))
            }
            Report::Modules(suffix) => {
                let before = module_reports(&self.root, suffix);
                (out_dir.to_path_buf(), Expected::Modules { suffix, before })
            }
        })
    }
}

/// Where a run's reports are found once it ends.
enum Expected {
    /// This file, when the run wrote it.
    One(PathBuf),
    /// Each module's `suffix` the run wrote or rewrote: `before` is what was
    /// there before it started, and when each was last written.
    Modules { suffix: &'static str, before: BTreeMap<PathBuf, Option<SystemTime>> },
}

impl Expected {
    fn collect(self, root: &Path) -> Vec<PathBuf> {
        match self {
            Expected::One(report) => [report].into_iter().filter(|r| r.is_file()).collect(),
            Expected::Modules { suffix, before } => module_reports(root, suffix)
                .into_iter()
                .filter(|(report, written)| before.get(report) != Some(written))
                .map(|(report, _)| report)
                .collect(),
        }
    }
}

fn failed(stderr: String) -> RunOutcome {
    RunOutcome::Failed { exit_code: None, stderr, cause: None }
}

/// Every `<module>/<suffix>` under `root` that is a regular file (never a
/// symlink), with its modification time, in path order: the per-module
/// reports of a Maven or Gradle run. Walks real directories only, never a
/// symlink, to [`MODULE_DEPTH`], and never into [`NOT_MODULES`].
fn module_reports(root: &Path, suffix: &str) -> BTreeMap<PathBuf, Option<SystemTime>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let report = dir.join(suffix);
        if let Some(meta) = fs::symlink_metadata(&report).ok().filter(fs::Metadata::is_file) {
            found.insert(report, meta.modified().ok());
        }
        if depth == MODULE_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && !NOT_MODULES.iter().any(|n| entry.file_name() == **n) {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    found
}
