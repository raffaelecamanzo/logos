//! A missing coverage **dev dependency**, added by the project's own package
//! manager under the per-project dependency consent ([FR-CV-18], [ADR-87] as
//! amended, S-749).
//!
//! Two registry tools can be found without what a coverage run needs, and
//! each is fixed by a dev dependency of the project, never by a tool Logos
//! installs ([`Need`]):
//! - vitest without a coverage provider gets `@vitest/coverage-v8` at the
//!   major of the vitest installed (read from `node_modules/vitest`). A major
//!   that cannot be read is refused, never guessed;
//! - the project `.venv`'s pytest without its pytest-cov plugin gets
//!   `pytest-cov`. A pytest on `PATH` is not the project's: its missing plugin
//!   is reported, and nothing is added to the project for it.
//!
//! jest, Maven and Gradle (JaCoCo is attached from the command line or an init
//! script) and `go test` need nothing added, so no detection of theirs is a
//! [`Need`].
//!
//! **The manager comes from the lockfile** ([`LOCKFILES`]): npm, pnpm, yarn or
//! bun; uv or poetry; pip when the only Python lock is a `requirements.txt` and
//! the project has a `.venv` to install into. No lockfile, the lockfile of a
//! manager Logos does not run, or the lockfiles of two managers are refused,
//! naming the command to run by hand, and nothing starts. The command is built
//! here from the manager and literals — its one value read from the
//! repository is the vitest major, a number — and the manager is the first on
//! the usable `PATH`, outside the project, spawned as an argv vector, never
//! through a shell ([NFR-SE-08]). pip runs as the project virtualenv's
//! `python -m pip`, the interpreter the project's pytest runs on.
//!
//! **Consent.** The command runs only once the project's dependency consent
//! names it exactly as shown ([`load_coverage_dependency_consent`]), recorded
//! by [`DEPENDENCY_CONSENT_FLAG`] in a terminal. Without it the run carries the
//! consent statement instead: the command, the files it changes, and that the
//! package manager may run dependency lifecycle scripts ([NFR-SE-02]). Logos
//! never commits: the manifest and lockfile change stays in the working tree
//! for the user to review.
//!
//! **No half-applied change** (S-750). An addition that started and does not
//! end added — cancelled, failed or timed out — puts back what it changed:
//! each file the consent named holds again what it held before, one it
//! created is removed, and the added package's own entries that were not
//! there before are removed — vitest's `node_modules/@vitest/coverage-v8`,
//! pytest-cov's package, dist-info and `.pth` hook in the project `.venv`'s
//! site-packages — so the project never reads as measured by a dependency its
//! manifest does not declare. Packages the manager pulled in beside it (such
//! as `coverage`) are left: they measure nothing on their own.
//!
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::detect::{read_json, HINT_PYTEST_COV, HINT_VITEST_PROVIDER};
use super::registry::OUTPUT_DIR;
use super::run::{real_output, write_failure_log, FAILURE_LOG_SUFFIX};
use super::{failure_output, CoverageRunner, Detection};
use crate::config::{load_coverage_dependency_consent, record_coverage_dependency_consent, ConfigError};
use crate::history::security::mask;
use crate::jobs::spawn::{self, find_on_path, is_executable};
use crate::security::registry::parse_version;

/// The flag, after a run's entry, that records the dependency consent.
pub const DEPENDENCY_CONSENT_FLAG: &str = "--dependency-consent";

/// How long a package manager may take to add a dependency before its
/// process group is killed.
const ADD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The vitest coverage provider Logos adds.
const VITEST_PROVIDER: &str = "@vitest/coverage-v8";

/// The pytest coverage plugin Logos adds.
const PYTEST_COV: &str = "pytest-cov";

/// The project virtualenv's interpreter, which pip runs as.
const VENV_PYTHON: &str = ".venv/bin/python";

/// The project virtualenv's pytest: the only pytest a dependency is added for.
const VENV_PYTEST: &str = ".venv/bin/pytest";

/// pip's lock: read only when no other Python lockfile is there.
const PIP_REQUIREMENTS: &str = "requirements.txt";

/// A coverage dev dependency a tool was found without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    /// vitest without a coverage provider.
    VitestProvider,
    /// The project `.venv`'s pytest without pytest-cov.
    PytestCov,
}

impl Need {
    /// The dependency `detection` says is missing in the project at `root`,
    /// if adding one would fix it. pytest-cov is missing for the project only
    /// when the pytest without it is the project's `.venv` one: a pytest on
    /// `PATH` reads the same hint once probed, and nothing is added for it.
    pub(crate) fn of(detection: &Detection, root: &Path) -> Option<Self> {
        match detection {
            Detection::NotInstalled { hint: Some(hint) } if *hint == HINT_VITEST_PROVIDER => Some(Self::VitestProvider),
            Detection::NotInstalled { hint: Some(hint) } if *hint == HINT_PYTEST_COV && is_executable(&root.join(VENV_PYTEST)) => {
                Some(Self::PytestCov)
            }
            _ => None,
        }
    }

    fn ecosystem(self) -> Ecosystem {
        match self {
            Need::VitestProvider => Ecosystem::Node,
            Need::PytestCov => Ecosystem::Python,
        }
    }

    /// What the consent statement says is missing.
    fn missing(self) -> &'static str {
        match self {
            Need::VitestProvider => "vitest is installed without a coverage provider",
            Need::PytestCov => "the project's pytest is installed without its pytest-cov plugin",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ecosystem {
    Node,
    Python,
}

impl Ecosystem {
    /// The manager a refusal's hand command names when the lockfile names none.
    fn default_manager(self) -> Manager {
        match self {
            Ecosystem::Node => Manager::Npm,
            Ecosystem::Python => Manager::Pip,
        }
    }
}

/// A package manager Logos runs to add a coverage dev dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Uv,
    Poetry,
    Pip,
}

impl Manager {
    /// The manager's name, and its binary's on `PATH`.
    pub fn name(self) -> &'static str {
        match self {
            Manager::Npm => "npm",
            Manager::Pnpm => "pnpm",
            Manager::Yarn => "yarn",
            Manager::Bun => "bun",
            Manager::Uv => "uv",
            Manager::Poetry => "poetry",
            Manager::Pip => "pip",
        }
    }

    /// The arguments that add `package` as a dev dependency, the program
    /// excluded.
    fn args(self, package: &str) -> Vec<String> {
        let fixed: &[&str] = match self {
            Manager::Npm => &["install", "--save-dev"],
            Manager::Pnpm | Manager::Yarn => &["add", "-D"],
            Manager::Bun | Manager::Uv => &["add", "--dev"],
            Manager::Poetry => &["add", "--group", "dev"],
            Manager::Pip => &["-m", "pip", "install"],
        };
        fixed.iter().map(ToString::to_string).chain([package.to_string()]).collect()
    }

    /// The command as the consent shows — and pins — it.
    fn command(self, package: &str) -> String {
        let program = match self {
            Manager::Pip => VENV_PYTHON,
            other => other.name(),
        };
        [program.to_string()].into_iter().chain(self.args(package)).collect::<Vec<_>>().join(" ")
    }

    /// The command a refusal names to run by hand: [`command`](Self::command),
    /// except pip's, which names the virtualenv's interpreter generically.
    fn by_hand(self, package: &str) -> String {
        match self {
            Manager::Pip => format!("python -m pip install {package}"),
            other => other.command(package),
        }
    }
}

/// Each lockfile a manager is read from, by ecosystem; `None` is the lockfile
/// of a manager Logos does not run.
const LOCKFILES: &[(Ecosystem, &str, Option<Manager>)] = &[
    (Ecosystem::Node, "package-lock.json", Some(Manager::Npm)),
    (Ecosystem::Node, "npm-shrinkwrap.json", Some(Manager::Npm)),
    (Ecosystem::Node, "pnpm-lock.yaml", Some(Manager::Pnpm)),
    (Ecosystem::Node, "yarn.lock", Some(Manager::Yarn)),
    (Ecosystem::Node, "bun.lock", Some(Manager::Bun)),
    (Ecosystem::Node, "bun.lockb", Some(Manager::Bun)),
    (Ecosystem::Node, "deno.lock", None),
    (Ecosystem::Python, "uv.lock", Some(Manager::Uv)),
    (Ecosystem::Python, "poetry.lock", Some(Manager::Poetry)),
    (Ecosystem::Python, "pdm.lock", None),
    (Ecosystem::Python, "Pipfile.lock", None),
    (Ecosystem::Python, "pixi.lock", None),
];

/// The command a refusal names to run by hand for `lock`, the lockfile of a
/// manager Logos does not run: that manager's own way to add `package` as a
/// dev dependency. Logos never runs it.
fn unknown_by_hand(lock: &str, package: &str) -> String {
    match lock {
        "deno.lock" => format!("deno add --dev npm:{package}"),
        "pdm.lock" => format!("pdm add --dev {package}"),
        "Pipfile.lock" => format!("pipenv install --dev {package}"),
        "pixi.lock" => format!("pixi add --pypi {package}"),
        other => format!("add {package} as a dev dependency with the manager of {other}"),
    }
}

/// Whether `lock` is the lockfile of a manager Logos does not run.
fn is_unknown(lock: &str) -> bool {
    LOCKFILES.iter().any(|(_, file, manager)| *file == lock && manager.is_none())
}

/// What the lockfiles at a project root say about an ecosystem's manager.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    /// One manager, and the lockfiles that name it.
    One(Manager, Vec<&'static str>),
    /// No lockfile.
    Nothing,
    /// Only the lockfile of a manager Logos does not run.
    Unknown(Vec<&'static str>),
    /// Lockfiles of more than one manager.
    Several(Vec<&'static str>, Vec<Manager>),
}

fn lockfiles(root: &Path, ecosystem: Ecosystem) -> Found {
    let present: Vec<(&'static str, Option<Manager>)> = LOCKFILES
        .iter()
        .filter(|(e, file, _)| *e == ecosystem && is_regular(&root.join(file)))
        .map(|(_, file, manager)| (*file, *manager))
        .collect();
    let files: Vec<&'static str> = present.iter().map(|(file, _)| *file).collect();
    let mut managers: Vec<Option<Manager>> = present.iter().map(|(_, m)| *m).collect();
    managers.dedup();
    match managers.as_slice() {
        [] if ecosystem == Ecosystem::Python && is_regular(&root.join(PIP_REQUIREMENTS)) => {
            Found::One(Manager::Pip, vec![PIP_REQUIREMENTS])
        }
        [] => Found::Nothing,
        [Some(manager)] => Found::One(*manager, files),
        [None] => Found::Unknown(files),
        _ => Found::Several(files, managers.into_iter().flatten().collect()),
    }
}

/// A regular file, never a symlink's target.
fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// The major of the vitest installed in the project, from
/// `node_modules/vitest/package.json`.
fn vitest_major(root: &Path) -> Option<u32> {
    let json = read_json(&root.join("node_modules/vitest/package.json"))?;
    let version = parse_version(json.get("version")?.as_str()?)?;
    version.split('.').next()?.parse().ok()
}

/// One missing coverage dev dependency in a run: what adding it takes, and
/// how it ended — or why Logos would not add it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencyRun {
    /// The registry tool that needs it.
    pub tool: &'static str,
    /// What is added: `@vitest/coverage-v8@<vitest major>` or `pytest-cov`.
    pub package: String,
    /// The package manager the lockfile names, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager: Option<&'static str>,
    /// The exact command, as the consent shows it; absent when refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The files the command changes, relative to the project root.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// What an addition that did not end added put back as it was (S-750),
    /// relative to the project root.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub restored: Vec<String>,
    #[serde(flatten)]
    pub outcome: DependencyOutcome,
}

/// How adding one coverage dev dependency ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DependencyOutcome {
    /// Not started: the project's dependency consent does not name the
    /// command. `consent` is the statement a terminal user consents to.
    Pending { consent: String },
    /// Not started: Logos will not choose a manager or a version here.
    /// `hand_command` is the command to run by hand.
    Refused { reason: String, hand_command: String },
    /// The package manager exited 0. The change is left uncommitted.
    Added,
    /// It exited non-zero or could not start: `stderr` is a masked excerpt,
    /// `log` holds the whole bounded output.
    Failed { exit_code: Option<i32>, stderr: String, log: Option<String> },
    /// Its timeout passed and its whole process group was killed.
    TimedOut { after_secs: u64 },
    /// The run was cancelled before or while it ran.
    Cancelled,
}

/// What adding one dependency takes, found before anything starts.
pub(crate) struct Plan {
    need: Need,
    run: DependencyRun,
    /// The binary and argv that add it; `None` when it is refused.
    start: Option<(PathBuf, Vec<OsString>)>,
}

impl CoverageRunner {
    /// The plan for adding what `need` names, for `tool`. Starts nothing.
    pub(crate) fn dependency_plan(&self, tool: &'static str, need: Need) -> Plan {
        let ecosystem = need.ecosystem();
        let (package, major_unknown) = match need {
            Need::VitestProvider => match vitest_major(&self.root) {
                Some(major) => (format!("{VITEST_PROVIDER}@{major}"), false),
                None => (format!("{VITEST_PROVIDER}@<vitest major>"), true),
            },
            Need::PytestCov => (PYTEST_COV.to_string(), false),
        };
        let refuse = |manager: Option<Manager>, reason: String, hand_command: String| Plan {
            need,
            run: DependencyRun {
                tool,
                package: package.clone(),
                manager: manager.map(Manager::name),
                command: None,
                files: Vec::new(),
                restored: Vec::new(),
                outcome: DependencyOutcome::Refused { reason, hand_command },
            },
            start: None,
        };
        let fallback = ecosystem.default_manager();
        let (manager, locks) = match lockfiles(&self.root, ecosystem) {
            Found::One(manager, locks) => (manager, locks),
            Found::Nothing => {
                let known: Vec<&str> = LOCKFILES
                    .iter()
                    .filter(|(e, _, m)| *e == ecosystem && m.is_some())
                    .map(|(_, file, _)| *file)
                    .chain((ecosystem == Ecosystem::Python).then_some(PIP_REQUIREMENTS))
                    .collect();
                let reason = format!(
                    "no lockfile names this project's package manager (Logos reads {}), so Logos will not guess \
                     one: add {package} as a dev dependency by hand with the project's package manager",
                    known.join(", ")
                );
                return refuse(None, reason, fallback.by_hand(&package));
            }
            Found::Unknown(locks) => {
                let what = if locks.len() == 1 { "is the lockfile of a package manager" } else { "are lockfiles of package managers" };
                let reason = format!(
                    "{} {what} Logos does not run: add {package} as a dev dependency by hand with it",
                    locks.join(", ")
                );
                let hand: Vec<String> = locks.iter().map(|lock| unknown_by_hand(lock, &package)).collect();
                return refuse(None, reason, hand.join(" or "));
            }
            Found::Several(locks, managers) => {
                let reason = format!(
                    "{} name more than one package manager, so Logos will not choose one: add {package} as a dev \
                     dependency by hand with the one the project uses",
                    locks.join(", ")
                );
                let unknown = locks.iter().filter(|lock| is_unknown(lock)).map(|lock| unknown_by_hand(lock, &package));
                let hand: Vec<String> = managers.iter().map(|m| m.by_hand(&package)).chain(unknown).collect();
                return refuse(None, reason, hand.join(" or "));
            }
        };
        if major_unknown {
            let reason = "the version of the installed vitest could not be read from node_modules/vitest/package.json, \
                          so Logos will not guess the provider's major: add it by hand at the major of the vitest \
                          in use"
                .to_string();
            return refuse(Some(manager), reason, manager.by_hand(&package));
        }
        let program = match manager {
            Manager::Pip => Some(self.root.join(VENV_PYTHON)).filter(|python| is_executable(python)),
            other => find_on_path(&self.search_dirs, other.name()),
        };
        let Some(program) = program else {
            let reason = match manager {
                Manager::Pip => format!(
                    "{PIP_REQUIREMENTS} names pip, but there is no project virtualenv ({VENV_PYTHON}) to add {package} \
                     to: add it by hand in the virtualenv the project's tests run in"
                ),
                other => format!(
                    "{} names {name}, but no {name} is on the PATH: add {package} by hand",
                    locks.join(", "),
                    name = other.name()
                ),
            };
            return refuse(Some(manager), reason, manager.by_hand(&package));
        };
        let files = match manager {
            Manager::Pip => vec![".venv/".to_string()],
            _ => {
                let manifest = match ecosystem {
                    Ecosystem::Node => "package.json",
                    Ecosystem::Python => "pyproject.toml",
                };
                [manifest].into_iter().chain(locks).map(str::to_string).collect()
            }
        };
        let argv = manager.args(&package).into_iter().map(OsString::from).collect();
        Plan {
            need,
            run: DependencyRun {
                tool,
                command: Some(manager.command(&package)),
                package,
                manager: Some(manager.name()),
                files,
                restored: Vec::new(),
                outcome: DependencyOutcome::Added,
            },
            start: Some((program, argv)),
        }
    }
}

impl Plan {
    /// What adding it takes, as the consent shows it: its command and files,
    /// or why Logos will not add it.
    pub(crate) fn run(&self) -> &DependencyRun {
        &self.run
    }
}

/// One plan, admitted: to be started, or already settled.
pub(crate) struct Step {
    need: Need,
    run: DependencyRun,
    start: Option<(PathBuf, Vec<OsString>)>,
}

/// Settle what the project's dependency consent allows of `plans` in the
/// project at `root`, before anything starts. A refused plan is reported as
/// it is. A plan whose command the consent names is to be started; one it
/// does not name is recorded and started with `consent`, else reported
/// [`Pending`](DependencyOutcome::Pending) with the consent statement, naming
/// `<entry> --dependency-consent`. Reads no record when there are no plans.
///
/// # Errors
/// As [`load_coverage_dependency_consent`] and
/// [`record_coverage_dependency_consent`]. Nothing is started.
pub(crate) fn admit(
    user_dir: Option<&Path>,
    root: &Path,
    plans: Vec<Plan>,
    consent: bool,
    entry: &str,
) -> Result<Vec<Step>, ConfigError> {
    if plans.is_empty() {
        return Ok(Vec::new());
    }
    let record = load_coverage_dependency_consent(user_dir)?;
    let mut granted = Vec::new();
    let steps: Vec<Step> = plans
        .into_iter()
        .map(|Plan { need, mut run, start }| {
            let command = run.command.clone().unwrap_or_default();
            let start = match start {
                Some(start) if record.covers(root, &command) => Some(start),
                Some(start) if consent => {
                    granted.push(command);
                    Some(start)
                }
                Some(_) => {
                    run.outcome = DependencyOutcome::Pending { consent: statement(need, &run, entry) };
                    None
                }
                None => None,
            };
            Step { need, run, start }
        })
        .collect();
    if !granted.is_empty() {
        record_coverage_dependency_consent(user_dir, root, &granted)?;
    }
    Ok(steps)
}

/// The tools among `plans` whose addition a run would start in the project at
/// `root`: each Logos would make (not refused) whose command `consent` is
/// about to record, or the recorded dependency consent already names. What
/// the execution consent must cover beside the installed tools. Reads no
/// record when there are no plans.
///
/// # Errors
/// As [`load_coverage_dependency_consent`].
pub(crate) fn would_add(
    user_dir: Option<&Path>,
    root: &Path,
    plans: &[Plan],
    consent: bool,
) -> Result<Vec<&'static str>, ConfigError> {
    if plans.is_empty() {
        return Ok(Vec::new());
    }
    let record = load_coverage_dependency_consent(user_dir)?;
    let covered = |plan: &&Plan| consent || plan.run.command.as_deref().is_some_and(|c| record.covers(root, c));
    Ok(plans.iter().filter(|plan| plan.start.is_some()).filter(covered).map(|plan| plan.run.tool).collect())
}

/// The dependency consent statement for `run`: the command, the files it
/// changes, that nothing is committed, and that lifecycle scripts may run.
fn statement(need: Need, run: &DependencyRun, entry: &str) -> String {
    format!(
        "{missing}: `{command}` adds {package} as a dev dependency with {manager}, changing {files} — Logos never \
         commits, so the change is left in the working tree for you to review. The package manager may run \
         dependency lifecycle scripts (install scripts), which execute code from the packages it fetches, and it \
         may reach the network. Run `{entry} --consent {DEPENDENCY_CONSENT_FLAG}` in a terminal to record this \
         project's dependency consent for that command, and its execution consent for {tool}, which the run then \
         starts, and add it (an MCP client cannot)",
        missing = need.missing(),
        tool = run.tool,
        command = run.command.as_deref().unwrap_or_default(),
        package = run.package,
        manager = run.manager.unwrap_or_default(),
        files = run.files.join(", "),
    )
}

impl Step {
    /// Whether [`finish`](Self::finish) starts a package manager: the
    /// addition is admitted, neither refused nor awaiting its consent.
    pub(crate) fn starts(&self) -> bool {
        self.start.is_some()
    }

    /// What the step adds, and the command, as the run reports it.
    pub(crate) fn run(&self) -> &DependencyRun {
        &self.run
    }

    /// Start an admitted step's package manager in the project root and
    /// settle how it ended; a settled step is returned as it is. An addition
    /// that does not end added puts back what it changed ([module docs](self)).
    pub(crate) fn finish(self, runner: &CoverageRunner) -> DependencyRun {
        let Step { need, mut run, start } = self;
        let Some((program, argv)) = start else { return run };
        let started = Instant::now();
        let before = Before::of(&runner.root, need, &run.files);
        run.outcome = add(runner, run.tool, &program, &argv);
        if run.outcome != DependencyOutcome::Added {
            run.restored = before.put_back(&runner.root);
        }
        trace_add(&run, started);
        run
    }
}

/// What the files an addition changes held before it started.
struct Before {
    /// Each file the consent named, with its bytes — `None` when it was not
    /// there. A directory (pip's `.venv/`), a symlink or a file that could not
    /// be read is not kept, so it is never written through or over.
    files: Vec<(String, Option<Vec<u8>>)>,
    need: Need,
    /// The added package's own entries that were there before: put back
    /// removes each of [`own_entries`] that is not among them.
    owned: BTreeSet<String>,
}

impl Before {
    fn of(root: &Path, need: Need, files: &[String]) -> Self {
        let files = files
            .iter()
            .filter(|file| !file.ends_with('/'))
            .filter_map(|file| {
                let path = root.join(file);
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.is_file() => fs::read(&path).ok().map(|bytes| (file.clone(), Some(bytes))),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Some((file.clone(), None)),
                    _ => None,
                }
            })
            .collect();
        Self { files, need, owned: own_entries(root, need) }
    }

    /// Put back each kept file that changed, remove one that appeared, and
    /// remove the provider package that appeared: what was put back, relative
    /// to `root`. Never follows a symlink: a file is published through
    /// [`atomic::publish`](crate::config::atomic::publish) — an exclusively
    /// created sibling renamed over it — and a symlink in its place is
    /// replaced or removed, not written through.
    fn put_back(self, root: &Path) -> Vec<String> {
        let mut restored = Vec::new();
        for (file, bytes) in self.files {
            let path = root.join(&file);
            let now = fs::symlink_metadata(&path);
            let done = match (&bytes, &now) {
                (Some(bytes), Ok(meta)) if meta.is_file() && fs::read(&path).ok().as_ref() == Some(bytes) => continue,
                (Some(bytes), _) => crate::config::atomic::publish(&path, bytes, None),
                (None, Ok(meta)) if meta.is_dir() => continue,
                (None, Ok(_)) => fs::remove_file(&path),
                (None, Err(_)) => continue,
            };
            match done {
                Ok(()) => restored.push(file),
                Err(e) => tracing::warn!(target: "logos::coverage", file, "could not put the file back: {e}"),
            }
        }
        for entry in own_entries(root, self.need).difference(&self.owned) {
            let path = root.join(entry);
            let removed = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
                Ok(_) => fs::remove_file(&path),
                Err(e) => Err(e),
            };
            match removed {
                Ok(()) => restored.push(entry.clone()),
                Err(e) => tracing::warn!(target: "logos::coverage", entry = %entry, "could not remove the added package: {e}"),
            }
        }
        restored
    }
}

/// The installed entries, relative to `root`, that belong to the package
/// `need` adds: vitest's `node_modules/@vitest/coverage-v8`; pytest-cov's
/// `pytest_cov` package, `pytest_cov-<version>.dist-info` and
/// `pytest-cov.pth` in each `.venv/lib/python*/site-packages`. Matched by
/// exact name, never by prefix, so `pytest_cover` is not pytest-cov's. Only
/// real directories are walked: a symlinked `node_modules`, `.venv` or
/// `site-packages` is not, so nothing outside the project is ever removed.
fn own_entries(root: &Path, need: Need) -> BTreeSet<String> {
    let real_dir = |path: &Path| fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
    let present = |rel: String| fs::symlink_metadata(root.join(&rel)).is_ok().then_some(rel);
    match need {
        Need::VitestProvider => {
            if !real_dir(&root.join("node_modules")) || !real_dir(&root.join("node_modules/@vitest")) {
                return BTreeSet::new();
            }
            present(format!("node_modules/{VITEST_PROVIDER}")).into_iter().collect()
        }
        Need::PytestCov => {
            let lib = root.join(".venv/lib");
            if !real_dir(&root.join(".venv")) || !real_dir(&lib) {
                return BTreeSet::new();
            }
            let pythons = fs::read_dir(&lib).into_iter().flatten().flatten();
            pythons
                .filter(|python| python.file_name().to_string_lossy().starts_with("python") && real_dir(&python.path()))
                .map(|python| python.path().join("site-packages"))
                .filter(|site| real_dir(site))
                .flat_map(|site| fs::read_dir(&site).into_iter().flatten().flatten())
                .filter(|entry| is_pytest_cov_entry(&entry.file_name().to_string_lossy()))
                .filter_map(|entry| Some(entry.path().strip_prefix(root).ok()?.to_string_lossy().into_owned()))
                .collect()
        }
    }
}

/// Whether a site-packages entry is pytest-cov's own: its package, its
/// `pytest_cov-<version>.dist-info`, or its `pytest-cov.pth` hook.
fn is_pytest_cov_entry(name: &str) -> bool {
    name == "pytest_cov"
        || name == "pytest-cov.pth"
        || name.strip_prefix("pytest_cov-").is_some_and(|rest| rest.ends_with(".dist-info"))
}


/// Run the package manager and settle how it ended.
fn add(runner: &CoverageRunner, tool: &str, program: &Path, argv: &[OsString]) -> DependencyOutcome {
    let finished = match spawn::run(program, argv, &runner.launch(ADD_TIMEOUT, &[])) {
        Ok(finished) => finished,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => return DependencyOutcome::Cancelled,
        Err(e) => return failed(&runner.root, tool, None, &format!("could not start {}: {e}", program.display())),
    };
    if crate::jobs::cancelled() {
        return DependencyOutcome::Cancelled;
    }
    match finished.status {
        None => DependencyOutcome::TimedOut { after_secs: ADD_TIMEOUT.as_secs() },
        Some(status) if status.success() => {
            clear_failure_log(&runner.root, tool);
            DependencyOutcome::Added
        }
        Some(status) => failed(&runner.root, tool, status.code(), &failure_output(&finished)),
    }
}

/// The failure log of `tool`'s dependency addition, under [`OUTPUT_DIR`].
fn failure_log(root: &Path, tool: &str) -> PathBuf {
    root.join(OUTPUT_DIR).join(format!("{tool}.dependency{FAILURE_LOG_SUFFIX}"))
}

/// Remove the log an earlier failed addition of `tool`'s dependency left — it
/// holds the manager's whole output — once one succeeds, as a tool's is once
/// it runs. Only inside a real `.logos/coverage/`: through a committed symlink
/// the removal would reach outside the project.
fn clear_failure_log(root: &Path, tool: &str) {
    let log = failure_log(root, tool);
    if real_output(root) {
        if let Err(e) = crate::security::remove_file_if_present(&log) {
            tracing::warn!(target: "logos::coverage", log = %log.display(), "could not clear the failure log: {e}");
        }
    }
}

/// A failed addition: its whole `output` written to
/// `<tool>.dependency.failed.log` under [`OUTPUT_DIR`], the payload's excerpt
/// masked.
fn failed(root: &Path, tool: &str, exit_code: Option<i32>, output: &str) -> DependencyOutcome {
    let log = failure_log(root, tool);
    DependencyOutcome::Failed { exit_code, stderr: mask::excerpt(output), log: write_failure_log(root, &log, output) }
}

/// The per-addition event: a log line naming the tool, the manager and how it
/// ended, never what the manager printed.
fn trace_add(run: &DependencyRun, started: Instant) {
    let outcome = match run.outcome {
        DependencyOutcome::Added => "added",
        DependencyOutcome::Failed { .. } => "failed",
        DependencyOutcome::TimedOut { .. } => "timed_out",
        DependencyOutcome::Cancelled => "cancelled",
        DependencyOutcome::Pending { .. } | DependencyOutcome::Refused { .. } => "not_started",
    };
    tracing::info!(
        target: "logos::coverage",
        tool = run.tool,
        manager = run.manager.unwrap_or(""),
        duration_ms = started.elapsed().as_millis() as u64,
        outcome,
        "coverage dependency addition",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pytest_cov_is_a_need_only_for_the_project_virtualenvs_pytest() {
        let root = tempfile::TempDir::new().unwrap();
        let missing = |hint| Detection::NotInstalled { hint: Some(hint) };
        // A probed pytest on PATH reads this hint too: no project venv, no need.
        assert_eq!(Need::of(&missing(HINT_PYTEST_COV), root.path()), None);
        assert_eq!(Need::of(&missing(HINT_VITEST_PROVIDER), root.path()), Some(Need::VitestProvider));
        let pytest = root.path().join(VENV_PYTEST);
        fs::create_dir_all(pytest.parent().unwrap()).unwrap();
        fs::write(&pytest, "#!/bin/sh\n").unwrap();
        assert_eq!(Need::of(&missing(HINT_PYTEST_COV), root.path()), None, "not executable");
        let mut perms = fs::metadata(&pytest).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&pytest, perms).unwrap();
        assert_eq!(Need::of(&missing(HINT_PYTEST_COV), root.path()), Some(Need::PytestCov));
        assert_eq!(Need::of(&Detection::NotInstalled { hint: None }, root.path()), None);
    }

    #[test]
    fn each_manager_adds_a_dev_dependency_with_a_fixed_argv() {
        let spec = "@vitest/coverage-v8@3";
        for (manager, command) in [
            (Manager::Npm, "npm install --save-dev @vitest/coverage-v8@3"),
            (Manager::Pnpm, "pnpm add -D @vitest/coverage-v8@3"),
            (Manager::Yarn, "yarn add -D @vitest/coverage-v8@3"),
            (Manager::Bun, "bun add --dev @vitest/coverage-v8@3"),
        ] {
            assert_eq!(manager.command(spec), command);
        }
        for (manager, command) in [
            (Manager::Uv, "uv add --dev pytest-cov"),
            (Manager::Poetry, "poetry add --group dev pytest-cov"),
            (Manager::Pip, ".venv/bin/python -m pip install pytest-cov"),
        ] {
            assert_eq!(manager.command(PYTEST_COV), command);
        }
        assert_eq!(Manager::Pip.by_hand(PYTEST_COV), "python -m pip install pytest-cov");
        // A manager Logos does not run is named by its own command, never another's.
        for (lock, hand) in [
            ("deno.lock", "deno add --dev npm:@vitest/coverage-v8@3"),
            ("pdm.lock", "pdm add --dev @vitest/coverage-v8@3"),
            ("Pipfile.lock", "pipenv install --dev @vitest/coverage-v8@3"),
            ("pixi.lock", "pixi add --pypi @vitest/coverage-v8@3"),
        ] {
            assert!(is_unknown(lock), "{lock}");
            assert_eq!(unknown_by_hand(lock, spec), hand);
        }
        assert!(!is_unknown("pnpm-lock.yaml"));
        assert_eq!(Manager::Pnpm.args(spec), ["add", "-D", spec], "the program is not an argument");
    }

    #[test]
    fn the_lockfiles_name_one_manager_or_none() {
        let root = tempfile::TempDir::new().unwrap();
        let touch = |file: &str| fs::write(root.path().join(file), "").unwrap();
        let clear = || {
            for entry in fs::read_dir(root.path()).unwrap() {
                fs::remove_file(entry.unwrap().path()).unwrap();
            }
        };
        assert_eq!(lockfiles(root.path(), Ecosystem::Node), Found::Nothing);
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::Nothing);
        // Written out, not read from LOCKFILES: a wrong row must fail here.
        for (ecosystem, file, manager) in [
            (Ecosystem::Node, "package-lock.json", Manager::Npm),
            (Ecosystem::Node, "npm-shrinkwrap.json", Manager::Npm),
            (Ecosystem::Node, "pnpm-lock.yaml", Manager::Pnpm),
            (Ecosystem::Node, "yarn.lock", Manager::Yarn),
            (Ecosystem::Node, "bun.lock", Manager::Bun),
            (Ecosystem::Node, "bun.lockb", Manager::Bun),
            (Ecosystem::Python, "uv.lock", Manager::Uv),
            (Ecosystem::Python, "poetry.lock", Manager::Poetry),
        ] {
            touch(file);
            assert_eq!(lockfiles(root.path(), ecosystem), Found::One(manager, vec![file]), "{file}");
            clear();
        }
        for (ecosystem, file) in [
            (Ecosystem::Node, "deno.lock"),
            (Ecosystem::Python, "pdm.lock"),
            (Ecosystem::Python, "Pipfile.lock"),
            (Ecosystem::Python, "pixi.lock"),
        ] {
            touch(file);
            touch(PIP_REQUIREMENTS);
            assert_eq!(lockfiles(root.path(), ecosystem), Found::Unknown(vec![file]), "{file} names no manager Logos runs");
            clear();
        }
        // Two lockfiles of one manager are one manager.
        touch("package-lock.json");
        touch("npm-shrinkwrap.json");
        assert_eq!(
            lockfiles(root.path(), Ecosystem::Node),
            Found::One(Manager::Npm, vec!["package-lock.json", "npm-shrinkwrap.json"])
        );
        clear();
        touch("deno.lock");
        assert_eq!(lockfiles(root.path(), Ecosystem::Node), Found::Unknown(vec!["deno.lock"]));
        touch("pnpm-lock.yaml");
        assert!(matches!(lockfiles(root.path(), Ecosystem::Node), Found::Several(..)), "a known and an unknown");
        clear();
        touch("pnpm-lock.yaml");
        touch("yarn.lock");
        assert_eq!(
            lockfiles(root.path(), Ecosystem::Node),
            Found::Several(vec!["pnpm-lock.yaml", "yarn.lock"], vec![Manager::Pnpm, Manager::Yarn])
        );
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::Nothing, "a node lockfile names no python manager");
        clear();
        // A requirements.txt is pip's lock only when no other python lock is there.
        touch(PIP_REQUIREMENTS);
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::One(Manager::Pip, vec![PIP_REQUIREMENTS]));
        touch("uv.lock");
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::One(Manager::Uv, vec!["uv.lock"]));
        touch("pdm.lock");
        assert!(matches!(lockfiles(root.path(), Ecosystem::Python), Found::Several(..)));
        clear();
        touch("Pipfile.lock");
        touch(PIP_REQUIREMENTS);
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::Unknown(vec!["Pipfile.lock"]));
        clear();
        // A symlinked lockfile is not read, pip's included.
        fs::write(root.path().join("elsewhere"), "").unwrap();
        std::os::unix::fs::symlink(root.path().join("elsewhere"), root.path().join("pnpm-lock.yaml")).unwrap();
        assert_eq!(lockfiles(root.path(), Ecosystem::Node), Found::Nothing);
        std::os::unix::fs::symlink(root.path().join("elsewhere"), root.path().join(PIP_REQUIREMENTS)).unwrap();
        assert_eq!(lockfiles(root.path(), Ecosystem::Python), Found::Nothing);
    }

    #[test]
    fn the_vitest_major_is_read_from_the_installed_package_only() {
        let root = tempfile::TempDir::new().unwrap();
        assert_eq!(vitest_major(root.path()), None);
        let package = root.path().join("node_modules/vitest/package.json");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        for (json, major) in [
            (r#"{"version": "3.2.4"}"#, Some(3)),
            (r#"{"version": "2.1.9"}"#, Some(2)),
            (r#"{"version": "10.0.0-beta.1"}"#, Some(10)),
            (r#"{"version": "workspace:*"}"#, None),
            (r#"{"version": 3}"#, None),
            (r#"{"name": "vitest"}"#, None),
            ("not json", None),
        ] {
            fs::write(&package, json).unwrap();
            assert_eq!(vitest_major(root.path()), major, "{json}");
        }
    }
}
