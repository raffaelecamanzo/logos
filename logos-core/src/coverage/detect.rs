//! Detection ([FR-CV-11]): whether each registered tool applies to the
//! project, whether it is installed, and at which version — with no network
//! and **without running any of the project's code**.
//!
//! Detection runs on read paths, before the execution consent has been asked
//! for, so it never starts a binary the repository supplies:
//! - a copy inside the project (a wrapper, `node_modules/.bin`, the `.venv`)
//!   is never run. Its version is read from a file beside it: the wrapper's
//!   distribution URL, the package's `package.json`, the distribution's
//!   `.dist-info` directory;
//! - a copy outside the project (on the usable `PATH`, or the user-level
//!   `path` override) is asked its version, with a deadline, from the
//!   filesystem root ([`PROBE_CWD`]), so that no project file — a `.mvn/`
//!   extension, a `pytest.ini`, a `go.mod` toolchain line, a
//!   `.cargo/config.toml` — is read by it;
//! - a copy Logos manages (`cargo-llvm-cov`, S-748) is found only under the
//!   user-level tools directory, at a path the manifest names
//!   ([`crate::tools::resolve`]); its version is its directory's.
//!
//! [FR-CV-11]: ../../../docs/specs/requirements/FR-CV-11.md

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::registry::{CoverageTool, Locate, Marker};
use crate::jobs::spawn::{self, find_on_path, is_executable, Launch};
use crate::security::registry::parse_version;

/// How long a version probe may take before its process group is killed.
pub(crate) const PROBE_DEADLINE: Duration = Duration::from_secs(30);

/// The most of a marker or version file that is read.
const FILE_CAP: u64 = 1024 * 1024;

/// One tool's detection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolState {
    /// The registry name.
    pub tool: &'static str,
    #[serde(flatten)]
    pub detection: Detection,
}

/// Whether a tool can run here. A state that is not [`Installed`] never
/// reads as 0 % or as clean ([BR-77]).
///
/// [`Installed`]: Detection::Installed
/// [BR-77]: ../../../docs/specs/software-spec.md#323-coverage--test-evidence
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Detection {
    /// The project has no marker of the tool's ecosystem.
    NotApplicable,
    /// Switched off by the repository's `[coverage_ingest].tools`; nothing was
    /// looked for.
    Disabled,
    /// No copy of the tool could be found, or it lacks what a coverage run
    /// needs; `hint` says what to do when there is more to say than "install
    /// it".
    NotInstalled {
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<&'static str>,
    },
    /// Found. `version` is `None` when it could not be read.
    Installed {
        binary: PathBuf,
        version: Option<String>,
        origin: Origin,
    },
}

/// Which copy of a tool runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The user-level `path` override.
    Override,
    /// The project's wrapper (`./mvnw`, `./gradlew`).
    Wrapper,
    /// The project's own install: `node_modules/.bin`, or the `.venv`.
    Project,
    /// The usable `PATH`.
    Path,
    /// The copy `logos coverage install` installed in the user-level tools
    /// directory (S-748).
    Managed,
}

/// The not-installed hint of a JS tool missing from `node_modules/.bin`.
pub(crate) const HINT_INSTALL_DEPENDENCIES: &str = "install the project's dependencies first";
/// The not-installed hint of a vitest without a coverage provider.
pub(crate) const HINT_VITEST_PROVIDER: &str =
    "no vitest coverage provider (@vitest/coverage-v8 or @vitest/coverage-istanbul) is installed";
/// The not-installed hint of a pytest without the pytest-cov plugin.
pub(crate) const HINT_PYTEST_COV: &str = "pytest is installed but its pytest-cov plugin is not";
/// The not-installed hint of a user-level `path` override that cannot run.
pub(crate) const HINT_OVERRIDE: &str = "the user-level path override is not an executable file";
/// The not-installed hint of a tool Logos can install (`cargo-llvm-cov`): the
/// command that does, with the `llvm-tools-preview` component it needs.
pub const HINT_COVERAGE_INSTALL: &str =
    "not on PATH: `logos coverage install --consent` installs it, with the llvm-tools-preview component";

/// Where a detection looks: the project, the user's override, and the
/// usable `PATH` and the `PATH` a probe runs with.
pub(crate) struct Lookup<'a> {
    pub root: &'a Path,
    pub override_path: Option<&'a Path>,
    pub search_dirs: &'a [PathBuf],
    pub child_path: Option<&'a OsString>,
    pub probes: Probes,
    /// The user-level tools directory a managed copy is found in; `None` when
    /// there is none, or when it is inside the project.
    pub tools_dir: Option<&'a Path>,
}

/// Whether detection may run a copy's version command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probes {
    /// Ask a copy outside the project for its version: the CLI and MCP
    /// readouts, and a run once its consent is given.
    Every,
    /// Start nothing: a copy outside the project reads installed with its
    /// version unknown, as one whose probe failed does — the web page's read
    /// on the serve path, and what a run asks consent for ([ADR-38]).
    ///
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    Never,
}

/// Whether `tool`'s marker is at `root`.
pub(crate) fn applies(tool: &CoverageTool, root: &Path) -> bool {
    match tool.marker {
        Marker::AnyOf(files) => files.iter().any(|f| root.join(f).is_file()),
        Marker::Package(package) => read_json(&root.join("package.json")).is_some_and(|json| {
            ["dependencies", "devDependencies"].iter().any(|section| {
                json.get(section)
                    .and_then(|deps| deps.get(package))
                    .is_some()
            })
        }),
    }
}

/// The copy of an applicable, enabled `tool` that would run, or why none
/// would.
pub(crate) fn locate(tool: &CoverageTool, at: &Lookup<'_>) -> Detection {
    if let Some(path) = at.override_path {
        return if is_executable(path) {
            installed(path.to_path_buf(), probe(tool, path, at), Origin::Override)
        } else {
            Detection::NotInstalled {
                hint: Some(HINT_OVERRIDE),
            }
        };
    }
    match tool.locate {
        Locate::Path(name) => from_path(tool, at, name),
        Locate::Managed(name) => {
            let manifest = crate::tools::Manifest::builtin();
            let managed = at.tools_dir.and_then(|dir| crate::tools::resolve(dir, manifest, tool.name));
            if let Some(binary) = find_on_path(at.search_dirs, name) {
                let version = probe(tool, &binary, at);
                // A version that could not be read is not shown to meet the
                // minimum, so a managed copy wins over it. Under
                // `Probes::Never` none was read: the user's own copy is named,
                // as it is the one a probed run picks unless it turns out to
                // be below the minimum.
                if at.probes == Probes::Never
                    || manifest.meets_minimum(tool.name, version.as_deref())
                    || managed.is_none()
                {
                    return installed(binary, version, Origin::Path);
                }
            }
            match managed {
                Some(copy) => installed(copy.binary, Some(copy.version), Origin::Managed),
                None => Detection::NotInstalled {
                    hint: Some(HINT_COVERAGE_INSTALL),
                },
            }
        }
        Locate::NodeBin {
            bin,
            package,
            requires,
        } => {
            let modules = at.root.join("node_modules");
            let binary = modules.join(".bin").join(bin);
            if !is_executable(&binary) {
                return Detection::NotInstalled {
                    hint: Some(HINT_INSTALL_DEPENDENCIES),
                };
            }
            if !requires.is_empty()
                && !requires
                    .iter()
                    .any(|r| modules.join(r).join("package.json").is_file())
            {
                return Detection::NotInstalled {
                    hint: Some(HINT_VITEST_PROVIDER),
                };
            }
            let version = read_json(&modules.join(package).join("package.json"))
                .and_then(|json| json.get("version")?.as_str().and_then(parse_version));
            installed(binary, version, Origin::Project)
        }
        Locate::Wrapper {
            wrapper,
            properties,
            on_path: name,
        } => {
            let binary = at.root.join(wrapper);
            if is_executable(&binary) {
                let version = read_capped(&at.root.join(properties))
                    .and_then(|text| distribution_version(&text));
                return installed(binary, version, Origin::Wrapper);
            }
            from_path(tool, at, name)
        }
        Locate::Venv { bin, dist } => {
            let venv = at.root.join(".venv");
            let binary = venv.join("bin").join(bin);
            if is_executable(&binary) {
                return match dist_info_version(&venv, dist) {
                    Some(version) => installed(binary, Some(version), Origin::Project),
                    None => Detection::NotInstalled {
                        hint: Some(HINT_PYTEST_COV),
                    },
                };
            }
            let Some(binary) = find_on_path(at.search_dirs, bin) else {
                return Detection::NotInstalled { hint: None };
            };
            // pytest lists the plugins it loaded: one without pytest-cov's
            // entry cannot measure coverage. One whose probe failed, or that
            // was not probed, is kept, its version unknown.
            match run_probe(tool, &binary, at) {
                Some(output) => match version_in(tool, &output) {
                    Some(version) => installed(binary, Some(version), Origin::Path),
                    None => Detection::NotInstalled {
                        hint: Some(HINT_PYTEST_COV),
                    },
                },
                None => installed(binary, None, Origin::Path),
            }
        }
    }
}

/// The first executable `name` on the usable `PATH`, its version probed.
fn from_path(tool: &CoverageTool, at: &Lookup<'_>, name: &str) -> Detection {
    match find_on_path(at.search_dirs, name) {
        Some(binary) => {
            let version = probe(tool, &binary, at);
            installed(binary, version, Origin::Path)
        }
        None => Detection::NotInstalled { hint: None },
    }
}

fn installed(binary: PathBuf, version: Option<String>, origin: Origin) -> Detection {
    Detection::Installed {
        binary,
        version,
        origin,
    }
}

/// The version `binary`'s version command prints.
fn probe(tool: &CoverageTool, binary: &Path, at: &Lookup<'_>) -> Option<String> {
    run_probe(tool, binary, at).and_then(|output| version_in(tool, &output))
}

/// The working directory of a version probe. Maven, pytest, Go and cargo
/// each look for their config in the working directory **and every directory
/// above it**, so a probe run from the temporary directory would read a
/// `pytest.ini` or a `.mvn/` any local user planted in a shared `/tmp`. The
/// filesystem root has no directory above it, and only an administrator can
/// write there.
#[cfg(unix)]
const PROBE_CWD: &str = "/";
#[cfg(not(unix))]
const PROBE_CWD: &str = "\\";

/// What `binary`'s version command printed, stdout then stderr, when it ran
/// and succeeded: from [`PROBE_CWD`], under [`PROBE_DEADLINE`]. `None`, with
/// nothing started, under [`Probes::Never`].
fn run_probe(tool: &CoverageTool, binary: &Path, at: &Lookup<'_>) -> Option<String> {
    if at.probes == Probes::Never {
        return None;
    }
    let args: Vec<OsString> = tool.version_args.iter().map(OsString::from).collect();
    let launch = Launch {
        cwd: Path::new(PROBE_CWD),
        env: tool.probe_env,
        vars: &[],
        isolated: None,
        path: at.child_path,
        deadline: Some(PROBE_DEADLINE),
    };
    spawn::run(binary, &args, &launch)
        .ok()
        .filter(|f| f.status.is_some_and(|s| s.success()))
        .map(|f| format!("{}\n{}", f.stdout, f.stderr))
}

/// The version in a probe's `output`: the first version-shaped token after
/// the tool's [`version_after`](CoverageTool::version_after) text when it has
/// one, else anywhere.
pub(crate) fn version_in(tool: &CoverageTool, output: &str) -> Option<String> {
    let text = match tool.version_after {
        Some(after) => &output[output.find(after)? + after.len()..],
        None => output,
    };
    parse_version(text)
}

/// The version in a wrapper's `distributionUrl`: `…/gradle-8.10.2-bin.zip`,
/// `…/apache-maven-3.9.9-bin.zip`.
pub(crate) fn distribution_version(properties: &str) -> Option<String> {
    let url = properties
        .lines()
        .find_map(|line| line.trim().strip_prefix("distributionUrl"))?
        .trim_start()
        .strip_prefix('=')?;
    parse_version(url.rsplit('/').next()?)
}

/// The version of the `dist` distribution installed in the virtualenv at
/// `venv`: from its `<dist>-<version>.dist-info` directory, in any
/// `lib/python*/site-packages`.
fn dist_info_version(venv: &Path, dist: &str) -> Option<String> {
    let prefix = format!("{dist}-");
    fs::read_dir(venv.join("lib"))
        .ok()?
        .flatten()
        .filter(|python| python.file_name().to_string_lossy().starts_with("python"))
        .filter_map(|python| fs::read_dir(python.path().join("site-packages")).ok())
        .flat_map(|entries| entries.flatten())
        .find_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_prefix(&prefix)?.strip_suffix(".dist-info")?;
            parse_version(version)
        })
}

/// `path` as JSON, when it is a regular file of at most [`FILE_CAP`] bytes
/// that parses.
pub(crate) fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&read_capped(path)?).ok()
}

/// `path`'s text, when it is a regular file (never a symlink's target) of at
/// most [`FILE_CAP`] bytes.
fn read_capped(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > FILE_CAP {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(FILE_CAP)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}
