//! The security orchestrator ([security-orchestrator], [ADR-80], S-665): runs
//! the security scanners the user has installed, or the [tool
//! installer](crate::tools) installed for them, with commands no repository
//! can choose.
//!
//! - [`Orchestrator::detect`] reports each [registered](registry::REGISTRY)
//!   scanner as installed (with its version and which [copy](CopyKind) it
//!   is), not installed, or disabled. The probe is the binary's version
//!   command, run with the update-check opt-outs the registry sets.
//! - [`Orchestrator::scan`] runs the selected, enabled, installed scanners one
//!   after the other, each to a SARIF file under `.logos/security/`, and
//!   reports one [`Outcome`] per scanner. One scanner failing never stops the
//!   others.
//! - A scoped run ([FR-SC-17], S-685) puts paths the user named — each
//!   confined to the root by [`resolve_scope`] before anything starts — in a
//!   path-capable scanner's fixed [path position](registry::PathSlot), and
//!   writes its report to [`SCOPED_DIR`], where the watcher never ingests it
//!   as a whole-project report. A scanner without a path position runs
//!   unscoped.
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
//! from the repository. A managed copy is looked for only in the user-level
//! tools directory, where [`crate::tools::resolve`] names it from the manifest.
//! A `PATH` copy below the manifest's minimum version does not win over a
//! managed copy (S-744).
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
//! output from carrying a secret into those payloads; [`context`] places each
//! listed finding in the graph (S-669); [`page`] composes all of it, with the
//! `[security]` gate's verdict, into what the web UI's Security page reads
//! (S-670). Every scanner process is started through [`crate::jobs::ChildGuard`],
//! so a scan the web UI runs as a [job](crate::jobs) is cancelled by killing
//! each scanner's process group (S-673).
//!
//! [security-orchestrator]: ../../../docs/specs/architecture/components/security-orchestrator.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
//! [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

pub use crate::history::security::mask;
pub mod context;
mod gate;
mod page;
mod probes;
pub mod registry;
mod rollup;
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
    load_config_from_root, load_user_config, user_config_dir, user_tools_dir, ConfigError,
    SecurityScan, UserSecurityScan,
};
use crate::tools::Manifest;
use registry::{Fill, Output, Scanner, ScopedPath, CODEQL_LANGUAGES, REGISTRY};
use crate::jobs::spawn::{self, inside_root, is_executable, usable_path_entries, Launch};

pub use gate::SecurityGateBand;
pub(crate) use gate::SecurityGateEvidence;
pub(crate) use page::{editions, listings};
pub use page::{EditionClassification, SecurityPage};
pub use registry::{scanner, WHOLE_MANIFEST_ONLY, WHOLE_PROJECT_ONLY};
pub use rollup::{
    Excluded, MemberCategory, MemberEdition, MemberScan, MemberSecurity, SeverityCounts,
};
pub use surface::{
    consent_pending, findings, scan, status, ConsentState, FindingsFilter, LicenceAcceptance, ListedFinding,
    RunOutcome, RunScope, ScanProgress, ScanState, ScannedReport, ScannerRun,
    ScannerStatus, SecurityFindings, SecurityScanSummary, SecurityStatus, StaleBecause,
    SuppressionFilter, VersionProbes, FAILURE_LOG_SUFFIX,
};

/// The reason a language that needs a build is not scanned ([FR-SC-05]).
///
/// [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
pub const REQUIRES_BUILD: &str = "requires a build — ingest a CI report";

/// Where scan output lands, relative to the project root.
pub const OUTPUT_DIR: &str = ".logos/security";

/// Where a scoped run's reports land, inside [`OUTPUT_DIR`]: a subdirectory,
/// so the watcher — which ingests only a report directly in [`OUTPUT_DIR`] —
/// never ingests a partial report as a whole-project one.
pub const SCOPED_DIR: &str = "scoped";

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
    /// out, exited with a failure, or printed no version. `copy` says which
    /// copy `binary` is.
    Installed {
        binary: PathBuf,
        version: Option<String>,
        copy: CopyKind,
    },
    NotInstalled,
    /// Disabled by the repository's `[security_scan]`; nothing was probed.
    Disabled,
}

/// Which copy of a scanner is used ([ADR-88] §5): the first found of the
/// user-level path override, a copy on the usable `PATH`, and the copy the
/// [tool installer](crate::tools) manages.
///
/// [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyKind {
    Override,
    Path,
    Managed,
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
/// overrides and the consent record), for scanner binaries, and for the
/// managed copies the [tool installer](crate::tools) installed.
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
    /// The user-level tools directory, if there is one. One inside the
    /// project root is ignored.
    pub tools_dir: Option<PathBuf>,
}

impl ScanEnvironment {
    /// [`user_config_dir`], the process `PATH` and [`user_tools_dir`].
    pub fn from_process() -> Self {
        Self {
            user_dir: user_config_dir(),
            search_path: std::env::var_os("PATH"),
            tools_dir: user_tools_dir(),
        }
    }
}

/// Which scanners a scan, an install or an uninstall covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every registered scanner.
    All,
    /// The named scanners, by registry name.
    Only(Vec<String>),
}

impl Selection {
    /// Every scanner when `all`, else the `named` ones — a surface's
    /// `--all` flag and its name list.
    pub fn named_or_all(named: Vec<String>, all: bool) -> Self {
        if all {
            Selection::All
        } else {
            Selection::Only(named)
        }
    }

    /// `Ok` when every name is a registered scanner.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] (exit 2) naming the first that is not.
    pub(crate) fn check_registered(&self) -> Result<(), ConfigError> {
        let Selection::Only(names) = self else {
            return Ok(());
        };
        match names.iter().find(|n| scanner(n).is_none()) {
            None => Ok(()),
            Some(bad) => Err(ConfigError::InvalidValue {
                key: "scanner".to_string(),
                message: format!(
                    "{bad:?} is not a registered scanner (expected one of: {})",
                    crate::config::SECURITY_SCANNERS.join(", ")
                ),
            }),
        }
    }
}

/// Which scanners a scan runs, and on which paths ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    /// The scanners to run.
    pub scanners: Selection,
    /// The paths to scan, as the user gave them: relative to the project root,
    /// or absolute inside it. Empty scans the whole project.
    pub paths: Vec<String>,
}

impl ScanRequest {
    /// Every installed, enabled scanner over the whole project.
    pub fn full() -> Self {
        Self { scanners: Selection::All, paths: Vec::new() }
    }

    /// The `scanners` named — every one when none is — over `paths`, the
    /// whole project when none is: a surface's `--scanner` and `--path` lists.
    /// A scanner is named by registry name (`semgrep`) or by the SARIF driver
    /// name its findings carry (`Semgrep OSS`), as the findings filter takes
    /// it: a branch review names scanners by driver.
    pub fn new(scanners: Vec<String>, paths: Vec<String>) -> Self {
        let scanners: Vec<String> = scanners.into_iter().map(registry_name).collect();
        let scanners = if scanners.is_empty() { Selection::All } else { Selection::Only(scanners) };
        Self { scanners, paths }
    }

    /// Whether the request names `s`, rather than taking every scanner.
    pub(crate) fn names(&self, s: &Scanner) -> bool {
        matches!(&self.scanners, Selection::Only(names) if names.iter().any(|n| n == s.name))
    }
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
    /// Where managed copies are looked for, if anywhere.
    tools_dir: Option<PathBuf>,
    /// The pinned and minimum versions: the [builtin](Manifest::builtin) one
    /// unless an embedder or a test passes another
    /// ([`with_manifest`](Self::with_manifest)).
    manifest: Manifest,
}

/// The copy of a scanner that is used, as the precedence chose it
/// ([ADR-88] §5).
///
/// [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledCopy {
    pub copy: CopyKind,
    pub binary: PathBuf,
    /// The version its probe printed; `None` when the probe failed, or when
    /// none ran ([`unprobed`](Self::unprobed) says why).
    pub version: Option<String>,
    /// For a managed copy older than the manifest's pin, the pinned version
    /// `logos security install --update` would install.
    pub update_available: Option<String>,
    /// Why no probe ran, when none did. Absent wherever every copy is probed:
    /// the CLI, MCP, and every scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unprobed: Option<Unprobed>,
}

/// Why a copy's version was not probed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unprobed {
    /// The project's scan consent does not name the scanner, and a page read
    /// starts no process for a scanner it does not ([ADR-38], [ADR-80] §4).
    ///
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    /// [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
    NotConsented,
    /// The readout asks for each scanner's state alone, which no version
    /// changes: `check`'s `[security]` evidence.
    StateOnly,
}

/// How [`Orchestrator::copy_in_use_with`] reads the version of a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// Run its version command.
    Run,
    /// Run it once per process for each binary, unchanged ([`probes`]).
    Cached,
    /// Run nothing: the version is unknown for this reason.
    Skip(Unprobed),
}

impl Orchestrator {
    /// Load the repository's `.logos/config.toml`, the user-level config from
    /// [`user_config_dir`], the process `PATH`, and the tools directory from
    /// [`user_tools_dir`].
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
        let mut orchestrator =
            Self::open_with(root, env.user_dir.as_deref(), env.search_path.clone())?;
        // A tools directory inside the project would let the repository plant
        // a "managed" scanner, exactly as an in-project `PATH` entry would.
        let canonical_root = root.canonicalize().ok();
        orchestrator.tools_dir = env
            .tools_dir
            .clone()
            .filter(|dir| !inside_root(root, canonical_root.as_deref(), dir));
        Ok(orchestrator)
    }

    /// [`open`](Self::open) with the user-level config directory and the
    /// executable search path given explicitly, and no managed copies.
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
            tools_dir: None,
            manifest: Manifest::builtin().clone(),
        })
    }

    /// Resolve managed copies and minimum versions against `manifest` instead
    /// of the builtin one (tests: a manifest pinning a newer version).
    #[must_use]
    pub fn with_manifest(mut self, manifest: Manifest) -> Self {
        self.manifest = manifest;
        self
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
                } else if self.has_copy(s) {
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
        self.scan_in(selection, None)
    }

    /// [`scan`](Self::scan), scoped to `scope` when it is given: a scanner
    /// with a [path position](registry::PathSlot) runs on those paths only,
    /// one without runs unscoped ([FR-SC-17]).
    ///
    /// # Errors
    /// As [`scan`](Self::scan).
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    pub(crate) fn scan_in(&self, selection: &Selection, scope: Option<&[ScopedPath]>) -> anyhow::Result<ScanReport> {
        selection.check_registered()?;
        let selected: Vec<&'static Scanner> = match selection {
            Selection::All => REGISTRY.iter().collect(),
            Selection::Only(names) => {
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
        // Walked once per call, and only when some scanner will run over the
        // whole project and take them: a run on named paths reads exactly those.
        let ignored = if selected.iter().any(|s| s.excludes_ignored() && (scope.is_none() || s.path_slot.is_none())) {
            ignored_dirs(&self.root)
        } else {
            Vec::new()
        };
        let outcomes = selected
            .into_iter()
            .map(|s| {
                let (version, outcome) = match self.detect_one(s) {
                    Detection::Disabled => (None, Outcome::Disabled),
                    Detection::NotInstalled => (None, Outcome::NotInstalled),
                    Detection::Installed { binary, version, .. } => {
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
                            self.run_single(s, &binary, &out_dir, scope, &ignored)
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
            vars: &[],
            isolated: None,
            path: self.child_path.as_ref(),
            deadline,
        }
    }

    fn detect_one(&self, s: &Scanner) -> Detection {
        if !self.switches.is_enabled(s.name) {
            return Detection::Disabled;
        }
        match self.locate(s, Probe::Run) {
            Some(found) => Detection::Installed {
                binary: found.binary,
                version: found.version,
                copy: found.copy,
            },
            None => Detection::NotInstalled,
        }
    }

    /// The copy of the registered scanner `name` that would run, whatever the
    /// repository's switch says (the install surfaces ask per user, not per
    /// project); `None` when there is none. Probes versions, as
    /// [`detect`](Self::detect) does.
    pub fn copy_in_use(&self, name: &str) -> Option<InstalledCopy> {
        self.copy_in_use_with(name, Probe::Run)
    }

    /// [`copy_in_use`](Self::copy_in_use), reading versions as `probe` says.
    /// A copy whose version is not probed is chosen as one whose probe failed:
    /// a `PATH` copy is not shown to meet the minimum.
    pub(crate) fn copy_in_use_with(&self, name: &str, probe: Probe) -> Option<InstalledCopy> {
        scanner(name).and_then(|s| self.locate(s, probe))
    }

    /// The user-level `path` override for `name`, when one is set — whether
    /// or not it can run. While it is set no other copy is ever used, so
    /// nothing should be installed for it.
    pub fn override_of(&self, name: &str) -> Option<&Path> {
        self.overrides.get(name).and_then(|o| o.path.as_deref())
    }

    /// The version `binary`'s version command prints, if it runs, succeeds
    /// and prints one.
    fn probe(&self, s: &Scanner, binary: &Path) -> Option<String> {
        let args: Vec<OsString> = s.version_args.iter().map(OsString::from).collect();
        let vars = self.runtime_vars(s, binary);
        let launch = Launch {
            vars: &vars,
            ..self.launch(s, Some(PROBE_DEADLINE))
        };
        spawn::run(binary, &args, &launch)
            .ok()
            .filter(|f| f.status.is_some_and(|st| st.success()))
            .and_then(|f| {
                registry::parse_version(&f.stdout).or_else(|| registry::parse_version(&f.stderr))
            })
    }

    /// `binary`'s version as `probe` reads it.
    fn version(&self, s: &Scanner, binary: &Path, probe: Probe) -> Option<String> {
        match probe {
            Probe::Run => self.probe(s, binary),
            Probe::Cached => probes::cached(s.name, binary, || self.probe(s, binary)),
            Probe::Skip(_) => None,
        }
    }

    /// The first of the scanner's [refusal files](Scanner::refuse_if_present)
    /// present at the root, if any.
    fn refusal(&self, s: &Scanner) -> Option<&'static str> {
        s.refuse_if_present
            .iter()
            .copied()
            .find(|f| fs::symlink_metadata(self.root.join(f)).is_ok())
    }

    /// Whether any copy of `s` is there, found without starting anything: the
    /// override when set (and only it), else a `PATH` copy or a managed copy.
    /// What a scan's consent check sees before any process may start.
    fn has_copy(&self, s: &Scanner) -> bool {
        match self.overrides.get(s.name).and_then(|o| o.path.as_ref()) {
            Some(path) => is_executable(path),
            None => self.on_path(s).is_some() || self.managed(s).is_some(),
        }
    }

    /// The copy to run, in precedence order ([ADR-88] §5, [FR-SC-05]):
    /// 1. the user-level `path`, when set — and nothing else then;
    /// 2. the first executable match on the [usable](usable_path_entries)
    ///    `PATH` entries, when its probed version is at or above the
    ///    manifest's minimum, or when there is no managed copy to prefer;
    /// 3. the managed copy ([`crate::tools::resolve`]).
    ///
    /// A `PATH` copy whose version cannot be read — or that `probe` does not
    /// probe — is not shown to meet the minimum, so a managed copy wins over it. A scanner the manifest does
    /// not list has no minimum.
    ///
    /// [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
    /// [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
    fn locate(&self, s: &Scanner, probe: Probe) -> Option<InstalledCopy> {
        let found = |copy, binary: PathBuf, version| InstalledCopy {
            copy,
            binary,
            version,
            update_available: None,
            unprobed: match probe {
                Probe::Skip(why) => Some(why),
                Probe::Run | Probe::Cached => None,
            },
        };
        if let Some(path) = self.overrides.get(s.name).and_then(|o| o.path.as_ref()) {
            return is_executable(path)
                .then(|| found(CopyKind::Override, path.clone(), self.version(s, path, probe)));
        }
        let managed = self.managed(s);
        if let Some(binary) = self.on_path(s) {
            let version = self.version(s, &binary, probe);
            if self.manifest.meets_minimum(s.name, version.as_deref()) || managed.is_none() {
                return Some(found(CopyKind::Path, binary, version));
            }
        }
        let managed = managed?;
        let pinned = self.manifest.tool(s.name).map(|e| e.version.as_str());
        let version = self.version(s, &managed.binary, probe);
        Some(InstalledCopy {
            update_available: pinned
                .filter(|p| !crate::tools::version_at_least(&managed.version, p))
                .map(str::to_string),
            ..found(CopyKind::Managed, managed.binary, version)
        })
    }

    /// The first executable match for `s` on the usable `PATH` entries.
    fn on_path(&self, s: &Scanner) -> Option<PathBuf> {
        self.search_dirs
            .iter()
            .flat_map(|dir| s.binaries.iter().map(move |b| dir.join(b)))
            .find(|candidate| is_executable(candidate))
    }

    /// What `binary` runs with besides the scanner's own variables: for the
    /// managed copy of a Java tool, the managed Java runtime
    /// ([`crate::tools::java_env`]); nothing for any other copy, which runs on
    /// whatever Java the user set it up with.
    fn runtime_vars(&self, s: &Scanner, binary: &Path) -> Vec<(&'static str, OsString)> {
        let needs_java = self.manifest.tool(s.name).is_some_and(|t| t.needs_java());
        let home = self
            .tools_dir
            .as_deref()
            .filter(|_| needs_java)
            .filter(|_| self.managed(s).is_some_and(|m| m.binary == binary))
            .and_then(|dir| crate::tools::managed_java_home(dir, &self.manifest));
        home.map(|home| crate::tools::java_env(&home).to_vec())
            .unwrap_or_default()
    }

    /// The managed copy of `s` in the tools directory, if there is one.
    fn managed(&self, s: &Scanner) -> Option<crate::tools::ManagedTool> {
        let tools_dir = self.tools_dir.as_deref()?;
        crate::tools::resolve(tools_dir, &self.manifest, s.name)
    }

    fn extra_args(&self, s: &Scanner) -> &[String] {
        self.overrides
            .get(s.name)
            .map_or(&[], |o| o.extra_args.as_slice())
    }

    /// Run a scanner that writes one report. Scoped, a path-capable scanner
    /// makes the runs its [slot](registry::PathSlot::runs) needs, in
    /// [`SCOPED_DIR`], and several runs' reports are merged into one. A
    /// whole-project run keeps out of the `ignored` directories.
    fn run_single(
        &self,
        s: &Scanner,
        binary: &Path,
        out_dir: &Path,
        scope: Option<&[ScopedPath]>,
        ignored: &[String],
    ) -> Outcome {
        let (dir, runs) = match (scope, s.path_slot) {
            (Some(scope), Some(slot)) => (out_dir.join(SCOPED_DIR), slot.runs(scope).into_iter().map(Some).collect()),
            _ => (out_dir.to_path_buf(), vec![None]),
        };
        if dir != out_dir {
            if let Err(e) = real_dir(&dir) {
                return failed_io("could not create the output directory", &dir, &e);
            }
        }
        let target = dir.join(format!("{}.sarif", s.name));
        // A stale report from an earlier run must never be credited to this one.
        if let Err(e) = remove_file_if_present(&target) {
            return failed_io("could not clear the previous report", &target, &e);
        }
        // What the scanner's own `getcwd` reports, which a runner that makes
        // `.` absolute walks from.
        let canonical_root = self.root.canonicalize().ok();
        let pinned_config = out_dir.join(PINNED_CONFIG);
        if s.run.contains(&registry::Arg::PinnedConfig) {
            if let Err(e) = write_empty(&pinned_config) {
                return failed_io("could not write the pinned config", &pinned_config, &e);
            }
        }
        let mut reports = Vec::with_capacity(runs.len());
        let mut code = 0;
        for (i, run) in runs.iter().enumerate() {
            let stem = if runs.len() == 1 { s.name.to_string() } else { format!("{}.{}", s.name, i + 1) };
            let report = dir.join(format!("{stem}.sarif"));
            let (out_arg, produced) = match s.output {
                Output::File => (report.clone(), report.clone()),
                Output::Dir { produced } => {
                    let out = dir.join(&stem);
                    (out.clone(), out.join(produced))
                }
            };
            // The directory is made real first, so that clearing the previous
            // report below never deletes through a symlink in its place.
            if let Output::Dir { .. } = s.output {
                if let Err(e) = real_dir(&out_arg) {
                    return failed_io("could not create the output directory", &out_arg, &e);
                }
            }
            for stale in [&report, &produced] {
                if let Err(e) = remove_file_if_present(stale) {
                    return failed_io("could not clear the previous report", stale, &e);
                }
            }
            let fill = Fill {
                out: &out_arg,
                extra: self.extra_args(s),
                scope: *run,
                language: "",
                database: Path::new(""),
                pinned_config: &pinned_config,
                ignored,
                root: canonical_root.as_deref().and_then(Path::to_str),
            };
            code = match self.step(s, binary, s.run, &fill) {
                Ok(code) => code,
                Err(outcome) => return outcome,
            };
            if produced != report {
                if !produced.is_file() {
                    return produced_or_failed(s, code, vec![produced], Vec::new());
                }
                if let Err(e) = fs::rename(&produced, &report) {
                    return failed_io("could not move the report to", &report, &e);
                }
            }
            if !report.is_file() {
                return produced_or_failed(s, code, vec![report], Vec::new());
            }
            reports.push(report);
        }
        // One report per scanner: a snapshot holds a scanner's runs together.
        if reports.len() > 1 {
            if let Err(e) = merge_sarif_runs(&reports, &target) {
                return failed_io("could not merge the scoped reports into", &target, &e);
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
                ignored: &[],
                root: None,
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
        let vars = self.runtime_vars(s, binary);
        let launch = Launch {
            vars: &vars,
            ..self.launch(s, None)
        };
        let finished = spawn::run(binary, &argv, &launch).map_err(|e| {
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

/// The registry name `name` stands for: itself when it is one, else the
/// scanner whose SARIF driver it names (ASCII case ignored). Anything else is
/// returned unchanged, for [`Selection::check_registered`] to refuse.
fn registry_name(name: String) -> String {
    if scanner(&name).is_some() {
        return name;
    }
    REGISTRY
        .iter()
        .find(|s| s.driver.eq_ignore_ascii_case(&name))
        .map_or(name, |s| s.name.to_string())
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

/// Confine a scoped scan's `paths` to the project `root` ([FR-SC-17],
/// [NFR-SE-04]) — before any process starts. Each resolves against the root
/// (an absolute one is taken as written), lexically and then through symbolic
/// links, and is refused, with exit 2, when it lands outside the root, does
/// not exist, is the root itself, is not UTF-8, or begins with `-` (a scanner
/// would read it as a flag). What is kept is each path relative to the root,
/// `/`-separated, once, in the order given — less any path inside another
/// given directory, which would only scan the same files twice (and, for a
/// scanner that runs once per path, report each of their results twice).
///
/// # Errors
/// [`ConfigError::InvalidValue`] naming the first refused path and why.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
pub(crate) fn resolve_scope(root: &Path, paths: &[String]) -> Result<Vec<ScopedPath>, ConfigError> {
    const OUTSIDE: &str = "resolves outside the project root; a scoped scan reads only paths inside it";
    let canonical_root = root.canonicalize().map_err(|e| ConfigError::InvalidValue {
        key: "path".to_string(),
        message: format!("the project root {} cannot be resolved: {e}", root.display()),
    })?;
    let mut scope: Vec<ScopedPath> = Vec::with_capacity(paths.len());
    for raw in paths {
        let refuse = |why: &str| ConfigError::InvalidValue {
            key: "path".to_string(),
            message: format!("{raw:?} {why}"),
        };
        if raw.is_empty() {
            return Err(refuse("is empty"));
        }
        let given = Path::new(raw);
        let joined = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
        let lexical = normalize_lexically(&joined).ok_or_else(|| refuse(OUTSIDE))?;
        if !lexical.starts_with(root) && !lexical.starts_with(&canonical_root) {
            return Err(refuse(OUTSIDE));
        }
        let resolved = lexical.canonicalize().map_err(|_| {
            if raw.contains(['*', '?', '[', '{']) {
                refuse("does not exist: a glob is not expanded, so name each file or directory")
            } else {
                refuse("does not exist")
            }
        })?;
        let rel = resolved
            .strip_prefix(&canonical_root)
            .map_err(|_| refuse("resolves outside the project root through a symbolic link"))?;
        if rel.as_os_str().is_empty() {
            return Err(refuse("is the project root itself: run a full scan instead"));
        }
        let rel = rel.to_str().ok_or_else(|| refuse("is not UTF-8"))?.replace('\\', "/");
        if rel.starts_with('-') {
            return Err(refuse("begins with '-', which a scanner would read as a flag"));
        }
        let path = ScopedPath { is_dir: resolved.is_dir(), rel };
        if !scope.contains(&path) {
            scope.push(path);
        }
    }
    let dirs: Vec<String> = scope.iter().filter(|p| p.is_dir).map(|p| p.rel.clone()).collect();
    scope.retain(|p| !dirs.iter().any(|dir| *dir != p.rel && crate::history::security::path_within(&p.rel, dir)));
    Ok(scope)
}

/// `path` with every `.` dropped and every `..` applied to what precedes it,
/// without touching the filesystem; `None` when a `..` climbs above the start.
fn normalize_lexically(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// The CodeQL languages present under `root`: the `(label, --language id)`
/// pairs Logos runs, and the labels of those that need a build. Each list is
/// in [`CODEQL_LANGUAGES`] order.
///
/// The walk is [`project_walk`]: a file the project ignores is no language.
fn codeql_languages(root: &Path) -> (Vec<(&'static str, &'static str)>, Vec<String>) {
    let mut present = [false; CODEQL_LANGUAGES.len()];
    for entry in project_walk(root).flatten() {
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

/// A walk of the files the project itself does not ignore: the root's own
/// `.gitignore` and `.ignore` files at every depth, but not ignore files above
/// the root or the user's global excludes, so what it yields depends on the
/// project alone. Hidden directories are walked; `.git` and `.logos` are not.
fn project_walk(root: &Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| !is_logos_or_git(e.file_name()))
        .build()
}

fn is_logos_or_git(name: &std::ffi::OsStr) -> bool {
    name == ".logos" || name == ".git"
}

/// The top-most directories under `root` that the project's own ignore files
/// exclude — the ones [`project_walk`] steps over — relative to the root,
/// `/`-separated, shallowest first and then by name (so a cut to the argument
/// budget drops the deepest): `target`, never `target/debug` beside it, and
/// never a file. A directory is found only where its parent is walked, so one
/// inside an ignored directory is never listed apart from it.
///
/// A path a scanner's flag could misread is left out, and that directory is
/// scanned as before: one that is not UTF-8, starts with `-`, or holds a
/// comma, a double quote or a control character (Bandit splits a list on
/// commas, and Trivy reads each value as CSV, keeping its first line), a glob
/// character or a backslash.
fn ignored_dirs(root: &Path) -> Vec<String> {
    // The root itself, whose walk entry is a symlink when the root is one.
    let mut walked: std::collections::HashSet<PathBuf> = std::collections::HashSet::from([root.to_path_buf()]);
    for entry in project_walk(root).flatten() {
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            walked.insert(entry.into_path());
        }
    }
    let mut ignored = Vec::new();
    for dir in &walked {
        let Ok(children) = fs::read_dir(dir) else { continue };
        for child in children.flatten() {
            let path = child.path();
            if !child.file_type().is_ok_and(|t| t.is_dir())
                || is_logos_or_git(&child.file_name())
                || walked.contains(&path)
            {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
            let Some(rel) = parts.map(|p| p.join("/")) else { continue };
            let misread = |c: char| c.is_control() || matches!(c, ',' | '"' | '*' | '?' | '[' | ']' | '{' | '}' | '\\');
            if !rel.starts_with('-') && !rel.contains(misread) {
                ignored.push(rel);
            }
        }
    }
    ignored.sort_by(|a, b| (a.matches('/').count(), a).cmp(&(b.matches('/').count(), b)));
    ignored
}
