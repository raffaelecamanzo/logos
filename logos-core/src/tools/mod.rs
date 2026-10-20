//! The tool installer ([tool-installer], [ADR-88], S-743): installs, resolves
//! and removes the scanners Logos manages, from the
//! [compiled-in manifest](Manifest::builtin): release binaries by the route
//! below, and the Python scanners through a managed `uv` ([`uv`], S-745).
//!
//! Managed tools live in one user-level **tools directory**
//! ([`user_tools_dir`]), never inside a repository: one `<tool>/<version>/`
//! directory per installed tool, holding only its binary (or, for a Python
//! scanner, its environment), and a `.staging/` directory beside them.
//!
//! An install of one release-binary tool, in order ([NFR-SE-10]):
//! 1. **Fetch** the artifact into `.staging/<tool>-<version>/`: by spawning the
//!    system `curl` with a fixed argv ([`curl_args`]: HTTPS only, the user's
//!    `.curlrc` ignored, no shell), or by copying it from an offline
//!    [`Source::Directory`]. Logos links no HTTP client ([NFR-SE-01]).
//! 2. **Verify** its SHA-256 in-process against the manifest. A mismatch
//!    deletes the staging directory, the download with it, and fails the tool
//!    with [`FailStage::Checksum`]. Nothing is unpacked or run before this
//!    step passes.
//! 3. **Unpack** the binary inside the staging directory (the system `tar`, for
//!    a `.tar.gz`), and check it is a regular file.
//! 4. **Move** the finished `<tool>-<version>` directory into place with one
//!    `rename`. An install interrupted before this step leaves no tool
//!    directory, only staging.
//!
//! An install holds a lock on its staging entry while it writes it
//! ([`StagingLock`]); the kernel drops the lock when the process ends, however
//! it ends. Every install starts by discarding each staging entry no running
//! install holds — what a killed `serve` or CLI left, of any tool — and leaves
//! one another process is still writing alone (S-759).
//!
//! A tool that needs more than its binary — Dependency-Check's libraries,
//! CodeQL's query packs — comes as a [tree](Layout::Tree) archive: step 3
//! unpacks it whole and checks its root holds the executable, and step 4
//! moves that root into place. Dependency-Check also needs the managed Java
//! runtime ([`java`], S-746), installed with it by the same route.
//!
//! The installer runs nothing it installed but the managed `uv`, itself
//! verified as above, which builds the Python scanners' environments. A
//! managed copy is found by
//! [`resolve`]: only under the tools directory, at the manifest's pinned
//! version or, failing that, the newest other version at or above the
//! minimum ([NFR-SE-08]) — an older one until [`Installer::update`] replaces
//! it. Installing the pinned version removes the versions below it; a newer
//! one, left by a later release sharing the tools directory, is kept.
//!
//! The installer never asks for consent: the surfaces that call it do
//! ([`surface`], S-744). Within one process installs run one at a time
//! ([`INSTALL_LOCK`]), so two members' `tool-install` jobs in one `serve`
//! never write the shared tools directory at once; a job cancelled while it
//! waits its turn stops waiting (S-747). Two processes installing the same
//! tool at the same version at once are not supported: the second fails that
//! tool, its staging held by the first.
//!
//! [tool-installer]: ../../../docs/specs/architecture/components/tool-installer.md
//! [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [NFR-SE-10]: ../../../docs/specs/requirements/NFR-SE-10.md

mod java;
mod manifest;
pub mod surface;
mod uv;

#[cfg(test)]
mod tests;

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub use java::{java_env, managed_java_home};
pub use manifest::{
    version_at_least, Archive, Artifact, Layout, Licence, ManagedPython, Manifest, OptIn, Platform,
    PythonPackage, ToolEntry, CODEQL_LICENCE,
};
pub use uv::{PYTHON_DIR, UV_CACHE_DIR};
use manifest::version_key;

pub use crate::config::user_tools_dir;
use crate::config::{ConfigError, USER_DATA_HOME_ENV};
use crate::jobs::lock::open_lock_file;
use crate::jobs::spawn::{self, is_executable, usable_path_entries, Launch};

/// The reason every tool fails when no `curl` is on the search path.
pub const CURL_NOT_FOUND: &str = "curl not found: install the `curl` package with your system \
     package manager, or install offline with `logos security install --from <dir>`";

/// The reason a tool is not installed on a platform the manifest has no
/// artifact for.
pub const MANAGED_UNAVAILABLE: &str = "managed install not available on this platform";

/// The staging directory's name inside the tools directory.
pub const STAGING_DIR: &str = ".staging";

/// How long one download may take before `curl` is killed.
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(30 * 60);
/// How long one unpack may take before `tar` or `unzip` is killed.
const UNPACK_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Held for the whole of every install in this process: the tools directory
/// is per user, shared by every member a `serve` answers for, and its staging
/// paths are per tool, so two installs at once could overwrite each other's
/// download. A second install waits for it through [`install_turn`], which a
/// cancel of the waiting job ends at once.
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How often a waiting install looks at the lock and at its job's cancel.
const INSTALL_TURN_POLL: Duration = Duration::from_millis(50);

/// This process's turn to install: [`INSTALL_LOCK`], polled rather than
/// blocked on, so that a [job](crate::jobs) cancelled while it waits for
/// another member's install stops waiting — `None` then, and nothing is
/// fetched. A poisoned lock guards nothing but the order of installs.
fn install_turn() -> Option<std::sync::MutexGuard<'static, ()>> {
    loop {
        match INSTALL_LOCK.try_lock() {
            Ok(turn) => return Some(turn),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if crate::jobs::cancelled() {
            return None;
        }
        std::thread::sleep(INSTALL_TURN_POLL);
    }
}

/// The variables through which `tar` and `unzip` read default options from
/// the environment, emptied for every unpack: the argv is all they get.
const UNPACK_ENV: [(&str, &str); 3] = [("TAR_OPTIONS", ""), ("UNZIP", ""), ("UNZIPOPT", "")];

/// Where an install reads its artifacts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Each artifact's manifest URL, through the system `curl`.
    Network,
    /// A directory of pre-downloaded artifacts, each under its URL's file name,
    /// verified exactly as a download is.
    Directory(PathBuf),
}

/// A managed copy of a tool, found by [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedTool {
    pub binary: PathBuf,
    pub version: String,
}

/// Every tool's part of one install, in the order asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallReport {
    pub tools: Vec<ToolInstall>,
}

/// One tool's install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolInstall {
    /// The registry name.
    pub tool: String,
    /// The pinned version.
    pub version: String,
    #[serde(flatten)]
    pub outcome: InstallOutcome,
}

/// What happened to one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InstallOutcome {
    Installed {
        binary: PathBuf,
    },
    /// The pinned version was already installed; nothing was fetched.
    AlreadyPresent {
        binary: PathBuf,
    },
    /// [`MANAGED_UNAVAILABLE`]: no artifact for this platform.
    Unavailable {
        reason: &'static str,
    },
    /// Nothing was installed; the staging directory was removed.
    Failed {
        stage: FailStage,
        reason: String,
    },
    /// The job the install ran on was cancelled while this tool was being
    /// installed: its download or unpack was killed, nothing was installed and
    /// the staging directory was removed (S-747).
    Cancelled,
}

/// One step of an install as it happens, for a surface that follows it (the
/// web UI's `tool-install` job, S-747): a tool's install starting, and its
/// outcome once settled. Serialised with a `phase` tag, beside the scan's own
/// steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum InstallProgress {
    InstallStarted { tool: String, version: String },
    InstallFinished(ToolInstall),
}

/// Where an install failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailStage {
    /// The artifact could not be fetched (or `curl` is missing).
    Download,
    /// The artifact's SHA-256 is not the manifest's; it was deleted.
    Checksum,
    /// The verified artifact did not yield the binary.
    Unpack,
    /// The tools directory could not be written.
    Install,
}

/// One tool's part of an uninstall.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolUninstall {
    pub tool: String,
    #[serde(flatten)]
    pub outcome: UninstallOutcome,
}

/// What happened to one tool's managed copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum UninstallOutcome {
    Removed,
    /// There was no managed copy to remove.
    NotInstalled,
    /// The managed copies could not be removed; some may already be gone.
    Failed {
        reason: String,
    },
}

/// The managed copy of `tool` in `tools_dir`: the version `manifest` pins, if
/// it is installed there and executable; else the newest installed version at
/// or above the manifest's minimum — a copy another release pinned: an older
/// one is kept until `install --update` replaces it, and a newer one, left
/// by a later release sharing the tools directory, is never replaced by an
/// older pin (S-744). Only
/// `<tools_dir>/<tool>/<version>/<binary>` (a Python tool's
/// `…/<version>/bin/<binary>`) is ever looked at, with the tool
/// and binary names the manifest gives and a dotted numeric version: no path
/// the repository or the environment names ([NFR-SE-08]).
///
/// [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
pub fn resolve(tools_dir: &Path, manifest: &Manifest, tool: &str) -> Option<ManagedTool> {
    resolve_entry(tools_dir, manifest, manifest.tool(tool)?)
}

/// [`resolve`] for one manifest entry, `uv` and the Java runtime included. A
/// Python tool's copy counts only while its environment's interpreter still
/// resolves, and a [Java](ToolEntry::needs_java) tool's only while the managed
/// Java runtime does: removing either leaves the tool to be installed again.
fn resolve_entry(tools_dir: &Path, manifest: &Manifest, entry: &ToolEntry) -> Option<ManagedTool> {
    if entry.needs_java() && resolve_entry(tools_dir, manifest, &manifest.java).is_none() {
        return None;
    }
    let at = |version: &str| {
        let dir = tools_dir.join(&entry.tool).join(version);
        let binary = dir.join(entry.executable());
        let interpreter_ok =
            entry.python.is_none() || is_executable(&dir.join("bin").join("python"));
        (is_executable(&binary) && interpreter_ok).then(|| ManagedTool {
            binary,
            version: version.to_string(),
        })
    };
    at(&entry.version).or_else(|| {
        installed_versions(&tools_dir.join(&entry.tool))
            .into_iter()
            .filter(|v| version_at_least(v, &entry.min_version))
            .max_by_key(|v| version_key(v))
            .and_then(|v| at(&v))
    })
}

/// The dotted numeric version directories under one tool's directory.
fn installed_versions(tool_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(tool_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| version_key(name).is_some())
        .collect()
}

/// Installs, resolves and removes managed tools in one tools directory.
#[derive(Debug, Clone)]
pub struct Installer {
    tools_dir: PathBuf,
    manifest: Manifest,
    search_path: Option<OsString>,
    platform: Option<Platform>,
    /// The project the install is run for, whose `PATH` entries are skipped.
    project_root: Option<PathBuf>,
}

impl Installer {
    /// An installer over `tools_dir`, the [builtin](Manifest::builtin)
    /// manifest, the process `PATH` (to find `curl` and `tar`) and the
    /// [current](Platform::current) platform.
    pub fn new(tools_dir: PathBuf) -> Self {
        Self {
            tools_dir,
            manifest: Manifest::builtin().clone(),
            search_path: std::env::var_os("PATH"),
            platform: Platform::current(),
            project_root: None,
        }
    }

    /// [`new`](Self::new) over [`user_tools_dir`].
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] (exit 2) naming [`MANAGED_UNAVAILABLE`]
    /// on a platform without managed installs, else when no tools directory
    /// can be resolved from the environment.
    pub fn from_process() -> Result<Self, ConfigError> {
        Self::from_env(user_tools_dir(), Platform::current())
    }

    /// [`from_process`](Self::from_process) over a resolved tools directory
    /// and platform. The platform is checked first, so Windows reports
    /// [`MANAGED_UNAVAILABLE`] rather than a missing directory.
    pub(crate) fn from_env(
        tools_dir: Option<PathBuf>,
        platform: Option<Platform>,
    ) -> Result<Self, ConfigError> {
        let invalid = |message: String| ConfigError::InvalidValue {
            key: "tools".to_string(),
            message,
        };
        if platform.is_none() {
            return Err(invalid(MANAGED_UNAVAILABLE.to_string()));
        }
        let tools_dir = tools_dir.ok_or_else(|| {
            invalid(format!(
                "there is no user-level tools directory; set ${USER_DATA_HOME_ENV} or $HOME"
            ))
        })?;
        Ok(Self::new(tools_dir).with_platform(platform))
    }

    /// Install from `manifest` instead of the builtin one (tests: fixture
    /// artifacts).
    #[must_use]
    pub fn with_manifest(mut self, manifest: Manifest) -> Self {
        self.manifest = manifest;
        self
    }

    /// Look `curl` and `tar` up on `search_path`, and give them only its
    /// absolute entries (outside the [project](Self::for_project), if one is
    /// set) as their `PATH`.
    #[must_use]
    pub fn with_search_path(mut self, search_path: Option<OsString>) -> Self {
        self.search_path = search_path;
        self
    }

    /// Run for the project at `root`: a search-path entry inside it is never
    /// used to find `curl` or `tar`, nor given to them as `PATH`, exactly as
    /// the [scan](crate::security::Orchestrator) reduces `PATH` ([NFR-SE-08]).
    /// Every caller that installs on behalf of a project passes its root.
    ///
    /// [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
    #[must_use]
    pub fn for_project(mut self, root: &Path) -> Self {
        self.project_root = Some(root.to_path_buf());
        self
    }

    /// Install for `platform`; `None` is a platform without managed installs.
    #[must_use]
    pub fn with_platform(mut self, platform: Option<Platform>) -> Self {
        self.platform = platform;
        self
    }

    pub fn tools_dir(&self) -> &Path {
        &self.tools_dir
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The managed copy of `tool`, as [`resolve`] finds it.
    pub fn resolve(&self, tool: &str) -> Option<ManagedTool> {
        resolve(&self.tools_dir, &self.manifest, tool)
    }

    /// Install each of `tools` (registry names) from `source`, one after the
    /// other. One tool failing never stops the others.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] for a name the manifest does not list,
    /// before anything is fetched. Every other failure is that tool's
    /// [`InstallOutcome::Failed`].
    pub fn install<S: AsRef<str>>(
        &self,
        tools: &[S],
        source: &Source,
    ) -> Result<InstallReport, ConfigError> {
        self.install_with(tools, source, false, &mut |_| {})
    }

    /// [`install`](Self::install), except that only the **pinned** version
    /// counts as present: a managed copy an earlier release pinned is replaced
    /// by the pinned one, and removed once it is in place
    /// (`install --update`, [FR-SC-22]).
    ///
    /// # Errors
    /// As [`install`](Self::install).
    ///
    /// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
    pub fn update<S: AsRef<str>>(
        &self,
        tools: &[S],
        source: &Source,
    ) -> Result<InstallReport, ConfigError> {
        self.install_with(tools, source, true, &mut |_| {})
    }

    /// [`install`](Self::install), or [`update`](Self::update) with
    /// `pinned_only`, handing each tool's start and outcome to `progress`.
    ///
    /// On a [job](crate::jobs)'s thread a cancel kills the download or unpack
    /// running — the tool is reported [`InstallOutcome::Cancelled`], with
    /// nothing of it left — and no further tool is started: the report lists
    /// the tools attempted until then, so a caller can say which landed.
    pub(crate) fn install_with<S: AsRef<str>>(
        &self,
        tools: &[S],
        source: &Source,
        pinned_only: bool,
        progress: &mut dyn FnMut(&InstallProgress),
    ) -> Result<InstallReport, ConfigError> {
        let entries = self.entries(tools)?;
        let Some(_one_at_a_time) = install_turn() else {
            return Ok(InstallReport { tools: Vec::new() });
        };
        // A symlink in place of the staging directory is removed, never
        // swept through; `install_one` fails each tool if it cannot be.
        let staging_root = self.tools_dir.join(STAGING_DIR);
        if unlink_if_symlink(&staging_root).is_ok() {
            sweep_stale_staging(&staging_root);
        }
        let search_dirs = match &self.project_root {
            Some(root) => usable_path_entries(root, self.search_path.as_deref()),
            None => absolute_entries(self.search_path.as_deref()),
        };
        let child_path = (!search_dirs.is_empty())
            .then(|| std::env::join_paths(&search_dirs).ok())
            .flatten();
        let ctx = Context {
            search_dirs: &search_dirs,
            child_path: child_path.as_ref(),
            source,
            uv: OnceCell::new(),
            java: OnceCell::new(),
        };
        let mut tools = Vec::with_capacity(entries.len());
        for entry in entries {
            if crate::jobs::cancelled() {
                break;
            }
            progress(&InstallProgress::InstallStarted {
                tool: entry.tool.clone(),
                version: entry.version.clone(),
            });
            let started = Instant::now();
            let (mut outcome, host, bytes) = self.install_one(entry, &ctx, pinned_only);
            // A process the cancel killed, or refused to start, fails its
            // stage; the staging it wrote is already gone.
            if crate::jobs::cancelled() && matches!(outcome, InstallOutcome::Failed { .. }) {
                outcome = InstallOutcome::Cancelled;
            }
            log_install(&entry.tool, &entry.version, host, bytes, started, &outcome);
            let tool = ToolInstall {
                tool: entry.tool.clone(),
                version: entry.version.clone(),
                outcome,
            };
            progress(&InstallProgress::InstallFinished(tool.clone()));
            tools.push(tool);
        }
        remove_dir_if_empty(&self.tools_dir.join(STAGING_DIR));
        // A call that reached for `uv` or the Java runtime but left no tool
        // that needs it in place keeps nothing it fetched for one.
        if ctx.uv.get().is_some() {
            self.reclaim_python_support();
        }
        if ctx.java.get().is_some() {
            self.reclaim_java_runtime();
        }
        Ok(InstallReport { tools })
    }

    /// Remove the managed copies of `tools`, every version, one after the
    /// other. A copy the user installed elsewhere is never touched: only
    /// `<tools dir>/<tool>/` is. One tool failing never stops the others.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] for a name the manifest does not list,
    /// before anything is removed. Every other failure is that tool's
    /// [`UninstallOutcome::Failed`].
    ///
    /// Once no Python tool is left, the managed Python, `uv`'s cache and `uv`
    /// go too; once no tool that needs it is left, so does the Java runtime.
    pub fn uninstall<S: AsRef<str>>(&self, tools: &[S]) -> Result<Vec<ToolUninstall>, ConfigError> {
        let entries = self.entries(tools)?;
        let removed = entries
            .into_iter()
            .map(|entry| {
                let dir = self.tools_dir.join(&entry.tool);
                let outcome = match remove_path(&dir) {
                    Ok(()) => UninstallOutcome::Removed,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => UninstallOutcome::NotInstalled,
                    Err(e) => UninstallOutcome::Failed {
                        reason: format!("could not remove {}: {e}", dir.display()),
                    },
                };
                ToolUninstall {
                    tool: entry.tool.clone(),
                    outcome,
                }
            })
            .collect();
        self.reclaim_python_support();
        self.reclaim_java_runtime();
        Ok(removed)
    }

    fn entries<S: AsRef<str>>(&self, tools: &[S]) -> Result<Vec<&ToolEntry>, ConfigError> {
        tools
            .iter()
            .map(|name| {
                let name = name.as_ref();
                self.manifest
                    .tool(name)
                    .ok_or_else(|| ConfigError::InvalidValue {
                        key: "tool".to_string(),
                        message: format!(
                            "{name:?} is not a tool Logos installs (expected one of: {})",
                            self.manifest
                                .tools
                                .iter()
                                .map(|t| t.tool.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    })
            })
            .collect()
    }

    /// One tool's install, with the source host and the bytes fetched for the
    /// install event. With `pinned_only`, an older managed copy does not count
    /// as present.
    fn install_one<'e>(
        &'e self,
        entry: &'e ToolEntry,
        ctx: &Context<'_>,
        pinned_only: bool,
    ) -> (InstallOutcome, &'e str, u64) {
        let Some(route) = self.route(entry) else {
            return (
                InstallOutcome::Unavailable {
                    reason: MANAGED_UNAVAILABLE,
                },
                "",
                0,
            );
        };
        let host = match (ctx.source, route) {
            (Source::Directory(_), _) => "",
            (Source::Network, Route::Release(artifact)) => artifact.host(),
            (Source::Network, Route::Python(_)) => self.manifest.python.index_host(),
        };
        let defect = match route {
            Route::Release(artifact) => artifact.defect(),
            Route::Python(package) => package.defect(&entry.tool, &entry.version),
        };
        if let Some(defect) = defect {
            return (
                failed(
                    FailStage::Download,
                    format!("the manifest entry is invalid: {defect}"),
                ),
                host,
                0,
            );
        }
        // The runtime first: a tool already in place that lost its runtime
        // needs only the runtime.
        if entry.needs_java() {
            let java = ctx.java.get_or_init(|| self.ensure_support(&self.manifest.java, ctx));
            if let Err(failure) = java {
                return (failure.clone(), host, 0);
            }
        }
        let present = resolve_entry(&self.tools_dir, &self.manifest, entry)
            .filter(|found| !pinned_only || version_at_least(&found.version, &entry.version));
        if let Some(found) = present {
            return (
                InstallOutcome::AlreadyPresent {
                    binary: found.binary,
                },
                host,
                0,
            );
        }
        let dest = self.tools_dir.join(&entry.tool).join(&entry.version);
        let staging = self
            .tools_dir
            .join(STAGING_DIR)
            .join(format!("{}-{}", entry.tool, entry.version));
        // A symlink in place of the tool's directory or of the staging
        // directory is removed, never followed: everything below is deleted or
        // written through those two paths.
        for dir in [
            self.tools_dir.join(&entry.tool),
            self.tools_dir.join(STAGING_DIR),
        ] {
            if let Err(e) = unlink_if_symlink(&dir) {
                return (
                    failed_io(FailStage::Install, "could not remove", &dir, &e),
                    host,
                    0,
                );
            }
        }
        if let Err(e) = fs::create_dir_all(self.tools_dir.join(STAGING_DIR)) {
            return (
                failed_io(FailStage::Install, "could not create", &self.tools_dir.join(STAGING_DIR), &e),
                host,
                0,
            );
        }
        // Held until the staging entry is gone again, below.
        let _held = match StagingLock::take(&staging) {
            Ok(Some(held)) => held,
            Ok(None) => {
                let reason = format!(
                    "another Logos process is using the staging of {} {}: try again once it ends",
                    entry.tool, entry.version
                );
                return (failed(FailStage::Install, reason), host, 0);
            }
            Err(e) => {
                return (
                    failed_io(FailStage::Install, "could not lock", &lock_path(&staging), &e),
                    host,
                    0,
                )
            }
        };
        // What an interrupted earlier install at this version left is
        // discarded, and so is a tool directory that does not hold a runnable
        // binary.
        for leftover in [&staging, &dest] {
            if let Err(e) = remove_path_if_present(leftover) {
                return (
                    failed_io(FailStage::Install, "could not remove", leftover, &e),
                    host,
                    0,
                );
            }
        }
        if let Err(e) = fs::create_dir_all(&staging) {
            return (
                failed_io(FailStage::Install, "could not create", &staging, &e),
                host,
                0,
            );
        }
        let outcome = match route {
            Route::Release(artifact) => self.stage_and_move(entry, artifact, &staging, &dest, ctx),
            Route::Python(package) => self.provision(entry, package, &staging, &dest, ctx),
        };
        let bytes = match &outcome {
            Ok(bytes) => *bytes,
            Err(_) => 0,
        };
        // On success the staging directory is empty; on failure it holds the
        // download or a partial unpack. Either way it goes.
        let _ = remove_path_if_present(&staging);
        if outcome.is_ok() {
            self.remove_other_versions(entry);
        }
        match outcome {
            Ok(_) => (
                InstallOutcome::Installed {
                    binary: dest.join(entry.executable()),
                },
                host,
                bytes,
            ),
            Err(outcome) => (outcome, host, bytes),
        }
    }

    /// How `entry` installs on this platform, if it can: its artifact, or —
    /// for a Python tool — its package, when the package, the managed Python
    /// and `uv` all have a build for the platform.
    fn route<'e>(&self, entry: &'e ToolEntry) -> Option<Route<'e>> {
        let platform = self.platform?;
        match &entry.python {
            None => entry.artifact(platform).map(Route::Release),
            Some(package) => (package.size(platform).is_some()
                && self.manifest.python.size(platform).is_some()
                && self.manifest.uv.artifact(platform).is_some())
            .then_some(Route::Python(package)),
        }
    }

    /// A support install — `uv`, the Java runtime — installed (or found) once
    /// per install call, through the release route, by the first tool that
    /// needs it. A failure fails every tool of the call that needs it, with
    /// the support's stage and reason.
    fn ensure_support(&self, entry: &ToolEntry, ctx: &Context<'_>) -> Result<PathBuf, InstallOutcome> {
        let started = Instant::now();
        let (outcome, host, bytes) = self.install_one(entry, ctx, false);
        log_install(&entry.tool, &entry.version, host, bytes, started, &outcome);
        match outcome {
            InstallOutcome::Installed { binary } | InstallOutcome::AlreadyPresent { binary } => {
                Ok(binary)
            }
            InstallOutcome::Failed { stage, reason } => Err(failed(
                stage,
                format!("{} {}: {reason}", entry.tool, entry.version),
            )),
            other @ (InstallOutcome::Unavailable { .. } | InstallOutcome::Cancelled) => Err(other),
        }
    }

    /// The consent line of a support install — `(name, version, host,
    /// download size)` — while it is not installed and has a build for
    /// `platform`.
    fn support_pending(
        &self,
        entry: &ToolEntry,
        platform: Platform,
    ) -> Option<(String, String, String, u64)> {
        let artifact = entry.artifact(platform)?;
        resolve_entry(&self.tools_dir, &self.manifest, entry)
            .is_none()
            .then(|| {
                (
                    entry.tool.clone(),
                    entry.version.clone(),
                    artifact.host().to_string(),
                    artifact.size,
                )
            })
    }

    /// When no tool that `needs` the support is left in the tools directory,
    /// remove the support's directories, `names`. Best effort: a leftover is
    /// logged and never used.
    fn reclaim_support(&self, needs: impl Fn(&ToolEntry) -> bool, names: &[&str]) {
        if self
            .manifest
            .tools
            .iter()
            .filter(|t| needs(t))
            .any(|t| self.tools_dir.join(&t.tool).exists())
        {
            return;
        }
        for name in names {
            let path = self.tools_dir.join(name);
            if let Err(e) = remove_path_if_present(&path) {
                tracing::warn!(
                    target: "logos::tools",
                    path = %path.display(),
                    "could not remove a support install no tool needs: {e}",
                );
            }
        }
    }

    /// Remove every version of `entry`'s tool below the pinned one, once the
    /// pinned one is in place: the copy an update replaces, or one below the
    /// minimum. A newer version — another release's pin — is kept. Best
    /// effort: a copy that cannot be removed is never used while the pinned
    /// one is there, so it is logged and left.
    fn remove_other_versions(&self, entry: &ToolEntry) {
        let tool_dir = self.tools_dir.join(&entry.tool);
        for version in installed_versions(&tool_dir) {
            if version_at_least(&version, &entry.version) {
                continue;
            }
            let old = tool_dir.join(&version);
            if let Err(e) = remove_path_if_present(&old) {
                tracing::warn!(
                    target: "logos::tools",
                    tool = entry.tool.as_str(),
                    version = version.as_str(),
                    "could not remove the replaced copy: {e}",
                );
            }
        }
    }

    /// Fetch, verify, unpack and move: `Ok` with the artifact's size once the
    /// tool is in place.
    fn stage_and_move(
        &self,
        entry: &ToolEntry,
        artifact: &Artifact,
        staging: &Path,
        dest: &Path,
        ctx: &Context<'_>,
    ) -> Result<u64, InstallOutcome> {
        let name = artifact.file_name().expect("checked by Artifact::defect");
        let download = staging.join(name);
        fetch(artifact, &download, staging, ctx)?;

        let actual = sha256_file(&download)
            .map_err(|e| failed_io(FailStage::Checksum, "could not read", &download, &e))?;
        if actual != artifact.sha256 {
            // Deleted before anything else happens; the caller removes the
            // staging directory as well.
            let _ = fs::remove_file(&download);
            return Err(failed(
                FailStage::Checksum,
                format!(
                    "checksum mismatch: {name} has SHA-256 {actual}, but the manifest pins {}; \
                     the download was deleted and nothing from it was run",
                    artifact.sha256
                ),
            ));
        }
        let bytes = fs::metadata(&download).map(|m| m.len()).unwrap_or(0);

        // The finished tool directory is built inside staging, so the move
        // below is one rename on one filesystem.
        let finished = match &artifact.layout {
            Layout::Tree { archive, root } => {
                unpack_tree(entry, &download, *archive, root, staging, ctx)?
            }
            Layout::TarGz { member } => {
                let extract = create_extract_dir(staging)?;
                untar(&download, &extract, Some(member), ctx)?;
                let unpacked = extract.join(member);
                if !is_regular_file(&unpacked) {
                    return Err(failed(
                        FailStage::Unpack,
                        format!("{name} holds no regular file {member}"),
                    ));
                }
                place_binary(entry, &unpacked, staging)?
            }
            Layout::Binary => place_binary(entry, &download, staging)?,
        };
        let binary = finished.join(&entry.binary);
        set_executable(&binary)
            .map_err(|e| failed_io(FailStage::Unpack, "could not make executable", &binary, &e))?;

        let parent = dest
            .parent()
            .expect("<tools>/<tool>/<version> has a parent");
        fs::create_dir_all(parent)
            .map_err(|e| failed_io(FailStage::Install, "could not create", parent, &e))?;
        fs::rename(&finished, dest)
            .map_err(|e| failed_io(FailStage::Install, "could not move the tool into", dest, &e))?;
        Ok(bytes)
    }
}

/// How one tool installs.
#[derive(Clone, Copy)]
enum Route<'e> {
    Release(&'e Artifact),
    Python(&'e PythonPackage),
}

/// What every tool of one install shares.
struct Context<'a> {
    /// The absolute search-path entries `curl`, `tar` and `uv` are looked up
    /// on.
    search_dirs: &'a [PathBuf],
    /// Those entries joined: the `PATH` `curl`, `tar` and `uv` run with.
    child_path: Option<&'a OsString>,
    source: &'a Source,
    /// The managed `uv`, installed or found by the first Python tool of the
    /// call; its failure is every Python tool's.
    uv: OnceCell<Result<PathBuf, InstallOutcome>>,
    /// The managed Java runtime, likewise for the Java tools.
    java: OnceCell<Result<PathBuf, InstallOutcome>>,
}

impl Context<'_> {
    fn find(&self, program: &str) -> Option<PathBuf> {
        self.search_dirs
            .iter()
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
    }

    fn launch<'a>(&'a self, cwd: &'a Path, deadline: Duration) -> Launch<'a> {
        Launch {
            cwd,
            env: &[],
            vars: &[],
            isolated: None,
            path: self.child_path,
            deadline: Some(deadline),
        }
    }
}

/// `curl`'s whole argv for one download: everything but `url` and `output` is
/// fixed. `-q` comes first so no `.curlrc` can add an option; only `https` is
/// allowed, for the request and for every redirect; an HTTP error fails.
pub fn curl_args(url: &str, output: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-q",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--fail",
        "--location",
        "--max-redirs",
        "10",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "30",
        "--output",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(output.as_os_str().to_os_string());
    args.push(OsString::from(url));
    args
}

/// Put the artifact at `download`, from the network or the offline directory.
fn fetch(
    artifact: &Artifact,
    download: &Path,
    staging: &Path,
    ctx: &Context<'_>,
) -> Result<(), InstallOutcome> {
    match ctx.source {
        Source::Directory(dir) => {
            let name = artifact.file_name().expect("checked by Artifact::defect");
            let from = dir.join(name);
            // Only a regular file: opening a FIFO would block the install with
            // no deadline.
            match fs::metadata(&from) {
                Ok(meta) if meta.is_file() => fs::copy(&from, download)
                    .map(|_| ())
                    .map_err(|e| failed_io(FailStage::Download, "could not read", &from, &e)),
                Ok(_) => Err(failed(
                    FailStage::Download,
                    format!("{} is not a regular file", from.display()),
                )),
                Err(e) => Err(failed_io(FailStage::Download, "could not read", &from, &e)),
            }
        }
        Source::Network => {
            let Some(curl) = ctx.find("curl") else {
                return Err(failed(FailStage::Download, CURL_NOT_FOUND.to_string()));
            };
            let args = curl_args(&artifact.url, download);
            let finished = spawn::run(&curl, &args, &ctx.launch(staging, DOWNLOAD_DEADLINE))
                .map_err(|e| {
                    failed(
                        FailStage::Download,
                        format!("could not start {}: {e}", curl.display()),
                    )
                })?;
            exited_cleanly("curl", &finished, FailStage::Download)
        }
    }
}

/// Extract `member` of the verified archive — all of it, without one — into
/// `extract`.
fn untar(
    archive: &Path,
    extract: &Path,
    member: Option<&str>,
    ctx: &Context<'_>,
) -> Result<(), InstallOutcome> {
    let mut args: Vec<OsString> = vec![
        "-x".into(),
        "-z".into(),
        "-f".into(),
        archive.as_os_str().to_os_string(),
        "-C".into(),
        extract.as_os_str().to_os_string(),
    ];
    args.extend(member.map(OsString::from));
    unpack_with("tar", &args, extract, ctx)
}

/// Extract the whole verified zip `archive` into `extract`. `-o` overwrites
/// rather than asking (stdin is closed); `unzip` skips any `../` member.
fn unzip(archive: &Path, extract: &Path, ctx: &Context<'_>) -> Result<(), InstallOutcome> {
    let args: Vec<OsString> = vec![
        "-q".into(),
        "-o".into(),
        archive.as_os_str().to_os_string(),
        "-d".into(),
        extract.as_os_str().to_os_string(),
    ];
    unpack_with("unzip", &args, extract, ctx)
}

/// Run the unpacker `program` with `args`, its default-option variables
/// emptied.
fn unpack_with(
    program: &str,
    args: &[OsString],
    extract: &Path,
    ctx: &Context<'_>,
) -> Result<(), InstallOutcome> {
    let Some(path) = ctx.find(program) else {
        return Err(failed(
            FailStage::Unpack,
            format!("{program} not found on PATH"),
        ));
    };
    let launch = Launch {
        env: &UNPACK_ENV,
        ..ctx.launch(extract, UNPACK_DEADLINE)
    };
    let finished = spawn::run(&path, args, &launch).map_err(|e| {
        failed(
            FailStage::Unpack,
            format!("could not start {}: {e}", path.display()),
        )
    })?;
    exited_cleanly(program, &finished, FailStage::Unpack)
}

/// The finished tool directory of a single-binary layout: `staging/tool`,
/// holding `from` moved in as the tool's binary.
fn place_binary(entry: &ToolEntry, from: &Path, staging: &Path) -> Result<PathBuf, InstallOutcome> {
    let finished = staging.join("tool");
    fs::create_dir(&finished)
        .map_err(|e| failed_io(FailStage::Install, "could not create", &finished, &e))?;
    let binary = finished.join(&entry.binary);
    fs::rename(from, &binary).map_err(|e| {
        failed_io(
            FailStage::Unpack,
            "could not move the binary to",
            &binary,
            &e,
        )
    })?;
    Ok(finished)
}

/// `staging/extract`, the directory an archive is unpacked into.
fn create_extract_dir(staging: &Path) -> Result<PathBuf, InstallOutcome> {
    let extract = staging.join("extract");
    fs::create_dir(&extract)
        .map_err(|e| failed_io(FailStage::Install, "could not create", &extract, &e))?;
    Ok(extract)
}

/// Whether `path` is a regular file, a symlink never followed.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// Unpack a [tree](Layout::Tree) archive inside `staging` and return its
/// `root` directory, once it is a real directory holding `entry`'s executable
/// as a regular file, made executable.
fn unpack_tree(
    entry: &ToolEntry,
    download: &Path,
    archive: Archive,
    root: &str,
    staging: &Path,
    ctx: &Context<'_>,
) -> Result<PathBuf, InstallOutcome> {
    let extract = create_extract_dir(staging)?;
    match archive {
        Archive::TarGz => untar(download, &extract, None, ctx)?,
        Archive::Zip => unzip(download, &extract, ctx)?,
    }
    let name = download.file_name().unwrap_or_default().to_string_lossy();
    let tree = extract.join(root);
    if !fs::symlink_metadata(&tree).is_ok_and(|m| m.file_type().is_dir()) {
        return Err(failed(
            FailStage::Unpack,
            format!("{name} holds no directory {root}"),
        ));
    }
    let executable = tree.join(entry.executable());
    if !is_regular_file(&executable) {
        return Err(failed(
            FailStage::Unpack,
            format!(
                "{name} holds no regular file {root}/{}",
                entry.executable().display()
            ),
        ));
    }
    Ok(tree)
}

fn exited_cleanly(
    program: &str,
    finished: &spawn::Finished,
    stage: FailStage,
) -> Result<(), InstallOutcome> {
    match finished.status {
        Some(status) if status.success() => Ok(()),
        Some(status) => {
            let how = status.code().map_or_else(
                || "was killed by a signal".to_string(),
                |c| format!("exited with {c}"),
            );
            let stderr = finished.stderr.trim();
            let reason = if stderr.is_empty() {
                format!("{program} {how}")
            } else {
                format!("{program} {how}: {stderr}")
            };
            Err(failed(stage, reason))
        }
        None => Err(failed(
            stage,
            format!("{program} did not finish in time and was killed"),
        )),
    }
}

/// The SHA-256 of `path`'s bytes, in lowercase hex.
///
/// # Errors
/// Any I/O error reading the file.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn failed(stage: FailStage, reason: String) -> InstallOutcome {
    InstallOutcome::Failed { stage, reason }
}

fn failed_io(stage: FailStage, what: &str, path: &Path, e: &io::Error) -> InstallOutcome {
    failed(stage, format!("{what} {}: {e}", path.display()))
}

/// The install event ([tool-installer] Observability): one per tool — `uv`
/// and the managed Python included — never the URL's path or query.
///
/// [tool-installer]: ../../../docs/specs/architecture/components/tool-installer.md
fn log_install(
    tool: &str,
    version: &str,
    host: &str,
    bytes: u64,
    started: Instant,
    outcome: &InstallOutcome,
) {
    let token = match outcome {
        InstallOutcome::Installed { .. } => "installed",
        InstallOutcome::AlreadyPresent { .. } => "already_present",
        InstallOutcome::Unavailable { .. } => "unavailable",
        InstallOutcome::Failed { .. } => "failed",
        InstallOutcome::Cancelled => "cancelled",
    };
    tracing::info!(
        target: "logos::tools",
        tool,
        version,
        host,
        bytes,
        duration_ms = started.elapsed().as_millis() as u64,
        outcome = token,
        "tool install",
    );
}

/// The absolute entries of `search_path`: a relative one would resolve
/// against the working directory.
fn absolute_entries(search_path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    search_path
        .map(|p| {
            std::env::split_paths(p)
                .filter(|d| d.is_absolute())
                .collect()
        })
        .unwrap_or_default()
}

fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Remove a file, a symlink (never followed) or a directory tree.
fn remove_path(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn remove_path_if_present(path: &Path) -> io::Result<()> {
    match remove_path(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The suffix of a staging entry's lock file, beside the entry.
const LOCK_SUFFIX: &str = ".lock";

/// `<entry>.lock`, the lock file of the staging entry `entry`.
fn lock_path(entry: &Path) -> PathBuf {
    let mut path = entry.as_os_str().to_owned();
    path.push(LOCK_SUFFIX);
    PathBuf::from(path)
}

/// An install's hold on its staging entry: an exclusive `flock` on the
/// entry's lock file, which the kernel releases when the process ends —
/// however it ends — so an entry whose lock can be taken belongs to no
/// running install. Dropping it removes the lock file, then releases the
/// lock.
struct StagingLock {
    path: PathBuf,
    _file: fs::File,
}

/// How many times [`StagingLock::take`] locks a lock file that turns out to
/// have been replaced. Each retry means another process removed the file in
/// between, which contention ends within one or two; the bound keeps a path
/// that never names the file it opened from looping for ever.
const LOCK_ATTEMPTS: usize = 8;

/// What locking one opened lock file came to.
enum Hold {
    Held(StagingLock),
    /// Another process holds it.
    Busy,
    /// Locked, but no longer the file at its path: its last holder removed
    /// it, so the lock guards nothing and is taken again on the current file.
    Unlinked,
}

impl StagingLock {
    /// Take the lock of the staging entry `entry`, creating its lock file;
    /// `None` while another process holds it. A symlink in place of the lock
    /// file is removed, and the file is opened without following one.
    ///
    /// # Errors
    /// The lock file cannot be opened or locked, or was replaced on every one
    /// of [`LOCK_ATTEMPTS`].
    fn take(entry: &Path) -> io::Result<Option<Self>> {
        let path = lock_path(entry);
        for _ in 0..LOCK_ATTEMPTS {
            match hold(open_lock_file(&path)?, &path)? {
                Hold::Held(held) => return Ok(Some(held)),
                Hold::Busy => return Ok(None),
                Hold::Unlinked => {}
            }
        }
        Err(io::Error::other("the lock file was replaced each time it was locked"))
    }
}

/// Lock `file`, opened from `path`. A holder removes the file before it
/// releases the lock, so a process that opened it before the removal can
/// lock it after: the lock counts only while `path` still names that file.
fn hold(file: fs::File, path: &Path) -> io::Result<Hold> {
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(Hold::Busy),
        Err(fs::TryLockError::Error(e)) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let locked = file.metadata()?;
        let current = match fs::symlink_metadata(path) {
            Ok(current) => current,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Hold::Unlinked),
            Err(e) => return Err(e),
        };
        if (locked.dev(), locked.ino()) != (current.dev(), current.ino()) {
            return Ok(Hold::Unlinked);
        }
    }
    Ok(Hold::Held(StagingLock { path: path.to_path_buf(), _file: file }))
}

impl Drop for StagingLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Discard every staging entry under `staging_root` that no running install
/// holds — the download or partial unpack of a process killed mid-install —
/// with its lock file. An entry whose lock another process holds is left
/// alone. Nothing here fails an install: an entry that cannot be removed is
/// logged and left, and the install of that tool removes it or fails.
fn sweep_stale_staging(staging_root: &Path) {
    let Ok(entries) = fs::read_dir(staging_root) else {
        return;
    };
    let names: BTreeSet<OsString> = entries
        .flatten()
        .map(|e| {
            let name = e.file_name();
            match name.to_str().and_then(|n| n.strip_suffix(LOCK_SUFFIX)) {
                Some(entry) => OsString::from(entry),
                None => name,
            }
        })
        .collect();
    for name in names {
        let entry = staging_root.join(&name);
        let held = match StagingLock::take(&entry) {
            Ok(Some(held)) => held,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(target: "logos::tools", entry = %entry.display(), "stale staging not locked: {e}");
                continue;
            }
        };
        match remove_path_if_present(&entry) {
            Ok(()) => tracing::info!(target: "logos::tools", entry = %entry.display(), "stale staging removed"),
            Err(e) => tracing::warn!(target: "logos::tools", entry = %entry.display(), "stale staging not removed: {e}"),
        }
        drop(held);
    }
}

fn unlink_if_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(path),
        _ => Ok(()),
    }
}

fn remove_dir_if_empty(path: &Path) {
    // Fails, harmlessly, while another tool's staging is still there.
    let _ = fs::remove_dir(path);
}
