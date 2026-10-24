//! The built-in coverage tool registry ([FR-CV-11], [ADR-87] §1): seven
//! entries, each an ecosystem marker, a way to find the tool, a detection
//! probe, a **fixed** argv template and the report it writes.
//!
//! The registry is data, like the [scanner registry](crate::security::registry).
//! A template is a list of [`Arg`]s: literals compiled into this file, plus
//! placeholders filled only from values Logos chooses (the report path under
//! [`OUTPUT_DIR`], the init script it writes there) or from the user-level
//! config (the extra arguments). Nothing in the repository fills a placeholder:
//! its `[coverage_ingest].tools` can only switch an entry on or off
//! ([NFR-SE-08]). The project root is the child's working directory.
//!
//! Where each tool comes from ([`Locate`]):
//! - a project wrapper (`./mvnw`, `./gradlew`) is preferred over the `PATH`
//!   binary, since the consent already covers running the project's code
//!   (CR-234 CRA-02);
//! - vitest and jest run only from `node_modules/.bin`, never through `npx`,
//!   so a run never downloads a package;
//! - pytest comes from the project's `.venv` first, then `PATH`;
//! - `cargo-llvm-cov` resolves as a scanner does ([FR-CV-17], S-748): the
//!   user-level override, then a `PATH` copy at or above the
//!   [manifest](crate::tools::Manifest::coverage)'s minimum, then the copy
//!   `logos coverage install` manages. A user's own copy is used, and never
//!   downloaded beside it.
//!
//! `cargo-llvm-cov` runs with `CARGO_LLVM_COV_SETUP=no`: without the
//! `llvm-tools-preview` component it would otherwise run `rustup component
//! add` itself — a closed stdin reads as *yes* to its prompt — and change the
//! user's toolchain with no consent. With it, a missing component fails the
//! tool, naming the command; `logos coverage install` runs that command once
//! its consent names it.
//!
//! Tested versions — the command lines below were written against these
//! versions' documentation; none was run here, so the stubbed argv tests pin
//! the shapes and a tool whose flags have since changed reports *failed* with
//! its stderr: cargo-llvm-cov 0.6, Vitest 3.2, Jest 29, pytest-cov 5.0 (pytest
//! 8), Maven 3.9 with the JaCoCo plugin [`JACOCO_VERSION`], Gradle 8 (7 and
//! later), Go 1.23. cargo-llvm-cov 0.8.7's `--version` was read on the
//! development machine.
//!
//! What each template asks for beyond the coverage itself:
//! - **Vitest** writes coverage only when every test passes unless
//!   `--coverage.reportOnFailure` is given, so it is: a failing suite's report
//!   is still evidence (CR-234 CRA-01).
//! - **Jest** gets `--ci`, so a test meeting a new snapshot fails instead of
//!   writing one into the project.
//! - **Maven** attaches JaCoCo from the command line at a pinned version, and
//!   **Gradle** through an init script Logos ships ([`GRADLE_INIT_SCRIPT`],
//!   CR-234 CRA-04), so neither build file is edited. Gradle runs with
//!   `--no-daemon`, so no daemon outlives the run's process group. A failing
//!   test stops either build before its report task, so a failing Maven or
//!   Gradle suite writes no report and reads *failed* with its stderr.
//!
//! [FR-CV-11]: ../../../docs/specs/requirements/FR-CV-11.md
//! [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use crate::history::coverage::CoverageFormat;

/// Where every report a run writes lands, relative to the project root.
/// Maven and Gradle write theirs in each module's build directory instead
/// ([`Report::Modules`]); only the Gradle init script goes here for them.
pub const OUTPUT_DIR: &str = ".logos/coverage";

/// The JaCoCo Maven plugin version, as a literal `concat!` can take.
macro_rules! jacoco_version {
    () => {
        "0.8.13"
    };
}

/// The JaCoCo Maven plugin version the Maven template pins.
pub const JACOCO_VERSION: &str = jacoco_version!();

/// How long a tool may run when the user-level config sets no `timeout_secs`
/// (CR-234 CRA-03).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The Gradle init script the Gradle template passes with `--init-script`,
/// written to [`GRADLE_INIT_FILE`] before each run (CR-234 CRA-04).
pub const GRADLE_INIT_SCRIPT: &str = include_str!("jacoco-init.gradle");

/// The init script's file name under [`OUTPUT_DIR`].
pub const GRADLE_INIT_FILE: &str = "jacoco-init.gradle";

/// One element of an invocation template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arg {
    /// A literal compiled into the registry.
    Lit(&'static str),
    /// The report path Logos chose under [`OUTPUT_DIR`]: the file, or the
    /// directory of a [`Report::Dir`].
    Out,
    /// The literal and [`Arg::Out`] joined into one element:
    /// `-coverprofile=<path>`.
    OutJoined(&'static str),
    /// The user-level `extra_args`, one argv element each, at this fixed
    /// position.
    Extra,
    /// The [init script](GRADLE_INIT_SCRIPT) Logos wrote under [`OUTPUT_DIR`].
    InitScript,
}

use Arg::{Extra, InitScript, Lit, Out, OutJoined};

/// The file whose presence at the project root selects a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marker {
    /// Any of these files.
    AnyOf(&'static [&'static str]),
    /// A `package.json` that declares this package in its `dependencies` or
    /// `devDependencies`.
    Package(&'static str),
}

/// Where the binary that runs comes from, when no user-level `path` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Locate {
    /// The first executable match on the usable `PATH`.
    Path(&'static str),
    /// `node_modules/.bin/<bin>` only. Its version is read from
    /// `node_modules/<package>/package.json`, and it counts as installed only
    /// when one of `requires` — a coverage provider — is installed beside it
    /// (none required when empty).
    NodeBin {
        bin: &'static str,
        package: &'static str,
        requires: &'static [&'static str],
    },
    /// The project's executable `<wrapper>`, its version read from the
    /// distribution URL in `properties`; else `on_path` on the usable `PATH`.
    Wrapper {
        wrapper: &'static str,
        properties: &'static str,
        on_path: &'static str,
    },
    /// `.venv/bin/<bin>`, counted as installed only when the virtualenv holds
    /// the `dist` distribution (whose version it is); else `<bin>` on the
    /// usable `PATH`.
    Venv {
        bin: &'static str,
        dist: &'static str,
    },
    /// The first executable match on the usable `PATH` when its version is at
    /// or above the manifest's minimum — or when there is no managed copy to
    /// prefer, or no version is probed ([`Probes::Never`](super::Probes::Never))
    /// — else the managed copy in the tools directory: the scanners'
    /// precedence ([ADR-88] §5).
    ///
    /// [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
    Managed(&'static str),
}

/// The report a run writes, and so what it collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Report {
    /// [`Arg::Out`] is this file under [`OUTPUT_DIR`].
    File(&'static str),
    /// [`Arg::Out`] is this directory under [`OUTPUT_DIR`], and the tool
    /// writes `produced` in it.
    Dir {
        dir: &'static str,
        produced: &'static str,
    },
    /// Each module writes this file, relative to its own directory; the run
    /// collects every one it wrote.
    Modules(&'static str),
}

/// One registered coverage tool.
#[derive(Debug)]
pub struct CoverageTool {
    /// The registry name: the `[coverage_ingest].tools` key and what the
    /// execution consent names.
    pub name: &'static str,
    pub(crate) marker: Marker,
    pub(crate) locate: Locate,
    /// The version command's arguments, for a binary outside the project. It
    /// prints a version and reaches no network.
    pub(crate) version_args: &'static [&'static str],
    /// When set, the version is the one printed after this text: pytest
    /// lists its plugins, and pytest-cov is one of them (a pytest whose list
    /// lacks it does not have the plugin); Go spells its version `go1.23.1`.
    pub(crate) version_after: Option<&'static str>,
    /// Variables set on the version probe: `GOTOOLCHAIN=local`, so `go
    /// version` never fetches a toolchain.
    pub(crate) probe_env: &'static [(&'static str, &'static str)],
    /// Variables set on the run itself.
    pub(crate) run_env: &'static [(&'static str, &'static str)],
    pub(crate) run: &'static [Arg],
    pub(crate) report: Report,
    /// The format of the report it writes.
    pub format: CoverageFormat,
}

/// The seven registered tools, in [`COVERAGE_TOOLS`](crate::config::COVERAGE_TOOLS) order.
pub static REGISTRY: [CoverageTool; 7] = [
    CoverageTool {
        name: "cargo-llvm-cov",
        marker: Marker::AnyOf(&["Cargo.toml"]),
        locate: Locate::Managed("cargo-llvm-cov"),
        // Started as cargo starts a subcommand: its name first.
        version_args: &["llvm-cov", "--version"],
        version_after: None,
        probe_env: &[],
        // Never `rustup component add` on its own (module docs).
        run_env: &[("CARGO_LLVM_COV_SETUP", "no")],
        run: &[
            Lit("llvm-cov"),
            Lit("--workspace"),
            Lit("--cobertura"),
            Lit("--output-path"),
            Out,
            Extra,
        ],
        report: Report::File("rust.xml"),
        format: CoverageFormat::Cobertura,
    },
    CoverageTool {
        name: "vitest",
        marker: Marker::Package("vitest"),
        locate: Locate::NodeBin {
            bin: "vitest",
            package: "vitest",
            requires: &["@vitest/coverage-v8", "@vitest/coverage-istanbul"],
        },
        version_args: &["--version"],
        version_after: None,
        probe_env: &[],
        run_env: &[],
        run: &[
            Lit("run"),
            Lit("--coverage.enabled"),
            Lit("--coverage.reporter=cobertura"),
            OutJoined("--coverage.reportsDirectory="),
            Lit("--coverage.reportOnFailure"),
            Extra,
        ],
        report: Report::Dir {
            dir: "vitest",
            produced: "cobertura-coverage.xml",
        },
        format: CoverageFormat::Cobertura,
    },
    CoverageTool {
        name: "jest",
        marker: Marker::Package("jest"),
        locate: Locate::NodeBin {
            bin: "jest",
            package: "jest",
            requires: &[],
        },
        version_args: &["--version"],
        version_after: None,
        probe_env: &[],
        run_env: &[],
        run: &[
            Lit("--ci"),
            Lit("--coverage"),
            Lit("--coverageReporters=cobertura"),
            OutJoined("--coverageDirectory="),
            Extra,
        ],
        report: Report::Dir {
            dir: "jest",
            produced: "cobertura-coverage.xml",
        },
        format: CoverageFormat::Cobertura,
    },
    CoverageTool {
        name: "pytest-cov",
        marker: Marker::AnyOf(&["pyproject.toml", "setup.cfg", "pytest.ini"]),
        locate: Locate::Venv {
            bin: "pytest",
            dist: "pytest_cov",
        },
        // Given twice, `--version` also lists the plugins pytest loaded.
        version_args: &["--version", "--version"],
        version_after: Some("pytest-cov-"),
        probe_env: &[],
        run_env: &[],
        run: &[Lit("--cov"), OutJoined("--cov-report=xml:"), Extra],
        report: Report::File("python.xml"),
        format: CoverageFormat::Cobertura,
    },
    CoverageTool {
        name: "maven",
        marker: Marker::AnyOf(&["pom.xml"]),
        locate: Locate::Wrapper {
            wrapper: "mvnw",
            properties: ".mvn/wrapper/maven-wrapper.properties",
            on_path: "mvn",
        },
        version_args: &["--version"],
        version_after: None,
        probe_env: &[],
        run_env: &[],
        run: &[
            Lit("--batch-mode"),
            Extra,
            Lit(concat!(
                "org.jacoco:jacoco-maven-plugin:",
                jacoco_version!(),
                ":prepare-agent"
            )),
            Lit("test"),
            Lit(concat!(
                "org.jacoco:jacoco-maven-plugin:",
                jacoco_version!(),
                ":report"
            )),
        ],
        report: Report::Modules("target/site/jacoco/jacoco.xml"),
        format: CoverageFormat::Jacoco,
    },
    CoverageTool {
        name: "gradle",
        marker: Marker::AnyOf(&["build.gradle", "build.gradle.kts"]),
        locate: Locate::Wrapper {
            wrapper: "gradlew",
            properties: "gradle/wrapper/gradle-wrapper.properties",
            on_path: "gradle",
        },
        version_args: &["--version"],
        version_after: None,
        probe_env: &[],
        run_env: &[],
        run: &[
            Lit("--no-daemon"),
            Lit("--init-script"),
            InitScript,
            Extra,
            Lit("test"),
            Lit("jacocoTestReport"),
        ],
        report: Report::Modules("build/reports/jacoco/test/jacocoTestReport.xml"),
        format: CoverageFormat::Jacoco,
    },
    CoverageTool {
        name: "go",
        marker: Marker::AnyOf(&["go.mod"]),
        locate: Locate::Path("go"),
        version_args: &["version"],
        version_after: Some("go version go"),
        probe_env: &[("GOTOOLCHAIN", "local")],
        run_env: &[],
        run: &[
            Lit("test"),
            OutJoined("-coverprofile="),
            Extra,
            Lit("./..."),
        ],
        report: Report::File("go.out"),
        format: CoverageFormat::Gocover,
    },
];

/// The registered tool named `name`.
pub fn tool(name: &str) -> Option<&'static CoverageTool> {
    REGISTRY.iter().find(|t| t.name == name)
}

/// What fills a template's placeholders.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fill<'a> {
    /// [`Arg::Out`]: the report file, or a [`Report::Dir`]'s directory.
    pub out: &'a Path,
    pub extra: &'a [String],
    pub init_script: &'a Path,
}

impl CoverageTool {
    /// The argv `fill` renders this tool's template to, the program excluded.
    pub(crate) fn argv(&self, fill: &Fill<'_>) -> Vec<OsString> {
        let mut argv = Vec::with_capacity(self.run.len() + fill.extra.len());
        for arg in self.run {
            match *arg {
                Lit(s) => argv.push(OsString::from(s)),
                Out => argv.push(fill.out.as_os_str().to_owned()),
                OutJoined(flag) => {
                    let mut joined = OsString::from(flag);
                    joined.push(fill.out.as_os_str());
                    argv.push(joined);
                }
                Extra => argv.extend(fill.extra.iter().map(OsString::from)),
                InitScript => argv.push(fill.init_script.as_os_str().to_owned()),
            }
        }
        argv
    }
}
