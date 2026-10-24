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
//! but what its own ignore files leave in or out, and both can only narrow
//! what is scanned — which the repository's `[security_scan]` switch can
//! already do wholesale ([NFR-SE-08]):
//! - the names of the directories they exclude ([`Arg::Ignored`]), each the
//!   value of an exclusion flag, a name a flag could misread never passed;
//! - the paths they leave in ([`Arg::Unignored`]), each a file or directory
//!   under the root that `.` would have read, never a symbolic link, never
//!   one starting with `-`, within the same argument budget.
//!
//! The scan root is the child's working directory, spelled `.` in the argv
//! wherever no list of unignored paths takes its place.
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
//! gosec is pinned at 2.29.0 (S-782), the release that adds Go 1.27 support: 2.21.4
//! cannot type a package that imports the standard library under it, and exits 1
//! with no report. Its command line above is the one proven at 2.21.4; the
//! hand-run harness, whose Go files now import the standard library, is re-run
//! on 2.29.0 by the maintainer, and until its rows are committed at the pin the
//! `exclude-dir-tree` evidence is the 2.21.4 run. A gosec of any version that
//! still prints `without types was imported` reads *failed* with a cause naming
//! its version, the local Go and the remedy (`surface::failure_cause`).
//!
//! OSV-Scanner is pinned at 2.7.0 (S-779), and its command line is the 1.9 one
//! with `--output-file` in place of `--output` (2.x keeps `--output` as a
//! deprecated alias that warns on every run). The argv, the flag spellings and
//! the root command's fallback to `scan source` are read from the source at the
//! tags, not from a run: 1.9.2 defines `--output` and the `--experimental-*`
//! flags and no `--output-file`, `--offline` or `--all-packages`; 2.7.0 the
//! reverse. The tested version above stays 1.9, the last a real run proved, and
//! the manifest's minimum with it, until the owner's harness run commits rows at
//! the pin: 2.0.0 extracts dependencies with osv-scalibr, so the proof that the
//! unignored targets keep OSV-Scanner out of ignored paths does not carry over
//! until the six rows are committed at 2.7.0, and a pin the run does not prove
//! is reverted. The harness probes with `--offline` and `--all-packages`, the
//! 2.x spellings of the `--experimental-offline` and
//! `--experimental-all-packages` its 1.9.2 run used. A 1.9.x copy has no
//! `--output-file`, so one that runs the current command line reads *failed*
//! with its stderr, and `security status` shows a managed 1.9.2 copy's update.
//! The minimum is not raised ahead of the run (S-782 decided the same for
//! gosec): a minimum above an installed managed copy makes the copy vanish
//! instead of offering its update. Still unrun, and the owner's run must show:
//! that the root command takes the flags before its subcommand, that `scan
//! source` takes the file targets (`.gitignore`, `go.mod`) the unignored list
//! can hold, and the exit status of an offline run with no local database.
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
//! Ignored directories (HF-5 of sprint 106, S-772). A whole-project run keeps
//! out of the directories the project's own ignore files exclude
//! ([`super::project_ignores`]): each top-most one passed through the
//! scanner's own flag ([`Arg::Ignored`]) after any `.logos` it already
//! excluded, or — for the two scanners below whose flags cannot — every path
//! outside them given as a target ([`Arg::Unignored`]). What each encodes, read
//! from the source at the pinned version:
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
//! - **Semgrep 1.85** and **OSV-Scanner** (proven at 1.9.2, see above) are
//!   given the project's unignored paths as their targets instead of `.`
//!   ([`Arg::Unignored`], S-772). The hand-run harness (S-771,
//!   `scripts/tests/fixtures/scanner-exclusions/`) shows this is the one
//!   mechanism that keeps either out of an ignored `target/` and keeps the
//!   near misses `src/target/` and `targets/`, in a git checkout, a linked
//!   worktree and a non-git copy alike. Their own selection does not:
//!   Semgrep reads the ignored directory in a non-git copy, and OSV-Scanner
//!   in a linked worktree (`.git/info/exclude: not a directory`) and in a
//!   non-git copy. Semgrep's `--exclude target` and `--exclude /target` both
//!   drop `src/target/` as well: a leading `/` does not anchor it.
//! - **gosec 2.21** is given one `-exclude-dir` per directory. It searches
//!   each expression, wrapped as `([\\/])?<value>([\\/])?` with every `/`
//!   escaped, in the directory of each `.go` file its walk of `./...` finds,
//!   relative to `.`, and loads no package whose directory matches (its
//!   `helpers.go`, read at 2.24.8). So `target` also drops `pkg/mytarget`
//!   and `src/target`, and `^target$` matches `target` but not
//!   `target/nested`, whose package the harness's `exclude-dir-anchored` leg
//!   still reads. `^target(/|$)` ([`Spelling::TreeRegex`]) skips both and
//!   keeps `src/target/` and `targets/` in all three checkout shapes: the
//!   harness's `exclude-dir-tree` leg at 2.21.4 (S-773).
//! - **Gitleaks 8.18** takes nothing: it reads committed history.
//! - **CodeQL** has no exclusion flag. A Logos-written `paths-ignore` config
//!   or the `LGTM_INDEX_FILTERS` its extractors read might keep it out, but
//!   the harness has never run CodeQL (it was not installed), so it takes
//!   neither and writes no config; it analyses only the languages
//!   [`super::codeql_languages`] finds in files the project does not ignore.
//!
//! Each whole-project run states what it did with the ignored directories
//! ([`Scanner::coverage`], [FR-SC-26]): `excluded` only by a mechanism the
//! hand-run harness proves — Semgrep's and OSV-Scanner's unignored targets,
//! and gosec's tree expressions ([`Scanner::exclusion_evidence`]) — and
//! otherwise `scanned`, with a one-line note. Trivy, Bandit, Checkov and
//! Dependency-Check still get their exclusion flags, but the harness has not
//! run them, so they read `scanned` ([`FLAG_UNPROVEN`]) until it does (BR-97);
//! so do Gitleaks and CodeQL, which take nothing, a run whose target list
//! could not be passed, and a gosec run not given every ignored directory.
//!
//! Applicability (S-769). gosec reads Go files and Bandit Python files, and
//! nothing else ([`Scanner::applies_to`]): on a project where no file it does
//! not ignore has one of those extensions, either is `not applicable` and is
//! never started, not even probed. Every other scanner reads many languages,
//! manifests or any file, so no extension rules one out; CodeQL decides per
//! language instead.
//!
//! Severity (S-768). OSV-Scanner's SARIF writer (`internal/output/sarif.go`)
//! writes `warning` on every result, at 1.9.2 and still at 2.7.0, so its level
//! rates nothing and a finding it gives no score reads `unrated`. From 2.0.0
//! (PR #1587, first in 2.0.0-rc1) a rule carries the worst CVSS score of its
//! advisories as `security-severity` when one has a score, which the parser
//! reads as the advisory's rating; a 1.9.2 report ingested from CI still reads
//! `unrated`, gated as `medium`. From 2.3.5 each result also carries
//! `partialFingerprints`; S-778 keys an OSV-Scanner finding by rule and
//! lockfile path instead, so no stored finding, baseline entry or fingerprint
//! suppression reads as new across the move to 2.x.
//!
//! [FR-SC-05]: ../../../docs/specs/requirements/FR-SC-05.md
//! [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
//! [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
//! [ADR-83]: ../../../docs/specs/architecture/decisions/ADR-83.md
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::CoverageKind;

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
    /// The project's ignored directories ([`super::project_ignores`]), spelled
    /// for this scanner's exclusion flag, in a whole-project run; nothing in a
    /// run on paths the user named, which reads exactly those.
    Ignored(Exclusion),
    /// The project's unignored paths ([`Targets`]) as the scan targets, in a
    /// whole-project run; nothing in a path-capable scanner's run on paths the
    /// user named, which the [`Arg::Paths`] after it reads instead.
    Unignored,
    /// `--propertyfile` and the private file holding the NVD API key Logos
    /// read from [`NVD_API_KEY_ENV`](super::NVD_API_KEY_ENV), when it hands
    /// one over ([FR-SC-27]); nothing otherwise. Dependency-Check's only: its
    /// CLI reads no variable for the key, and a key in argv is visible to
    /// every process listing.
    ///
    /// [FR-SC-27]: ../../../docs/specs/requirements/FR-SC-27.md
    NvdKeyFile,
}

use Arg::{Database, Extra, Ignored, Language, Lit, NvdKeyFile, Out, Paths, PinnedConfig, Unignored};

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
    /// `^target(/|$)`: a regular expression that matches the directory and
    /// every directory below it, relative to `.` with no `./`, and no other
    /// directory that merely contains the name — gosec's spelling.
    TreeRegex,
}

/// The coverage note of a scanner that takes the ignored directories through
/// an exclusion flag ([`Arg::Ignored`]) the hand-run harness has not run:
/// its flag narrows the scan, but no committed evidence shows it is anchored
/// on the pinned version, so its outcome reads `scanned` (BR-97).
pub(crate) const FLAG_UNPROVEN: &str = "exclusion flag not proven by the harness";

/// The bytes the ignored directories may add to one scanner's argv. Far
/// below Linux's 128 KiB limit on one element (Bandit's whole list is one)
/// and the 1 MiB `ARG_MAX` of macOS, so a project with thousands of ignored
/// directories still starts every scanner: past it, the deepest are left out
/// and scanned as before. It caps a list of unignored paths
/// ([`Targets::Paths`]) too: past it, the scanner is given `.` instead, and
/// its coverage reads `scanned`.
pub(crate) const IGNORED_ARGV_BUDGET: usize = 64 * 1024;

/// What the project ignores, as a whole-project run passes it
/// ([`super::project_ignores`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ignores {
    /// The top-most ignored directories an exclusion flag can take
    /// ([`Arg::Ignored`]), shallowest first.
    pub dirs: Vec<String>,
    /// How many top-most ignored directories no exclusion flag is given,
    /// because it could misread the name: a scanner that takes the list reads
    /// those.
    pub misread: usize,
    /// What fills [`Arg::Unignored`].
    pub targets: Targets,
}

impl Ignores {
    /// Nothing ignored — or nothing walked, when no scanner runs over the
    /// whole project.
    pub(crate) const NONE: Ignores = Ignores { dirs: Vec::new(), misread: 0, targets: Targets::Root };

    fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.misread == 0
    }
}

/// What fills [`Arg::Unignored`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Targets {
    /// The project ignores no directory: `.`.
    Root,
    /// The fewest paths, files and directories, that hold every unignored
    /// path and no ignored directory, relative to the root, sorted:
    /// `.gitignore app go.mod src targets` beside an ignored `target`. The
    /// directories holding an ignored one are named by their entries instead.
    Paths(Vec<String>),
    /// The paths cannot be passed, for this reason, so `.` is: the scanner
    /// reads the ignored directories its own selection does not skip.
    Unpassable(&'static str),
}

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
            Spelling::TreeRegex => format!("^{}(/|$)", regex::escape(dir)),
        }
    }
}

/// The `ignored` directories spelled for `exclusion`, in order, while each
/// with its flag or separator fits [`IGNORED_ARGV_BUDGET`]; the rest are left
/// out.
fn fitting(ignored: &[String], root: Option<&str>, exclusion: Exclusion) -> Vec<String> {
    let (spell, overhead) = match exclusion {
        // The flag and the value, each with its terminating NUL.
        Exclusion::Repeat { flag, spell } => (spell, flag.len() + 2),
        Exclusion::Append { spell } => (spell, 1),
    };
    let mut used = 0;
    ignored
        .iter()
        .map(|dir| spell.write(dir, root))
        .take_while(|value| {
            used += overhead + value.len();
            used <= IGNORED_ARGV_BUDGET
        })
        .collect()
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
    /// For a scanner a whole-project run passes neither [`Arg::Ignored`] nor
    /// [`Arg::Unignored`]: why it reads the ignored directories: the
    /// [`CoverageKind`] and the coverage note its outcome carries
    /// ([`Scanner::coverage`]).
    pub(crate) reads_ignored: Option<(CoverageKind, &'static str)>,
    /// The files it reads, for a scanner that reads one language's files only:
    /// without one it has nothing to scan and is not started
    /// ([`Scanner::not_applicable`], S-769).
    pub(crate) applies_to: Option<Applies>,
    /// For a scanner whose [`Arg::Ignored`] flag the hand-run harness proves
    /// anchored on its pinned version, the mechanism of the
    /// `scripts/tests/fixtures/scanner-exclusions/verdicts.tsv` rows that show
    /// it — ignored directories skipped, near misses kept, in a git checkout,
    /// a linked worktree and a non-git copy — rendered with exactly this
    /// template's argv. `None` for an unproven flag, which reads `scanned`
    /// ([`FLAG_UNPROVEN`]), and for a scanner that takes none.
    pub(crate) exclusion_evidence: Option<&'static str>,
}

/// The files a single-language scanner reads, by extension, and how its
/// `not applicable` reason names them. Only gosec and Bandit have one: every
/// other scanner reads many languages, manifests or any file, so none of its
/// inputs can be ruled out by extension (CodeQL decides per language,
/// [`super::codeql_languages`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Applies {
    /// The extensions, without the dot, as the file name spells them.
    pub extensions: &'static [&'static str],
    /// The files, for the reason: `Go source file (.go)`.
    pub files: &'static str,
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
            // The whole-project targets; a scoped run's are `Paths`.
            Unignored,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &[], scoped: ScopedPaths::Positional, keeps_full_findings: false }),
        refuse_if_present: &[],
        reads_ignored: None,
        applies_to: None,
        exclusion_evidence: None,
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
        reads_ignored: None,
        applies_to: None,
        exclusion_evidence: None,
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
        reads_ignored: Some((CoverageKind::History, "it reads the committed history, which holds any file committed inside an ignored directory")),
        applies_to: None,
        exclusion_evidence: None,
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
            Lit("--output-file"),
            Out,
            Extra,
            Lit("--recursive"),
            Unignored,
        ],
        output: Output::File,
        // 1: vulnerabilities were found, which is a successful scan.
        ok_exit: &[0, 1],
        path_slot: None,
        refuse_if_present: &[],
        reads_ignored: None,
        applies_to: None,
        exclusion_evidence: None,
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
        reads_ignored: None,
        applies_to: Some(Applies { extensions: &["py", "pyw"], files: "Python source file (.py, .pyw)" }),
        exclusion_evidence: None,
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
            Ignored(Exclusion::Repeat { flag: "-exclude-dir", spell: Spelling::TreeRegex }),
            Extra,
            Paths,
        ],
        output: Output::File,
        ok_exit: &[0],
        path_slot: Some(PathSlot { full: &["./..."], scoped: ScopedPaths::Packages, keeps_full_findings: false }),
        refuse_if_present: &[],
        reads_ignored: None,
        applies_to: Some(Applies { extensions: &["go"], files: "Go source file (.go)" }),
        exclusion_evidence: Some("exclude-dir-tree"),
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
        reads_ignored: None,
        applies_to: None,
        exclusion_evidence: None,
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
        reads_ignored: Some((CoverageKind::Unproven, "no exclusion is proven for it, so its database holds the ignored directories' files")),
        applies_to: None,
        exclusion_evidence: None,
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
            NvdKeyFile,
            Extra,
        ],
        output: Output::Dir { produced: "dependency-check-report.sarif" },
        ok_exit: &[0],
        path_slot: None,
        refuse_if_present: &[],
        reads_ignored: None,
        applies_to: None,
        exclusion_evidence: None,
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
    /// The project's unignored paths (only read by [`Arg::Unignored`]: in a
    /// path-capable scanner's run, only when `scope` is `None`; a scanner with
    /// no path position takes them whatever the scope).
    pub targets: &'a Targets,
    /// The project root as the scanner's working directory reports it
    /// (canonical); `None` when it is not UTF-8. Only read by
    /// [`Spelling::RootRegex`].
    pub root: Option<&'a str>,
    /// The private file holding the NVD API key, when Logos hands one over
    /// (only read by [`Arg::NvdKeyFile`]).
    pub nvd_key_file: Option<&'a Path>,
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
                NvdKeyFile => {
                    if let Some(file) = fill.nvd_key_file {
                        argv.push(OsString::from("--propertyfile"));
                        argv.push(file.as_os_str().to_owned());
                    }
                }
                Ignored(_) if fill.scope.is_some() => {}
                // A scoped run's paths are the path position's; a scanner
                // without one runs whole whatever the scope.
                Unignored if fill.scope.is_some() && self.path_slot.is_some() => {}
                Ignored(exclusion @ Exclusion::Repeat { flag, .. }) => {
                    for value in self.budgeted(fill, exclusion) {
                        argv.push(OsString::from(flag));
                        argv.push(OsString::from(value));
                    }
                }
                Ignored(exclusion @ Exclusion::Append { .. }) => {
                    let last = argv.last_mut().expect("a comma list follows the flag's first value");
                    for value in self.budgeted(fill, exclusion) {
                        last.push(",");
                        last.push(value);
                    }
                }
                Unignored => match fill.targets {
                    Targets::Paths(paths) => argv.extend(paths.iter().map(OsString::from)),
                    Targets::Root | Targets::Unpassable(_) => argv.push(OsString::from(".")),
                },
            }
        }
        argv
    }

    /// [`fitting`] the ignored directories, with a warning when some are left
    /// out.
    fn budgeted(&self, fill: &Fill<'_>, exclusion: Exclusion) -> Vec<String> {
        let values = fitting(fill.ignored, fill.root, exclusion);
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

    /// How a whole-project run passes the ignored directories to an exclusion
    /// flag ([`Arg::Ignored`]), when it does.
    fn exclusion(&self) -> Option<Exclusion> {
        self.run.iter().find_map(|a| match *a {
            Ignored(exclusion) => Some(exclusion),
            _ => None,
        })
    }

    /// Whether a whole-project run passes the ignored directories to an
    /// exclusion flag ([`Arg::Ignored`]).
    #[cfg(test)]
    pub(crate) fn takes_exclusion_flag(&self) -> bool {
        self.exclusion().is_some()
    }

    /// What a whole-project run with this argv does with the project's
    /// ignored directories ([FR-SC-26]), from the same [`Ignores`] the run is
    /// [rendered](Self::render) with: `excluded` when the project ignores
    /// none, when its [`Arg::Unignored`] targets could be passed, or when its
    /// exclusion flag ([`Arg::Ignored`]) is proven
    /// ([`exclusion_evidence`](Self::exclusion_evidence)) and given every
    /// ignored directory — none left out for a misreadable name or past the
    /// budget; otherwise `scanned`, with a [`CoverageKind`] naming how and a note
    /// saying why. An unproven flag reads `scanned`
    /// ([`FLAG_UNPROVEN`]) however much of the list it is given.
    ///
    /// The budget is measured without the root, which only
    /// [`Spelling::RootRegex`] — no proven flag's — spells.
    ///
    /// [FR-SC-26]: ../../../docs/specs/requirements/FR-SC-26.md
    pub(crate) fn coverage(&self, ignores: &Ignores) -> super::Coverage {
        if ignores.is_empty() {
            return super::Coverage::EXCLUDED;
        }
        if let Some((kind, note)) = self.reads_ignored {
            return super::Coverage::scanned(kind, note.to_string());
        }
        if self.run.contains(&Unignored) {
            return match ignores.targets {
                Targets::Unpassable(why) => super::Coverage::scanned(CoverageKind::WholeProject, format!(
                    "it is given the whole project, because {why}"
                )),
                Targets::Root | Targets::Paths(_) => super::Coverage::EXCLUDED,
            };
        }
        let Some(exclusion) = self.exclusion().filter(|_| self.exclusion_evidence.is_some()) else {
            return super::Coverage::scanned(CoverageKind::Unproven, FLAG_UNPROVEN.to_string());
        };
        if ignores.misread > 0 {
            return super::Coverage::scanned(CoverageKind::Partial, format!(
                "it is not given {} ignored director{} whose name an exclusion flag could misread",
                ignores.misread,
                if ignores.misread == 1 { "y" } else { "ies" }
            ));
        }
        let given = fitting(&ignores.dirs, None, exclusion).len();
        if given < ignores.dirs.len() {
            let cut = ignores.dirs.len() - given;
            return super::Coverage::scanned(CoverageKind::Partial, format!(
                "it is not given {cut} ignored director{} past the 64 KiB argument budget",
                if cut == 1 { "y" } else { "ies" }
            ));
        }
        super::Coverage::EXCLUDED
    }

    /// Why this scanner has nothing to scan, when it does not: its
    /// [`applies_to`](Self::applies_to) names extensions and `has_file_with`
    /// finds no file the project keeps with one of them. `None` — it may
    /// run — for a scanner without a rule, and `has_file_with` is then not
    /// asked.
    pub(crate) fn not_applicable(&self, has_file_with: impl FnOnce(&[&str]) -> bool) -> Option<String> {
        let applies = self.applies_to?;
        (!has_file_with(applies.extensions))
            .then(|| format!("no {} among the files the project does not ignore", applies.files))
    }

    /// Whether this scanner takes the NVD API key ([`Arg::NvdKeyFile`]):
    /// Dependency-Check alone.
    pub(crate) fn takes_nvd_key(&self) -> bool {
        self.run.contains(&NvdKeyFile)
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

/// Whether the user's own `extra_args` already hand Dependency-Check its NVD
/// API key — `--nvdApiKey`, or a properties file of their own
/// (`--propertyfile`, `-P`), each as its own element or joined to its value
/// (`--nvdApiKey=<key>`, `-P<file>`) — so that Logos adds no file of its own
/// ([FR-SC-27]: the user's own setting wins).
///
/// [FR-SC-27]: ../../../docs/specs/requirements/FR-SC-27.md
pub(crate) fn supplies_nvd_key(extra: &[String]) -> bool {
    extra.iter().any(|arg| {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        matches!(flag, "--nvdApiKey" | "--propertyfile") || arg.starts_with("-P")
    })
}

/// The first version-shaped token (`1.2`, `v8.18.4`, `10.0.4`) in a version
/// command's output, without a leading `v`.
pub(crate) fn parse_version(output: &str) -> Option<String> {
    static VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bv?(\d+(?:\.\d+)+)").expect("static regex"));
    VERSION.captures(output).map(|c| c[1].to_string())
}
