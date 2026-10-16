//! The security orchestrator ([security-orchestrator], [ADR-80], S-665): runs
//! the security scanners the user has installed, with commands no repository
//! can choose.
//!
//! - [`Orchestrator::detect`] reports each [registered](registry::REGISTRY)
//!   scanner as installed (with its version), not installed, or disabled. The
//!   probe is the binary's version command, run with the update-check opt-outs
//!   the registry sets.
//! - [`Orchestrator::scan`] runs the selected, enabled, installed scanners one
//!   after the other, each to a SARIF file under `.logos/security/`, and
//!   reports one [`Outcome`] per scanner. One scanner failing never stops the
//!   others.
//!
//! **Command provenance** ([NFR-SE-08]). Every argv comes from a fixed
//! registry template. The repository config may only enable or disable a
//! scanner, and it is loaded — and rejected, if it says anything else — before
//! any process is spawned. The binary path and the extra flags come from the
//! [user-level config](crate::config::UserConfig) alone. A scanner is spawned
//! with `std::process::Command` and an argv vector, never through a shell.
//! `PATH` is reduced to its absolute entries outside the project root, both to
//! find the scanner and as the `PATH` the scanner itself runs with, so neither
//! the scanner nor a tool it starts by name (`git`, `go`, `java`) is resolved
//! from the repository.
//!
//! **No repository-chosen code** ([NFR-SE-02]). A scanner that would run code a
//! repository config file names is closed off: Checkov is not started while
//! the root holds a `.checkov.yaml` (it can name external Python checks), Trivy
//! reads an empty config Logos writes instead of the repository's
//! `trivy.yaml`, Dependency-Check's package-manager analyzers are off, and gosec
//! runs with `GOTOOLCHAIN=local`. Output is written only inside
//! [`OUTPUT_DIR`]: a symlinked `.logos` or `.logos/security` refuses the scan,
//! and a symlink in place of any file or directory Logos writes below it is
//! removed, never followed.
//!
//! **No build** ([NFR-SE-02]). CodeQL runs with `--build-mode=none`, and only
//! on languages whose extraction evaluates nothing from the project; every
//! other language is reported [`REQUIRES_BUILD`] and nothing is run for it.
//!
//! The orchestrator itself never ingests and never asks for consent. The
//! [`surface`] module composes it with the evidence store, the consent record
//! and the suppression file into what `logos security scan|status|findings`
//! and their MCP twins return (S-667); [`mask`] keeps a failed scanner's
//! output from carrying a secret into those payloads.
//!
//! [security-orchestrator]: ../../../docs/specs/architecture/components/security-orchestrator.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

pub use crate::history::security::mask;
pub mod registry;
mod spawn;
mod surface;

#[cfg(all(test, unix))]
mod surface_tests;
#[cfg(all(test, unix))]
mod tests;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::config::{
    load_config_from_root, load_user_config, user_config_dir, ConfigError, SecurityScan,
    UserSecurityScan,
};
use registry::{Fill, Output, Scanner, CODEQL_LANGUAGES, REGISTRY};
use spawn::Launch;

pub use registry::scanner;
pub use surface::{
    findings, scan, status, ConsentState, FindingsFilter, ListedFinding, RunOutcome, ScanState,
    ScannedReport, ScannerRun, ScannerStatus, SecurityFindings, SecurityScanSummary,
    SecurityStatus, StaleBecause, SuppressionFilter, FAILURE_LOG_SUFFIX,
};

/// The reason a language that needs a build is not scanned ([FR-SC-05]).
///
/// [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
pub const REQUIRES_BUILD: &str = "requires a build — ingest a CI report";

/// Where scan output lands, relative to the project root.
pub const OUTPUT_DIR: &str = ".logos/security";

/// The empty config file pinned on scanners that would otherwise auto-load one
/// from the repository, inside [`OUTPUT_DIR`].
const PINNED_CONFIG: &str = "empty-config.yaml";

/// How long a version probe may take before its process group is killed.
const PROBE_DEADLINE: Duration = Duration::from_secs(30);

/// One scanner's detection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannerState {
    /// The registry name.
    pub scanner: &'static str,
    #[serde(flatten)]
    pub detection: Detection,
}

/// Whether a scanner can run here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Detection {
    /// Found. `version` is `None` when the version probe failed to start, timed
    /// out, exited with a failure, or printed no version.
    Installed { binary: PathBuf, version: Option<String> },
    NotInstalled,
    /// Disabled by the repository's `[security_scan]`; nothing was probed.
    Disabled,
}

/// Whether a scanner would run here, decided without starting anything: the
/// repository's switch and a binary lookup, with no version probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Installed,
    NotInstalled,
    Disabled,
}

/// Where the security surfaces look for the user-level directory (the
/// overrides and the consent record) and for scanner binaries.
///
/// [`from_process`](Self::from_process) is what every shipped surface uses.
/// An embedder or a test passes explicit values instead, so that neither has to
/// change the process environment, and so that the scanner search path can
/// differ from the `PATH` Logos itself runs `git` with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEnvironment {
    /// The user-level config directory, if there is one.
    pub user_dir: Option<PathBuf>,
    /// The executable search path scanners are looked up on.
    pub search_path: Option<OsString>,
}

impl ScanEnvironment {
    /// [`user_config_dir`] and the process `PATH`.
    pub fn from_process() -> Self {
        Self {
            user_dir: user_config_dir(),
            search_path: std::env::var_os("PATH"),
        }
    }
}

/// Which scanners a scan covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every registered scanner.
    All,
    /// The named scanners, by registry name.
    Only(Vec<String>),
}

/// A scan's per-scanner results, in registry order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    pub outcomes: Vec<ScannerOutcome>,
}

/// One scanner's part of a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannerOutcome {
    /// The registry name.
    pub scanner: &'static str,
    /// The SARIF driver name the produced reports carry.
    pub driver: &'static str,
    /// The detected version, when the scanner is installed.
    pub version: Option<String>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// What happened to one scanner ([ADR-80] §7).
///
/// [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// It ran and wrote these SARIF files (absolute paths). For CodeQL,
    /// `requires_build` lists the languages present but skipped as
    /// [`REQUIRES_BUILD`]; it is empty for every other scanner.
    Ran {
        sarif: Vec<PathBuf>,
        requires_build: Vec<String>,
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
    /// It did not produce a report: it exited with a code its template does not
    /// accept, wrote no SARIF, could not be started, or was not started because
    /// the repository holds a config that could make it run code. `exit_code` is
    /// the scanner's own exit code, `None` when it did not exit by itself or was
    /// never started. `stderr` is the captured tail of its stderr (the head of
    /// its stdout when stderr was empty), or Logos's own explanation when no
    /// scanner output applies.
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

/// The scanner runner for one project root, its repository switches and the
/// user's overrides resolved.
#[derive(Debug, Clone)]
pub struct Orchestrator {
    root: PathBuf,
    switches: SecurityScan,
    overrides: UserSecurityScan,
    /// The usable `PATH` entries: absolute, and outside the project root.
    search_dirs: Vec<PathBuf>,
    /// [`search_dirs`](Self::search_dirs) joined: the `PATH` every scanner
    /// process runs with (`None` when no entry is usable).
    child_path: Option<OsString>,
}

impl Orchestrator {
    /// Load the repository's `.logos/config.toml`, the user-level config from
    /// [`user_config_dir`], and the process `PATH`.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2) when either config file is invalid — notably
    /// a repository `[security_scan]` carrying anything but `enabled`. Nothing
    /// is spawned before this returns.
    pub fn open(root: &Path) -> Result<Self, ConfigError> {
        Self::open_in(root, &ScanEnvironment::from_process())
    }

    /// [`open`](Self::open) in an explicit [`ScanEnvironment`].
    ///
    /// # Errors
    /// As [`open`](Self::open).
    pub fn open_in(root: &Path, env: &ScanEnvironment) -> Result<Self, ConfigError> {
        Self::open_with(root, env.user_dir.as_deref(), env.search_path.clone())
    }

    /// [`open`](Self::open) with the user-level config directory and the
    /// executable search path given explicitly.
    ///
    /// # Errors
    /// As [`open`](Self::open).
    pub fn open_with(
        root: &Path,
        user_dir: Option<&Path>,
        search_path: Option<OsString>,
    ) -> Result<Self, ConfigError> {
        let switches = load_config_from_root(root)?.security_scan;
        let overrides = load_user_config(user_dir)?.security_scan;
        let search_dirs = usable_path_entries(root, search_path.as_deref());
        let child_path = (!search_dirs.is_empty())
            .then(|| std::env::join_paths(&search_dirs).ok())
            .flatten();
        Ok(Self {
            root: root.to_path_buf(),
            switches,
            overrides,
            search_dirs,
            child_path,
        })
    }

    /// Detect every registered scanner, in registry order. A disabled scanner
    /// is not probed.
    pub fn detect(&self) -> Vec<ScannerState> {
        REGISTRY
            .iter()
            .map(|s| ScannerState {
                scanner: s.name,
                detection: self.detect_one(s),
            })
            .collect()
    }

    /// Every registered scanner's [`Availability`], in registry order. Starts
    /// no process.
    pub fn availability(&self) -> Vec<(&'static Scanner, Availability)> {
        REGISTRY
            .iter()
            .map(|s| {
                let availability = if !self.switches.is_enabled(s.name) {
                    Availability::Disabled
                } else if self.locate(s).is_some() {
                    Availability::Installed
                } else {
                    Availability::NotInstalled
                };
                (s, availability)
            })
            .collect()
    }

    /// Run the selected scanners, blocking until each has finished.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] for a selected name that is not
    /// registered (before anything is spawned), a symlinked `.logos` or
    /// [`OUTPUT_DIR`], or an I/O error creating [`OUTPUT_DIR`]. A scanner's own
    /// failure is its [`Outcome::Failed`], never an error.
    pub fn scan(&self, selection: &Selection) -> anyhow::Result<ScanReport> {
        let selected: Vec<&'static Scanner> = match selection {
            Selection::All => REGISTRY.iter().collect(),
            Selection::Only(names) => {
                if let Some(bad) = names.iter().find(|n| scanner(n).is_none()) {
                    return Err(ConfigError::InvalidValue {
                        key: "scanner".to_string(),
                        message: format!(
                            "{bad:?} is not a registered scanner (expected one of: {})",
                            crate::config::SECURITY_SCANNERS.join(", ")
                        ),
                    }
                    .into());
                }
                REGISTRY
                    .iter()
                    .filter(|s| names.iter().any(|n| n == s.name))
                    .collect()
            }
        };
        // A symlink committed in place of either directory would send every
        // report, and every file Logos writes for a scanner, outside the project.
        let out_dir = self.root.join(OUTPUT_DIR);
        for dir in [self.root.join(".logos"), out_dir.clone()] {
            if is_symlink(&dir) {
                anyhow::bail!(
                    "{} is a symbolic link; refusing to write scan output through it",
                    dir.display()
                );
            }
        }
        fs::create_dir_all(&out_dir)?;
        let outcomes = selected
            .into_iter()
            .map(|s| {
                let (version, outcome) = match self.detect_one(s) {
                    Detection::Disabled => (None, Outcome::Disabled),
                    Detection::NotInstalled => (None, Outcome::NotInstalled),
                    Detection::Installed { binary, version } => {
                        let outcome = if let Some(file) = self.refusal(s) {
                            Outcome::Failed {
                                exit_code: None,
                                stderr: format!(
                                    "not run: the repository's {file} can make {} run code it \
                                     names; remove it, or run {} in CI and ingest its SARIF",
                                    s.name, s.name
                                ),
                            }
                        } else if s.per_language() {
                            self.run_codeql(s, &binary, &out_dir)
                        } else {
                            self.run_single(s, &binary, &out_dir)
                        };
                        (version, outcome)
                    }
                };
                ScannerOutcome {
                    scanner: s.name,
                    driver: s.driver,
                    version,
                    outcome,
                }
            })
            .collect();
        Ok(ScanReport { outcomes })
    }

    fn launch<'a>(&'a self, s: &'a Scanner, deadline: Option<Duration>) -> Launch<'a> {
        Launch {
            cwd: &self.root,
            env: s.env,
            path: self.child_path.as_ref(),
            deadline,
        }
    }

    fn detect_one(&self, s: &Scanner) -> Detection {
        if !self.switches.is_enabled(s.name) {
            return Detection::Disabled;
        }
        let Some(binary) = self.locate(s) else {
            return Detection::NotInstalled;
        };
        let args: Vec<OsString> = s.version_args.iter().map(OsString::from).collect();
        let version = spawn::run(&binary, &args, &self.launch(s, Some(PROBE_DEADLINE)))
            .ok()
            .filter(|f| f.status.is_some_and(|st| st.success()))
            .and_then(|f| {
                registry::parse_version(&f.stdout).or_else(|| registry::parse_version(&f.stderr))
            });
        Detection::Installed { binary, version }
    }

    /// The first of the scanner's [refusal files](Scanner::refuse_if_present)
    /// present at the root, if any.
    fn refusal(&self, s: &Scanner) -> Option<&'static str> {
        s.refuse_if_present
            .iter()
            .copied()
            .find(|f| fs::symlink_metadata(self.root.join(f)).is_ok())
    }

    /// The binary to run: the user-level `path` when set, else the first
    /// executable match on the [usable](usable_path_entries) `PATH` entries.
    fn locate(&self, s: &Scanner) -> Option<PathBuf> {
        if let Some(path) = self.overrides.get(s.name).and_then(|o| o.path.as_ref()) {
            return is_executable(path).then(|| path.clone());
        }
        self.search_dirs
            .iter()
            .flat_map(|dir| s.binaries.iter().map(move |b| dir.join(b)))
            .find(|candidate| is_executable(candidate))
    }

    fn extra_args(&self, s: &Scanner) -> &[String] {
        self.overrides
            .get(s.name)
            .map_or(&[], |o| o.extra_args.as_slice())
    }

    fn run_single(&self, s: &Scanner, binary: &Path, out_dir: &Path) -> Outcome {
        let target = out_dir.join(format!("{}.sarif", s.name));
        let (out_arg, produced) = match s.output {
            Output::File => (target.clone(), target.clone()),
            Output::Dir { produced } => {
                let dir = out_dir.join(s.name);
                (dir.clone(), dir.join(produced))
            }
        };
        // The directory is made real first, so that clearing the previous
        // report below never deletes through a symlink in its place.
        if let Output::Dir { .. } = s.output {
            if let Err(e) = real_dir(&out_arg) {
                return failed_io("could not create the output directory", &out_arg, &e);
            }
        }
        // A stale report from an earlier run must never be credited to this one.
        for stale in [&target, &produced] {
            if let Err(e) = remove_file_if_present(stale) {
                return failed_io("could not clear the previous report", stale, &e);
            }
        }
        let pinned_config = out_dir.join(PINNED_CONFIG);
        if s.run.contains(&registry::Arg::PinnedConfig) {
            if let Err(e) = write_empty(&pinned_config) {
                return failed_io("could not write the pinned config", &pinned_config, &e);
            }
        }
        let fill = Fill {
            out: &out_arg,
            extra: self.extra_args(s),
            scope: None,
            language: "",
            database: Path::new(""),
            pinned_config: &pinned_config,
        };
        let code = match self.step(s, binary, s.run, &fill) {
            Ok(code) => code,
            Err(outcome) => return outcome,
        };
        if produced != target {
            if !produced.is_file() {
                return produced_or_failed(s, code, vec![produced], Vec::new());
            }
            if let Err(e) = fs::rename(&produced, &target) {
                return failed_io("could not move the report to", &target, &e);
            }
        }
        produced_or_failed(s, code, vec![target], Vec::new())
    }

    fn run_codeql(&self, s: &Scanner, binary: &Path, out_dir: &Path) -> Outcome {
        let (runnable, needs_build) = codeql_languages(&self.root);
        if runnable.is_empty() {
            return if needs_build.is_empty() {
                Outcome::NotApplicable {
                    reason: "no source file in a language CodeQL analyses".to_string(),
                }
            } else {
                Outcome::RequiresBuild {
                    languages: needs_build,
                    reason: REQUIRES_BUILD,
                }
            };
        }
        let databases = out_dir.join("codeql");
        if let Err(e) = real_dir(&databases) {
            return failed_io("could not create the database directory", &databases, &e);
        }
        // One report for every language: a snapshot is keyed by HEAD and
        // scanner, so per-language reports ingested one by one would each
        // replace the last. The per-language reports sit in `codeql/`, beside
        // their databases, where the watcher does not admit them.
        let merged = out_dir.join("codeql.sarif");
        if let Err(e) = remove_file_if_present(&merged) {
            return failed_io("could not clear the previous report", &merged, &e);
        }
        let mut sarif = Vec::new();
        let mut code = 0;
        for (label, id) in runnable {
            let target = databases.join(format!("{label}.sarif"));
            if let Err(e) = remove_file_if_present(&target) {
                return failed_io("could not clear the previous report", &target, &e);
            }
            // CodeQL creates the database itself; only a symlink in its place
            // is removed first.
            let database = databases.join(label);
            if is_symlink(&database) {
                if let Err(e) = fs::remove_file(&database) {
                    return failed_io("could not remove the symbolic link", &database, &e);
                }
            }
            let fill = Fill {
                out: &target,
                extra: self.extra_args(s),
                scope: None,
                language: id,
                database: &database,
                pinned_config: Path::new(""),
            };
            // The creation template has no extra-flags position: extras go to
            // the analysis only.
            for step in [s.prepare, s.run] {
                code = match self.step(s, binary, step, &fill) {
                    Ok(code) => code,
                    Err(outcome) => return outcome,
                };
            }
            sarif.push(target);
        }
        if sarif.iter().any(|p| !p.is_file()) {
            return produced_or_failed(s, code, sarif, needs_build);
        }
        if let Err(e) = merge_sarif_runs(&sarif, &merged) {
            return failed_io("could not merge the language reports into", &merged, &e);
        }
        produced_or_failed(s, code, vec![merged], needs_build)
    }

    /// Spawn one template step in the root: `Ok` with the accepted exit code,
    /// or `Err` carrying the failed outcome.
    fn step(
        &self,
        s: &Scanner,
        binary: &Path,
        template: &[registry::Arg],
        fill: &Fill<'_>,
    ) -> Result<i32, Outcome> {
        let argv = s.render(template, fill);
        let finished = spawn::run(binary, &argv, &self.launch(s, None)).map_err(|e| {
            Outcome::Failed {
                exit_code: None,
                stderr: format!("could not start {}: {e}", binary.display()),
            }
        })?;
        let code = finished.status.and_then(|st| st.code());
        if let Some(code) = code.filter(|c| s.ok_exit.contains(c)) {
            return Ok(code);
        }
        let stderr = if finished.stderr.trim().is_empty() {
            finished.stdout
        } else {
            finished.stderr
        };
        Err(Outcome::Failed {
            exit_code: code,
            stderr: stderr.trim().to_string(),
        })
    }
}

/// `Ran` when every reported file exists, else `Failed` naming the first one
/// missing, with the exit code the scanner returned.
fn produced_or_failed(
    s: &Scanner,
    code: i32,
    sarif: Vec<PathBuf>,
    requires_build: Vec<String>,
) -> Outcome {
    match sarif.iter().find(|p| !p.is_file()) {
        Some(missing) => Outcome::Failed {
            exit_code: Some(code),
            stderr: format!(
                "{} exited with {code}, which its template accepts, but wrote no SARIF at {}",
                s.name,
                missing.display()
            ),
        },
        None => Outcome::Ran {
            sarif,
            requires_build,
        },
    }
}

/// Write `target` as one SARIF log holding every run of `reports`, in order —
/// the ingest stores a log's runs of one tool as one snapshot. A report that
/// is not JSON or has no `runs` array is an error; its content is never quoted.
fn merge_sarif_runs(reports: &[PathBuf], target: &Path) -> std::io::Result<()> {
    let invalid = |report: &Path, why: &str| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: {why}", report.display()))
    };
    let mut runs = Vec::new();
    for report in reports {
        let log: serde_json::Value =
            serde_json::from_slice(&fs::read(report)?).map_err(|_| invalid(report, "not JSON"))?;
        let Some(serde_json::Value::Array(more)) = log.get("runs") else {
            return Err(invalid(report, "has no `runs` array"));
        };
        runs.extend(more.iter().cloned());
    }
    let merged = serde_json::json!({ "version": "2.1.0", "runs": runs });
    write_new(target, &serde_json::to_vec(&merged)?)
}

fn failed_io(what: &str, path: &Path, e: &std::io::Error) -> Outcome {
    Outcome::Failed {
        exit_code: None,
        stderr: format!("{what} {}: {e}", path.display()),
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Make `path` a real directory: a symlink in its place is removed, never
/// followed, then the directory is created if absent.
fn real_dir(path: &Path) -> std::io::Result<()> {
    if is_symlink(path) {
        fs::remove_file(path)?;
    }
    fs::create_dir_all(path)
}

/// Write `path` as a new empty file. Whatever was there — a file, or a
/// symlink a repository committed — is removed first, never written through.
fn write_empty(path: &Path) -> std::io::Result<()> {
    write_new(path, b"")
}

/// Write `path` as a new file holding `bytes`, removing whatever was there
/// first, so a symlink in its place is replaced rather than written through.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    remove_file_if_present(path)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The `PATH` entries a scanner may come from, and the only ones its process
/// sees: absolute, and not inside the project root (compared both as written
/// and canonicalised). An empty or relative entry would resolve against the
/// working directory, which is the repository.
fn usable_path_entries(root: &Path, search_path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let Some(search_path) = search_path else {
        return Vec::new();
    };
    let canonical_root = root.canonicalize().ok();
    let inside_root = |dir: &Path| {
        dir.starts_with(root)
            || canonical_root.as_ref().is_some_and(|r| {
                dir.canonicalize().is_ok_and(|d| d.starts_with(r))
            })
    };
    std::env::split_paths(search_path)
        .filter(|dir| dir.is_absolute() && !inside_root(dir))
        .collect()
}

/// The CodeQL languages present under `root`: the `(label, --language id)`
/// pairs Logos runs, and the labels of those that need a build. Each list is
/// in [`CODEQL_LANGUAGES`] order.
///
/// The walk honours the root's own `.gitignore` and `.ignore` files at every
/// depth, but not ignore files above the root or the user's global excludes,
/// so the answer depends on the project alone. Hidden directories are walked;
/// `.git` and `.logos` are not.
fn codeql_languages(root: &Path) -> (Vec<(&'static str, &'static str)>, Vec<String>) {
    let mut present = [false; CODEQL_LANGUAGES.len()];
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".logos" && e.file_name() != ".git")
        .build();
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) else {
            continue;
        };
        for (i, lang) in CODEQL_LANGUAGES.iter().enumerate() {
            if lang.extensions.contains(&ext) {
                present[i] = true;
            }
        }
    }
    let mut runnable = Vec::new();
    let mut needs_build = Vec::new();
    for (lang, _) in CODEQL_LANGUAGES.iter().zip(present).filter(|(_, p)| *p) {
        match lang.codeql_id {
            Some(id) => runnable.push((lang.label, id)),
            None => needs_build.push(lang.label.to_string()),
        }
    }
    (runnable, needs_build)
}

/// A regular file the current user may execute.
fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
