//! Tests for the coverage tool registry and runner ([S-735], [FR-CV-11],
//! [NFR-SE-08]).
//!
//! No real coverage tool runs here. Every tool is a stub shell script — in a
//! temporary `bin/` that is the **only** entry of the search path, or where
//! the project keeps it (`node_modules/.bin`, `./mvnw`, `.venv/bin`) — so a
//! tool installed on this machine is never found. A stub records each
//! invocation's working directory and argv, answers its version command, and
//! copies its fixture project's committed report
//! (`tests/fixtures/coverage/projects/<tool>/`) to where its argv says the
//! report goes. The tests assert the argv against a literal copy of each
//! template and ingest every report the run collects.
//!
//! [S-735]: ../../../docs/planning/journal.md#s-735-a-coverage-tool-registry-detects-each-ecosystem-and-runs-its-tool-with-a-fixed-argv
//! [FR-CV-11]: ../../../docs/specs/requirements/FR-CV-11.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::detect::{
    distribution_version, version_in, HINT_COVERAGE_INSTALL, HINT_INSTALL_DEPENDENCIES, HINT_OVERRIDE,
    HINT_PYTEST_COV, HINT_VITEST_PROVIDER,
};
use super::registry::{tool, GRADLE_INIT_SCRIPT, JACOCO_VERSION, REGISTRY};
use super::{CoverageRunner, Detection, Origin, RunOutcome};
use crate::config::{ConfigError, EffectiveCoverage, EffectiveCoverageIngest, COVERAGE_TOOLS};
use crate::history::coverage::ingest;

/// The version text each stub prints for its version command, in the shape
/// the real tool prints it, and the version detection must read from it.
const VERSIONS: &[(&str, &str, &str)] = &[
    ("cargo-llvm-cov", "cargo-llvm-cov 0.6.13", "0.6.13"),
    ("vitest", "vitest/3.2.4 darwin-arm64 node-v22.11.0", "3.2.4"),
    ("jest", "29.7.0", "29.7.0"),
    (
        "pytest-cov",
        "This is pytest version 8.3.2, imported from /usr/lib/python3/pytest/__init__.py\n\
         registered third-party plugins:\n  pytest-cov-5.0.0 at /usr/lib/python3/pytest_cov/plugin.py",
        "5.0.0",
    ),
    (
        "maven",
        "Apache Maven 3.9.9 (8e8579a9e76f7d015ee5ec7bfcdc97d260186937)\nMaven home: /opt/maven\n\
         Java version: 21.0.4, vendor: Eclipse Adoptium",
        "3.9.9",
    ),
    (
        "gradle",
        "\n------------------------------------------------------------\nGradle 8.10.2\n\
         ------------------------------------------------------------\n\nBuild time:    2024-09-23 21:28:39 UTC\n\
         Kotlin:        1.9.24\nGroovy:        3.0.22",
        "8.10.2",
    ),
    ("go", "go version go1.23.1 darwin/arm64", "1.23.1"),
];

/// Each template's argv, written out literally (`{out}` is the project's
/// `.logos/coverage`, `--x1 --x2` the user-level `extra_args` at the
/// template's fixed position), so a change to a template has to change this
/// too.
const ARGV: &[(&str, &[&str])] = &[
    (
        "cargo-llvm-cov",
        &[
            "llvm-cov",
            "--workspace",
            "--cobertura",
            "--output-path",
            "{out}/rust.xml",
            "--x1",
            "--x2",
        ],
    ),
    (
        "vitest",
        &[
            "run",
            "--coverage.enabled",
            "--coverage.reporter=cobertura",
            "--coverage.reportsDirectory={out}/vitest",
            "--coverage.reportOnFailure",
            "--x1",
            "--x2",
        ],
    ),
    (
        "jest",
        &[
            "--ci",
            "--coverage",
            "--coverageReporters=cobertura",
            "--coverageDirectory={out}/jest",
            "--x1",
            "--x2",
        ],
    ),
    (
        "pytest-cov",
        &["--cov", "--cov-report=xml:{out}/python.xml", "--x1", "--x2"],
    ),
    (
        "maven",
        &[
            "--batch-mode",
            "--x1",
            "--x2",
            "org.jacoco:jacoco-maven-plugin:0.8.13:prepare-agent",
            "test",
            "org.jacoco:jacoco-maven-plugin:0.8.13:report",
        ],
    ),
    (
        "gradle",
        &[
            "--no-daemon",
            "--init-script",
            "{out}/jacoco-init.gradle",
            "--x1",
            "--x2",
            "test",
            "jacocoTestReport",
        ],
    ),
    (
        "go",
        &["test", "-coverprofile={out}/go.out", "--x1", "--x2", "./..."],
    ),
];

/// The shell that copies the fixture report(s) to where the argv says, per
/// tool. `$fixture` is the fixture project directory.
const WRITES: &[(&str, &str)] = &[
    (
        "cargo-llvm-cov",
        r#"prev=; for a in "$@"; do [ "$prev" = --output-path ] && /bin/cp "$fixture/fixture-report.xml" "$a"; prev=$a; done"#,
    ),
    (
        "vitest",
        r#"for a in "$@"; do case $a in --coverage.reportsDirectory=*) d=${a#*=}; /bin/mkdir -p "$d"; /bin/cp "$fixture/fixture-report.xml" "$d/cobertura-coverage.xml";; esac; done"#,
    ),
    (
        "jest",
        r#"for a in "$@"; do case $a in --coverageDirectory=*) d=${a#*=}; /bin/mkdir -p "$d"; /bin/cp "$fixture/fixture-report.xml" "$d/cobertura-coverage.xml";; esac; done"#,
    ),
    (
        "pytest-cov",
        r#"for a in "$@"; do case $a in --cov-report=xml:*) /bin/cp "$fixture/fixture-report.xml" "${a#--cov-report=xml:}";; esac; done"#,
    ),
    (
        "maven",
        r#"for m in core api; do /bin/mkdir -p "$m/target/site/jacoco"; /bin/cp "$fixture/fixture-report-$m.xml" "$m/target/site/jacoco/jacoco.xml"; done"#,
    ),
    (
        "gradle",
        r#"prev=; for a in "$@"; do [ "$prev" = --init-script ] && init=$a; prev=$a; done
/usr/bin/grep -q "apply plugin: 'jacoco'" "$init" || { echo 'no init script' >&2; exit 3; }
for m in app lib; do /bin/mkdir -p "$m/build/reports/jacoco/test"; /bin/cp "$fixture/fixture-report-$m.xml" "$m/build/reports/jacoco/test/jacocoTestReport.xml"; done"#,
    ),
    (
        "go",
        r#"for a in "$@"; do case $a in -coverprofile=*) /bin/cp "$fixture/fixture-report.out" "${a#-coverprofile=}";; esac; done"#,
    ),
];

fn row<T: Copy>(table: &[(&str, T)], name: &str) -> T {
    table
        .iter()
        .find(|r| r.0 == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .1
}

fn version(name: &str) -> (&'static str, &'static str) {
    let r = VERSIONS.iter().find(|r| r.0 == name).expect("listed");
    (r.1, r.2)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/coverage/projects")
        .join(name)
}

fn write_executable(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary world: a project root, a stub `bin/` that is the whole search
/// path, a record directory, and a user-level config directory.
struct World {
    tmp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    rec: PathBuf,
    user: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp root");
        let base = tmp.path().canonicalize().unwrap();
        let (root, bin, rec, user) = (
            base.join("project"),
            base.join("bin"),
            base.join("rec"),
            base.join("user"),
        );
        for dir in [&root, &bin, &rec, &user] {
            fs::create_dir_all(dir).unwrap();
        }
        Self {
            tmp,
            root,
            bin,
            rec,
            user,
        }
    }

    /// A world whose project is a copy of the fixture project `name`.
    fn with_project(name: &str) -> Self {
        let w = Self::new();
        copy_tree(&fixture(name), &w.root);
        w
    }

    /// Put a stub of `tool` at `path`, recording under `record`: it answers
    /// the version command, and otherwise copies the fixture report where its
    /// argv says, then runs `extra` and exits `exit`.
    fn stub_at(&self, path: &Path, tool: &str, record: &str, exit: i32, extra: &str) {
        let t = super::registry::tool(tool).expect("registered");
        let (text, _) = version(tool);
        let body = format!(
            "#!/bin/sh\nfixture='{fixture}'\n{{ printf 'cwd=%s\\n' \"$(pwd -P)\"; printf 'GOTOOLCHAIN=%s\\n' \"${{GOTOOLCHAIN-}}\"; printf 'CARGO_LLVM_COV_SETUP=%s\\n' \"${{CARGO_LLVM_COV_SETUP-}}\"; printf 'PATH=%s\\n' \"$PATH\"; \
             for a in \"$@\"; do printf 'arg=%s\\n' \"$a\"; done; printf -- '--\\n'; }} >> '{rec}'\n\
             if [ \"$*\" = '{version_args}' ]; then printf '%s\\n' '{text}'; exit 0; fi\n{writes}\n{extra}\nexit {exit}\n",
            fixture = fixture(tool).display(),
            rec = self.rec.join(record).display(),
            version_args = t.version_args.join(" "),
            text = text.replace('\'', "'\\''"),
            writes = row(WRITES, tool),
        );
        write_executable(path, &body);
    }

    /// A stub of `tool` on the search path, under `binary`.
    fn stub(&self, tool: &str, binary: &str) {
        self.stub_at(&self.bin.join(binary), tool, tool, 0, "");
    }

    /// Install `package` at `version` under the project's `node_modules`.
    fn node_package(&self, package: &str, version: &str) {
        write(
            &self
                .root
                .join("node_modules")
                .join(package)
                .join("package.json"),
            &format!("{{\"name\": \"{package}\", \"version\": \"{version}\"}}"),
        );
    }

    /// Each invocation of the stub recorded under `record`, as its lines.
    fn invocations(&self, record: &str) -> Vec<Vec<String>> {
        fs::read_to_string(self.rec.join(record))
            .unwrap_or_default()
            .split("--\n")
            .filter(|s| !s.is_empty())
            .map(|s| s.lines().map(str::to_string).collect())
            .collect()
    }

    /// The argv of each invocation recorded under `record`.
    fn argvs(&self, record: &str) -> Vec<Vec<String>> {
        self.invocations(record)
            .into_iter()
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(|l| l.strip_prefix("arg=").map(str::to_string))
                    .collect()
            })
            .collect()
    }

    fn user_config(&self, toml: &str) {
        write(&self.user.join("config.toml"), toml);
    }

    fn repo_config(&self, toml: &str) {
        write(&self.root.join(".logos/config.toml"), toml);
    }

    fn open(&self) -> Result<CoverageRunner, ConfigError> {
        CoverageRunner::open_with(
            &self.root,
            Some(&self.user),
            Some(self.bin.clone().into_os_string()),
        )
    }

    fn detection(&self, tool: &str) -> Detection {
        let states = self.open().unwrap().detect();
        states
            .into_iter()
            .find(|s| s.tool == tool)
            .expect("every tool has a state")
            .detection
    }

    /// Commit the project, so an ingest has a HEAD to anchor to.
    fn commit(&self) {
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["add", "-A"],
            &["commit", "-q", "-m", "fixture"],
        ] {
            self.git(args);
        }
    }

    /// Run git in the project, kept from every enclosing repository and from
    /// git's background maintenance, and return its stdout.
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
            .args(args)
            .env("GIT_CEILING_DIRECTORIES", self.tmp.path())
            .output()
            .expect("git is on PATH");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// What a run sees of this world: the user-level directory, and the stub
    /// `bin/` as the whole search path.
    fn env(&self) -> crate::security::ScanEnvironment {
        crate::security::ScanEnvironment {
            user_dir: Some(self.user.clone()),
            search_path: Some(self.bin.clone().into_os_string()),
            tools_dir: None,
            nvd_api_key: None,
        }
    }

    /// `coverage run` over this world as `request` asks, each step handed to
    /// `progress`, each report ingested against the project's source files.
    fn run_with(
        &self,
        request: &super::RunRequest<'_>,
        progress: impl FnMut(&super::RunProgress),
    ) -> anyhow::Result<super::CoverageRunSummary> {
        let indexed = self.indexed();
        super::run(&self.root, &self.env(), request, progress, |report: &Path, format: Option<crate::history::coverage::CoverageFormat>, cfg: &EffectiveCoverageIngest| {
            ingest(&self.root, report, format, &EffectiveCoverage::default(), cfg, &indexed)
        })
    }

    /// The project's source files, as the index would list them.
    fn indexed(&self) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![self.root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.file_type().unwrap().is_dir() {
                    if ![".git", ".logos", "node_modules", "target", "build", ".venv"]
                        .contains(&name.as_str())
                    {
                        pending.push(path);
                    }
                } else if ["rs", "js", "py", "java", "go"]
                    .iter()
                    .any(|e| path.extension().is_some_and(|x| x == *e))
                {
                    found.push(
                        path.strip_prefix(&self.root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        found.sort();
        found
    }
}

/// The command a run's consent refusal names.
const ENTRY: &str = "logos coverage run";

/// What a failing build or package manager prints: a secret it must leave in
/// its log only.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

/// `tool`'s outcome in `summary`.
fn outcome<'a>(summary: &'a super::CoverageRunSummary, tool: &str) -> &'a super::ToolOutcome {
    &summary.tools.iter().find(|t| t.tool == tool).unwrap_or_else(|| panic!("{tool} has an outcome")).outcome
}

/// Install `tool` where the template runs it from, as a stub that exits
/// `exit`, plus whatever it needs beside it.
fn install(w: &World, tool: &str, exit: i32) {
    match tool {
        "vitest" | "jest" => {
            w.stub_at(
                &w.root.join("node_modules/.bin").join(tool),
                tool,
                tool,
                exit,
                "",
            );
            w.node_package(tool, version(tool).1);
            if tool == "vitest" {
                w.node_package("@vitest/coverage-v8", "3.2.4");
            }
        }
        // The wrapper is preferred over the PATH binary: both are there.
        "maven" => {
            w.stub_at(&w.root.join("mvnw"), tool, tool, exit, "");
            w.stub_at(&w.bin.join("mvn"), tool, "mvn-on-path", exit, "");
        }
        "pytest-cov" => w.stub_at(&w.bin.join("pytest"), tool, tool, exit, ""),
        _ => w.stub_at(&w.bin.join(tool), tool, tool, exit, ""),
    }
}

fn expected_argv(w: &World, tool: &str) -> Vec<String> {
    let out = w.root.join(".logos/coverage");
    row(ARGV, tool)
        .iter()
        .map(|a| a.replace("{out}", &out.display().to_string()))
        .collect()
}

#[test]
fn the_registry_holds_the_seven_tools_in_config_order() {
    let names: Vec<&str> = REGISTRY.iter().map(|t| t.name).collect();
    assert_eq!(names, COVERAGE_TOOLS);
    for table in [
        VERSIONS.iter().map(|r| r.0).collect::<Vec<_>>(),
        ARGV.iter().map(|r| r.0).collect(),
    ] {
        assert_eq!(table, COVERAGE_TOOLS, "every tool is pinned here");
    }
    assert!(
        tool("custom").is_none(),
        "the custom command is not a registry entry"
    );
    assert_eq!(JACOCO_VERSION, "0.8.13");
}

/// AC1: each applicable tool is installed with its version or not installed;
/// a tool whose marker is absent is not applicable — and nothing reaches the
/// network, because nothing that could is ever started: the stubs are the
/// only programs found.
#[test]
fn detection_reports_installed_with_version_not_installed_and_not_applicable() {
    let w = World::new();
    write(&w.root.join("Cargo.toml"), "[package]\nname = \"x\"\n");
    write(&w.root.join("go.mod"), "module example.com/x\n");
    write(&w.root.join("pom.xml"), "<project/>");
    write(
        &w.root.join("package.json"),
        r#"{"devDependencies": {"jest": "^29"}}"#,
    );
    w.stub("cargo-llvm-cov", "cargo-llvm-cov");
    w.stub("go", "go");
    install(&w, "jest", 0);

    let states = w.open().unwrap().detect();
    let got: Vec<(&str, &Detection)> = states.iter().map(|s| (s.tool, &s.detection)).collect();
    let installed = |tool: &str, binary: PathBuf, origin| Detection::Installed {
        binary,
        version: Some(version(tool).1.to_string()),
        origin,
    };
    assert_eq!(
        got,
        [
            (
                "cargo-llvm-cov",
                &installed("cargo-llvm-cov", w.bin.join("cargo-llvm-cov"), Origin::Path)
            ),
            ("vitest", &Detection::NotApplicable),
            (
                "jest",
                &installed(
                    "jest",
                    w.root.join("node_modules/.bin/jest"),
                    Origin::Project
                )
            ),
            ("pytest-cov", &Detection::NotApplicable),
            ("maven", &Detection::NotInstalled { hint: None }),
            ("gradle", &Detection::NotApplicable),
            ("go", &installed("go", w.bin.join("go"), Origin::Path)),
        ]
    );
    // A tool that does not apply is never looked for, let alone started.
    let bare = World::new();
    bare.stub("go", "go");
    assert!(bare
        .open()
        .unwrap()
        .detect()
        .iter()
        .all(|s| s.detection == Detection::NotApplicable));
    assert!(bare.invocations("go").is_empty(), "nothing was probed");
}

/// The detection probe runs from the filesystem root — outside the project,
/// and below no directory where anyone else could plant a config the tool
/// searches upward for — and `go version` with `GOTOOLCHAIN=local`, so a
/// `go.mod` toolchain line can never make it fetch a toolchain; a disabled
/// tool is not probed.
#[test]
fn a_probe_runs_outside_the_project_and_a_disabled_tool_is_not_probed() {
    let w = World::new();
    write(
        &w.root.join("go.mod"),
        "module example.com/x\n\ngo 1.21\n\ntoolchain go1.99.0\n",
    );
    write(&w.root.join("Cargo.toml"), "[package]\nname = \"x\"\n");
    w.stub("go", "go");
    w.stub("cargo-llvm-cov", "cargo-llvm-cov");
    w.repo_config("[coverage_ingest]\ntools = { cargo-llvm-cov = false }\n");

    assert_eq!(w.detection("cargo-llvm-cov"), Detection::Disabled);
    assert!(
        w.invocations("cargo-llvm-cov").is_empty(),
        "a disabled tool is not probed"
    );
    assert!(matches!(w.detection("go"), Detection::Installed { .. }));
    let probe = &w.invocations("go")[0];
    let cwd = probe.iter().find_map(|l| l.strip_prefix("cwd=")).unwrap();
    assert_eq!(
        cwd, "/",
        "probed from {cwd}: only the filesystem root has no directory above it"
    );
    assert!(
        probe.contains(&"GOTOOLCHAIN=local".to_string()),
        "{probe:?}"
    );
}

/// Detection runs before any consent, so it never starts a binary the
/// project supplies: the wrapper's, the `node_modules`' and the `.venv`'s
/// versions are read from files beside them.
#[test]
fn detection_never_runs_a_binary_the_project_supplies() {
    let w = World::new();
    write(&w.root.join("pom.xml"), "<project/>");
    write(&w.root.join("build.gradle"), "");
    write(&w.root.join("pyproject.toml"), "[project]\nname = \"x\"\n");
    write(
        &w.root.join("package.json"),
        r#"{"devDependencies": {"vitest": "^3", "@vitest/coverage-v8": "^3"}}"#,
    );
    w.stub_at(&w.root.join("mvnw"), "maven", "mvnw", 0, "");
    write(
        &w.root.join(".mvn/wrapper/maven-wrapper.properties"),
        "wrapperVersion=3.3.2\ndistributionUrl=https://repo.maven.apache.org/maven2/org/apache/maven/apache-maven/3.9.6/apache-maven-3.9.6-bin.zip\n",
    );
    w.stub_at(&w.root.join("gradlew"), "gradle", "gradlew", 0, "");
    write(
        &w.root.join("gradle/wrapper/gradle-wrapper.properties"),
        "distributionBase=GRADLE_USER_HOME\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-8.10.2-bin.zip\n",
    );
    w.stub_at(
        &w.root.join(".venv/bin/pytest"),
        "pytest-cov",
        "venv-pytest",
        0,
        "",
    );
    fs::create_dir_all(
        w.root
            .join(".venv/lib/python3.12/site-packages/pytest_cov-5.0.0.dist-info"),
    )
    .unwrap();
    install(&w, "vitest", 0);
    // The same tools on PATH too: the project's copies still win.
    w.stub("maven", "mvn");
    w.stub("gradle", "gradle");
    w.stub("pytest-cov", "pytest");

    let found = |tool: &str| match w.detection(tool) {
        Detection::Installed {
            binary,
            version,
            origin,
        } => (binary, version, origin),
        other => panic!("{tool}: {other:?}"),
    };
    assert_eq!(
        found("maven"),
        (w.root.join("mvnw"), Some("3.9.6".into()), Origin::Wrapper)
    );
    assert_eq!(
        found("gradle"),
        (
            w.root.join("gradlew"),
            Some("8.10.2".into()),
            Origin::Wrapper
        )
    );
    assert_eq!(
        found("pytest-cov"),
        (
            w.root.join(".venv/bin/pytest"),
            Some("5.0.0".into()),
            Origin::Project
        )
    );
    assert_eq!(
        found("vitest"),
        (
            w.root.join("node_modules/.bin/vitest"),
            Some("3.2.4".into()),
            Origin::Project
        )
    );
    for record in [
        "mvnw",
        "gradlew",
        "venv-pytest",
        "vitest",
        "maven",
        "gradle",
        "pytest-cov",
    ] {
        assert!(
            w.invocations(record).is_empty(),
            "{record} was started by a detection"
        );
    }
}

/// What a coverage run needs beyond the binary: `node_modules`, a vitest
/// coverage provider, pytest's pytest-cov plugin — each missing one reads as
/// not installed, with what to do.
#[test]
fn a_tool_without_what_a_run_needs_is_not_installed_with_a_hint() {
    let w = World::new();
    write(
        &w.root.join("package.json"),
        r#"{"devDependencies": {"vitest": "^3", "jest": "^29"}}"#,
    );
    write(&w.root.join("setup.cfg"), "[metadata]\nname = x\n");
    let hint = |h: &'static str| Detection::NotInstalled { hint: Some(h) };
    assert_eq!(
        w.detection("vitest"),
        hint(HINT_INSTALL_DEPENDENCIES),
        "no node_modules"
    );
    assert_eq!(w.detection("jest"), hint(HINT_INSTALL_DEPENDENCIES));
    assert_eq!(
        w.detection("pytest-cov"),
        Detection::NotInstalled { hint: None }
    );

    w.stub_at(
        &w.root.join("node_modules/.bin/vitest"),
        "vitest",
        "vitest",
        0,
        "",
    );
    w.node_package("vitest", "3.2.4");
    // A package that only shares the prefix is not a coverage provider.
    w.node_package("@vitest/coverage-v8-shim", "1.0.0");
    assert_eq!(w.detection("vitest"), hint(HINT_VITEST_PROVIDER));

    // pytest on PATH without the plugin; then a .venv pytest without it.
    write_executable(
        &w.bin.join("pytest"),
        "#!/bin/sh\nprintf 'This is pytest version 8.3.2\\nregistered third-party plugins:\\n  pytest-xdist-3.6.1 at /x\\n'\n",
    );
    assert_eq!(w.detection("pytest-cov"), hint(HINT_PYTEST_COV));
    w.stub_at(
        &w.root.join(".venv/bin/pytest"),
        "pytest-cov",
        "venv-pytest",
        0,
        "",
    );
    fs::create_dir_all(
        w.root
            .join(".venv/lib/python3.12/site-packages/pytest_xdist-3.6.1.dist-info"),
    )
    .unwrap();
    assert_eq!(w.detection("pytest-cov"), hint(HINT_PYTEST_COV));
}

/// The `package.json` marker is a declared dependency of that exact name:
/// its near misses select nothing.
#[test]
fn a_package_marker_ignores_its_near_misses() {
    for (label, json) in [
        (
            "a prefixed package",
            r#"{"devDependencies": {"vitest-mock": "1", "jest-cli": "29"}}"#,
        ),
        (
            "a scoped sibling",
            r#"{"devDependencies": {"@vitest/ui": "3", "@jest/core": "29"}}"#,
        ),
        (
            "a script naming it",
            r#"{"scripts": {"test": "vitest run && jest"}}"#,
        ),
        (
            "a peer dependency",
            r#"{"peerDependencies": {"vitest": "3", "jest": "29"}}"#,
        ),
        ("malformed JSON", r#"{"devDependencies": {"vitest": "3""#),
    ] {
        let w = World::new();
        write(&w.root.join("package.json"), json);
        assert_eq!(w.detection("vitest"), Detection::NotApplicable, "{label}");
        assert_eq!(w.detection("jest"), Detection::NotApplicable, "{label}");
    }
    let w = World::new();
    write(
        &w.root.join("package.json"),
        r#"{"dependencies": {"jest": "29"}}"#,
    );
    assert_ne!(
        w.detection("jest"),
        Detection::NotApplicable,
        "a runtime dependency counts too"
    );
}

#[test]
fn versions_are_read_from_each_tools_own_spelling() {
    for (name, text, expected) in VERSIONS {
        assert_eq!(
            version_in(tool(name).unwrap(), text).as_deref(),
            Some(*expected),
            "{name}"
        );
    }
    let go = tool("go").unwrap();
    assert_eq!(
        version_in(go, "go version go1.22 linux/amd64").as_deref(),
        Some("1.22")
    );
    assert_eq!(version_in(go, "go version devel +abc linux/amd64"), None);
    let pytest = tool("pytest-cov").unwrap();
    assert_eq!(
        version_in(pytest, "This is pytest version 8.3.2"),
        None,
        "pytest's own version is not pytest-cov's"
    );
    assert_eq!(
        distribution_version("distributionUrl = https://x/apache-maven-3.9.9-bin.zip\n").as_deref(),
        Some("3.9.9")
    );
    assert_eq!(
        distribution_version("wrapperUrl=https://x/maven-wrapper-3.3.2.jar\n"),
        None
    );
}

/// AC2 (UAT-CV-06): a committed `[coverage_ingest]` carrying a path, a
/// command or a flag key fails to load before anything is spawned; the same
/// override in the user-level config takes effect — the binary it names
/// runs, with the extra arguments at the template's position.
#[test]
fn a_committed_override_fails_before_any_spawn_and_the_user_level_one_takes_effect() {
    let w = World::with_project("go");
    let custom = w.tmp.path().canonicalize().unwrap().join("tools/go-custom");
    w.stub_at(&custom, "go", "go-custom", 0, "");
    w.stub("go", "go");

    for committed in [
        format!(
            "[coverage_ingest.tools.go]\npath = \"{}\"\n",
            custom.display()
        ),
        "[coverage_ingest.tools]\ngo = \"go test -run X ./...\"\n".to_string(),
        "[coverage_ingest]\nextra_args = [\"-race\"]\n".to_string(),
    ] {
        w.repo_config(&committed);
        let err = w.open().expect_err("a committed override fails the load");
        assert_eq!(err.exit_code(), 2, "{err}");
    }
    assert!(
        w.invocations("go-custom").is_empty() && w.invocations("go").is_empty(),
        "nothing was spawned"
    );

    fs::remove_file(w.root.join(".logos/config.toml")).unwrap();
    w.user_config(&format!(
        "[coverage_ingest.tools.go]\npath = \"{}\"\nextra_args = [\"-race\", \"-count=1\"]\ntimeout_secs = 120\n",
        custom.display()
    ));
    let runner = w.open().unwrap();
    assert_eq!(
        runner.timeout(tool("go").unwrap()),
        Duration::from_secs(120)
    );
    assert!(matches!(
        w.detection("go"),
        Detection::Installed { binary, origin: Origin::Override, .. } if binary == custom
    ));
    let outcome = runner.run_tool("go").unwrap();
    assert!(matches!(outcome, RunOutcome::Ran { .. }), "{outcome:?}");
    let out = w.root.join(".logos/coverage/go.out");
    assert_eq!(
        w.argvs("go-custom").last().unwrap(),
        &[
            "test".to_string(),
            format!("-coverprofile={}", out.display()),
            "-race".into(),
            "-count=1".into(),
            "./...".into()
        ]
    );
    assert!(
        w.invocations("go").is_empty(),
        "the PATH go never runs while the override is set"
    );

    // An override that cannot run is not silently replaced by another copy.
    w.user_config("[coverage_ingest.tools.go]\npath = \"/nonexistent/go\"\n");
    assert_eq!(
        w.detection("go"),
        Detection::NotInstalled {
            hint: Some(HINT_OVERRIDE)
        }
    );
    // And a relative one fails the user-level load.
    w.user_config("[coverage_ingest.tools.go]\npath = \"bin/go\"\n");
    assert_eq!(w.open().unwrap_err().exit_code(), 2);
}

/// AC3, against stubs: each template, run on its fixture project, starts its
/// tool in the project root with the literal argv above, and the report its
/// fixture holds — the shape the real tool writes for that project — lands
/// where the argv says and ingests, every report path bound to a file of the
/// project. That the real tool writes that report from that argv is the
/// ignored [`real_tools_against_their_fixture_projects`] run's to show.
#[test]
fn each_template_argv_puts_its_fixture_report_where_ingest_reads_it() {
    for t in &REGISTRY {
        let w = World::with_project(t.name);
        w.commit();
        install(&w, t.name, 0);
        // The user's extra arguments, at each template's fixed position.
        w.user_config(&format!(
            "[coverage_ingest.tools.{}]\nextra_args = [\"--x1\", \"--x2\"]\n",
            t.name
        ));
        let runner = w.open().unwrap();
        assert_eq!(runner.timeout(t), Duration::from_secs(30 * 60), "the default timeout");
        let outcome = runner.run_tool(t.name).unwrap();
        let RunOutcome::Ran { reports } = outcome else {
            panic!("{}: {outcome:?}", t.name)
        };

        let runs = w.argvs(t.name);
        assert_eq!(
            runs.last().unwrap(),
            &expected_argv(&w, t.name),
            "{}'s argv",
            t.name
        );
        let cwd = w.invocations(t.name).last().unwrap()[0].clone();
        assert_eq!(
            cwd,
            format!("cwd={}", w.root.display()),
            "{} runs in the project root",
            t.name
        );
        let path = format!("PATH={}", w.bin.display());
        assert!(
            w.invocations(t.name).last().unwrap().contains(&path),
            "{} runs with the usable search path only",
            t.name
        );
        let modules = if matches!(t.name, "maven" | "gradle") {
            2
        } else {
            1
        };
        assert_eq!(reports.len(), modules, "{}: {reports:?}", t.name);

        let indexed = w.indexed();
        for report in &reports {
            let summary = ingest(
                &w.root,
                report,
                Some(t.format),
                &EffectiveCoverage::default(),
                &EffectiveCoverageIngest::default(),
                &indexed,
            )
            .unwrap_or_else(|e| panic!("{}: {} does not ingest: {e:#}", t.name, report.display()));
            assert!(summary.matched_files >= 1, "{}: {summary:?}", t.name);
            assert!(summary.unmatched.is_empty(), "{}: {summary:?}", t.name);
        }
    }
}

/// The Gradle template attaches JaCoCo through the init script Logos writes
/// under `.logos/coverage/`, and no build file changes (CR-234 CRA-04).
#[test]
fn the_gradle_init_script_attaches_jacoco_without_editing_the_build() {
    let w = World::with_project("gradle");
    let builds = ["settings.gradle", "build.gradle", "app/build.gradle"];
    let before: Vec<Vec<u8>> = builds
        .iter()
        .map(|f| fs::read(w.root.join(f)).unwrap())
        .collect();
    // A file a repository committed in the script's place is replaced, never
    // written through.
    let elsewhere = w.tmp.path().join("elsewhere.gradle");
    fs::write(&elsewhere, "untouched").unwrap();
    fs::create_dir_all(w.root.join(".logos/coverage")).unwrap();
    std::os::unix::fs::symlink(
        &elsewhere,
        w.root.join(".logos/coverage/jacoco-init.gradle"),
    )
    .unwrap();
    install(&w, "gradle", 0);

    let outcome = w.open().unwrap().run_tool("gradle").unwrap();
    assert!(matches!(outcome, RunOutcome::Ran { .. }), "{outcome:?}");
    let script = w.root.join(".logos/coverage/jacoco-init.gradle");
    assert_eq!(fs::read_to_string(&script).unwrap(), GRADLE_INIT_SCRIPT);
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "untouched");
    let after: Vec<Vec<u8>> = builds
        .iter()
        .map(|f| fs::read(w.root.join(f)).unwrap())
        .collect();
    assert_eq!(before, after, "no build file was edited");
    for needed in [
        "plugins.withId('java')",
        "apply plugin: 'jacoco'",
        "xml.required.set(true)",
        "dependsOn tasks.named('test')",
    ] {
        assert!(
            GRADLE_INIT_SCRIPT.contains(needed),
            "the init script lacks {needed}"
        );
    }
}

/// A non-zero exit with a report is *ran — tests failed* and the report is
/// kept; without one it is *failed* with the end of what it printed; an exit 0 with
/// no report is *failed* too, never *ran*.
#[test]
fn a_failing_suite_keeps_its_report_and_a_missing_report_is_a_failure() {
    let w = World::with_project("cargo-llvm-cov");
    w.stub_at(
        &w.bin.join("cargo-llvm-cov"),
        "cargo-llvm-cov",
        "cargo-llvm-cov",
        101,
        "",
    );
    let outcome = w.open().unwrap().run_tool("cargo-llvm-cov").unwrap();
    assert_eq!(
        outcome,
        RunOutcome::TestsFailed {
            exit_code: Some(101),
            reports: vec![w.root.join(".logos/coverage/rust.xml")],
            cause: None,
        }
    );

    let w = World::with_project("go");
    let silent =
        "#!/bin/sh\n[ \"$1\" = version ] && { echo 'go version go1.23.1 linux/amd64'; exit 0; }\n";
    write_executable(
        &w.bin.join("go"),
        &format!("{silent}echo 'FAIL example.com/calc [build failed]' >&2\nexit 2\n"),
    );
    let outcome = w.open().unwrap().run_tool("go").unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Failed {
            exit_code: Some(2),
            stderr: "FAIL example.com/calc [build failed]\n".to_string(),
            cause: None
        }
    );
    write_executable(&w.bin.join("go"), &format!("{silent}exit 0\n"));
    assert!(matches!(
        w.open().unwrap().run_tool("go").unwrap(),
        RunOutcome::Failed {
            exit_code: Some(0),
            ..
        }
    ));
}

/// A report an earlier run left is never credited to this one: the report
/// file is cleared first, and a module report the run did not write or
/// rewrite is not collected — however recently it was written.
#[test]
fn a_stale_report_is_never_credited() {
    let w = World::with_project("go");
    let stale = w.root.join(".logos/coverage/go.out");
    write(&stale, "mode: set\nexample.com/calc/calc.go:3.24,5.2 1 1\n");
    write_executable(
        &w.bin.join("go"),
        "#!/bin/sh\n[ \"$1\" = version ] && echo 'go version go1.23.1'\nexit 0\n",
    );
    assert!(matches!(
        w.open().unwrap().run_tool("go").unwrap(),
        RunOutcome::Failed { .. }
    ));
    assert!(
        !stale.exists(),
        "the earlier report was cleared before the run"
    );

    let w = World::with_project("maven");
    let old = w.root.join("core/target/site/jacoco/jacoco.xml");
    write(&old, "stale");
    let past = fs::File::options().write(true).open(&old).unwrap();
    past.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    // This run writes only the api module's report.
    let body = "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'Apache Maven 3.9.9'; exit 0; }\n\
                /bin/mkdir -p api/target/site/jacoco && : > api/target/site/jacoco/jacoco.xml\nexit 0\n";
    write_executable(&w.bin.join("mvn"), body);
    let outcome = w.open().unwrap().run_tool("maven").unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Ran {
            reports: vec![w.root.join("api/target/site/jacoco/jacoco.xml")]
        }
    );
}

/// AC4: the per-tool timeout kills the tool's whole process group — a
/// grandchild it left running included — and the run reads *timed out*.
#[test]
fn the_timeout_kills_the_whole_process_group() {
    let w = World::with_project("go");
    let pids = w.rec.join("pids");
    let hang = format!(
        "/bin/sleep 60 &\necho $$ $! > '{}'\n/bin/sleep 60\n",
        pids.display()
    );
    w.stub_at(&w.bin.join("go"), "go", "go", 0, &hang);
    w.user_config("[coverage_ingest.tools.go]\ntimeout_secs = 1\n");

    let start = Instant::now();
    let outcome = w.open().unwrap().run_tool("go").unwrap();
    assert_eq!(outcome, RunOutcome::TimedOut { after_secs: 1 });
    assert!(
        start.elapsed() < crate::perf::scaled(Duration::from_secs(20)),
        "{:?}",
        start.elapsed()
    );
    let recorded = fs::read_to_string(&pids).expect("the tool started its grandchild");
    for pid in recorded
        .split_whitespace()
        .map(|p| p.parse::<u32>().unwrap())
    {
        assert!(
            crate::jobs::tests::gone(pid),
            "process {pid} survived the timeout"
        );
    }
}

/// A tool that cannot run starts nothing, a name the registry does not hold
/// is refused, and a symlinked `.logos/coverage` is refused before any spawn.
#[test]
fn nothing_starts_for_a_tool_that_cannot_run_or_an_output_that_escapes() {
    let w = World::with_project("go");
    let runner = w.open().unwrap();
    assert_eq!(
        runner.run_tool("go").unwrap(),
        RunOutcome::NotRun {
            detection: Detection::NotInstalled { hint: None }
        }
    );
    assert_eq!(
        runner.run_tool("cargo-llvm-cov").unwrap(),
        RunOutcome::NotRun {
            detection: Detection::NotApplicable
        }
    );
    assert_eq!(runner.run_tool("custom").unwrap_err().exit_code(), 2);

    w.stub("go", "go");
    let outside = w.tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(w.root.join(".logos")).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join(".logos/coverage")).unwrap();
    let outcome = w.open().unwrap().run_tool("go").unwrap();
    assert!(
        matches!(&outcome, RunOutcome::Failed { stderr, .. } if stderr.contains("symbolic link")),
        "{outcome:?}"
    );
    assert_eq!(
        w.argvs("go").len(),
        1,
        "only the version probe ran: {:?}",
        w.argvs("go")
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    // `.logos` itself a symlink: refused the same way.
    let w = World::with_project("go");
    w.stub("go", "go");
    let outside = w.tmp.path().join("outside-logos");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join(".logos")).unwrap();
    let outcome = w.open().unwrap().run_tool("go").unwrap();
    assert!(
        matches!(&outcome, RunOutcome::Failed { stderr, .. } if stderr.contains("symbolic link")),
        "{outcome:?}"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "nothing written through .logos");
}

/// The owner's hand run (CR-234 CRA-04 included): every real tool found on
/// `PATH` or in the project runs its template against its fixture project,
/// and its reports must ingest. A tool not found is reported as not run,
/// never as passed, and a run where no tool ran fails. Maven, Gradle and the
/// JS tools fetch dependencies, so this never runs in the default suite.
#[test]
#[ignore = "runs the real coverage tools, which may reach the network: the owner's hand run"]
fn real_tools_against_their_fixture_projects() {
    assert!(
        std::env::var_os("LOGOS_COVERAGE_REAL_TOOLS").is_some(),
        "set LOGOS_COVERAGE_REAL_TOOLS=1 to run the real coverage tools (see the fixture README)"
    );
    let mut verdicts = Vec::new();
    for t in &REGISTRY {
        let w = World::with_project(t.name);
        w.commit();
        let runner =
            CoverageRunner::open_with(&w.root, Some(&w.user), std::env::var_os("PATH")).unwrap();
        let verdict = match runner.run_tool(t.name).unwrap() {
            RunOutcome::NotRun { detection } => format!("NOT RUN ({detection:?})"),
            RunOutcome::Ran { reports } | RunOutcome::TestsFailed { reports, .. } => {
                let indexed = w.indexed();
                let ingested: Result<Vec<_>, _> = reports
                    .iter()
                    .map(|r| {
                        ingest(
                            &w.root,
                            r,
                            Some(t.format),
                            &EffectiveCoverage::default(),
                            &EffectiveCoverageIngest::default(),
                            &indexed,
                        )
                    })
                    .collect();
                match ingested {
                    Ok(s) if s.iter().all(|s| s.matched_files > 0) => {
                        format!("PASS ({} report(s))", s.len())
                    }
                    Ok(s) => format!("FAIL (nothing matched: {s:?})"),
                    Err(e) => format!("FAIL (ingest: {e:#})"),
                }
            }
            other => format!("FAIL ({other:?})"),
        };
        verdicts.push((t.name, verdict));
    }
    for (name, verdict) in &verdicts {
        println!("{name:16} {verdict}");
    }
    assert!(
        verdicts.iter().all(|(_, v)| !v.starts_with("FAIL")),
        "{verdicts:#?}"
    );
    assert!(
        verdicts.iter().any(|(_, v)| v.starts_with("PASS")),
        "no tool ran: nothing was verified"
    );
}

/// A cancel while the tool runs kills its group and reads *cancelled*, and no
/// report a module wrote before it is credited; a run cancelled before it
/// starts starts nothing.
#[test]
fn a_cancelled_run_reads_cancelled_and_credits_nothing() {
    let w = World::with_project("maven");
    let body = "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'Apache Maven 3.9.9'; exit 0; }\n\
                /bin/mkdir -p core/target/site/jacoco && : > core/target/site/jacoco/jacoco.xml\n\
                : > started\n/bin/sleep 30\nexit 0\n";
    write_executable(&w.bin.join("mvn"), body);
    let runner = w.open().unwrap();
    let started = w.root.join("started");
    let begun = Instant::now();
    let outcome = crate::jobs::foreground(|| {
        // This run's cancel alone: `cancel_foreground` would reach every
        // foreground run of the test binary.
        let run = crate::jobs::this_run().expect("a foreground run");
        let canceller = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !started.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            run.cancel();
        });
        let outcome = runner.run_tool("maven").unwrap();
        canceller.join().unwrap();
        outcome
    });
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(
        begun.elapsed() < crate::perf::scaled(Duration::from_secs(20)),
        "{:?}",
        begun.elapsed()
    );

    let w = World::with_project("go");
    w.stub("go", "go");
    let runner = w.open().unwrap();
    let outcome = crate::jobs::foreground(|| {
        crate::jobs::this_run().expect("a foreground run").cancel();
        runner.run_tool("go").unwrap()
    });
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(w.argvs("go").iter().all(|a| a.first().map(String::as_str) == Some("version")), "{:?}", w.argvs("go"));
}

/// A module report written in the same second just before the run — a
/// hand-run `mvn verify` — is not this run's: a run that writes nothing reads
/// *failed*. One the run rewrites is credited.
#[test]
fn a_module_report_written_just_before_the_run_is_not_credited() {
    let w = World::with_project("maven");
    let report = w.root.join("core/target/site/jacoco/jacoco.xml");
    write(&report, "written just before");
    let version = "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'Apache Maven 3.9.9'; exit 0; }\n";
    write_executable(&w.bin.join("mvn"), &format!("{version}echo '[ERROR] COMPILATION ERROR' >&2\nexit 1\n"));
    let outcome = w.open().unwrap().run_tool("maven").unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { exit_code: Some(1), .. }), "{outcome:?}");

    let rewrite = "/bin/sleep 0.05\nprintf rewritten > core/target/site/jacoco/jacoco.xml\nexit 0\n";
    write_executable(&w.bin.join("mvn"), &format!("{version}{rewrite}"));
    let outcome = w.open().unwrap().run_tool("maven").unwrap();
    assert_eq!(outcome, RunOutcome::Ran { reports: vec![report] });
}

/// The per-module walk collects every module's report however deep (to its
/// bound), in path order, and never one under a directory no module lives in
/// (`node_modules`, `build`), one that is a symlink, or one reached through a
/// symlinked directory.
#[test]
fn the_module_walk_collects_real_reports_only_in_path_order() {
    let w = World::with_project("maven");
    let outside = w.tmp.path().canonicalize().unwrap().join("outside");
    let body = format!(
        "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'Apache Maven 3.9.9'; exit 0; }}\n\
         mk() {{ /bin/mkdir -p \"$1/target/site/jacoco\" && printf x > \"$1/target/site/jacoco/jacoco.xml\"; }}\n\
         mk zeta; mk alpha/one/two; mk node_modules/pkg; mk build/generated\n\
         mk '{out}/linked'; /bin/ln -s '{out}/linked' linked\n\
         printf x > '{out}/report.xml'; /bin/mkdir -p lib/target/site/jacoco\n\
         /bin/ln -s '{out}/report.xml' lib/target/site/jacoco/jacoco.xml\nexit 0\n",
        out = outside.display()
    );
    fs::create_dir_all(&outside).unwrap();
    write_executable(&w.bin.join("mvn"), &body);
    let outcome = w.open().unwrap().run_tool("maven").unwrap();
    let report = |module: &str| w.root.join(module).join("target/site/jacoco/jacoco.xml");
    assert_eq!(outcome, RunOutcome::Ran { reports: vec![report("alpha/one/two"), report("zeta")] });
}


/// A directory report (vitest, jest) an earlier run left is cleared before
/// the run, so a run that writes nothing reads *failed*; and a symlink
/// committed in place of the report directory is replaced by a real one,
/// never written or cleared through.
#[test]
fn a_directory_report_is_cleared_and_never_reached_through_a_link() {
    let w = World::with_project("jest");
    let silent = "#!/bin/sh\nexit 0\n";
    write_executable(&w.root.join("node_modules/.bin/jest"), silent);
    w.node_package("jest", "29.7.0");
    let stale = w.root.join(".logos/coverage/jest/cobertura-coverage.xml");
    write(&stale, "from an earlier run");
    let outcome = w.open().unwrap().run_tool("jest").unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { exit_code: Some(0), .. }), "{outcome:?}");
    assert!(!stale.exists(), "the earlier report was cleared");

    let w = World::with_project("vitest");
    write_executable(&w.root.join("node_modules/.bin/vitest"), silent);
    w.node_package("vitest", "3.2.4");
    w.node_package("@vitest/coverage-v8", "3.2.4");
    let outside = w.tmp.path().join("outside");
    write(&outside.join("cobertura-coverage.xml"), "precious");
    fs::create_dir_all(w.root.join(".logos/coverage")).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join(".logos/coverage/vitest")).unwrap();
    let outcome = w.open().unwrap().run_tool("vitest").unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { exit_code: Some(0), .. }), "{outcome:?}");
    assert_eq!(fs::read_to_string(outside.join("cobertura-coverage.xml")).unwrap(), "precious");
    let dir = fs::symlink_metadata(w.root.join(".logos/coverage/vitest")).unwrap();
    assert!(dir.is_dir(), "the link was replaced by a real directory");
}

/// Every alternative a marker or a requirement lists counts on its own, and
/// the files detection reads are taken only as they are: a `.dist-info` that
/// merely starts with the distribution's name is another distribution, and a
/// `package.json` that is a symlink or past the size cap selects nothing.
#[test]
fn each_alternative_counts_and_only_real_files_are_read() {
    for (marker, tool) in [("pytest.ini", "pytest-cov"), ("setup.cfg", "pytest-cov"), ("build.gradle.kts", "gradle")] {
        let w = World::new();
        write(&w.root.join(marker), "");
        assert_ne!(w.detection(tool), Detection::NotApplicable, "{marker} selects {tool}");
    }

    let w = World::new();
    write(&w.root.join("package.json"), r#"{"devDependencies": {"vitest": "^3"}}"#);
    w.stub_at(&w.root.join("node_modules/.bin/vitest"), "vitest", "vitest", 0, "");
    w.node_package("vitest", "3.2.4");
    w.node_package("@vitest/coverage-istanbul", "3.2.4");
    assert!(matches!(w.detection("vitest"), Detection::Installed { .. }), "the istanbul provider is one");

    let w = World::new();
    write(&w.root.join("pyproject.toml"), "");
    w.stub_at(&w.root.join(".venv/bin/pytest"), "pytest-cov", "venv-pytest", 0, "");
    fs::create_dir_all(w.root.join(".venv/lib/python3.12/site-packages/pytest_cov_extra-1.0.dist-info")).unwrap();
    assert_eq!(w.detection("pytest-cov"), Detection::NotInstalled { hint: Some(HINT_PYTEST_COV) });

    let w = World::new();
    let outside = w.tmp.path().join("package.json");
    fs::write(&outside, r#"{"devDependencies": {"vitest": "^3"}}"#).unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join("package.json")).unwrap();
    assert_eq!(w.detection("vitest"), Detection::NotApplicable, "a symlinked package.json");
    let w = World::new();
    let padding = " ".repeat(1024 * 1024);
    write(&w.root.join("package.json"), &format!(r#"{{"devDependencies": {{"vitest": "^3"}}}}{padding}"#));
    assert_eq!(w.detection("vitest"), Detection::NotApplicable, "a package.json past the cap");
}

/// `coverage run` (S-737), over this module's stub harness.
#[path = "run_tests.rs"]
mod run_tests;

/// A `cargo-llvm-cov` stub printing `version`, at `path`.
fn llvm_cov_at(path: &Path, version: &str) {
    write_executable(path, &format!("#!/bin/sh\necho 'cargo-llvm-cov {version}'\n"));
}

/// `cargo-llvm-cov` resolves as a scanner does ([FR-CV-17], S-748): the
/// override, then a `PATH` copy at or above the manifest's minimum (or with no
/// managed copy to prefer), then the managed copy under the tools directory
/// at a version the manifest accepts — never one inside the project.
///
/// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
#[test]
fn cargo_llvm_cov_resolves_override_then_path_at_the_minimum_then_the_managed_copy() {
    let w = World::new();
    write(&w.root.join("Cargo.toml"), "[package]\nname = \"x\"\n");
    let tools = w.tmp.path().join("tools");
    let entry = crate::tools::Manifest::builtin().coverage_tool("cargo-llvm-cov").unwrap();
    let managed = tools.join("cargo-llvm-cov").join(&entry.version).join("cargo-llvm-cov");
    let detect = |tools_dir: Option<PathBuf>| {
        let runner = w.open().unwrap().with_tools_dir(tools_dir);
        runner.detect().into_iter().find(|s| s.tool == "cargo-llvm-cov").unwrap().detection
    };
    let at = |binary: &Path, version: &str, origin| Detection::Installed {
        binary: binary.to_path_buf(),
        version: Some(version.to_string()),
        origin,
    };

    // Neither: the hint names the install command.
    assert_eq!(detect(Some(tools.clone())), Detection::NotInstalled { hint: Some(HINT_COVERAGE_INSTALL) });
    assert!(HINT_COVERAGE_INSTALL.contains("logos coverage install"));
    // A copy below the minimum, or at no dotted version, is no managed copy.
    llvm_cov_at(&tools.join("cargo-llvm-cov/0.5.0/cargo-llvm-cov"), "0.5.0");
    llvm_cov_at(&tools.join("cargo-llvm-cov/latest/cargo-llvm-cov"), "0.9.1");
    assert_eq!(detect(Some(tools.clone())), Detection::NotInstalled { hint: Some(HINT_COVERAGE_INSTALL) });

    llvm_cov_at(&managed, &entry.version);
    assert_eq!(detect(Some(tools.clone())), at(&managed, &entry.version, Origin::Managed));
    assert_eq!(detect(None), Detection::NotInstalled { hint: Some(HINT_COVERAGE_INSTALL) }, "no tools directory");

    // A PATH copy below the minimum loses to the managed copy, and is used
    // only while there is none.
    let own = w.bin.join("cargo-llvm-cov");
    llvm_cov_at(&own, "0.5.9");
    assert_eq!(detect(Some(tools.clone())), at(&managed, &entry.version, Origin::Managed));
    assert_eq!(detect(None), at(&own, "0.5.9", Origin::Path));
    // At the minimum, the user's own copy wins.
    llvm_cov_at(&own, &entry.min_version);
    assert_eq!(detect(Some(tools.clone())), at(&own, &entry.min_version, Origin::Path));
    // A read that probes nothing (the web page) names the copy a probed run
    // picks here, its version unknown — not the managed copy beside it.
    let unprobed = w.open().unwrap().with_tools_dir(Some(tools.clone()));
    let state = unprobed.detect_probing(super::Probes::Never).into_iter().find(|s| s.tool == "cargo-llvm-cov").unwrap();
    assert_eq!(state.detection, Detection::Installed { binary: own.clone(), version: None, origin: Origin::Path });
    // The override wins over both.
    let pinned = w.tmp.path().join("own/cargo-llvm-cov");
    llvm_cov_at(&pinned, "0.6.13");
    w.user_config(&format!("[coverage_ingest.tools.cargo-llvm-cov]\npath = \"{}\"\n", pinned.display()));
    assert_eq!(detect(Some(tools.clone())), at(&pinned, "0.6.13", Origin::Override));

    // A tools directory inside the project would let it plant a "managed"
    // copy: it is ignored.
    let w = World::new();
    write(&w.root.join("Cargo.toml"), "[package]\nname = \"x\"\n");
    let planted = w.root.join("tools");
    llvm_cov_at(&planted.join("cargo-llvm-cov").join(&entry.version).join("cargo-llvm-cov"), &entry.version);
    let runner = w.open().unwrap().with_tools_dir(Some(planted));
    assert_eq!(
        runner.copy_in_use("cargo-llvm-cov", super::Probes::Every),
        Some(Detection::NotInstalled { hint: Some(HINT_COVERAGE_INSTALL) })
    );
}

/// `cargo-llvm-cov` runs with `CARGO_LLVM_COV_SETUP=no`, so a missing
/// `llvm-tools-preview` fails it instead of letting it run `rustup component
/// add` with no consent; no other tool gets the variable.
#[test]
fn cargo_llvm_cov_never_adds_a_toolchain_component_on_its_own() {
    let w = World::with_project("cargo-llvm-cov");
    w.stub("cargo-llvm-cov", "cargo-llvm-cov");
    assert!(matches!(w.open().unwrap().run_tool("cargo-llvm-cov").unwrap(), RunOutcome::Ran { .. }));
    let run = w.invocations("cargo-llvm-cov").pop().unwrap();
    assert!(run.contains(&"CARGO_LLVM_COV_SETUP=no".to_string()), "{run:?}");

    let w = World::with_project("go");
    w.stub("go", "go");
    assert!(matches!(w.open().unwrap().run_tool("go").unwrap(), RunOutcome::Ran { .. }));
    let run = w.invocations("go").pop().unwrap();
    // Logos adds nothing for `go`: it sees exactly what this process inherits
    // (empty on a plain test run, `no` when the suite itself runs under
    // cargo-llvm-cov, which sets the variable for its tests).
    let inherited = std::env::var("CARGO_LLVM_COV_SETUP").unwrap_or_default();
    assert!(run.contains(&format!("CARGO_LLVM_COV_SETUP={inherited}")), "{run:?}");
}

/// The coverage dev-dependency addition (S-749), over this module's stub harness.
#[path = "deps_tests.rs"]
mod deps_tests;
