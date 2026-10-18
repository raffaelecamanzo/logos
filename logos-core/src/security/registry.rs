//! The built-in scanner registry ([FR-SC-05], [ADR-80]): nine entries, each a
//! detection probe and a **fixed** SARIF invocation template.
//!
//! The registry is data. A template is a list of [`Arg`]s: literals compiled
//! into this file, plus placeholders filled only from values Logos chooses (the
//! output path under `.logos/security/`, the CodeQL language and database from
//! [`CODEQL_LANGUAGES`], an empty config file it writes itself), from the
//! user-level config (the extra flags), or, in a scoped run, from the user's
//! own path arguments, each confined to the project root before any spawn
//! ([`super::resolve_scope`]). Nothing in the repository fills a placeholder
//! but the names of the directories its own ignore files exclude
//! ([`Arg::Ignored`]): each the value of an exclusion flag, so it can only
//! narrow what is scanned — which the repository's `[security_scan]` switch
//! can already do wholesale — and a name a flag could misread is never
//! passed. The scan root is the child's working directory, spelled `.` in
//! the argv ([NFR-SE-08]).
//!
//! Tested versions — the command lines below were written or last verified
//! against these, and a scanner whose flags have since changed reports
//! *failed* with its stderr:
//! Semgrep 1.85, Trivy 0.74, Gitleaks 8.18, OSV-Scanner 1.9, Bandit 1.7.10,
//! gosec 2.21, Checkov 3.2, CodeQL 2.19, Dependency-Check 13.0.
//!
//! Dependency-Check's template was written against 10.0 and verified at 13.0.0
//! when that release was pinned (S-746): its `--advancedHelp` still lists every
//! flag of the template. 13.0 is also the minimum the manifest accepts, because
//! earlier lines broke on changes to the NVD data they read. CodeQL's command
//! line was not re-verified at the managed 2.27.2 bundle, so 2.19 stands.
//!
//! Path arguments at those versions ([FR-SC-17], CR-219 CRA-02), read from each
//! scanner's source at its release tag, and what each [`ScopedPaths`] encodes:
//! - **Semgrep 1.85** takes any number of file or directory targets as
//!   positional arguments, and reports a path as it was given (`src/a.py`,
//!   which is also how a full scan of `.` spells it).
//! - **Bandit 1.7.10** likewise; `--recursive` already walks a directory.
//! - **gosec 2.21** loads Go *packages*: a positional argument is a package
//!   directory (`./dir`, or `./dir/...` with its subpackages); a `.go` file is
//!   not scanned, so a file is passed as the directory holding it.
//! - **Gitleaks 8.18** reads one `--source` (a repeated flag keeps the last),
//!   and `detect` in its default git mode scans the history of the repository
//!   `--source` names, never one file of it. A scoped run is therefore one run
//!   per path with `--no-git`, which reads that file or directory as it is now.
//!   That is other evidence than the full scan's, so its ingest keeps the full
//!   scan's findings in those paths, and a secret it finds that is one of them
//!   keeps that finding's fingerprint ([`PathSlot::keeps_full_findings`]). For
//!   Gitleaks, path is not the only new variable: content mode is too
//!   ([ADR-83] as amended by S-685).
//! - **Checkov 3.2** takes `--file` for a file and `--directory` for a
//!   directory; given both, it runs the directories and passes the files into
//!   each, so a scoped run is one run per path with the flag for its kind.
//! - **Trivy, OSV-Scanner and Dependency-Check** resolve a whole manifest or
//!   lockfile tree; **CodeQL** analyses a database of every file. None has a
//!   path position: a scoped scan reports the three *whole-manifest only* and
//!   CodeQL *whole-project only*.
//!
//! None of the five is installed where this was written, so the facts come
//! from the source, not from a run; the stubbed argv tests pin the shapes, and
//! a scanner that rejects one reports *failed* with its stderr.
//!
//! Ignored directories (HF-5 of sprint 106). A whole-project run keeps out of
//! the directories the project's own ignore files exclude
//! ([`super::ignored_dirs`]), each top-most one passed through the scanner's
//! own flag ([`Arg::Ignored`]) after the `.logos` it already excluded. What
//! each encodes, read from the source at the pinned version:
//! - **Trivy 0.74** matches `--skip-dirs` with `doublestar.Match` against the
//!   whole path relative to the scan root and does not descend into a match,
//!   so `target` skips `target/` and not `src/target/`. The flag is a pflag
//!   string slice: it repeats, and each value is read as CSV.
//! - **Bandit 1.7.10** keeps only the last `--exclude` and splits it on
//!   commas; a directory it finds there becomes a `dir/*` glob, matched with
//!   `fnmatch` against the `./`-prefixed paths of its walk of `.`.
//! - **Checkov 3.2** searches each `--skip-path` as a regular expression in
//!   the path its runners join to the walked directory — `.` as given, or
//!   its absolute form, which the Terraform and SAST runners walk — and also
//!   skips a path the value is a substring of; an expression anchored at
//!   either root is never a substring of a path.
//! - **Dependency-Check 13.0** adds each `--exclude` to an Ant
//!   `DirectoryScanner` based at the scanned directory, so `target/**` is
//!   anchored there.
//! - **Semgrep 1.85**, **OSV-Scanner 1.9** and **Gitleaks 8.18** take nothing:
//!   in a git repository Semgrep's targets are `git ls-files` (tracked, plus
//!   untracked with `--exclude-standard`), OSV-Scanner skips what the
//!   repository's `.gitignore` files and `.git/info/exclude` match, and
//!   Gitleaks reads committed history. Semgrep's `--exclude` would also
//!   match the name at any depth.
//! - **gosec 2.21** takes nothing: its `-exclude-dir` is an unanchored
//!   expression (`target` drops `pkg/mytarget` too), applied after a walk it
//!   makes anyway. **CodeQL** has no exclusion flag, only the
//!   `LGTM_INDEX_FILTERS` its extractors read; it analyses only the languages
//!   [`super::codeql_languages`] finds in files the project does not ignore.
//!
//! [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
//! [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
//! [ADR-83]: ../../../docs/specs/architecture/decisions/ADR-83.md
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
    /// The project's ignored directories ([`super::ignored_dirs`]), spelled
    /// for this scanner's exclusion flag, in a whole-project run; nothing in a
    /// run on paths the user named, which reads exactly those.
    Ignored(Exclusion),
}

use Arg::{Database, Extra, Ignored, Language, Lit, Out, Paths, PinnedConfig};

/// How [`Arg::Ignored`] passes the ignored directories — the shapes the
/// [module docs](self) record per scanner and tested version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exclusion {
    /// `flag` and the directory, once per directory.
    Repeat { flag: &'static str, spell: Spelling },
    /// `,` and the directory appended to the element before, once per
    /// directory: a comma list, for a flag whose repeat keeps only the last.
    Append { spell: Spelling },
}

/// How one ignored directory, `target` or `notes/old`, is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spelling {
    /// `target`
    Rel,
    /// `./target`
    DotSlash,
    /// `target/**`, an Ant pattern for everything below it.
    AntTree,
    /// `^(\./|/abs/root/)?target(/|$)`: a regular expression that matches
    /// the directory and what is below it as the scanner joins them to `.`
    /// or to the absolute root, and no other path that merely contains the
    /// name.
    RootRegex,
}

/// The bytes the ignored directories may add to one scanner's argv. Far
/// below Linux's 128 KiB limit on one element (Bandit's whole list is one)
/// and the 1 MiB `ARG_MAX` of macOS, so a project with thousands of ignored
/// directories still starts every scanner: past it, the deepest are left out
/// and scanned as before.
pub(crate) const IGNORED_ARGV_BUDGET: usize = 64 * 1024;

impl Spelling {
    fn write(self, dir: &str, root: Option<&str>) -> String {
        match self {
            Spelling::Rel => dir.to_string(),
            Spelling::DotSlash => format!("./{dir}"),
            Spelling::AntTree => format!("{dir}/**"),
            Spelling::RootRegex => {
                let root = root.map(|r| format!("|{}/", regex::escape(r))).unwrap_or_default();
                format!(r"^(\./{root})?{}(/|$)", regex::escape(dir))
            }
        }
    }
}

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
/// [`Arg::Paths`] in a full scan, and how a scoped run's paths fill it.
///
/// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PathSlot {
    pub full: &'static [&'static str],
    pub scoped: ScopedPaths,
    /// `true` when a scoped run reads other evidence than a full scan does, so
    /// its ingest keeps the full scan's findings in the scanned paths
    /// ([`crate::history::security::IngestScope::keeps_full_findings`]):
    /// Gitleaks alone, whose scoped run reads the files with `--no-git` and
    /// whose full scan reads their git history.
    pub keeps_full_findings: bool,
}

/// How a scoped run's paths fill [`Arg::Paths`] — the shapes the [module
/// docs](self) record per scanner and tested version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopedPaths {
    /// Every path as a positional argument, in one run.
    Positional,
    /// Every path's Go package as a positional argument, in one run: a file's
    /// directory (`./dir`), or a directory with its subpackages (`./dir/...`).
    Packages,
    /// One run per path: `prefix`, then `file_flag` or `dir_flag` for the
    /// path's kind, then the path.
    OnePerRun {
        prefix: &'static [&'static str],
        file_flag: &'static str,
        dir_flag: &'static str,
    },
}

/// One path of a scoped run, already confined to the project root: relative to
/// it, `/`-separated, never empty, never starting with `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ScopedPath {
    pub rel: String,
    pub is_dir: bool,
}

impl PathSlot {
    /// The runs a scoped scan over `scope` makes: one for every path at once,
    /// or one per path for [`ScopedPaths::OnePerRun`].
    pub(crate) fn runs<'s>(&self, scope: &'s [ScopedPath]) -> Vec<&'s [ScopedPath]> {
        match self.scoped {
            ScopedPaths::OnePerRun { .. } => scope.chunks(1).collect(),
            ScopedPaths::Positional | ScopedPaths::Packages => vec![scope],
        }
    }

    /// The argv elements `scope` fills the position with.
    fn render(&self, scope: &[ScopedPath], argv: &mut Vec<OsString>) {
        match self.scoped {
            ScopedPaths::Positional => argv.extend(scope.iter().map(|p| OsString::from(&p.rel))),
            ScopedPaths::Packages => {
                let mut packages: Vec<String> = Vec::with_capacity(scope.len());
                for p in scope {
                    let package = if p.is_dir {
                        format!("./{}/...", p.rel)
                    } else {
                        match p.rel.rsplit_once('/') {
                            Some((dir, _)) => format!("./{dir}"),
                            None => ".".to_string(),
                        }
                    };
                    if !packages.contains(&package) {
                        packages.push(package);
                    }
                }
                argv.extend(packages.into_iter().map(OsString::from));
            }
            ScopedPaths::OnePerRun { prefix, file_flag, dir_flag } => {
                argv.extend(prefix.iter().map(OsString::from));
                for p in scope {
                    argv.push(OsString::from(if p.is_dir { dir_flag } else { file_flag }));
                    argv.push(OsString::from(&p.rel));
                }
            }
        }
    }
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
        path_slot: Some(PathSlot { full: &["."], scoped: ScopedPaths::Positional, keeps_full_findings: false }),
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
            Ignored(Exclusion::Repeat { flag: "--skip-dirs", spell: Spelling::Rel }),
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
        path_slot: Some(PathSlot {
            full: &["--source", "."],
            scoped: ScopedPaths::OnePerRun { prefix: &["--no-git"], file_flag: "--source", dir_flag: "--source" },
            keeps_full_findings: true,
        }),
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
            Ignored(Exclusion::Append { spell: Spelling::DotSlash }),
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["."], scoped: ScopedPaths::Positional, keeps_full_findings: false }),
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
        path_slot: Some(PathSlot { full: &["./..."], scoped: ScopedPaths::Packages, keeps_full_findings: false }),
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
            Ignored(Exclusion::Repeat { flag: "--skip-path", spell: Spelling::RootRegex }),
            Extra,
            Paths,
        ],
        output: Output::Dir { produced: "results_sarif.sarif" },
        ok_exit: &[0],
        path_slot: Some(PathSlot {
            full: &["--directory", "."],
            scoped: ScopedPaths::OnePerRun { prefix: &[], file_flag: "--file", dir_flag: "--directory" },
            keeps_full_findings: false,
        }),
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
            Ignored(Exclusion::Repeat { flag: "--exclude", spell: Spelling::AntTree }),
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

/// What a scoped scan reports for Trivy, OSV-Scanner and Dependency-Check,
/// which resolve a whole manifest or lockfile tree ([FR-SC-17]).
pub const WHOLE_MANIFEST_ONLY: &str = "whole-manifest only";

/// What a scoped scan reports for CodeQL, which analyses a database of every
/// file.
pub const WHOLE_PROJECT_ONLY: &str = "whole-project only";

/// The registered scanner called `name`.
pub fn scanner(name: &str) -> Option<&'static Scanner> {
    REGISTRY.iter().find(|s| s.name == name)
}

/// The values a template's placeholders are filled from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fill<'a> {
    pub out: &'a Path,
    pub extra: &'a [String],
    /// `None` is a full scan; `Some` the paths of one scoped run
    /// ([`PathSlot::runs`]).
    pub scope: Option<&'a [ScopedPath]>,
    /// The CodeQL language id (only read by [`Arg::Language`]).
    pub language: &'a str,
    /// The CodeQL database directory (only read by [`Arg::Database`]).
    pub database: &'a Path,
    /// The empty config file (only read by [`Arg::PinnedConfig`]).
    pub pinned_config: &'a Path,
    /// The project's ignored directories (only read by [`Arg::Ignored`], and
    /// only when `scope` is `None`).
    pub ignored: &'a [String],
    /// The project root as the scanner's working directory reports it
    /// (canonical); `None` when it is not UTF-8. Only read by
    /// [`Spelling::RootRegex`].
    pub root: Option<&'a str>,
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
                        Some(scope) => slot.render(scope, &mut argv),
                    }
                }
                Language => argv.push(OsString::from(format!("--language={}", fill.language))),
                Database => argv.push(fill.database.as_os_str().to_owned()),
                PinnedConfig => argv.push(fill.pinned_config.as_os_str().to_owned()),
                Ignored(_) if fill.scope.is_some() => {}
                Ignored(Exclusion::Repeat { flag, spell }) => {
                    // The flag and the value, each with its terminating NUL.
                    for value in self.budgeted(fill, spell, flag.len() + 2) {
                        argv.push(OsString::from(flag));
                        argv.push(OsString::from(value));
                    }
                }
                Ignored(Exclusion::Append { spell }) => {
                    let last = argv.last_mut().expect("a comma list follows the flag's first value");
                    for value in self.budgeted(fill, spell, 1) {
                        last.push(",");
                        last.push(value);
                    }
                }
            }
        }
        argv
    }

    /// The ignored directories spelled, in order, while each with its
    /// `overhead` fits [`IGNORED_ARGV_BUDGET`]; the rest are left out, with a
    /// warning.
    fn budgeted(&self, fill: &Fill<'_>, spell: Spelling, overhead: usize) -> Vec<String> {
        let mut used = 0;
        let values: Vec<String> = fill
            .ignored
            .iter()
            .map(|dir| spell.write(dir, fill.root))
            .take_while(|value| {
                used += overhead + value.len();
                used <= IGNORED_ARGV_BUDGET
            })
            .collect();
        if values.len() < fill.ignored.len() {
            tracing::warn!(
                target: "logos::security",
                scanner = self.name,
                "{} of {} ignored directories exceed the argument budget and are scanned",
                fill.ignored.len() - values.len(),
                fill.ignored.len()
            );
        }
        values
    }

    /// Whether a whole-project run passes the project's ignored directories.
    pub(crate) fn excludes_ignored(&self) -> bool {
        self.run.iter().any(|a| matches!(a, Ignored(_)))
    }

    /// Whether this scanner runs once per CodeQL language.
    pub(crate) fn per_language(&self) -> bool {
        !self.prepare.is_empty()
    }

    /// Why a scoped scan cannot narrow this scanner, or `None` when it has a
    /// path position: [`WHOLE_MANIFEST_ONLY`] for a dependency scanner,
    /// [`WHOLE_PROJECT_ONLY`] for CodeQL.
    pub(crate) fn unscoped_reason(&self) -> Option<&'static str> {
        match self.path_slot {
            Some(_) => None,
            None if self.per_language() => Some(WHOLE_PROJECT_ONLY),
            None => Some(WHOLE_MANIFEST_ONLY),
        }
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
