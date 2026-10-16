//! The built-in scanner registry ([FR-SC-05], [ADR-80]): nine entries, each a
//! detection probe and a **fixed** SARIF invocation template.
//!
//! The registry is data. A template is a list of [`Arg`]s: literals compiled
//! into this file, plus placeholders filled only from values Logos chooses (the
//! output path under `.logos/security/`, the CodeQL language and database from
//! [`CODEQL_LANGUAGES`], an empty config file it writes itself), from the
//! user-level config (the extra flags), or, in a later scoped run, from the
//! user's own path arguments. Nothing in the repository fills a placeholder:
//! the scan root is the child's working directory, spelled `.` in the argv
//! ([NFR-SE-08]).
//!
//! Tested versions — the command lines below were written against these, and a
//! scanner whose flags have since changed reports *failed* with its stderr:
//! Semgrep 1.85, Trivy 0.74, Gitleaks 8.18, OSV-Scanner 1.9, Bandit 1.7.10,
//! gosec 2.21, Checkov 3.2, CodeQL 2.19, Dependency-Check 10.0.
//!
//! [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// One element of an invocation template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arg {
    /// A literal compiled into the registry.
    Lit(&'static str),
    /// The output path Logos chose under `.logos/security/` (a file or, for
    /// [`Output::Dir`], a directory).
    Out,
    /// The user-level `extra_args`, one argv element each, at this fixed
    /// position (before any positional argument, so flag parsers that stop at
    /// the first positional still read them).
    Extra,
    /// The fixed path position ([`PathSlot`]): the full-scan paths, or a scoped
    /// run's paths.
    Paths,
    /// `--language=<id>` for the CodeQL language being analysed.
    Language,
    /// The CodeQL database directory Logos chose under `.logos/security/`.
    Database,
    /// An empty config file Logos writes under `.logos/security/` before the
    /// run, so the scanner never auto-loads one from the repository.
    PinnedConfig,
}

use Arg::{Database, Extra, Language, Lit, Out, Paths, PinnedConfig};

/// Where a scanner writes its SARIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    /// [`Arg::Out`] is the SARIF file itself.
    File,
    /// [`Arg::Out`] is a directory the scanner writes `produced` into; the
    /// orchestrator moves it to `<scanner>.sarif` next to the others.
    Dir { produced: &'static str },
}

/// The fixed path position of a path-capable scanner ([FR-SC-17]): what fills
/// [`Arg::Paths`] in a full scan, and the flag each path of a scoped run
/// carries, if the scanner needs one.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PathSlot {
    pub full: &'static [&'static str],
    pub per_path_flag: Option<&'static str>,
}

/// One registered scanner.
#[derive(Debug)]
pub struct Scanner {
    /// The registry name: the `[security_scan.<name>]` key and the output stem.
    pub name: &'static str,
    /// The `tool.driver.name` its SARIF carries — the name the ingest stores
    /// ([S-663]'s `scanner_name`).
    ///
    /// [S-663]: ../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted
    pub driver: &'static str,
    /// Executable names looked up on `PATH`, first match wins.
    pub(crate) binaries: &'static [&'static str],
    /// The version command's arguments. Prints a version; reaches no network.
    pub(crate) version_args: &'static [&'static str],
    /// Variables set on every spawn of this scanner: update-check and metrics
    /// opt-outs, so neither the probe nor the run phones home on Logos's behalf.
    pub(crate) env: &'static [(&'static str, &'static str)],
    /// CodeQL only: the database-creation step run once per language before
    /// [`run`](Self::run). Empty for every other scanner.
    pub(crate) prepare: &'static [Arg],
    /// The SARIF-producing step.
    pub(crate) run: &'static [Arg],
    pub(crate) output: Output,
    /// Exit codes that mean "ran" (findings included).
    pub(crate) ok_exit: &'static [i32],
    pub(crate) path_slot: Option<PathSlot>,
    /// Repository files that would make the scanner run code they name (a
    /// config auto-loaded from the scan root). When one is present at the root
    /// the scanner is not started ([NFR-SE-02]).
    ///
    /// [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
    pub(crate) refuse_if_present: &'static [&'static str],
}

/// The nine registered scanners, in [`SECURITY_SCANNERS`](crate::config::SECURITY_SCANNERS) order.
pub static REGISTRY: [Scanner; 9] = [
    Scanner {
        name: "semgrep",
        driver: "Semgrep OSS",
        binaries: &["semgrep"],
        version_args: &["--version"],
        env: &[("SEMGREP_ENABLE_VERSION_CHECK", "0"), ("SEMGREP_SEND_METRICS", "off")],
        prepare: &[],
        run: &[
            Lit("scan"),
            Lit("--config"),
            Lit("p/default"),
            Lit("--sarif"),
            Lit("--output"),
            Out,
            Lit("--metrics"),
            Lit("off"),
            Lit("--disable-version-check"),
            Lit("--quiet"),
            Lit("--exclude"),
            Lit(".logos"),
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["."], per_path_flag: None }),
        refuse_if_present: &[],
    },
    Scanner {
        name: "trivy",
        driver: "Trivy",
        binaries: &["trivy"],
        version_args: &["--version"],
        env: &[("TRIVY_SKIP_VERSION_CHECK", "true")],
        prepare: &[],
        run: &[
            Lit("fs"),
            Lit("--scanners"),
            Lit("vuln,secret,misconfig"),
            Lit("--format"),
            Lit("sarif"),
            Lit("--output"),
            Out,
            Lit("--skip-dirs"),
            Lit(".logos"),
            // Without `--config`, Trivy reads `trivy.yaml` from the working
            // directory: a repository could name the DB, cache or module hosts.
            Lit("--config"),
            PinnedConfig,
            Extra,
            Lit("."),
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: None,
        refuse_if_present: &[],
    },
    Scanner {
        name: "gitleaks",
        driver: "gitleaks",
        binaries: &["gitleaks"],
        version_args: &["version"],
        env: &[],
        prepare: &[],
        run: &[
            Lit("detect"),
            Lit("--redact"),
            Lit("--no-banner"),
            Lit("--exit-code"),
            Lit("0"),
            Lit("--report-format"),
            Lit("sarif"),
            Lit("--report-path"),
            Out,
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["--source", "."], per_path_flag: Some("--source") }),
        refuse_if_present: &[],
    },
    Scanner {
        name: "osv-scanner",
        driver: "osv-scanner",
        binaries: &["osv-scanner"],
        version_args: &["--version"],
        env: &[],
        prepare: &[],
        run: &[
            Lit("--format"),
            Lit("sarif"),
            Lit("--output"),
            Out,
            Extra,
            Lit("--recursive"),
            Lit("."),
        ],
        output: Output::File,
        // 1: vulnerabilities were found, which is a successful scan.
        ok_exit: &[0, 1],
        path_slot: None,
        refuse_if_present: &[],
    },
    Scanner {
        name: "bandit",
        driver: "Bandit",
        binaries: &["bandit"],
        version_args: &["--version"],
        env: &[],
        prepare: &[],
        run: &[
            Lit("--recursive"),
            Lit("--format"),
            Lit("sarif"),
            Lit("--output"),
            Out,
            Lit("--exit-zero"),
            Lit("--quiet"),
            Lit("--exclude"),
            Lit("./.logos"),
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["."], per_path_flag: None }),
        refuse_if_present: &[],
    },
    Scanner {
        name: "gosec",
        driver: "gosec",
        binaries: &["gosec"],
        version_args: &["-version"],
        // Never fetch and run the toolchain a repository's `go.mod` names.
        env: &[("GOTOOLCHAIN", "local")],
        prepare: &[],
        run: &[
            Lit("-fmt"),
            Lit("sarif"),
            Lit("-out"),
            Out,
            Lit("-no-fail"),
            Lit("-quiet"),
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["./..."], per_path_flag: None }),
        refuse_if_present: &[],
    },
    Scanner {
        name: "checkov",
        driver: "Checkov",
        binaries: &["checkov"],
        version_args: &["--version"],
        env: &[],
        prepare: &[],
        run: &[
            Lit("--output"),
            Lit("sarif"),
            Lit("--output-file-path"),
            Out,
            Lit("--soft-fail"),
            Lit("--quiet"),
            Lit("--skip-path"),
            Lit(".logos"),
            Extra,
            Paths,
        ],
        output: Output::Dir { produced: "results_sarif.sarif" },
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["--directory", "."], per_path_flag: Some("--file") }),
        // Checkov auto-loads this from the scanned directory, and it can name
        // external checks — Python that Checkov imports and runs.
        refuse_if_present: &[".checkov.yaml", ".checkov.yml"],
    },
    Scanner {
        name: "codeql",
        driver: "CodeQL",
        binaries: &["codeql"],
        version_args: &["version", "--format=terse"],
        env: &[],
        // `--build-mode=none` is a literal: no CodeQL run ever builds the
        // project ([NFR-SE-02]). A language without that mode is never passed.
        //
        // [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
        prepare: &[
            Lit("database"),
            Lit("create"),
            Database,
            Language,
            Lit("--build-mode=none"),
            Lit("--source-root=."),
            Lit("--overwrite"),
        ],
        run: &[
            Lit("database"),
            Lit("analyze"),
            Database,
            Lit("--format=sarif-latest"),
            Lit("--output"),
            Out,
            Extra,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: None,
        refuse_if_present: &[],
    },
    Scanner {
        name: "dependency-check",
        driver: "dependency-check",
        binaries: &["dependency-check", "dependency-check.sh"],
        version_args: &["--version"],
        env: &[],
        prepare: &[],
        run: &[
            Lit("--scan"),
            Lit("."),
            Lit("--format"),
            Lit("SARIF"),
            Lit("--out"),
            Out,
            Lit("--exclude"),
            Lit("**/.logos/**"),
            // These analyzers start the project's package manager (Yarn honours
            // a committed `yarnPath`), Go, bundle-audit or mix_audit.
            Lit("--disableYarnAudit"),
            Lit("--disablePnpmAudit"),
            Lit("--disableBundleAudit"),
            Lit("--disableGolangMod"),
            Lit("--disableMixAudit"),
            Extra,
        ],
        output: Output::Dir { produced: "dependency-check-report.sarif" },
        ok_exit: &[0],
        path_slot: None,
        refuse_if_present: &[],
    },
];

/// The registered scanner called `name`.
pub fn scanner(name: &str) -> Option<&'static Scanner> {
    REGISTRY.iter().find(|s| s.name == name)
}

/// The values a template's placeholders are filled from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fill<'a> {
    pub out: &'a Path,
    pub extra: &'a [String],
    /// `None` is a full scan; `Some` the paths of a scoped run.
    pub scope: Option<&'a [String]>,
    /// The CodeQL language id (only read by [`Arg::Language`]).
    pub language: &'a str,
    /// The CodeQL database directory (only read by [`Arg::Database`]).
    pub database: &'a Path,
    /// The empty config file (only read by [`Arg::PinnedConfig`]).
    pub pinned_config: &'a Path,
}

impl Scanner {
    /// Render one step's template into the argv handed to `Command::args`.
    pub(crate) fn render(&self, step: &[Arg], fill: &Fill<'_>) -> Vec<OsString> {
        let mut argv = Vec::with_capacity(step.len() + fill.extra.len());
        for arg in step {
            match *arg {
                Lit(s) => argv.push(OsString::from(s)),
                Out => argv.push(fill.out.as_os_str().to_owned()),
                Extra => argv.extend(fill.extra.iter().map(OsString::from)),
                Paths => {
                    let slot = self
                        .path_slot
                        .expect("a template with a path position declares its slot");
                    match fill.scope {
                        None => argv.extend(slot.full.iter().map(OsString::from)),
                        Some(paths) => {
                            for path in paths {
                                argv.extend(slot.per_path_flag.map(OsString::from));
                                argv.push(OsString::from(path));
                            }
                        }
                    }
                }
                Language => argv.push(OsString::from(format!("--language={}", fill.language))),
                Database => argv.push(fill.database.as_os_str().to_owned()),
                PinnedConfig => argv.push(fill.pinned_config.as_os_str().to_owned()),
            }
        }
        argv
    }

    /// Whether this scanner runs once per CodeQL language.
    pub(crate) fn per_language(&self) -> bool {
        !self.prepare.is_empty()
    }
}

/// A CodeQL language as Logos detects it from file extensions.
#[derive(Debug)]
pub(crate) struct CodeqlLanguage {
    /// The label reported and used in output names (`codeql/<label>.sarif`).
    pub label: &'static str,
    /// The `--language` id when Logos runs the language; `None` when it needs a
    /// build (or evaluates the project to resolve dependencies), so Logos never
    /// runs it.
    pub codeql_id: Option<&'static str>,
    pub extensions: &'static [&'static str],
}

/// The languages CodeQL analyses, and which of them Logos runs.
///
/// `--build-mode=none` is necessary but not sufficient ([NFR-SE-02]): for some
/// languages that mode still evaluates the project to find its dependencies.
/// Java's resolves them through the project's Maven or Gradle build (a
/// committed `gradlew` included), C#'s restores them through MSBuild, which
/// runs targets the project defines, and Rust's loads the workspace through
/// Cargo, which honours the project's `.cargo/config.toml`. So only the
/// languages whose extraction reads source and nothing else run: JavaScript /
/// TypeScript, Python and Ruby. Every other language — those, plus Go, Kotlin,
/// Swift and C/C++, which extract only from a build — is [`REQUIRES_BUILD`].
///
/// [NFR-SE-02]: ../../../docs/specs/requirements/NFR-SE-02.md
/// [`REQUIRES_BUILD`]: super::REQUIRES_BUILD
pub(crate) static CODEQL_LANGUAGES: [CodeqlLanguage; 10] = [
    CodeqlLanguage {
        label: "javascript-typescript",
        codeql_id: Some("javascript-typescript"),
        extensions: &["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"],
    },
    CodeqlLanguage { label: "python", codeql_id: Some("python"), extensions: &["py"] },
    CodeqlLanguage { label: "ruby", codeql_id: Some("ruby"), extensions: &["rb"] },
    CodeqlLanguage {
        label: "c-cpp",
        codeql_id: None,
        extensions: &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"],
    },
    CodeqlLanguage { label: "csharp", codeql_id: None, extensions: &["cs"] },
    CodeqlLanguage { label: "go", codeql_id: None, extensions: &["go"] },
    CodeqlLanguage { label: "java", codeql_id: None, extensions: &["java"] },
    CodeqlLanguage { label: "kotlin", codeql_id: None, extensions: &["kt", "kts"] },
    CodeqlLanguage { label: "rust", codeql_id: None, extensions: &["rs"] },
    CodeqlLanguage { label: "swift", codeql_id: None, extensions: &["swift"] },
];

/// The first version-shaped token (`1.2`, `v8.18.4`, `10.0.4`) in a version
/// command's output, without a leading `v`.
pub(crate) fn parse_version(output: &str) -> Option<String> {
    static VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bv?(\d+(?:\.\d+)+)").expect("static regex"));
    VERSION.captures(output).map(|c| c[1].to_string())
}
